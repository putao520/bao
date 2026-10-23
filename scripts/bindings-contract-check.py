#!/usr/bin/env python3
"""bindings-contract-check.py — codegen 契约静态门禁(杀「盲写 DOM impl」类错误)

codegen 是纯 Python,可脱离 cargo 运行(加载链镜像 servo codegen run.py:全语料
parse + finish + Configuration + CGBindingRoot,实测全语料 <1s)。本工具把生成物
`pub trait XMethods` 当唯一契约真源,对 impl 文件做静态 diff。

用法:
  # 写前读契约:打印目标 webidl 文件全部接口的 trait 方法签名清单
  python3 -I scripts/bindings-contract-check.py --contract EditContext.webidl

  # 写后静态验收:impl 文件 vs 生成 trait 的方法集 diff + cx/Fallible 三态核对
  python3 -I scripts/bindings-contract-check.py --check src/.../editcontext.rs
  python3 -I scripts/bindings-contract-check.py --check foo.rs --webidl EditContext.webidl

  # 判别力自检(3 绿 + 2 红形态 + YELLOW import 反模式)
  python3 -I scripts/bindings-contract-check.py --selftest

退出码:0=GREEN  1=RED  2=错误(接口定位失败/生成失败/自检失败)
YELLOW(import 反模式/动态宏)不改变退出码,只在报告中标注。

判别面:
  * 方法集 diff:缺失(缺 trait 项/漏 event_handler!)/多余(Get 前缀错名)= RED
  * 签名三态:`cx: &mut` 有无 / `-> Fallible<>` 有无 与 trait 不符 = RED
  * impl 深度 1 的宏调用(event_handler!/make_getter! 族/global_event_handlers! 复合宏)
    按 script/dom/macros.rs 的宏定义递归展开还原方法名;无法展开的动态宏降级 YELLOW
  * import 反模式表:cell::Cell / 裸 codegen:: / 裸 reflector:: / 非 js:: 的
    HandleObject → YELLOW

限制(诚实边界):纯静态字符串级,不做类型推导;不解析 trait 泛型特化差异;
  动态宏(css_properties_accessors! 等)不可静态展开的 impl 块缺失项不判 RED;
  feature-gate 死桩(gpucanvascontext 类 webgpu stub,靠 mod 层 cfg 挡在编译外)
  会如实报 RED——契约违约真实存在,是否可编译属 feature 图知识,非字符串级职责。
"""
import os
import re
import sys
import argparse

sys.dont_write_bytecode = True

# ---------------------------------------------------------------------------
# codegen 加载链(镜像 /tmp/enumprobe/probe.py 实证形态)
# ---------------------------------------------------------------------------
SB_ROOT = os.environ.get(
    "BCC_SCRIPT_BINDINGS",
    "/home/putao/code/rust/bao/vendor/servo/components/script_bindings",
)
CODEGEN_DIR = os.path.join(SB_ROOT, "codegen")
WEBIDLS_DIR = os.path.join(SB_ROOT, "webidls")
PARSER_DIR = os.path.join(SB_ROOT, "third_party", "WebIDL", "parser")
PLY_DIR = os.path.join(SB_ROOT, "third_party", "ply")
MACROS_RS = os.environ.get(
    "BCC_MACROS_RS",
    "/home/putao/code/rust/bao/vendor/servo/components/script/dom/macros.rs",
)
CACHE_DIR = os.environ.get("BCC_CACHE", "/tmp/bao-bindings-contract-cache")


class Sig:
    """一个 trait 方法签名(名称 + 归一化文本 + 三态标志)。"""

    __slots__ = ("name", "text", "has_cx", "fallible")

    def __init__(self, name, text):
        self.name = name
        self.text = normalize_sig(text)
        # 三态之一「引擎句柄参」:SM153 fork 契约首参为 `realm: &mut CurrentRealm`
        # 或传统 `cx: &mut JSContext`——两者同属「引擎上下文可变句柄」,有无须一致
        self.has_cx = bool(re.search(r"&mut\s*(?:\w+::)*(?:JSContext|CurrentRealm)\b", text))
        # ErrorResult 是 Fallible<()> 的别名(script_bindings/error.rs:96);
        # 裸 `Result<T, Error>` 是 Fallible<T> 的原形——类型层面三者等价
        self.fallible = bool(
            re.search(r"->\s*(?:Fallible<|ErrorResult\b|(?:\w+::)*Result<[^;]*,\s*(?:\w+::)*Error\s*>)", text)
        )

    def flags(self):
        f = []
        if self.has_cx:
            f.append("cx")
        if self.fallible:
            f.append("fallible")
        if "Option<" in self.text:
            f.append("optional")
        return f

    def display(self):
        body = self.text if self.text.startswith("fn ") else "fn %s%s" % (self.name, self.text)
        return "%s%s" % (body, ("  [%s]" % ",".join(self.flags())) if self.flags() else "")


