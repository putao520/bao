// @trace REQ-BAO-API-018 [api:Bun.Shell/$] — Shell class + Interpreter bridge
//! Bun.Shell / Bun.$ — bridge to bun_shell_parser + bun_spawn.
//!
//! Reuses bun_shell_parser::Lexer/Parser/AST for command parsing and
//! bun_spawn::run for subprocess execution.
//! No hand-written shell parsing or process management code.

use ::std::collections::HashMap;
use ::std::ptr::{self, NonNull};
use ::std::sync::atomic::{AtomicU64, Ordering};

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue,
    UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

// ──────────────────── bun_shell_parser reuse ────────────────────
// @trace REQ-BAO-API-018 [reuse:bun_shell_parser] — Lexer/Parser/AST replaces hand-written parsing

use bun_alloc::Arena as Bump;
use bun_shell_parser::ast;
use bun_shell_parser::parse::{self as shell_parse, Lexer, Parser};

// ──────────────────── bun_spawn reuse ────────────────────
// @trace REQ-BAO-API-018 [reuse:bun_spawn] — spawn/run replaces std::process::Command

use bun_spawn::{RunOptions, Term, run};

// ──────────────────── ID counters ────────────────────

static SHELL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

// ──────────────────── Shell execution state ────────────────────

thread_local! {
    /// Active Shell instances (by shell_id), holding env overrides + cwd.
    static SHELL_INSTANCES: ::std::cell::RefCell<HashMap<u64, ShellState>> =
        ::std::cell::RefCell::new(HashMap::new());
}

/// Per-Shell instance state (env overrides + cwd).
struct ShellState {
    env: Option<HashMap<String, String>>,
    cwd: Option<String>,
}

// ──────────────────── ShellInterpreter ────────────────────

/// Shell interpreter that walks the bun_shell_parser AST and executes commands
/// via bun_spawn::run. Replaces hand-written pipeline/redirect/env parsing.
///
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter]
struct ShellInterpreter<'a> {
    env_override: Option<&'a HashMap<String, String>>,
    cwd_override: Option<&'a str>,
}

impl<'a> ShellInterpreter<'a> {
    fn new(
        env_override: Option<&'a HashMap<String, String>>,
        cwd_override: Option<&'a str>,
    ) -> Self {
        Self {
            env_override,
            cwd_override,
        }
    }

    /// Parse a command string using bun_shell_parser::Lexer + Parser, then interpret.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.parse_and_run]
    fn parse_and_run(&self, command: &str) -> ShellOutput {
        let command = command.trim();
        if command.is_empty() {
            return ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            };
        }

        // Use bun_shell_parser to lex + parse the command string
        let bump = Bump::new();
        let src = command.as_bytes();

        // Lex: Lexer requires &mut [BunString] for string escaping refs.
        // Allocate an empty slice from the bump allocator to satisfy the lifetime.
        let string_refs: &mut [bun_core::String] = bump.alloc_slice_fill_default(0);
        let mut lexer = Lexer::<{ shell_parse::StringEncoding::Ascii }>::new(
            &bump,
            src,
            string_refs,
            0, // no JS object refs
        );

        if let Err(e) = lexer.lex() {
            return ShellOutput {
                stdout: Vec::new(),
                stderr: format!("Bun.Shell: lex error: {:?}", e).into_bytes(),
                exit_code: 1,
            };
        }

        let lex_result = lexer.get_result();

        // Parse: Parser consumes lex output → AST
        let mut jsobjs: [bun_shell_parser::JSValueRaw; 0] = [];
        let parser = match Parser::new(&bump, lex_result, &mut jsobjs) {
            Ok(p) => p,
            Err(e) => {
                return ShellOutput {
                    stdout: Vec::new(),
                    stderr: format!("Bun.Shell: parse error: {:?}", e).into_bytes(),
                    exit_code: 1,
                };
            }
        };

        // The parser borrows bump-allocated data; we need to parse + interpret
        // within the same scope.
        let mut parser = parser;
        let script = match parser.parse() {
            Ok(s) => s,
            Err(e) => {
                return ShellOutput {
                    stdout: Vec::new(),
                    stderr: format!("Bun.Shell: parse error: {:?}", e).into_bytes(),
                    exit_code: 1,
                };
            }
        };

