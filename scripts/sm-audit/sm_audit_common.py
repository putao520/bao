#!/usr/bin/env python3
"""SM-EVOLUTION #30 UpstreamAudit 共享工具(定位/基线/分类)

背景:
    mozjs 上游前移时需要自动回答「SM 新增/删除/改变了哪些 embedding
    capability?Bao 用没用?」(#30)。三脚本(extract_inventory / usage_map /
    drift)共用:仓库定位、bindgen jsapi.rs 发现、基线元数据(bao commit +
    mozjs 版本 + jsapi md5)、capability 域猜测规则。

capability 域对齐 .claude/sm-capability-ledger.json 的 13 域(人工裁决层),
本模块只做 symbol→域的启发式猜测(报告字段名 category_guess,禁当裁决)。

用法:
    由 scripts/sm-audit/ 三脚本 import;不单独执行。
"""

import glob
import hashlib
import os
import re
import subprocess
import datetime

REPO_HINT = "scripts/sm-audit"

# 13 ledger 域(顺序即匹配优先级;support 为非 SM 面标记,非 capability 域)
CATEGORY_RULES = [
    ("wasm", re.compile(r"Wasm|WebAssembly")),
    ("structured-clone/serialization", re.compile(r"StructuredClone|CloneData")),
    ("shared-memory/atomics", re.compile(r"SharedArrayBuffer|\bSAB\b|Atomics|GrowableSharedArray")),
    ("intl/locale/timezone", re.compile(r"\bIntl\b|Locale|DateTimeFormat|TimeFormat|NumberFormat|PluralRules|ListFormat|RelativeTime|Collator|Segmenter|DisplayNames|TimeZone|ForceUTC|Fdlibm|IsExecutionContextsTimeFrozen")),
    ("interrupt/cancellation", re.compile(r"Interrupt|TerminateProcessing|StopDraining|RequestInterrupt")),
    ("jobs/promise/event-loop", re.compile(r"\bJob\b|Jobs\b|Promise|Microtask|EnqueueJob|DrainingJobQueue|JobQueue|SetJobQueue|RunJobs")),
    ("debugger/profiling/memory", re.compile(r"Debugger|Breakpoint|OnStep|OnPop|ScriptFrameIter|FrameIter|SavedFrame|Capture.*Stack|BuildStack|StackFormat|StackCapture|Profiling|Profile|Sampling|MemoryUse|MallocSizeOf|SizeOf|MemoryStats|CollectMemoryStats|DescribeScriptedCaller|EnableTraceLogger|StartTraceLogger|StopTraceLogger|GetEnterStackLimit|JitcodeIRTableIntervalForLine")),
    ("GC/rooting/heap/weak-refs", re.compile(r"\bGC\b|GC[A-Z_]|KGC|Root|Trace|Tracer|\bZone\b|ZoneOf|Weak|Barrier|\bHeap\b|HeapPtr|Nursery|Sweep|Finaliz|Allocat|IncrementalGC|Shrink|GCThing|AboutToBeFinal|PersistentRooted|AddRawValueRoot|RemoveRawValueRoot")),
    ("compile/stencil/module/xdr/cache", re.compile(r"Compile|Stencil|Transcode|\bXDR\b|XDR_|Decode|EncodeScript|EncodeInterpretedFunction|Module|FrontendContext|SourceText|\bLazy\b|Delazif|OffThread|Instantiate|Evaluate|ScriptMetadata|CompileOptions|ReadOnlyCompileOptions|OwningCompileOptions|ScriptEnvironment|ScriptSource|CompilationStorage")),
    ("runtime/context/realm/compartment/zone", re.compile(r"Realm|Compartment|\bZone\b|CurrentGlobalOrNull|NewGlobalObject|FireOnNewGlobalObject|GlobalObject|Context|Runtime|Engine|\bInit\b|JS_Init|ShutDown|DestroyContext|MainContext|EnterCompartment|LeaveCompartment|CompartmentName|GetTopLevelCompression")),
    ("principals/security/options", re.compile(r"Principal|Security|CrossOrigin|CrossCompartment|Wrapper|Nuke|Remap|CheckedUnwrap|Subsumes|OriginAttributes|SystemZone|IsSystemZone|IsSystemCompartment|SetSecurityCallbacks")),
    ("embedding-hooks/callbacks", re.compile(r"Callback|\bHook\b|Hooks|Handler|Reporter|RejectionTracker|WarningReporter|SetPromiseRejectionTracker|ScriptEnvironmentPreparer|DescribeNatives|SetDOMCallbacks|FinalizeCallback|SweepThing|TraceThing|WeakPointerZonesCallback|SetSupportDifferentialTesting|EnqueuePromiseJob")),
]

CORE_VALUE = "core-value/object-surface"
SUPPORT = "support(non-SM)"
GLUE = "glue(mozjs-crate)"

# 非 SpiderMonkey 面(bindgen 连带生成的 C++ 支撑命名空间)
SUPPORT_NS_PREFIXES = (
    "root::std", "root::__gnu_cxx", "root::__pstl", "root::fmt",
    "root::ipc", "root::baseprofiler", "root::profiler", "root::mozilla",
    "root::__detail",
)


def find_repo_root() -> str:
    """从脚本位置向上定位仓库根(含 scripts/sm-audit 的目录)。"""
    d = os.path.dirname(os.path.abspath(__file__))
    for _ in range(4):
        if os.path.basename(os.path.dirname(d)) == "scripts" and os.path.basename(d) == REPO_HINT:
            return os.path.dirname(os.path.dirname(d))
        d = os.path.dirname(d)
    # 兜底:向上找 .git
    d = os.path.dirname(os.path.abspath(__file__))
    while d != "/":
        if os.path.isdir(os.path.join(d, ".git")):
            return d
        d = os.path.dirname(d)
    raise SystemExit("sm_audit_common: cannot locate repo root from " + os.path.abspath(__file__))


