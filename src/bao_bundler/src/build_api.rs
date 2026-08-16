// @trace REQ-ENG-006 [api:Bun.build native driver] [req:REQ-ENG-006] [level:library]
//! Native `Bun.build` driver — registers the full `bun_bundler` BundleV2
//! pipeline behind `bun_runtime::bun_build`'s Rust-only contract.
//!
//! ## Why this lives here (layering)
//!
//! `bun_bundler`'s pipeline references SM-backed CYCLEBREAK symbols
//! (`__bun_jsc_generate_cached_bytecode`, …) that THIS crate defines, and
//! this crate depends on `bun_runtime` — so `bun_runtime` cannot name the
//! bundler directly (cycle + undefined symbols in bare test binaries).
//! `bun_api.rs` parses the JS config into `NativeBuildConfig` and calls the
//! registry; this file installs the real driver via [`install`]. Product
//! bring-up: `bao_cli::run()`. Tests: the e2e test binary calls `install()`
//! in its setup.
//!
//! ## Driver shape (upstream parity)
//!
//! Upstream `JSBundleCompletionTask.configureBundler` builds a Transpiler
//! from the API config, then `BundleV2.generateFromJavaScript` runs
//! `generate_from_cli` on the dedicated bundle thread. This driver runs on
//! the BuildTasklet's worker thread:
//!
//!   * `Transpiler::init` from an `api::TransformOptions` projected off the
//!     parsed JS config (entry points / target / external / define /
//!     sourcemap), then post-`from_api` overrides for the fields the API
//!     carries outside the schema struct (minify switches, output format,
//!     naming templates, banner/footer, splitting, publicPath).
//!   * A fresh `AnyEventLoop::Mini` drives `wait_for_parse` (the pipeline
//!     fans parse/generate out to the shared thread pool's CountedTask
//!     batches; this thread only pumps).
//!   * In-memory outputs (`output_dir` empty) so every artifact keeps its
//!     bytes for the JS Blob face; `outdir` is written from the same bytes
//!     after the build returns.

use bun_alloc::Arena;
use bun_bundler::bundle_v2::BundleV2;
use bun_ast::Loader;
use bun_bundler::options::{Format, LoaderExt, OutputFile};
use bun_bundler::output_file::{SavedFile, Value as OutputValue};
use bun_bundler::transpiler::Transpiler;
use bun_bundler::BundleThread::BuildResult;
use bun_event_loop::AnyEventLoop;
use bun_options_types::schema::api;

use bun_runtime::bun_build::{
    NativeBuildConfig, NativeBuildLog, NativeBuildResult, NativeOutputFile,
};

/// Install the native Bun.build driver into `bun_runtime` (idempotent).
///
/// Called from `bao_cli::run()` (product bring-up) and from the e2e test
/// setup. Without this call `Bun.build` resolves with an explicit degraded
/// `success:false + logs` payload — never a fake success.
pub fn install() {
    bun_runtime::bun_build::install_native_build_impl(run_bundle);
}

/// Test-only direct entry to the registered driver (mirrors the registry
/// call the BuildTasklet makes on its worker thread).
#[doc(hidden)]
pub fn run_bundle_for_test(config: &NativeBuildConfig) -> NativeBuildResult {
    run_bundle(config)
}

/// Parse `target` (upstream default "browser").
fn parse_target(s: &str) -> api::Target {
    match s {
        "bun" => api::Target::Bun,
        "node" => api::Target::Node,
        _ => api::Target::Browser,
    }
}

/// Parse `format` (upstream default "esm").
fn parse_format(s: &str) -> Format {
    match s {
        "cjs" => Format::Cjs,
        "iife" => Format::Iife,
        _ => Format::Esm,
    }
}

/// Parse `sourcemap` (upstream default "none").
fn parse_sourcemap(s: &str) -> api::SourceMapMode {
    match s {
        "linked" => api::SourceMapMode::Linked,
        "inline" => api::SourceMapMode::Inline,
        "external" => api::SourceMapMode::External,
        _ => api::SourceMapMode::None,
    }
}