def normalize_sig(text):
    """归一化签名文本:去 r# 前缀、压空白、压 `() `空参形态;保护 `::` 不被压散。"""
    t = text.replace("r#", "")
    t = t.replace("::", "\x00")
    t = re.sub(r"\s+", " ", t).strip()
    t = re.sub(r"\(\s+\)", "()", t)
    t = re.sub(r"\(\s*", "(", t)
    t = re.sub(r",\s*\)", ")", t)
    t = re.sub(r"\s*\)", ")", t)
    t = re.sub(r"\s*,\s*", ", ", t)
    t = re.sub(r"\s*->\s*", " -> ", t)
    t = re.sub(r"\s*:\s*", ": ", t)
    t = t.replace("\x00", "::")
    return t.rstrip(";").strip()


# ---------------------------------------------------------------------------
# 轻量 Rust 词法:括号深度扫描 + 字符串/字符/注释跳过
# ---------------------------------------------------------------------------
CHAR_LITERAL_RE = re.compile(r"'(\\.|\\x[0-9a-fA-F]{2}|\\u\{[0-9a-fA-F_]{1,6}\}|[^\\'])'")


def strip_r_hash(name):
    """去掉 raw identifier 的 r# 前缀(精确前缀,不用 lstrip 字符集)。"""
    return re.sub(r"^r#", "", name)


def skip_string_or_comment(text, i):
    """若 text[i:] 起始是字符串/字符/注释则跳过整个字素,返回新 index 或 None。"""
    c = text[i]
    if c == "/" and i + 1 < len(text):
        if text[i + 1] == "/":
            j = text.find("\n", i)
            return len(text) if j == -1 else j + 1
        if text[i + 1] == "*":
            j = text.find("*/", i + 2)
            return len(text) if j == -1 else j + 2
    if c == '"':
        j = i + 1
        while j < len(text):
            if text[j] == "\\":
                j += 2
                continue
            if text[j] == '"':
                return j + 1
            j += 1
        return len(text)
    if c == "'":
        # 字符字面量('\n' 'x' '\'')vs lifetime('a):仅当闭合引号形态成立才算字面量
        m = CHAR_LITERAL_RE.match(text, i)
        if m:
            return m.end()
        return None  # lifetime tick,单字符前进
    return None


def depth_walk(text, start, end):
    """产出 (index, depth) 序列——text[start:end] 每个非字素位置所处的花括号深度。

    depth 从 0 起算(调用方通常传入 impl 块体,体本身相对深度 0)。
    """
    depth = 0
    i = start
    out = []
    while i < end:
        j = skip_string_or_comment(text, i)
        if j is not None:
            i = j
            continue
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
        out.append((i, depth, c))
        i += 1
    return out


def match_balanced(text, open_idx, open_ch="{", close_ch="}"):
    """从 open_idx(指向 open_ch)找到配对 close 的 index。"""
    depth = 0
    i = open_idx
    n = len(text)
    while i < n:
        j = skip_string_or_comment(text, i)
        if j is not None:
            i = j
            continue
        c = text[i]
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def match_balanced_generic(text, open_idx):
    """尖括号配对(容忍嵌套泛型);遇到明显非泛型字符提前放弃返回 -1。"""
    depth = 0
    i = open_idx
    n = len(text)
    while i < n:
        c = text[i]
        if c == "<":
            depth += 1
        elif c == ">":
            depth -= 1
            if depth == 0:
                return i
        elif c == "(":
            j = match_balanced(text, i, "(", ")")
            if j == -1:
                return -1
            i = j
        elif c in ";" + "{":
            return -1
        i += 1
    return -1


def split_top_level(args_text, sep=","):
    """按顶层 sep 切分(括号/尖括号/字符串感知)。"""
    parts = []
    depth = 0
    cur = []
    i = 0
    n = len(args_text)
    while i < n:
        j = skip_string_or_comment(args_text, i)
        if j is not None:
            cur.append(args_text[i:j])
            i = j
            continue
        c = args_text[i]
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
        if c == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if cur:
        parts.append("".join(cur))
    return [p.strip() for p in parts]


