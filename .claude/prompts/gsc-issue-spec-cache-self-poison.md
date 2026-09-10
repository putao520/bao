# gsc 上游 issue 草稿(待用户确认后提交 · 禁自修 gsc 代码)

## 标题
[BUG] spec-cache 空快照自续存:precomputeSpecSnapshot→getReqCoverage 环形读缓存,冷启动永不重解析,completion-audit 永久误判 REQ 未满足

## 现象
Stop hook completion-audit 持续阻断:「SPEC 完成度校验:demand 内 REQ/OPT/TASK 未满足:REQ-XXX」。SPEC 真源 status=implemented,但 `.gsc/spec-cache.json` 的 `parseResults.specReqs=[]`、`parseResults.files={}`(file_hashes 正常 9 文件)。快照时间戳持续刷新(每次 hook 读时 applyIncremental/预计算重写),但内容恒空——**毒快照自续存,永不自愈**。

## 根因(两处)
1. **环形依赖**:`spec-cache-worker.mjs precomputeSpecSnapshot()` 委托 `getCoverage()`(默认 completion-audit 的 `getReqCoverage`),而 getReqCoverage 第一步就是 `store.get(10_000)` 读缓存——新鲜毒缓存直接返回空,precompute 把空写回。worker 永远无法用真解析覆盖毒快照。
2. **冷启动无全量解析**:getReqCoverage 在 `store.get`→null 且 `store.read()`→null(无 baseline)时,`snapshot` 保持 undefined→specReqs=[]——PERF 优化(REQ-PERF-001)把全量 parse 从消费路径移除时,把 bootstrap 全量解析也一并删除;缓存一旦为空(任何一次历史空写),永久为空。

## 复现
```bash
rm .gsc/spec-cache.json
cd gsc/hooks/node && node --input-type=module -e "
import { precomputeSpecSnapshot } from './spec-cache-worker.mjs';
const s=await precomputeSpecSnapshot({specDir:'/path/to/proj/SPEC'});
console.log(s.parseResults.specReqs.length);"   # → 0(应为全量 REQ 数)
```
解析层本身健康:`parseChangedSpecFiles` 同文件返回 39 reqs。

## 修复建议(供参考)
- precomputeSpecSnapshot 不应经 getReqCoverage 读缓存;直接 `parseChangedSpecFiles(specDir, listSpecFiles(specDir), {})` 全量解析。
- 或 getReqCoverage 无 baseline 时回退全量解析(接受一次冷启动开销)。

## 临时运维处置(数据修复,非代码修复)
用真实解析重建快照(已在本项目执行,155 reqs 恢复):
```js
const files=listSpecFiles(specDir);
const res=await parseChangedSpecFiles(specDir, files, {});
const specReqs=res.parseResults.specReqs;
store.set({ coverage:{...res.coverage, reqs:specReqs}, parseResults:res.parseResults, auditResults:{} }, currentFileHashes(files));
```