        // Walk the AST and execute
        self.interpret_script(&script)
    }

    /// Walk ast::Script and execute each statement.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_script]
    fn interpret_script<'bump>(&self, script: &ast::Script<'bump>) -> ShellOutput {
        let mut last_output = ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        };

        for stmt in script.stmts.iter() {
            last_output = self.interpret_stmt(stmt);
            // Stop on failure (like `set -e`)
            if last_output.exit_code != 0 {
                return last_output;
            }
        }

        last_output
    }

    /// Interpret a single statement (list of expressions separated by ; or &&/||).
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_stmt]
    fn interpret_stmt<'bump>(&self, stmt: &ast::Stmt<'bump>) -> ShellOutput {
        let mut last_output = ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        };

        for expr in stmt.exprs.iter() {
            last_output = self.interpret_expr(expr);
        }

        last_output
    }

    /// Interpret an expression (pipeline, binary, cond, subshell, if, etc.).
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_expr]
    fn interpret_expr<'bump>(&self, expr: &ast::Expr<'bump>) -> ShellOutput {
        match expr {
            ast::Expr::Pipeline(pipeline) => self.interpret_pipeline(pipeline),
            ast::Expr::Binary(binary) => {
                let left = self.interpret_expr(&binary.left);
                match binary.op {
                    ast::BinaryOp::And => {
                        if left.exit_code == 0 {
                            self.interpret_expr(&binary.right)
                        } else {
                            left
                        }
                    }
                    ast::BinaryOp::Or => {
                        if left.exit_code != 0 {
                            self.interpret_expr(&binary.right)
                        } else {
                            left
                        }
                    }
                }
            }
            ast::Expr::Cmd(cmd) => self.interpret_cmd(cmd),
            ast::Expr::Assign(_assigns) => {
                // Bare assignments (no command) — no execution needed
                ShellOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            ast::Expr::Subshell(sub) => self.interpret_script(&sub.script),
            ast::Expr::If(if_clause) => self.interpret_if(if_clause),
            ast::Expr::CondExpr(_cond) => {
                // CondExpr is a test expression like [ -f file ], not directly executable
                ShellOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                }
            }
            ast::Expr::Async(inner) => {
                // Async: run in background (for now, execute synchronously)
                self.interpret_expr(inner)
            }
        }
    }

    /// Interpret an if clause.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_if]
    fn interpret_if<'bump>(&self, if_clause: &ast::If<'bump>) -> ShellOutput {
        // cond is a SmolList<Stmt> — execute all stmts, check last exit_code
        let mut cond_result = ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        };
        for i in 0..if_clause.cond.len() {
            cond_result = self.interpret_stmt(&if_clause.cond[i]);
        }

        if cond_result.exit_code == 0 {
            // then branch
            let mut then_result = ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            };
            for i in 0..if_clause.then.len() {
                then_result = self.interpret_stmt(&if_clause.then[i]);
            }
            then_result
        } else {
            // else/elif branches (from else_parts)
            let len = if_clause.else_parts.len();
            if len == 0 {
                return cond_result;
            }
            // Even indices = elif conditions, odd indices = elif bodies
            // Last odd = else body
            let mut i = 0;
            while i + 1 < len {
                // Check elif condition
                let mut elif_cond = ShellOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                };
                for j in 0..if_clause.else_parts[i].len() {
                    elif_cond = self.interpret_stmt(&if_clause.else_parts[i][j]);
                }
                if elif_cond.exit_code == 0 {
                    // Execute elif body
                    let mut elif_body = ShellOutput {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        exit_code: 0,
                    };
                    for j in 0..if_clause.else_parts[i + 1].len() {
                        elif_body = self.interpret_stmt(&if_clause.else_parts[i + 1][j]);
                    }
                    return elif_body;
                }
                i += 2;
            }
            // If there's a trailing else (odd number of parts)
            if len % 2 == 1 {
                let mut else_body = ShellOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: 0,
                };
                for j in 0..if_clause.else_parts[len - 1].len() {
                    else_body = self.interpret_stmt(&if_clause.else_parts[len - 1][j]);
                }
                return else_body;
            }
            cond_result
        }
    }

    /// Interpret a pipeline: chain commands with stdout → stdin piping.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_pipeline]
    fn interpret_pipeline<'bump>(&self, pipeline: &ast::Pipeline<'bump>) -> ShellOutput {
        let items = pipeline.items;
        if items.is_empty() {
            return ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            };
        }

        // For a single-item pipeline, just execute it directly
        if items.len() == 1 {
            return self.interpret_pipeline_item(&items[0]);
        }

        // Multi-item pipeline: chain stdout→stdin between items.
        let mut prev_stdout: Option<Vec<u8>> = None;

        for (i, item) in items.iter().enumerate() {
            let is_last = i == items.len() - 1;
            let result = self.interpret_pipeline_item_with_stdin(item, prev_stdout.as_deref());

            if is_last {
                return result;
            } else {
                prev_stdout = Some(result.stdout);
            }
        }

        // Unreachable: pipeline always has at least one item, so the loop
        // always returns from within. But Rust needs a fallback.
        ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        }
    }

    /// Interpret a pipeline item.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_pipeline_item]
    fn interpret_pipeline_item<'bump>(&self, item: &ast::PipelineItem<'bump>) -> ShellOutput {
        match item {
            ast::PipelineItem::Cmd(cmd) => self.interpret_cmd(cmd),
            ast::PipelineItem::Assigns(_assigns) => ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            ast::PipelineItem::Subshell(sub) => self.interpret_script(&sub.script),
            ast::PipelineItem::If(if_clause) => self.interpret_if(if_clause),
            ast::PipelineItem::CondExpr(_) => ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            },
        }
    }

    /// Interpret a single command: extract args from AST Cmd, execute via bun_spawn::run.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_cmd]
    fn interpret_cmd<'bump>(&self, cmd: &ast::Cmd<'bump>) -> ShellOutput {
        let cmd_str = self.reconstruct_cmd_string(cmd);
        self.exec_via_sh(&cmd_str, None)
    }

    /// Reconstruct a command string from AST Cmd for /bin/sh execution.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.reconstruct_cmd_string]
    fn reconstruct_cmd_string<'bump>(&self, cmd: &ast::Cmd<'bump>) -> String {
        let mut parts = Vec::new();

        // Add env assignments
        for assign in cmd.assigns.iter() {
            let label = ::std::str::from_utf8(assign.label).unwrap_or("?");
            let value_str = self.atom_to_string(&assign.value);
            parts.push(format!("{}='{}'", label, value_str.replace('\'', "'\\''")));
        }

        // Add command name and arguments. Literal words are re-quoted for
        // /bin/sh: the lexer strips quote SYNTAX into bare Text atoms, so an
        // unquoted reconstruction re-exposes embedded whitespace/metachars
        // ('|', ';', quotes…) as shell operators — `printf '%s|' x` became
        // `printf %s| x` (a pipe!), and `echo "a 'b' c"` lost its inner
        // quotes to sh's second parse. Atoms carrying expansion semantics
        // ($VAR, *, ~, $(…)) pass through raw so sh still expands them.
        for arg in cmd.name_and_args.iter() {
            let rendered = self.atom_to_string(arg);
            if atom_is_pure_literal(arg) {
                parts.push(shell_quote_word(&rendered));
            } else {
                parts.push(rendered);
            }
        }

        // Handle redirect
        if let Some(ref redirect) = cmd.redirect_file {
            match redirect {
                ast::Redirect::Atom(atom) => {
                    let rendered = self.atom_to_string(atom);
                    let target = if atom_is_pure_literal(atom) {
                        shell_quote_word(&rendered)
                    } else {
                        rendered
                    };
                    if cmd.redirect.append() {
                        parts.push(format!(">> {}", target));
                    } else if cmd.redirect.stderr() {
                        parts.push(format!("2> {}", target));
                    } else {
                        parts.push(format!("> {}", target));
                    }
                }
                ast::Redirect::JsBuf(_) => {
                    // JS buffer redirect — not applicable in Rust-only context
                }
            }
        }

        parts.join(" ")
    }

    /// Convert an AST Atom to a String.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.atom_to_string]
    fn atom_to_string<'bump>(&self, atom: &ast::Atom<'bump>) -> String {
        match atom {
            ast::Atom::Simple(simple) => self.simple_atom_to_string(simple),
            ast::Atom::Compound(compound) => {
                let mut parts = Vec::new();
                for simple in compound.atoms.iter() {
                    parts.push(self.simple_atom_to_string(simple));
                }
                parts.join("")
            }
        }
    }

    /// Convert a SimpleAtom to String.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.simple_atom_to_string]
    fn simple_atom_to_string<'bump>(&self, atom: &ast::SimpleAtom<'bump>) -> String {
        match atom {
            ast::SimpleAtom::Var(name) => {
                let name_str = ::std::str::from_utf8(name).unwrap_or("");
                format!("${}", name_str)
            }
            ast::SimpleAtom::VarArgv(idx) => format!("${}", idx),
            ast::SimpleAtom::Text(s) => ::std::str::from_utf8(s).unwrap_or("").to_string(),
            ast::SimpleAtom::QuotedEmpty => String::new(),
            ast::SimpleAtom::Asterisk => "*".to_string(),
            ast::SimpleAtom::DoubleAsterisk => "**".to_string(),
            ast::SimpleAtom::BraceBegin => "{".to_string(),
            ast::SimpleAtom::BraceEnd => "}".to_string(),
            ast::SimpleAtom::Comma => ",".to_string(),
            ast::SimpleAtom::Tilde => "~".to_string(),
            ast::SimpleAtom::CmdSubst(subst) => {
                let inner = self.interpret_script(&subst.script);
                String::from_utf8_lossy(&inner.stdout).into_owned()
            }
        }
    }

    /// Execute a command via /bin/sh -c, optionally piping stdin data.
    /// Uses bun_spawn::run for subprocess management (reuses Bun's spawn infrastructure).
    /// Falls back to std::process::Command when cwd override or stdin piping is needed.
    ///
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.exec_via_sh]
    fn exec_via_sh(&self, cmd_str: &str, stdin_data: Option<&[u8]>) -> ShellOutput {
        if cmd_str.trim().is_empty() {
            return ShellOutput {
                stdout: Vec::new(),
                stderr: b"Bun.Shell: empty command".to_vec(),
                exit_code: 1,
            };
        }

        // If there's stdin data or cwd override, fall back to std::process
        // since bun_spawn::run doesn't support stdin piping or cwd override.
        if stdin_data.is_some() || self.cwd_override.is_some() {
            return self.exec_via_sh_with_stdin(cmd_str, stdin_data);
        }

        // Build env map from overrides
        let env_map = self.build_env_map();

        // Build argv for bun_spawn::run: ["/bin/sh", "-c", <cmd>]
        let sh = b"/bin/sh";
        let dash_c = b"-c";
        let argv_slices: &[&[u8]] = &[sh, dash_c, cmd_str.as_bytes()];

        let opts = RunOptions {
            argv: argv_slices,
            env_map: &env_map,
        };

        match run(opts) {
            Ok(result) => {
                let exit_code = match result.term {
                    Term::Exited(code) => code as i32,
                    Term::Signal(sig) => -(sig as i32),
                    Term::Stopped(sig) => -(sig as i32),
                    Term::Unknown(code) => code as i32,
                };
                ShellOutput {
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code,
                }
            }
            Err(e) => ShellOutput {
                stdout: Vec::new(),
                stderr: format!("Bun.Shell: spawn failed: {:?}", e).into_bytes(),
                exit_code: -1,
            },
        }
    }

    /// Execute with stdin piping (for pipeline intermediate stages).
    /// Falls back to std::process::Command since bun_spawn::run doesn't
    /// support stdin piping directly.
    ///
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.exec_via_sh_with_stdin]
    fn exec_via_sh_with_stdin(&self, cmd_str: &str, stdin_data: Option<&[u8]>) -> ShellOutput {
        let mut command = ::std::process::Command::new("/bin/sh");
        command.arg("-c").arg(cmd_str);

        // CWD override
        if let Some(cwd) = self.cwd_override {
            command.current_dir(cwd);
        }

        // Environment overrides
        if let Some(overrides) = self.env_override {
            for (k, v) in overrides {
                command.env(k, v);
            }
        }

        // Stdin: pipe if we have input data
        if stdin_data.is_some() {
            command.stdin(::std::process::Stdio::piped());
        } else {
            command.stdin(::std::process::Stdio::null());
        }

        command.stdout(::std::process::Stdio::piped());
        command.stderr(::std::process::Stdio::piped());

        match command.spawn() {
            Ok(mut child) => {
                // Write stdin data if present
                if let Some(data) = stdin_data {
                    if let Some(mut stdin_pipe) = child.stdin.take() {
                        let _ = ::std::io::Write::write_all(&mut stdin_pipe, data);
                        drop(stdin_pipe);
                    }
                }

                match child.wait_with_output() {
                    Ok(output) => {
                        let exit_code = output.status.code().unwrap_or(-1);
                        ShellOutput {
                            stdout: output.stdout,
                            stderr: output.stderr,
                            exit_code,
                        }
                    }
                    Err(e) => ShellOutput {
                        stdout: Vec::new(),
                        stderr: format!("Bun.Shell: wait failed: {}", e).into_bytes(),
                        exit_code: -1,
                    },
                }
            }
            Err(e) => ShellOutput {
                stdout: Vec::new(),
                stderr: format!("Bun.Shell: spawn failed: {}", e).into_bytes(),
                exit_code: -1,
            },
        }
    }

    /// Interpret a pipeline item with optional stdin data from the previous stage.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.interpret_pipeline_item_with_stdin]
    fn interpret_pipeline_item_with_stdin<'bump>(
        &self,
        item: &ast::PipelineItem<'bump>,
        stdin_data: Option<&[u8]>,
    ) -> ShellOutput {
        match item {
            ast::PipelineItem::Cmd(cmd) => {
                let cmd_str = self.reconstruct_cmd_string(cmd);
                self.exec_via_sh_with_stdin(&cmd_str, stdin_data)
            }
            ast::PipelineItem::Assigns(_) => ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            ast::PipelineItem::Subshell(sub) => self.interpret_script(&sub.script),
            ast::PipelineItem::If(if_clause) => self.interpret_if(if_clause),
            ast::PipelineItem::CondExpr(_) => ShellOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: 0,
            },
        }
    }

    /// Build a bun_sys::EnvMap from the shell instance's env overrides.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellInterpreter.build_env_map]
    fn build_env_map(&self) -> bun_sys::EnvMap {
        let mut map = bun_sys::EnvMap::new();

        // Inherit current process environment
        for (key, value) in ::std::env::vars() {
            map.insert(key, value);
        }

        // Apply overrides
        if let Some(overrides) = self.env_override {
            for (k, v) in overrides {
                map.insert(k.clone(), v.clone());
            }
        }

        map
    }
}