# ---------------------------------------------------------------------------
# 宏定义解释器:从 macros.rs 还原「调用点 → 生成方法名」
# ---------------------------------------------------------------------------
class MacroRule:
    __slots__ = ("tokens", "body", "fn_slots", "exact")

    def __init__(self, tokens, body, exact=True):
        self.tokens = tokens  # [("var", name) | ("lit", text)] 序列(重复段截断)
        self.exact = exact  # 无重复段 → 实参数须与 pattern token 数精确相等
        self.body = body
        self.fn_slots = set(re.findall(r"\bfn\s+\$(\w+)\b", body))

    def matches(self, args):
        """pattern tokens 与调用实参匹配:字面量须相等,变量按位绑定。

        非 exact(重复段截断)才允许前缀匹配;否则空 pattern () 不会吞掉带参调用。
        """
        if self.exact and len(self.tokens) != len(args):
            return None
        if len(self.tokens) > len(args):
            return None
        bind = {}
        for (kind, val), arg in zip(self.tokens, args):
            if kind == "var":
                bind[val] = arg.strip()
            elif val != arg.strip():
                return None
        return bind


def _parse_macro_rules(src):
    """解析 macro_rules! NAME ( arms ); → {name: [MacroRule,...]}。

    arm 形态:(pattern) => (body);——pattern 提取顶层前缀 flat 变量序列,
    遇 $(...) 重复段即止(方法名位参都在重复段之前,实证于全部相关宏)。
    """
    rules = {}
    for m in re.finditer(r"macro_rules!\s+(\w+)\s*\(", src):
        name = m.group(1)
        open_idx = m.end() - 1
        close_idx = match_balanced(src, open_idx, "(", ")")
        if close_idx == -1:
            continue
        arms_text = src[open_idx + 1 : close_idx]
        arms = []
        # 逐 arm:第一个顶层 (...) 是 pattern,=> 后第一个顶层定界体是 body
        # (Rust 允许展开体用 (...) / {...} / [...] 三种定界,委托臂惯用 {...})
        i = 0
        n = len(arms_text)
        while i < n:
            while i < n and arms_text[i] in " \t\n\r;":
                i += 1
            if i >= n:
                break
            if arms_text[i] != "(":
                # #[macro_export] 等属性已在外层;非 ( 开头即坏 arm,跳过
                i += 1
                continue
            p_close = match_balanced(arms_text, i, "(", ")")
            if p_close == -1:
                break
            pattern = arms_text[i + 1 : p_close]
            j = p_close + 1
            while j < n and arms_text[j] in " \t\n\r":
                j += 1
            if arms_text[j : j + 2] == "=>":
                j += 2
                while j < n and arms_text[j] in " \t\n\r":
                    j += 1
                if j < n and arms_text[j] in "([{":
                    open_ch = arms_text[j]
                    close_ch = {"(": ")", "{": "}", "[": "]"}[open_ch]
                    b_close = match_balanced(arms_text, j, open_ch, close_ch)
                    if b_close != -1:
                        arms.append((pattern, arms_text[j + 1 : b_close]))
                        i = b_close + 1
                        continue
            i = p_close + 1
        # pattern → token 序列(变量/字面量),重复段截断
        parsed = []
        for pattern, body in arms:
            tokens = []
            exact = True
            for tok in split_top_level(pattern):
                mm = re.fullmatch(r"\$\s*(\w+)\s*:\s*(\w+)", tok)
                if mm:
                    tokens.append(("var", mm.group(1)))
                elif tok.startswith("$("):
                    exact = False  # 重复段:方法名位参都在重复段之前,截断
                    break
                else:
                    tokens.append(("lit", tok))
            parsed.append(MacroRule(tokens, body, exact))
        if parsed:
            rules[name] = parsed
    return rules


# 无法静态展开的动态宏:方法名由数据派生(css 属性名等),命中即降级 YELLOW
DYNAMIC_MACROS = {"css_properties_accessors"}


