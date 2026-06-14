# SPEC 56 Error Remediation Plan

## Target: 56 SPEC validation errors → 0

## Error Categories

### Cat-1: 37 Invalid API IDs (含 `/`)
`post-/evaluate-js` → `api-evaluate-js` 等批量重命名

### Cat-2: 4 Chinese Invalid IDs
- `内存安全-—-零-ub` → `nfr-memory-safety`
- `panic-收敛-—-生产零意外崩溃` → `nfr-panic-convergence`
- `零拷贝热路径-—-原生-rust-性能` → `nfr-zero-copy-hotpath`
- `代码惯用性-—-zig-翻译痕迹清除` → `nfr-idiomatic-code`

### Cat-3: 15 Missing Deliverables
创建实际架构设计内容

### Cat-4: Cross-ref cleanup
修复因 ID 变更导致的 xref 断链

## Execution Order
1. Cat-2 (4 Chinese IDs) — 最简单，DOM batch rename
2. Cat-1 (37 API IDs) — DOM batch rename
3. Cat-3 (15 Deliverables) — 需要架构设计内容
4. Cat-4 (Cross-ref cleanup) — spec check fix