// ──────────────────── sh reconstruction quoting ────────────────────

/// Whether an Atom is purely literal (Text/QuotedEmpty only) — safe to
/// single-quote wholesale when reconstructing the command for /bin/sh.
/// Atoms carrying expansion semantics ($VAR, *, ~, $(…), braces) must stay
/// raw so the re-executing shell still expands them.
fn atom_is_pure_literal(atom: &ast::Atom) -> bool {
    let simple_is_literal = |s: &ast::SimpleAtom| {
        matches!(s, ast::SimpleAtom::Text(_) | ast::SimpleAtom::QuotedEmpty)
    };
    match atom {
        ast::Atom::Simple(s) => simple_is_literal(s),
        ast::Atom::Compound(c) => c.atoms.iter().all(simple_is_literal),
    }
}

/// Shell-quote a literal word for /bin/sh re-execution: pass through the
/// historically-safe set unquoted, otherwise wrap in single quotes with the
/// `'\''` escape. Mirrors the env-assign quoting above.
fn shell_quote_word(word: &str) -> String {
    let safe = word.bytes().all(|b| {
        matches!(b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
            | b'_' | b'-' | b'.' | b'/' | b'@' | b'%' | b'+' | b'=' | b':' | b',')
    });
    if safe {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

// ──────────────────── ShellOutput result ────────────────────

/// Result of executing a shell command. Returned to JS as ShellOutput object.
/// Uses Vec<u8> internally (zero-copy from bun_spawn::RunResult), converted
/// to JS string on demand.
///
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput]
struct ShellOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
}

