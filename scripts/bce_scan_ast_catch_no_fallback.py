#!/usr/bin/env python3
"""BCE-20260621-014 AST 扫描 catch 块吞错不回退 grep 检测器（结构化扫描器）

背景:
    spec_govern 的 scanReqRefs / scanTraceAnnotations / scanDefinitions 三个扫描函数
    用 streamPipeline.processChunk 包裹 AST 解析 (try/catch)。原实现 catch 块仅
    `return null/[]`，未把 AST 失败的文件加入 grepOnlyFiles 队列。当常驻 MCP 进程
    中 Language.load(wasm) 失败时，parseFile 抛错被吞，文件既不进 AST 结果也不进
    grep 队列 → scanReqRefs 返回 0 → req_coverage 审计的「代码实现」列恒为 N/A。

检测模式（见 bugPattern BCE-20260621-014）:
    任何 processChunk 的 chunk.map callback 中存在
        try { ... await parseFile(...) ... } catch { return (null|[]) }
    且 catch 块**无** `grepOnlyFiles.push(filePath)` 前置 — 即 AST→grep 兜底链断裂。

排除（白名单,安全形态）:
    - 非 processChunk 路径的 catch（getFileFingerprint / grepReqIds 自身 catch）
    - catch 块内有 grepOnlyFiles.push（已修复）

退出码:
    0 = 无命中（可 commit）
    1 = 有命中（阻断,见根治模板）

用法:
    python3 scripts/bce_scan_ast_catch_no_fallback.py [gsc_ast_scanner_path]
    默认扫描 ~/code/claude/gsc/mcp/src/spec/audit/ast-scanner.mjs

根治模板:
    catch 块在 return null/[] 前加 grepOnlyFiles.push(filePath)。
"""

import os
import re
import sys

DEFAULT_TARGET = os.path.expanduser(
    "~/code/claude/gsc/mcp/src/spec/audit/ast-scanner.mjs"
)

# processChunk AST handler 的行范围（ast-scanner.mjs 内）
# 通过识别 `processChunk:` 关键字动态定位，避免硬编码行号
PROCESS_CHUNK_PATTERN = re.compile(r"^\s*processChunk:\s*async", re.MULTILINE)


def find_process_chunk_ranges(content: str):
    """返回 [(start_line, end_line)] 列表，每个 processChunk handler 的行范围。
    end_line = 下一个 reduce:/initial:/classify:/chunkSize:/queueSize: 关键字的行号。
    """
    lines = content.split("\n")
    ranges = []
    in_pc = False
    start = None
    depth = 0
    terminator = re.compile(
        r"^\s*(reduce|initial|classify|chunkSize|queueSize|onChunk|extensions|rootDir)\s*[:=]"
    )
    for i, line in enumerate(lines):
        if PROCESS_CHUNK_PATTERN.search(line):
            in_pc = True
            start = i + 1
            depth = 0
            continue
        if in_pc:
            if terminator.search(line):
                ranges.append((start, i))
                in_pc = False
                continue
    if in_pc and start:
        ranges.append((start, len(lines)))
    return ranges


def scan(path: str):
    if not os.path.isfile(path):
        print(f"[BCE-014] target not found: {path}", file=sys.stderr)
        # 文件不存在不算命中（环境差异），退出 0
        return 0
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        content = f.read()
    lines = content.split("\n")

    pc_ranges = find_process_chunk_ranges(content)
    findings = []

    catch_re = re.compile(r"^\s*\}\s*catch\s*\{")
    close_re = re.compile(r"^\s*\}\s*$")
    return_re = re.compile(r"^\s*return\s+(null|\[\])\s*;?\s*$")
    push_re = re.compile(r"grepOnlyFiles\.push")

    def in_process_chunk(line_num):
        for s, e in pc_ranges:
            if s <= line_num <= e:
                return True
        return False

    i = 0
    while i < len(lines):
        line = lines[i]
        m = catch_re.match(line)
        if m and in_process_chunk(i + 1):
            # gather catch body
            body = []
            j = i
            # first line is `} catch {`, advance to body
            for j in range(i, min(i + 8, len(lines))):
                body.append(lines[j])
                if j > i and close_re.match(lines[j]):
                    break
            body_text = "\n".join(body)
            has_push = bool(push_re.search(body_text))
            has_return = any(return_re.match(b) for b in body)
            if has_return and not has_push:
                findings.append(
                    {
                        "line": i + 1,
                        "snippet": body_text.strip(),
                        "reason": "processChunk catch return null/[] without grepOnlyFiles.push",
                    }
                )
            i = j + 1
            continue
        i += 1

    if findings:
        print(f"[BCE-014] RESIDUAL: {len(findings)} instance(s) in {path}", file=sys.stderr)
        for fnd in findings:
            print(f"  line {fnd['line']}: {fnd['reason']}", file=sys.stderr)
            for ln in fnd["snippet"].splitlines()[:5]:
                print(f"    | {ln}", file=sys.stderr)
        return 1
    print(f"[BCE-014] CLEAN: 0 residual in {path}")
    return 0


def main():
    target = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_TARGET
    sys.exit(scan(target))


if __name__ == "__main__":
    main()
