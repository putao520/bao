#!/usr/bin/env python3
"""BCE-20260619-012 GC-unsafe Handle 检测器（行扫描器，格式免疫）

背景:
    SpiderMonkey 的 Handle<T> 是 GC 根的薄封装。直接手搓
    `Handle::<*mut JSObject> { ptr: &some_local }` 会绕过 root 机制,
    下一次 GC 可能回收对象 → use-after-GC。必须走 `rooted!` + `.handle()`。

检测模式（见 bugPattern BCE-20260619-012）:
    1. Handle::<*mut JSObject> { ptr: &IDENT } (IDENT 非 null) — 危险
    2. Handle::<*mut JSString> { ptr: &IDENT } — 危险
    3. Handle::<Value/JSVal> { ptr: &IDENT }
       (IDENT 来自 ObjectValue/StringValue/to_object 等 GC 来源) — 危险

排除（白名单,安全形态）:
    - MutableHandle（输出参数,由调用方 root,安全）
    - IDENT 持有 Int32/Double/Boolean/Undefined/Private/Null/NumberValue（非 GC 类型）
    - IDENT 含 null 字样（空 handle,安全）

退出码:
    0 = 无命中（可 commit）
    1 = 有命中（阻断,见根治模板）

用法:
    python3 scripts/bce_scan_gc_unsafe.py [root1 root2 ...]
    默认扫描 src/

根治模板:
    将 `Handle::<*mut JSObject> { ptr: &wrapped_local }` 替换为:
        rooted!(in(cx) let x_root = <gc_value>);
        x_root.handle().into()
    对 Value 同理: 用 `RootedValue` + `.handle().into()`。
"""
import os
import re
import sys

# 非 GC 类型 Value 构造器（持有这些的 Handle 是安全的,数值不被 GC 移动）
SAFE_VAL = [
    'Int32Value',
    'DoubleValue',
    'BooleanValue',
    'UndefinedValue',
    'PrivateValue',
    'NullValue',
    'NumberValue',
]

# Handle 类型回溯窗口（向上找 Handle::<...> 或 MutableHandle 标记）
BACKTRACK_LINES = 12


def scan_file(fp):
    """扫描单个 .rs 文件,返回命中列表 [(file, line_no, ident, handle_type), ...]"""
    try:
        with open(fp, encoding='utf-8', errors='replace') as f:
            lines = f.read().split('\n')
    except OSError:
        return []

    hits = []
    for i, line in enumerate(lines):
        # 只匹配 struct 字面量语法 `{ ptr: &IDENT }` / `{ptr: &IDENT}`,
        # 避免误匹配 `foo.ptr: &bar` 这类语句
        m = re.search(r'\{\s*ptr:\s*&(\w+)', line)
        if not m:
            continue
        ident = m.group(1)
        # `ptr: &mut ...` 是 MutableHandle 的语法特征,跳过
        if ident == 'mut' or ident.startswith('mut'):
            continue

        # 向上回溯,确定 Handle 类型和是否 MutableHandle
        handle_type = None
        is_mutable = False
        for j in range(i, max(-1, i - BACKTRACK_LINES), -1):
            l = lines[j]
            if 'MutableHandle' in l:
                is_mutable = True
                break
            hm = re.search(r'Handle::<([^>]+)>', l)
            if hm:
                handle_type = hm.group(1).strip()
                break

        # MutableHandle 是输出参数（调用方 root）,安全
        # 没回溯到 Handle 类型说明不是手搓 Handle,跳过
        if is_mutable or handle_type is None:
            continue

        # 取前 15 行作为上下文,用于判断 Value 来源
        ctx = '\n'.join(lines[max(0, i - 15):i])

        if 'JSObject' in handle_type or 'JSString' in handle_type:
            # GC 对象类型:IDENT 非 null 即危险（null 是合法的空 handle）
            if 'null' not in ident.lower():
                hits.append((fp, i + 1, ident, handle_type))
        elif (handle_type.endswith('Value')
              or handle_type.endswith('JSVal')
              or handle_type.endswith('Val')):
            # Value 类型:IDENT 必须由非 GC 构造器赋值才算安全。
            # 要求 `IDENT = <SafeCtor>` 或 `let IDENT = <SafeCtor>` 形态,
            # 仅共现不算（避免误报）。
            safe_pattern = re.compile(
                r'\b' + re.escape(ident) + r'\s*[:=]\s*(?:mut\s+)?(?:let\s+)?'
                r'(?:.*=)?\s*(' + '|'.join(SAFE_VAL) + r')\b'
            )
            if not safe_pattern.search(ctx):
                hits.append((fp, i + 1, ident, handle_type))
    return hits


def iter_rs_files(root):
    """遍历 root 下所有 .rs 文件,跳过 target/ 和 .git/。"""
    if os.path.isfile(root):
        yield root
        return
    for dp, _, fns in os.walk(root):
        if '/target/' in dp or '/.git/' in dp:
            continue
        for fn in fns:
            if fn.endswith('.rs'):
                yield os.path.join(dp, fn)


def main():
    roots = sys.argv[1:] or ['src']
    all_hits = []
    for root in roots:
        for fp in iter_rs_files(root):
            all_hits += scan_file(fp)

    if all_hits:
        print(f"BCE-012 GC-unsafe Handle 残留 {len(all_hits)} 处:")
        for fp, ln, ident, typ in all_hits:
            print(f"  {fp}:{ln}: &{ident}  [{typ}]")
        print("\n根治模板: rooted!(in(cx) let x_root = <gc_value>); x_root.handle().into()")
        print("         对 Value: RootedValue + .handle().into()")
        sys.exit(1)

    print("BCE-012 GC-unsafe Handle 残留: 0 (clean)")
    sys.exit(0)


if __name__ == '__main__':
    main()