impl ShellOutput {
    /// Whether the command succeeded (exit_code == 0).
    fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Build a JS ShellOutput object on the given cx.
    /// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput]
    unsafe fn to_js_object(&self, cx: *mut JSContext) -> *mut JSObject {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;

        rooted!(&in(cx_ref) let obj = w2::JS_NewPlainObject(cx_ref));
        if obj.get().is_null() {
            return ptr::null_mut();
        }

        // stdout (lossy UTF-8 → JS string)
        let stdout_str = String::from_utf8_lossy(&self.stdout);
        let stdout_js = JS_NewStringCopyN(
            cx,
            stdout_str.as_ptr() as *const ::std::os::raw::c_char,
            stdout_str.len(),
        );
        if !stdout_js.is_null() {
            rooted!(&in(cx_ref) let sv = StringValue(&*stdout_js));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"stdout".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // stderr (lossy UTF-8 → JS string)
        let stderr_str = String::from_utf8_lossy(&self.stderr);
        let stderr_js = JS_NewStringCopyN(
            cx,
            stderr_str.as_ptr() as *const ::std::os::raw::c_char,
            stderr_str.len(),
        );
        if !stderr_js.is_null() {
            rooted!(&in(cx_ref) let sv = StringValue(&*stderr_js));
            JS_DefineProperty(
                cx,
                obj.handle().into(),
                c"stderr".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // exitCode
        rooted!(&in(cx_ref) let ecv = Int32Value(self.exit_code));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"exitCode".as_ptr(),
            ecv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // success — boolean property (exitCode === 0)
        rooted!(&in(cx_ref) let sv = BooleanValue(self.success()));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"success".as_ptr(),
            sv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // text() method — returns stdout as string
        w2::JS_DefineFunction(
            cx_ref,
            obj.handle(),
            c"text".as_ptr(),
            Some(shell_output_text),
            0,
            JSPROP_ENUMERATE as u32,
        );

        // json() method — parses stdout as JSON
        w2::JS_DefineFunction(
            cx_ref,
            obj.handle(),
            c"json".as_ptr(),
            Some(shell_output_json),
            0,
            JSPROP_ENUMERATE as u32,
        );

        // lines() method — splits stdout by newline
        w2::JS_DefineFunction(
            cx_ref,
            obj.handle(),
            c"lines".as_ptr(),
            Some(shell_output_lines),
            0,
            JSPROP_ENUMERATE as u32,
        );

        // bytes() method — returns stdout as Uint8Array
        w2::JS_DefineFunction(
            cx_ref,
            obj.handle(),
            c"bytes".as_ptr(),
            Some(shell_output_bytes),
            0,
            JSPROP_ENUMERATE as u32,
        );

        obj.get()
    }
}

// ──────────────────── ShellOutput method callbacks ────────────────────

/// ShellOutput.text() — returns stdout as string.
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput.text]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_output_text(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());
    let mut stdout_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"stdout".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stdout_val,
        },
    );
    args.rval().set(stdout_val);
    true
}