class MacroExpander:
    def __init__(self, rules):
        self.rules = rules
        self.warnings = []

    def expand(self, name, args, depth=0):
        """返回 (method_names:set, expandable:bool)。"""
        if depth > 6:
            self.warnings.append("宏 %s! 展开深度超限" % name)
            return set(), False
        arms = self.rules.get(name)
        if not arms:
            return set(), False
        arm = None
        bind = None
        for cand in arms:
            bind = cand.matches(args)
            if bind is not None:
                arm = cand
                break
        if arm is None:
            return set(), False
        names = set()
        expandable = True
        for slot in arm.fn_slots:
            if slot in bind:
                names.add(strip_r_hash(bind[slot]).strip())
            else:
                expandable = False  # fn 槽位变元未绑定(重复段位参)——不可全展开
        # 体内嵌套宏调用:先代入绑定,再递归展开(未知嵌套宏只代表无名可收,不影响可展开性)
        body = re.sub(r"\$(\w+)", lambda m: bind.get(m.group(1), m.group(0)), arm.body)
        for nm in re.finditer(r"\b([a-z_]\w*)!\s*\(", body):
            mname = nm.group(1)
            if mname not in self.rules or mname in DYNAMIC_MACROS:
                continue
            arg_open = nm.end() - 1
            arg_close = match_balanced(body, arg_open, "(", ")")
            if arg_close == -1:
                continue
            raw_args = split_top_level(body[arg_open + 1 : arg_close])
            sub_args = [
                re.sub(r"\$(\w+)", lambda m: bind.get(m.group(1), m.group(0)), a).strip()
                for a in raw_args
            ]
            sub_names, sub_ok = self.expand(mname, sub_args, depth + 1)
            names |= sub_names
            if not sub_ok:
                expandable = False
        return names, expandable


# ---------------------------------------------------------------------------
# codegen 引擎:全语料 parse → 单文件生成 → trait 方法集提取
# ---------------------------------------------------------------------------
class CodegenEngine:
    def __init__(self):
        self._parser = None
        self._config = None

    def _ensure(self):
        if self._config is not None:
            return
        for p in (PLY_DIR, PARSER_DIR, CODEGEN_DIR):
            sys.path.insert(0, p)
        import WebIDL  # noqa
        from configuration import Configuration  # noqa
        import codegen  # noqa

        os.makedirs(CACHE_DIR, exist_ok=True)
        parser = WebIDL.Parser(CACHE_DIR)
        if not os.path.isdir(WEBIDLS_DIR):
            raise RuntimeError("webidls 目录不存在: %s" % WEBIDLS_DIR)
        for f in sorted(os.listdir(WEBIDLS_DIR)):
            if f.endswith(".webidl"):
                path = os.path.join(WEBIDLS_DIR, f)
                with open(path, encoding="utf-8") as fh:
                    parser.parse(fh.read(), path)
        self._config = Configuration(os.path.join(CODEGEN_DIR, "Bindings.conf"), parser.finish())
        self._codegen = codegen

    def generate(self, webidl_path):
        """返回 {trait_name: {"generic": str, "methods": [Sig]}}(该文件全部接口)。"""
        self._ensure()
        root = self._codegen.CGBindingRoot(self._config, "Bindings", webidl_path)
        out = root.define()
        traits = {}
        for m in re.finditer(r"\bpub\s+trait\s+(\w+Methods)\b", out):
            tname = m.group(1)
            body_open = out.find("{", m.end())
            if body_open == -1:
                continue
            body_close = match_balanced(out, body_open)
            if body_close == -1:
                continue
            if tname in traits:
                continue
            body = out[body_open + 1 : body_close]
            sigs = []
            for fm in re.finditer(r"\bfn\s+(r#\w+|\w+)\s*\(", body):
                open_paren = fm.end() - 1
                close_paren = match_balanced(body, open_paren, "(", ")")
                if close_paren == -1:
                    continue
                semi = body.find(";", close_paren)
                seg = body[fm.start() : (semi + 1) if semi != -1 else close_paren + 1]
                sigs.append(Sig(strip_r_hash(fm.group(1)), seg))
            traits[tname] = {
                "generic": "",
                "methods": sigs,
            }
        # trait 泛型头(仅展示用)
        for m in re.finditer(r"pub\s+trait\s+(\w+Methods)(<[^{]*>)?\s*\{", out):
            if m.group(1) in traits and not traits[m.group(1)]["generic"]:
                traits[m.group(1)]["generic"] = (m.group(2) or "").strip()
        return traits

    def find_defining_file(self, interface):
        """接口名 → 定义它的 webidl 路径(优先非 partial 定义,退回任意提及)。"""
        full = re.compile(r"(?<!partial\s)\binterface\s+%s\b" % re.escape(interface))
        partial = re.compile(r"\bpartial\s+interface\s+%s\b" % re.escape(interface))
        non_partial_hits, partial_hits = [], []
        for f in sorted(os.listdir(WEBIDLS_DIR)):
            if not f.endswith(".webidl"):
                continue
            path = os.path.join(WEBIDLS_DIR, f)
            with open(path, encoding="utf-8") as fh:
                t = fh.read()
            if full.search(t):
                non_partial_hits.append(path)
            elif partial.search(t):
                partial_hits.append(path)
        if non_partial_hits:
            return non_partial_hits[0]
        if partial_hits:
            return partial_hits[0]
        return None


