# Bao (包子) — 顶层 Makefile
# 仅承载 lint / BCE 门禁等开发者工具 target,真正的构建由 cargo 驱动。

.PHONY: bce-check bce-gc-unsafe bce-gc-unsafe-ast bce-gc-unsafe-py bce-ast-catch-fallback bce-spec-id

# BCE 防复发门禁集合 —— 未来新增的 BCE 扫描器在这里追加依赖
# @trace REQ-SPEC-002
# 本门禁集合体现 REQ-SPEC-002 的契约:确定性批量任务(模式扫描/id 校验等
# 纯机械化、无架构决策的工作)用 Makefile target + Python/Rust 脚本一次性
# 完成,而非启动 six-node-dev.js 多 epoch loop 流程(那是为需要需求讨论/
# 方案对比/架构决策的复杂任务设计的)。每个 bce-* target 都是一次性脚本门禁。
bce-check: bce-gc-unsafe bce-ast-catch-fallback bce-spec-id
	@echo "BCE gate: all scans clean"

# BCE-20260619-012: GC-unsafe Handle 门禁
# 主门禁用 AST 检测器 (bao_lints, 格式免疫, syn 解析); Python 行扫描器作交叉验证。
# 任一残留 >0 退出码非 0,阻断本地 commit / CI 合并。
bce-gc-unsafe: bce-gc-unsafe-ast bce-gc-unsafe-py
	@echo "BCE-012 gate: AST + py both clean"

# 主门禁: syn-AST 检测器 (SSOT, 格式免疫)
bce-gc-unsafe-ast:
	@cargo run -p bao_lints -- --check src/

# 交叉验证: Python 行扫描器
bce-gc-unsafe-py:
	@python3 scripts/bce_scan_gc_unsafe.py src/

# BCE-20260621-014: AST 扫描 catch 块吞错不回退 grep 门禁
# 检测 gsc MCP 源码 (~/code/claude/gsc) 的 processChunk catch 块是否有 grep 兜底。
# 残留 >0 阻断（AST→grep 兜底链断裂会导致 scanReqRefs 返回 0 → req_coverage Code=N/A）。
bce-ast-catch-fallback:
	@python3 scripts/bce_scan_ast_catch_no_fallback.py

# REQ-SPEC-001: SPEC API element id 格式门禁 (method-path 退化防复发)
# @trace REQ-SPEC-001
# 扫描 .spec/*.html 中带 data-api= 的 <section>/<div>,校验 id 是否符合
# API-{DOMAIN}-{N} 格式。--baseline 抑制 .github/baseline-spec-id.txt 中列出的
# 历史技术债务,只对「新增违规」(回归) 失败。新增 API 必须用 spec_write 让
# id-registry 分配 API-{DOMAIN}-{N},禁止手写 method-path id,禁止追加 baseline。
bce-spec-id:
	@cargo run -p bao_lints -- spec-id .spec/ --baseline .github/baseline-spec-id.txt