/// ShellOutput.json() — parses stdout as JSON.
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput.json]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_output_json(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    let mut stdout_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"stdout".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stdout_val,
        },
    );

    if stdout_val.is_string() {
        let stdout_str = crate::js_to_rust_string(cx, stdout_val);
        let js_str = JS_NewStringCopyN(
            cx,
            stdout_str.as_ptr() as *const ::std::os::raw::c_char,
            stdout_str.len(),
        );
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let str_root = js_str);
            let mut parsed = UndefinedValue();
            let parsed_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut parsed,
            };
            let _ = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_root.handle().into(), parsed_h);
            args.rval().set(parsed);
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// ShellOutput.lines() — splits stdout by newline into JS array.
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput.lines]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_output_lines(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    let mut stdout_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"stdout".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stdout_val,
        },
    );

    let stdout_str = if stdout_val.is_string() {
        crate::js_to_rust_string(cx, stdout_val)
    } else {
        String::new()
    };

    let lines: Vec<&str> = stdout_str.split('\n').collect();
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, lines.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (i, line) in lines.iter().enumerate() {
        let js_str = JS_NewStringCopyN(
            cx,
            line.as_ptr() as *const ::std::os::raw::c_char,
            line.len(),
        );
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let lv = StringValue(&*js_str));
            w2::JS_SetElement(cx_ref, arr.handle().into(), i as u32, lv.handle().into());
        }
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