# ---------------------------------------------------------------------------
# impl 文件扫描:impl XMethods 块 → 方法集(含宏展开)
# ---------------------------------------------------------------------------
class ImplBlock:
    __slots__ = ("interface", "for_type", "methods", "unknown_macros", "dynamic", "span")

    def __init__(self, interface, for_type, span):
        self.interface = interface
        self.for_type = for_type
        self.methods = {}
        self.unknown_macros = []
        self.dynamic = False
        self.span = span


def find_impl_blocks(text):
    blocks = []
    for m in re.finditer(r"\bimpl\b", text):
        i = m.end()
        n = len(text)
        # 可选 impl<...> 泛型
        while i < n and text[i] in " \t\n\r":
            i += 1
        if i < n and text[i] == "<":
            close = match_balanced_generic(text, i)
            if close == -1:
                continue
            i = close + 1
            while i < n and text[i] in " \t\n\r":
                i += 1
        tm = re.match(r"((?:\w+::)*)(\w+Methods)\b", text[i:])
        if not tm:
            continue
        trait_name = tm.group(2)
        interface = trait_name[: -len("Methods")]
        j = i + tm.end()
        while j < n and text[j] in " \t\n\r":
            j += 1
        if j < n and text[j] == "<":
            close = match_balanced_generic(text, j)
            if close == -1:
                continue
            j = close + 1
            while j < n and text[j] in " \t\n\r":
                j += 1
        fm = re.match(r"for\s+(\w+)", text[j:])
        if not fm:
            continue
        for_type = fm.group(1)
        body_open = text.find("{", j + fm.end())
        if body_open == -1:
            continue
        body_close = match_balanced(text, body_open)
        if body_close == -1:
            continue
        blocks.append(
            ImplBlock(
                interface,
                for_type,
                (body_open + 1, body_close),
            )
        )
    return blocks


def capture_impl_signature(text, fn_start):
    """从 `fn` 起扫描签名文本,到括号深度 0 的 `{` 或 `;` 止。"""
    i = fn_start
    paren = 0
    n = len(text)
    while i < n:
        j = skip_string_or_comment(text, i)
        if j is not None:
            i = j
            continue
        c = text[i]
        if c == "(":
            paren += 1
        elif c == ")":
            paren -= 1
        elif c in "{;" and paren == 0:
            return text[fn_start:i]
        i += 1
    return text[fn_start:n]


def scan_impl_block_methods(text, block, expander):
    """在 impl 块体(块自身花括号之外)内收集:直接成员(体相对深度 0)的 fn 与宏展开方法。"""
    start, end = block.span
    depth = 0
    i = start
    while i < end:
        j = skip_string_or_comment(text, i)
        if j is not None:
            i = j
            continue
        c = text[i]
        if c == "{":
            depth += 1
            i += 1
            continue
        if c == "}":
            depth -= 1
            i += 1
            continue
        if depth != 0:
            i += 1
            continue
        fm = re.match(r"\bfn\s+(r#\w+|\w+)\b", text[i:])
        if fm:
            name = strip_r_hash(fm.group(1))
            sig = capture_impl_signature(text, i)
            block.methods.setdefault(name, Sig(name, sig))
            i += fm.end()
            continue
        mm = re.match(r"\b([a-z_]\w*)!\s*\(", text[i:])
        if mm:
            mname = mm.group(1)
            arg_open = i + mm.end() - 1
            arg_close = match_balanced(text, arg_open, "(", ")")
            if arg_close == -1:
                i += mm.end()
                continue
            args = split_top_level(text[arg_open + 1 : arg_close])
            if mname in DYNAMIC_MACROS:
                block.dynamic = True
            else:
                names, ok = expander.expand(mname, args)
                for nm in names:
                    nm = strip_r_hash(nm).strip()
                    if re.fullmatch(r"[A-Za-z_]\w*", nm):
                        block.methods.setdefault(nm, Sig(nm, "(macro %s!)" % mname))
                if not ok:
                    block.unknown_macros.append(mname)
            i = arg_close + 1
            continue
        i += 1