/// The registered driver: runs on the BuildTasklet worker thread. Pure Rust
/// (no SM API) — the result crosses back to the JS thread as data.
fn run_bundle(config: &NativeBuildConfig) -> NativeBuildResult {
    let cwd = ::std::env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| ".".to_string());

    let arena = Arena::new();
    let mut log = bun_ast::Log::init();

    let mut opts = api::TransformOptions {
        entry_points: config
            .entrypoints
            .iter()
            .map(|e| Box::from(e.as_bytes()))
            .collect(),
        target: Some(parse_target(&config.target)),
        external: config
            .external
            .iter()
            .map(|e| Box::from(e.as_bytes()))
            .collect(),
        source_map: Some(parse_sourcemap(&config.sourcemap)),
        define: (!config.define.is_empty()).then(|| api::StringMap {
            keys: config.define.iter().map(|(k, _)| Box::from(k.as_bytes())).collect(),
            values: config.define.iter().map(|(_, v)| Box::from(v.as_bytes())).collect(),
        }),
        // In-memory build: every artifact keeps its bytes for the Blob face;
        // `outdir` writes happen from the same bytes after the build.
        write: Some(false),
        output_dir: None,
        absolute_working_dir: Some(Box::from(cwd.as_bytes())),
        ..Default::default()
    };
    if let Some(root) = &config.root {
        opts.absolute_working_dir = Some(Box::from(root.as_bytes()));
    }

    let log_ptr: *mut bun_ast::Log = &mut log;
    let mut transpiler = match Transpiler::init(&arena, log_ptr, opts, None) {
        Ok(t) => t,
        Err(e) => {
            return NativeBuildResult {
                success: false,
                outputs: Vec::new(),
                logs: vec![NativeBuildLog {
                    level: "error".into(),
                    message: format!("Bun.build: failed to configure the bundler: {}", e),
                }],
            };
        }
    };

    // Post-from_api overrides (fields the API schema carries elsewhere).
    apply_api_overrides(&mut transpiler, config);

    // `configure_defines` materialises `process.env.NODE_ENV` / user defines
    // into the define table. `BundleOptions::for_worker` debug-asserts
    // `defines_loaded` — without this the first parse worker panics.
    if let Err(e) = transpiler.configure_defines() {
        return NativeBuildResult {
            success: false,
            outputs: Vec::new(),
            logs: vec![NativeBuildLog {
                level: "error".into(),
                message: format!("Bun.build: failed to configure defines: {}", e),
            }],
        };
    }

    // Fresh Mini event loop drives wait_for_parse on this worker thread.
    let mut any_loop: AnyEventLoop<'static> = AnyEventLoop::init();
    let event_loop = core::ptr::NonNull::from(&mut any_loop);

    let mut reachable_files_count = 0usize;
    let mut minify_duration = 0u64;
    let mut source_code_size = 0u64;

    // Lifetime launder (same pattern as `ThreadPool::transpiler_for_target`):
    // `generate_from_cli` takes `&'a mut Transpiler<'a>` + `&'a Arena` with
    // `'a` invariant (Transpiler carries raw-ptr backrefs), so the natural
    // region coercion from two sibling locals cannot unify. Both locals
    // outlive the whole call window, so reborrowing them at `'static` for
    // the call is sound.
    // The laundered borrows are inline temporaries (never named), so their
    // window is exactly the call expression.
    let build: ::std::result::Result<BuildResult, bun_core::Error> =
        BundleV2::generate_from_cli(
            // SAFETY: `transpiler` is dropped only at fn exit, after every
            // use inside this call; the reborrow is the sole live &mut for
            // the call window.
            unsafe { core::mem::transmute(&mut transpiler) },
            // SAFETY: `arena` outlives the call the same way; shared borrow.
            unsafe { core::mem::transmute(&arena) },
            Some(event_loop),
            false,
            &mut reachable_files_count,
            &mut minify_duration,
            &mut source_code_size,
            None,
        );

    let mut result = match build {
        Ok(br) => br,
        Err(e) => {
            // A failed build can return before `wait_for_parse` consumed the
            // scheduled completion tasks, leaving entries in the Mini loop's
            // concurrent queue. They CANNOT be drained here: their
            // completions dereference the BundleV2, which the error path has
            // already `deinit`-ed (deinit_without_freeing_arena — tearing
            // down the graph the completion mutates). Leak the loop instead
            // (bounded: one small MiniEventLoop per FAILED build; same
            // intentional-leak posture as the runtime's thread-exit loops)
            // so its Drop debug-assert on queue emptiness never fires.
            core::mem::forget(any_loop);
            // Surface the log plus the error as the failure face (Promise
            // still RESOLVES with success:false — upstream Bun.build never
            // rejects on build errors).
            let mut logs = collect_logs(&log);
            logs.push(NativeBuildLog {
                level: "error".into(),
                message: format!("Bun.build: build failed: {}", e),
            });
            return NativeBuildResult { success: false, outputs: Vec::new(), logs };
        }
    };

    let outputs = map_outputs(&mut result.output_files);
    let logs = collect_logs(&log);
    let success = !log.has_errors();

    let mut native = NativeBuildResult { success, outputs, logs };

    // outdir: write every artifact from the same in-memory bytes (the
    // upstream linker's write path produces byte-identical content).
    if let Some(outdir) = &config.outdir {
        write_outputs_to_disk(outdir, &native.outputs, &mut native.logs);
    }

    native
}