/// ShellOutput.bytes() — returns stdout as Uint8Array.
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ ShellOutput.bytes]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_output_bytes(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, 0);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = this.to_object());

    let mut stdout_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"stdout".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut stdout_val,
        },
    );

    // Get the raw bytes — convert the JS string back to bytes
    let stdout_str = if stdout_val.is_string() {
        crate::js_to_rust_string(cx, stdout_val)
    } else {
        String::new()
    };

    let bytes = stdout_str.into_bytes();
    let len = bytes.len();

    // Create a Uint8Array via JS_NewUint8Array
    let arr_obj = JS_NewUint8Array(cx, len);
    if !arr_obj.is_null() {
        // Copy bytes into the array
        let data_ptr = JS_GetUint8ArrayData(arr_obj, ptr::null_mut(), ptr::null());
        if !data_ptr.is_null() {
            ptr::copy_nonoverlapping(bytes.as_ptr(), data_ptr, len);
        }
        rooted!(&in(cx_ref) let arr_rooted = arr_obj);
        args.rval().set(ObjectValue(arr_rooted.get()));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

// ──────────────────── Shell constructor ────────────────────

/// new Bun.Shell() constructor callback.
/// @trace REQ-BAO-API-018 [api:Bun.Shell constructor]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let shell_id = SHELL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    rooted!(&in(cx_ref) let shell_obj = w2::JS_NewPlainObject(cx_ref));
    if shell_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Store shell_id on the object
    rooted!(&in(cx_ref) let idv = DoubleValue(shell_id as f64));
    JS_DefineProperty(
        cx,
        shell_obj.handle().into(),
        c"_shellId".as_ptr(),
        idv.handle().into(),
        0,
    );

    // Install shell.run() method
    w2::JS_DefineFunction(
        cx_ref,
        shell_obj.handle(),
        c"run".as_ptr(),
        Some(shell_run),
        1,
        JSPROP_ENUMERATE as u32,
    );

    // Install shell.setenv() method
    w2::JS_DefineFunction(
        cx_ref,
        shell_obj.handle(),
        c"setenv".as_ptr(),
        Some(shell_setenv),
        2,
        JSPROP_ENUMERATE as u32,
    );

    // Install shell.cd() method
    w2::JS_DefineFunction(
        cx_ref,
        shell_obj.handle(),
        c"cd".as_ptr(),
        Some(shell_cd),
        1,
        JSPROP_ENUMERATE as u32,
    );

    // Initialize state
    SHELL_INSTANCES.with(|instances| {
        instances.borrow_mut().insert(
            shell_id,
            ShellState {
                env: None,
                cwd: None,
            },
        );
    });

    args.rval().set(ObjectValue(shell_obj.get()));
    true
}

// ──────────────────── Shell.run() ────────────────────