# ---------------------------------------------------------------------------
# import 反模式真源表(YELLOW)
# ---------------------------------------------------------------------------
IMPORT_RULES = [
    (
        re.compile(r"(?<!std::)(?<!::)\bcell::Cell\b"),
        "Cell 必须来自 std::cell::Cell(script_bindings::cell 仅导出 DomRefCell/Ref/RefCell/RefMut,无 Cell)",
    ),
    (
        re.compile(r"^\s*use\s+(?:crate::)?codegen::", re.M),
        "codegen 真源路径 = crate::dom::bindings::codegen::Bindings(裸 codegen:: 是盲写路径)",
    ),
    (
        re.compile(r"^\s*use\s+(?:crate::)?reflector::|^\s*use\s+script_bindings::Reflector\b", re.M),
        "reflector 域真源 = script_bindings::reflector::* 或 crate::dom::bindings::reflector::*",
    ),
    (
        re.compile(r"^\s*use\s+(?!(?:js|mozjs)::)[^;\n]*\bHandleObject\b", re.M),
        "HandleObject 真源 = js(mozjs)::rust / jsapi 路径(如 use js::rust::HandleObject)",
    ),
]


def check_imports(text):
    hits = []
    for pat, msg in IMPORT_RULES:
        m = pat.search(text)
        if m:
            line_no = text.count("\n", 0, m.start()) + 1
            hits.append((line_no, m.group(0).strip()[:60], msg))
    return hits


# ---------------------------------------------------------------------------
# diff 与报告
# ---------------------------------------------------------------------------
def diff_block(block, required_sigs, findings):
    actual = set(block.methods)
    required = {s.name for s in required_sigs}
    missing = sorted(required - actual)
    extra = sorted(actual - required)
    for name in missing:
        findings.append(("RED", "%s for %s: 缺失 trait 方法 fn %s" % (block.interface, block.for_type, name)))
    for name in extra:
        findings.append(
            ("RED", "%s for %s: 多余方法 fn %s(trait 无此项——Get/Set 前缀错名或自创方法)" % (block.interface, block.for_type, name))
        )
    req_by_name = {s.name: s for s in required_sigs}
    for name in sorted(actual & required):
        impl_sig = block.methods[name]
        trait_sig = req_by_name[name]
        if impl_sig.text.startswith("(macro"):
            continue  # 宏展开名只有名称证据,签名核对以手写项为准
        if impl_sig.has_cx != trait_sig.has_cx:
            findings.append(
                (
                    "RED",
                    "%s for %s: fn %s cx 参数三态不符(impl %s / trait %s)"
                    % (
                        block.interface,
                        block.for_type,
                        name,
                        "有 cx: &mut" if impl_sig.has_cx else "无 cx",
                        "有 cx: &mut" if trait_sig.has_cx else "无 cx",
                    ),
                )
            )
        if impl_sig.fallible != trait_sig.fallible:
            findings.append(
                (
                    "RED",
                    "%s for %s: fn %s 返回 Fallible 三态不符(impl %s / trait %s)"
                    % (
                        block.interface,
                        block.for_type,
                        name,
                        "-> Fallible<>" if impl_sig.fallible else "裸返回",
                        "-> Fallible<>" if trait_sig.fallible else "裸返回",
                    ),
                )
            )
    return missing, extra


