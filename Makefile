# Bao (包子) — 顶层 Makefile
# 仅承载 lint / BCE 门禁等开发者工具 target,真正的构建由 cargo 驱动。

.PHONY: bce-check bce-gc-unsafe bce-gc-unsafe-ast bce-gc-unsafe-py

# BCE 防复发门禁集合 —— 未来新增的 BCE 扫描器在这里追加依赖
bce-check: bce-gc-unsafe
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