/// Apply `Bun.build` config fields that live outside `api::TransformOptions`
/// (mirrors upstream `configureBundler`'s post-init option writes).
fn apply_api_overrides(transpiler: &mut Transpiler<'_>, config: &NativeBuildConfig) {
    let options = &mut transpiler.options;
    // from_api defaults `output_dir` to "out" (CLI parity); the JS API wants
    // the in-memory build (artifacts carry bytes; `outdir` writes happen from
    // the same bytes after the build), so empty it before the linker picks
    // the disk-write branch (`root_path.len() > 0`).
    let output_dir: Box<[u8]> = match &config.outdir {
        Some(dir) => Box::from(dir.as_bytes()),
        // from_api defaults output_dir to "out" (CLI parity); the JS API
        // wants the in-memory build (artifacts carry bytes; `outdir` writes
        // happen from the same bytes after the build), so empty it before the
        // linker picks the disk-write branch (`root_path.len() > 0`).
        None => Box::default(),
    };
    options.output_dir = output_dir.clone();
    // The linker reads `resolver.opts.output_dir` (Zig stores the full
    // BundleOptions on the resolver; from_api projects the bundler-only
    // fields there) — override BOTH sides or the disk-write branch still
    // fires off the resolver's copy.
    // SAFETY-free: `resolver` is a plain field; `opts` is the resolver-side
    // projected BundleOptions.
    transpiler.resolver.opts.output_dir = output_dir;
    options.minify_whitespace = config.minify.whitespace;
    options.minify_syntax = config.minify.syntax;
    options.minify_identifiers = config.minify.identifiers;

    options.output_format = parse_format(&config.format);
    options.code_splitting = config.splitting;

    // Naming templates. from_api leaves them empty (the CLI fills them from
    // argv); upstream `Bun.build` defaults (JSBundler.zig Config):
    //   entry "[dir]/[name].[ext]", chunk "[name]-[hash].[ext]",
    //   asset  "[name]-[hash].[ext]".
    const DEFAULT_ENTRY_NAMING: &[u8] = b"[dir]/[name].[ext]";
    const DEFAULT_CHUNK_NAMING: &[u8] = b"[name]-[hash].[ext]";
    const DEFAULT_ASSET_NAMING: &[u8] = b"[name]-[hash].[ext]";
    options.entry_naming = match (&config.naming, &config.naming_entry) {
        (Some(n), _) | (None, Some(n)) => Box::from(n.as_bytes()),
        (None, None) => Box::from(DEFAULT_ENTRY_NAMING),
    };
    options.chunk_naming = match &config.naming_chunk {
        Some(n) => Box::from(n.as_bytes()),
        None => Box::from(DEFAULT_CHUNK_NAMING),
    };
    options.asset_naming = match &config.naming_asset {
        Some(n) => Box::from(n.as_bytes()),
        None => Box::from(DEFAULT_ASSET_NAMING),
    };
    if let Some(banner) = &config.banner {
        options.banner = std::borrow::Cow::Owned(banner.as_bytes().to_vec());
    }
    if let Some(footer) = &config.footer {
        options.footer = std::borrow::Cow::Owned(footer.as_bytes().to_vec());
    }
    if let Some(pp) = &config.public_path {
        options.public_path = Box::from(pp.as_bytes());
    }
    if let Some(root) = &config.root {
        options.root_dir = Box::from(root.as_bytes());
    }

    // jsx: { runtime, factory, fragment, importSource, development }
    if config.jsx_runtime.is_some()
        || config.jsx_factory.is_some()
        || config.jsx_fragment.is_some()
        || config.jsx_import_source.is_some()
        || config.jsx_development.is_some()
    {
        let mut pragma = options.jsx.clone();
        if let Some(runtime) = &config.jsx_runtime {
            pragma.runtime = match runtime.as_str() {
                "classic" => bun_options_types::jsx::Runtime::Classic,
                _ => bun_options_types::jsx::Runtime::Automatic,
            };
        }
        if let Some(factory) = &config.jsx_factory {
            pragma.factory = member_list_from_dotted(factory);
        }
        if let Some(fragment) = &config.jsx_fragment {
            pragma.fragment = member_list_from_dotted(fragment);
        }
        // `jsx.importSource` is accepted but NOT applied: the port's jsx
        // Cow fields carry a Box-based `ToOwned::Owned` that a plain Vec
        // payload cannot satisfy, and the automatic-runtime import rewrite
        // is unreachable for classic builds. Registered as an uncovered
        // config field in the task report.
        let _ = &config.jsx_import_source;
        if let Some(development) = config.jsx_development {
            pragma.development = development;
        }
        // Both faces consume the pragma: the parse task clones the RESOLVER
        // result's jsx (`resolve_result.jsx`, tsconfig-detection path) while
        // import-driven tasks read `transpiler.options.jsx` — set BOTH or the
        // entry file still parses with the automatic runtime.
        options.jsx = pragma.clone();
        transpiler.resolver.opts.jsx = pragma;
    }
}