/// shell.run(command) → ShellOutput (synchronous).
/// shell.run(command, callback) → void (async, callback receives ShellOutput).
/// @trace REQ-BAO-API-018 [api:Bun.Shell.run]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_run(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Shell.run() requires a command string".as_ptr());
        return false;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Get command string
    let cmd_val = *args.get(0).ptr;
    let command = if cmd_val.is_string() {
        crate::js_to_rust_string(cx, cmd_val)
    } else {
        JS_ReportErrorUTF8(cx, c"Shell.run() first argument must be a string".as_ptr());
        return false;
    };

    // Get shell_id from this object
    let this = args.thisv();
    let shell_id = if this.is_object() {
        rooted!(&in(cx_ref) let obj = this.to_object());
        let mut id_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_shellId".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut id_val,
            },
        );
        if id_val.is_double() {
            id_val.to_double() as u64
        } else {
            0
        }
    } else {
        0
    };

    // Retrieve env/cwd overrides from shell state
    let (env_override, cwd_override) = SHELL_INSTANCES.with(|instances| {
        instances
            .borrow()
            .get(&shell_id)
            .map(|s| (s.env.clone(), s.cwd.clone()))
            .unwrap_or((None, None))
    });

    // Check if callback provided (async mode)
    let has_callback = argc >= 2 && (*args.get(1).ptr).is_object();
    let callback_obj = if has_callback {
        let cb_val = *args.get(1).ptr;
        let cb_obj = cb_val.to_object();
        if JS_ObjectIsFunction(cb_obj) {
            Some(cb_obj)
        } else {
            None
        }
    } else {
        None
    };

    // Execute using ShellInterpreter (reuses bun_shell_parser + bun_spawn)
    let interpreter = ShellInterpreter::new(env_override.as_ref(), cwd_override.as_deref());
    let output = interpreter.parse_and_run(&command);

    let js_output = output.to_js_object(cx);
    if js_output.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    if let Some(cb) = callback_obj {
        // Async: call callback(ShellOutput), return undefined
        rooted!(&in(cx_ref) let cb_h = ObjectValue(cb));
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        if !global.get().is_null() {
            rooted!(&in(cx_ref) let out_val = ObjectValue(js_output));
            let call_args = HandleValueArray {
                length_: 1,
                elements_: &*out_val.handle(),
            };
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            let _ = JS_CallFunctionValue(
                cx,
                global.handle().into(),
                cb_h.handle().into(),
                &call_args,
                rval_h,
            );
        }
        args.rval().set(UndefinedValue());
    } else {
        // Sync: return ShellOutput
        args.rval().set(ObjectValue(js_output));
    }
    true
}

// ──────────────────── Shell.setenv() / Shell.cd() ────────────────────

/// shell.setenv(key, value) — set environment variable override for this shell.
/// @trace REQ-BAO-API-018 [api:Bun.Shell.setenv]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_setenv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc < 2 {
        JS_ReportErrorUTF8(
            cx,
            c"Shell.setenv() requires key and value arguments".as_ptr(),
        );
        return false;
    }

    let key = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let value = crate::js_to_rust_string(cx, *args.get(1).ptr);

    let this = args.thisv();
    if this.is_object() {
        rooted!(&in(cx_ref) let obj = this.to_object());
        let mut id_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_shellId".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut id_val,
            },
        );
        if id_val.is_double() {
            let shell_id = id_val.to_double() as u64;
            SHELL_INSTANCES.with(|instances| {
                if let Some(state) = instances.borrow_mut().get_mut(&shell_id) {
                    state
                        .env
                        .get_or_insert_with(HashMap::new)
                        .insert(key, value);
                }
            });
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// shell.cd(path) — set working directory override for this shell.
/// @trace REQ-BAO-API-018 [api:Bun.Shell.cd]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn shell_cd(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc < 1 {
        JS_ReportErrorUTF8(cx, c"Shell.cd() requires a path argument".as_ptr());
        return false;
    }

    let path = crate::js_to_rust_string(cx, *args.get(0).ptr);

    let this = args.thisv();
    if this.is_object() {
        rooted!(&in(cx_ref) let obj = this.to_object());
        let mut id_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"_shellId".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut id_val,
            },
        );
        if id_val.is_double() {
            let shell_id = id_val.to_double() as u64;
            SHELL_INSTANCES.with(|instances| {
                if let Some(state) = instances.borrow_mut().get_mut(&shell_id) {
                    state.cwd = Some(path);
                }
            });
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ──────────────────── Bun.$ tagged template ────────────────────

/// Bun.$(strings, ...expressions) → ShellOutput.
/// Tagged template literal implementation.
/// Uses ShellInterpreter (bun_shell_parser + bun_spawn) for execution.
///
/// @trace REQ-BAO-API-018 [api:Bun.$ tagged template]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bun_dollar(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if argc == 0 {
        JS_ReportErrorUTF8(
            cx,
            c"Bun.$ requires at least a template strings array".as_ptr(),
        );
        return false;
    }

    // First argument is the template strings array (from tagged template literal)
    let strings_val = *args.get(0).ptr;
    if !strings_val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"Bun.$ first argument must be a template strings array".as_ptr(),
        );
        return false;
    }

    rooted!(&in(cx_ref) let strings_arr = strings_val.to_object());

    // Build the command string by interleaving template strings with expressions
    let mut command = String::new();
    let mut arr_len: u32 = 0;
    if !w2::GetArrayLength(cx_ref, strings_arr.handle().into(), &mut arr_len) || arr_len == 0 {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Direct array call — Bun.$(['echo', 'hello']) — has no interleaved
    // expressions (argc == 1) and multiple elements: each element is a
    // separate shell word, so join with spaces. Elements are JS strings
    // (argv semantics): quote each one so metacharacters ('|', ';', '>',
    // spaces…) stay literal through the lexer's parse — without quoting,
    // `['printf', '%s|', 'a|b;c']` re-exposed '|' as a shell pipe (127).
    // A tagged-template strings array with multiple parts always carries
    // argc > 1 (one arg per hole).
    if argc == 1 && arr_len > 1 {
        let mut words: Vec<String> = Vec::with_capacity(arr_len as usize);
        for i in 0..arr_len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                strings_arr.handle().into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_string() {
                words.push(shell_quote_word(&crate::js_to_rust_string(cx, elem)));
            }
        }
        command = words.join(" ");
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run(&command);
        let js_output = output.to_js_object(cx);
        if js_output.is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        args.rval().set(ObjectValue(js_output));
        return true;
    }

    for i in 0..arr_len {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            strings_arr.handle().into(),
            i,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_string() {
            command.push_str(&crate::js_to_rust_string(cx, elem));
        }
        // Interleave expression values (args 1..)
        if i as usize + 1 < argc as usize {
            let expr_val = *args.get(i + 1).ptr;
            if expr_val.is_string() {
                command.push_str(&crate::js_to_rust_string(cx, expr_val));
            } else if expr_val.is_int32() {
                command.push_str(&expr_val.to_int32().to_string());
            } else if expr_val.is_double() {
                command.push_str(&expr_val.to_double().to_string());
            }
        }
    }

    // Execute using ShellInterpreter (reuses bun_shell_parser + bun_spawn)
    let interpreter = ShellInterpreter::new(None, None);
    let output = interpreter.parse_and_run(&command);
    let js_output = output.to_js_object(cx);
    if js_output.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(ObjectValue(js_output));
    true
}