def git_head(repo: str):
    """返回 (full_hash, short, dirty, branch)。"""
    def run(*args):
        return subprocess.run(["git", "-C", repo, *args],
                              capture_output=True, text=True, check=True).stdout.strip()
    full = run("rev-parse", "HEAD")
    short = run("rev-parse", "--short=8", "HEAD")
    dirty = bool(run("status", "--porcelain"))
    branch = run("rev-parse", "--abbrev-ref", "HEAD")
    return full, short, dirty, branch


def cargo_name_version(manifest: str):
    """从 Cargo.toml 抽 (name, version)(仅取段落头部键,免 toml 依赖)。"""
    name = ver = None
    with open(manifest, "r", encoding="utf-8") as f:
        for line in f:
            m = re.match(r'^\s*name\s*=\s*"([^"]+)"', line)
            if m and name is None:
                name = m.group(1)
                continue
            m = re.match(r'^\s*version\s*=\s*"([^"]+)"', line)
            if m and ver is None:
                ver = m.group(1)
            if name and ver:
                break
    return name, ver


def mozjs_baseline(repo: str) -> dict:
    """vendored mozjs crate 版本基线。"""
    out = {}
    for key, sub in (("crate", "mozjs"), ("sys_crate", "mozjs-sys")):
        manifest = os.path.join(repo, "vendor", "mozjs", sub, "Cargo.toml")
        if os.path.isfile(manifest):
            n, v = cargo_name_version(manifest)
            out[key] = f"{n} {v}" if n else sub
        else:
            out[key] = f"<missing {sub}>"
    return out


def discover_jsapi(repo: str):
    """发现 bindgen jsapi.rs:env BAO_SM_AUDIT_JSAPI > CARGO build out 全局最新 > repo target/。

    返回 (path, md5, copies_uniform)。多副本 md5 不一致时 copies_uniform=False
    (漂移分析置信度降级,仍取 mtime 最新)。
    """
    env = os.environ.get("BAO_SM_AUDIT_JSAPI")
    candidates = []
    if env and os.path.isfile(env):
        candidates.append(env)
    else:
        roots = [os.environ.get("CARGO_BUILD_DIR", "/var/cargo-builds")]
        for root in roots:
            if not root or not os.path.isdir(root):
                continue
            for prof in ("debug", "test-ci", "test-ci-dbg"):
                # 布局 /var/cargo-builds/<h1>/<h2>/<profile>/build/... 与
                # <CARGO_BUILD_DIR>/<profile>/build/... 两种形态都找
                for pat in (
                    os.path.join(root, "*", "*", prof, "build", "bao-mozjs-sys-*",
                                 "out", "build", "jsapi.rs"),
                    os.path.join(root, prof, "build", "bao-mozjs-sys-*",
                                 "out", "build", "jsapi.rs"),
                ):
                    candidates.extend(glob.glob(pat))
        # repo 内 target/ 残迹兜底
        candidates.extend(glob.glob(os.path.join(
            repo, "target", "*", "build", "bao-mozjs-sys-*", "out", "build", "jsapi.rs")))
    if not candidates:
        raise SystemExit(
            "sm_audit_common: jsapi.rs not found. Run `cargo build -p bao-mozjs-sys` first, "
            "or set BAO_SM_AUDIT_JSAPI=<path to bindgen jsapi.rs>")
    candidates.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    newest = candidates[0]

    def md5(p):
        h = hashlib.md5()
        with open(p, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        return h.hexdigest()

    newest_md5 = md5(newest)
    uniform = all(md5(p) == newest_md5 for p in candidates[1:20])  # 抽前 20 副本
    return newest, newest_md5, uniform


def experimental_terms(repo: str) -> set:
    """experimental 头文件词集(vendor/mozjs/src-js/.../js/public/experimental/*.h)。

    symbol 命中 → stability_guess=experimental(issue #30 要求 experimental→public 轴)。
    """
    terms = set()
    base = os.path.join(repo, "vendor", "mozjs", "src-js", "mozjs", "js",
                        "public", "experimental")
    if not os.path.isdir(base):
        return terms
    word = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
    for name in sorted(os.listdir(base)):
        if name.endswith((".h", ".hpp")):
            with open(os.path.join(base, name), "r", encoding="utf-8",
                      errors="replace") as f:
                terms.update(word.findall(f.read()))
    return terms


def classify_surface(rust_path: str) -> str:
    """public / friend / internal / glue / support(jsapi.rs 命名空间推断)。"""
    if rust_path.startswith("root::glue"):
        return "glue"
    if rust_path.startswith(SUPPORT_NS_PREFIXES):
        return "support"
    if "detail" in rust_path or "shadow" in rust_path:
        return "internal"
    if rust_path.startswith("root::js"):
        return "friend"
    return "public"


def categorize(rust_path: str, symbol: str) -> str:
    """symbol → ledger 13 域之一 / glue / support / core-value 兜底(启发式猜测)。"""
    surface = classify_surface(rust_path)
    if surface == "support":
        return SUPPORT
    if surface == "glue":
        return GLUE
    text = f"{rust_path}::{symbol}"
    for cat, rx in CATEGORY_RULES:
        if rx.search(text):
            return cat
    return CORE_VALUE


def now_iso() -> str:
    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def today() -> str:
    return datetime.date.today().isoformat()