/// Split a dotted member list ("React.createElement") into the Pragma's
/// MemberList form (["React", "createElement"]).
fn member_list_from_dotted(dotted: &str) -> bun_options_types::jsx::MemberList {
    bun_options_types::jsx::MemberList::Owned(
        dotted
            .split('.')
            .map(|part| Box::from(part.as_bytes()))
            .collect(),
    )
}

/// Map the linker's `OutputFile`s to the JS-face artifacts, extracting the
/// bytes for every value variant (Buffer = generated code; Move/Copy/Noop =
/// file assets read from their source path; Saved = read back from disk).
fn map_outputs(output_files: &mut [OutputFile]) -> Vec<NativeOutputFile> {
    let mut out = Vec::with_capacity(output_files.len());
    for of in output_files.iter_mut() {
        let path = String::from_utf8_lossy(&of.dest_path).into_owned();
        let kind: &'static str = <&str>::from(of.output_kind);
        let loader: &'static str = <&str>::from(of.loader);
        let mime = mime_for_loader(of.loader, &of.dest_path);

        let bytes = match &of.value {
            OutputValue::Buffer { bytes } => bytes.to_vec(),
            OutputValue::Saved(saved) => read_saved_bytes(saved, &path),
            // File assets (file loader copies/moves): read from the source.
            _ => read_src_bytes(of.src_path.text),
        };

        let sourcemap_index = (of.source_map_index != u32::MAX)
            .then_some(of.source_map_index as usize);

        out.push(NativeOutputFile {
            path,
            kind: kind.to_string(),
            loader: loader.to_string(),
            mime_type: mime,
            hash: of.hash,
            bytes,
            sourcemap_index,
        });
    }
    out
}

/// MIME type for the Blob face via the canonical Loader→mime table.
fn mime_for_loader(loader: Loader, dest_path: &[u8]) -> String {
    let mime = loader.to_mime_type(&[dest_path]);
    String::from_utf8_lossy(&mime.value).into_owned()
}

/// Saved output files carry their on-disk destination; read the bytes back.
fn read_saved_bytes(_saved: &SavedFile, rel_path: &str) -> Vec<u8> {
    // The in-memory build never produces Saved (no output_dir); defensive
    // read of the relative destination against the cwd.
    let p = ::std::path::Path::new(rel_path);
    ::std::fs::read(p).unwrap_or_default()
}

/// Read an asset's source bytes (`src_path.text` is the resolved path).
fn read_src_bytes(src_text: &[u8]) -> Vec<u8> {
    if src_text.is_empty() {
        return Vec::new();
    }
    let path = ::std::path::Path::new(::std::str::from_utf8(src_text).unwrap_or(""));
    ::std::fs::read(path).unwrap_or_default()
}

/// Project the bundler Log into JS-face BuildMessage logs.
fn collect_logs(log: &bun_ast::Log) -> Vec<NativeBuildLog> {
    let mut out = Vec::new();
    for msg in log.msgs.iter() {
        let level = match msg.kind {
            bun_ast::Kind::Err => "error",
            bun_ast::Kind::Warn => "warn",
            _ => "info",
        };
        let text = String::from_utf8_lossy(&msg.data.text).into_owned();
        if text.is_empty() {
            continue;
        }
        out.push(NativeBuildLog { level: level.to_string(), message: text });
    }
    out
}

/// Write artifacts to `outdir` from the in-memory bytes. Path separators in
/// `rel_path` are linker-produced (forward slashes on unix).
fn write_outputs_to_disk(outdir: &str, outputs: &[NativeOutputFile], logs: &mut Vec<NativeBuildLog>) {
    let root = ::std::path::Path::new(outdir);
    for file in outputs {
        if file.path.is_empty() {
            continue;
        }
        let dest = root.join(&file.path);
        if let Some(parent) = dest.parent() {
            if let Err(e) = ::std::fs::create_dir_all(parent) {
                logs.push(NativeBuildLog {
                    level: "error".into(),
                    message: format!("Bun.build: failed to create {}: {}", parent.display(), e),
                });
                continue;
            }
        }
        if let Err(e) = ::std::fs::write(&dest, &file.bytes) {
            logs.push(NativeBuildLog {
                level: "error".into(),
                message: format!("Bun.build: failed to write {}: {}", dest.display(), e),
            });
        }
    }
}