def run_check(impl_path, engine, expander, webidl_override=None, verbose=False):
    findings = []
    errors = []
    with open(impl_path, encoding="utf-8") as f:
        text = f.read()
    blocks = find_impl_blocks(text)
    if not blocks:
        print("YELLOW: %s 中未发现 impl *Methods 块(仅 import 反模式核查)" % impl_path)
    verdict = "GREEN"
    for block in blocks:
        webidl = webidl_override or engine.find_defining_file(block.interface)
        if not webidl:
            # 非 codegen trait(VirtualMethods/FragmentMethods 等手写 trait)——非本工具管辖,跳过
            findings.append(("SKIP", "%s for %s: 接口未在 webidls/ 定义(手写 trait,跳过)" % (block.interface, block.for_type)))
            continue
        try:
            traits = engine.generate(webidl)
        except Exception as exc:  # noqa
            errors.append("codegen 生成 %s 失败: %s: %s" % (webidl, type(exc).__name__, exc))
            continue
        required = traits.get(block.interface + "Methods")
        if required is None:
            errors.append(
                "生成物 %s 中无 pub trait %sMethods(%s 不定义该接口?)" % (os.path.basename(webidl), block.interface, os.path.basename(webidl))
            )
            continue
        scan_impl_block_methods(text, block, expander)
        missing, extra = diff_block(block, required["methods"], findings)
        if block.dynamic:
            findings.append(
                ("YELLOW", "%s for %s: 含动态宏,缺失项不判 RED(方法集 diff 不完备)" % (block.interface, block.for_type))
            )
        for um in block.unknown_macros:
            findings.append(
                ("YELLOW", "%s for %s: 宏 %s! 无法静态展开(缺失项可能漏报)" % (block.interface, block.for_type, um))
            )
        if verbose:
            print("--- impl 块 %s for %s (契约 %s, %d 方法) ---" % (block.interface, block.for_type, os.path.basename(webidl), len(required["methods"])))
            print("  missing: %s" % (missing or "无"))
            print("  extra:   %s" % (extra or "无"))
            print("  impl 方法数: %d(含宏展开)" % len(block.methods))
    for line_no, snippet, msg in check_imports(text):
        findings.append(("YELLOW", "%s:%d import 反模式 `%s` — %s" % (impl_path, line_no, snippet, msg)))
    if errors:
        for e in errors:
            print("ERROR: %s" % e)
        return 2
    seen = set()
    for level, msg in findings:
        if msg in seen:
            continue
        seen.add(msg)
        print("%s: %s" % (level, msg))
        if level == "RED":
            verdict = "RED"
    print("RESULT(%s): %s" % (os.path.basename(impl_path), verdict))
    return 0 if verdict == "GREEN" else 1


def run_contract(webidl_paths, engine):
    rc = 0
    for path in webidl_paths:
        if not os.path.isabs(path):
            cand = os.path.join(WEBIDLS_DIR, path)
            path = cand if os.path.exists(cand) else path
        try:
            traits = engine.generate(path)
        except Exception as exc:  # noqa
            print("ERROR: 生成 %s 失败: %s: %s" % (path, type(exc).__name__, exc))
            rc = 2
            continue
        print("===== %s (%d trait) =====" % (path, len(traits)))
        for tname in sorted(traits):
            info = traits[tname]
            print("--- trait %s%s (%d 方法) ---" % (tname, info["generic"], len(info["methods"])))
            for s in info["methods"]:
                print("  %s" % s.display())
    return rc


# ---------------------------------------------------------------------------
# 自检:3 绿 + 2 红形态 + YELLOW
# ---------------------------------------------------------------------------
GREEN_SAMPLES = [
    ("/home/putao/code/rust/bao/vendor/servo/components/script/dom/editcontext.rs", None),
    ("/home/putao/code/rust/bao/vendor/servo/components/script/dom/webvtt/vttcue.rs", None),
    ("/home/putao/code/rust/bao/vendor/servo/components/script/dom/textupdateevent.rs", None),
]


def _red_sample_missing_geton():
    """红形态 1:缺 GetOn——event_handler! 行被漏写(textupdate 缺席)。"""
    return """// synthetic RED sample: missing GetOntextupdate/SetOntextupdate
use codegen::Bindings::EditContextBinding::EditContextMethods;

impl EditContextMethods<crate::DomTypeHolder> for EditContext {
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        options: &EditContextInit,
    ) -> DomRoot<EditContext> {
        EditContext::new(cx, window, proto, options.text.clone(), options.selectionStart, options.selectionEnd)
    }

    event_handler!(textformatupdate, GetOntextformatupdate, SetOntextformatupdate);
    event_handler!(
        characterboundsupdate,
        GetOncharacterboundsupdate,
        SetOncharacterboundsupdate
    );

    fn GetText(&self) -> DOMString {
        self.text.borrow().clone()
    }

    fn SetText(&self, value: DOMString) {
        *self.text.borrow_mut() = value;
    }

    fn GetSelectionStart(&self) -> u32 {
        self.selection_start.get()
    }

    fn handle_editing_action(&self, action: EditingAction) -> bool {
        false
    }
}
"""