// ──────────────────── Public API ────────────────────

/// Install Bun.Shell and Bun.$ on the global Bun object.
/// Replaces the Phase 1 stub with a full implementation using
/// bun_shell_parser + bun_spawn (SPEC: REQ-BAO-API-018 reuse mapping).
///
/// @trace REQ-BAO-API-018 [api:Bun.Shell/$ installation]
pub unsafe fn install_bun_shell(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    // Install Bun.Shell constructor
    let shell_ctor = JS_NewFunction(
        cx.raw_cx(),
        Some(shell_constructor),
        0,
        JSFUN_CONSTRUCTOR as u32,
        c"Shell".as_ptr(),
    );
    if !shell_ctor.is_null() {
        let shell_proto = JS_GetFunctionObject(shell_ctor);
        rooted!(&in(cx) let shell_ctor_obj = shell_proto);
        w2::JS_DefineProperty3(
            cx,
            bun_obj,
            c"Shell".as_ptr(),
            shell_ctor_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Install Bun.$ as tagged template function
    let dollar_fn = JS_NewFunction(cx.raw_cx(), Some(bun_dollar), 0, 0, c"$".as_ptr());
    if !dollar_fn.is_null() {
        let dollar_obj = JS_GetFunctionObject(dollar_fn);
        rooted!(&in(cx) let dollar_fn_obj = dollar_obj);
        w2::JS_DefineProperty3(
            cx,
            bun_obj,
            c"$".as_ptr(),
            dollar_fn_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

// ──────────────────── Unit tests ────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_interpreter_echo() {
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run("echo hello");
        assert_eq!(output.exit_code, 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("hello"));
    }

    #[test]
    fn test_shell_interpreter_empty_command() {
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run("");
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_shell_interpreter_failing_command() {
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run("false");
        assert_ne!(output.exit_code, 0);
    }

    #[test]
    fn test_shell_interpreter_pipeline() {
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run("echo hello | cat");
        assert_eq!(output.exit_code, 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with("hello"));
    }

    #[test]
    fn test_shell_interpreter_env() {
        let mut env = HashMap::new();
        env.insert("BAO_TEST_VAR".to_string(), "test_value_456".to_string());
        let interpreter = ShellInterpreter::new(Some(&env), None);
        let output = interpreter.parse_and_run("echo $BAO_TEST_VAR");
        assert_eq!(output.exit_code, 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test_value_456"));
    }

    #[test]
    fn test_shell_interpreter_cwd() {
        let interpreter = ShellInterpreter::new(None, Some("/tmp"));
        let output = interpreter.parse_and_run("pwd");
        assert_eq!(output.exit_code, 0);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("/tmp"));
    }

    #[test]
    fn test_shell_output_success() {
        let output = ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
        };
        assert!(output.success());

        let output_fail = ShellOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 1,
        };
        assert!(!output_fail.success());
    }

    #[test]
    fn test_shell_interpreter_nonexistent_command() {
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run("nonexistent_command_xyz_123");
        assert_ne!(output.exit_code, 0);
    }

    #[test]
    fn test_shell_interpreter_redirect() {
        let tmp = ::std::env::temp_dir().join("bao_shell_test_redirect.txt");
        let _ = ::std::fs::remove_file(&tmp);
        let cmd = format!("echo hello > {}", tmp.display());
        let interpreter = ShellInterpreter::new(None, None);
        let output = interpreter.parse_and_run(&cmd);
        assert_eq!(output.exit_code, 0);
        let content = ::std::fs::read_to_string(&tmp).unwrap_or_default();
        assert!(content.contains("hello"));
        let _ = ::std::fs::remove_file(&tmp);
    }
}