def _red_sample_wrong_prefix():
    """红形态 2:Get 前缀错名——`fn getOntextupdate` 小写 g 开头,trait 是 `fn GetOntextupdate`。"""
    return """// synthetic RED sample: wrong-case Get-prefix name (getOntextupdate instead of GetOntextupdate)
impl EditContextMethods<crate::DomTypeHolder> for EditContext {
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        options: &EditContextInit,
    ) -> DomRoot<EditContext> {
        EditContext::new(cx, window, proto, options.text.clone(), options.selectionStart, options.selectionEnd)
    }

    fn getOntextupdate(&self, cx: &mut JSContext) -> Option<RootedCallback<EventHandlerNonNull>> {
        None
    }
}
"""


def run_selftest(engine, expander, tmpdir="/tmp/bcc-selftest"):
    os.makedirs(tmpdir, exist_ok=True)
    failures = []

    def expect(name, path, want_rc, want_substrings):
        rc = 0
        import io
        import contextlib

        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            try:
                rc = run_check(path, engine, expander)
            except Exception as exc:  # noqa
                print("selftest exception: %r" % exc)
                rc = 2
        out = buf.getvalue()
        ok = rc == want_rc and all(s in out for s in want_substrings)
        print("[%s] %s (rc=%d, want %d)" % ("PASS" if ok else "FAIL", name, rc, want_rc))
        if not ok:
            failures.append(name)
            print("--- 实际输出 ---")
            print(out)

    print("== 自检 1/5:绿样本 editcontext.rs ==")
    expect("GREEN editcontext.rs", GREEN_SAMPLES[0][0], 0, ["RESULT"])
    print("== 自检 2/5:绿样本 vttcue.rs ==")
    expect("GREEN vttcue.rs", GREEN_SAMPLES[1][0], 0, ["RESULT"])
    print("== 自检 3/5:绿样本 textupdateevent.rs ==")
    expect("GREEN textupdateevent.rs", GREEN_SAMPLES[2][0], 0, ["RESULT"])

    p1 = os.path.join(tmpdir, "red_missing_geton.rs")
    with open(p1, "w", encoding="utf-8") as f:
        f.write(_red_sample_missing_geton())
    print("== 自检 4/5:红形态 缺 GetOn ==")
    expect(
        "RED missing-GetOn",
        p1,
        1,
        ["缺失 trait 方法 fn GetOntextupdate", "缺失 trait 方法 fn SetOntextupdate", "import 反模式"],
    )

    p2 = os.path.join(tmpdir, "red_wrong_prefix.rs")
    with open(p2, "w", encoding="utf-8") as f:
        f.write(_red_sample_wrong_prefix())
    print("== 自检 5/5:红形态 Get 前缀错名 ==")
    expect(
        "RED wrong-prefix",
        p2,
        1,
        ["缺失 trait 方法 fn GetOntextupdate", "多余方法 fn getOntextupdate"],
    )

    print("== 自检结果: %s ==" % ("全部 PASS" if not failures else "FAIL: %s" % failures))
    return 0 if not failures else 1


# ---------------------------------------------------------------------------
def main(argv=None):
    ap = argparse.ArgumentParser(description="codegen 契约静态门禁(--contract 读契约 / --check 验收 / --selftest)")
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--contract", nargs="+", metavar="WEBIDL", help="打印 webidl 文件全部接口的 trait 方法签名契约")
    g.add_argument("--check", metavar="IMPL_RS", help="对 impl 文件做 trait 契约静态 diff")
    g.add_argument("--selftest", action="store_true", help="内置判别力自检(3 绿 + 2 红形态)")
    ap.add_argument("--webidl", metavar="FILE", help="--check 时显式指定接口定义 webidl(默认自动定位)")
    ap.add_argument("-v", "--verbose", action="store_true", help="打印每块 diff 明细")
    args = ap.parse_args(argv)

    engine = CodegenEngine()
    try:
        with open(MACROS_RS, encoding="utf-8") as f:
            expander = MacroExpander(_parse_macro_rules(f.read()))
    except OSError as exc:
        print("ERROR: 读宏定义失败 %s: %s" % (MACROS_RS, exc))
        return 2

    if args.selftest:
        return run_selftest(engine, expander)
    if args.contract:
        return run_contract(args.contract, engine)
    if args.check:
        return run_check(args.check, engine, expander, webidl_override=args.webidl, verbose=args.verbose)
    return 2


if __name__ == "__main__":
    sys.exit(main())
