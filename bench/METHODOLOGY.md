# Benchmark Methodology

> **可复现是第一原则**。本文档定义如何跑、跑几次、如何报告。
> 任何性能数字必须能由独立第三方在同样硬件、同样 profile、同样命令上复现。
>
> 本文档是 issue #19 Phase A 的交付物:六要素(硬件/OS/toolchain/commit/冷暖/重复与统计)、
> 指标单位与噪声处理、benchmark 与功能测试的 deadline 隔离、机器可读结果格式。
> 运行入口:`bench/run.sh`;结果格式:`bench/schema/bench-result.schema.json`。

---

## 1. 六要素(每个报告数字的强制上下文)

报告每个 benchmark 时**必须**列出以下环境信息(缺一不可)。harness 自动采集进每个
结果 JSON 的 `environment` 块;人工报告必须与其一致。

### 必填

| 要素 | 内容 | 采集方式 |
|------|------|----------|
| **硬件** | CPU 型号 + 核数;RAM 总量 | `/proc/cpuinfo` `model name`、`available_parallelism`、`/proc/meminfo MemTotal`(自动) |
| **OS** | kernel 版本完整串 | `/proc/version`(自动) |
| **Toolchain** | rustc 版本 | `rustc -V`(run.sh 注入 `BAO_BENCH_RUSTC`) |
| **Commit** | git commit hash + dirty 标记 | run.sh 注入 `BAO_BENCH_GIT_COMMIT` / `BAO_BENCH_GIT_DIRTY` |
| **冷暖状态** | cold = 进程首次迭代(引擎/JIT/OS cache 冷);warm = warmup 丢弃后的稳态 | 每个结果文件按 cold/warm 分开记录,禁止只报 warm 冒充全部 |
| **重复次数与统计方法** | 见 §3 | 参数块 `parameters` 自动记录 |

另加:`hostname`、`DISPLAY`(xvfb 与否)、`loadavg`(跑时机器负载,共享机披露)、
`cargo_profile`(构建档位)、日期(ISO 8601 UTC)。

### 推荐

- **CPU governor**:`performance`(避免动态调频抖动)
- **温度**:跑前 / 跑后(避免 thermal throttle)
- **其他后台进程**:列出或确认空闲;`loadavg` 已自动入档,loadavg 异常高时应改窗口重跑
- **对照版本**(若对照跑):Node `node --version`、Bun `bun --version`、Chromium `--version`

## 2. 构建模式与档位锁定

**正式报告数字默认 `--release`(fat-LTO)**。当前全部 seed 基线钉在 **`test-ci` 档**
(opt-level 2、strip,test fleet 同档,构建缓存可复用)——seed 文件的
`environment.cargo_profile` 如实记录。

```bash
# seed 基线档位(本批全部结果)
export CARGO_TARGET_DIR=/var/cargo-builds/3c/6184ceb77072ba
cargo run -p bench-harness --profile test-ci -- <bench> [flags]

# 正式报告档位(慢,fat-LTO)
cargo build --release -p bench-harness
```

**铁律:跨档位、跨 host 的数字禁止直接比较。** test-ci 档数字是下界参考
(无 LTO);对照与回归判定必须在同档位、同 host、同命令内进行。
debug 档数字可作开发期参考,但不计入正式报告。

## 3. 跑几次 / 取什么

### 重复策略(两类重复,都要)

1. **进程内迭代**(in-process iterations):每个 bench 在单进程内循环 N 次,
   逐次 `Instant` 计时。runtime 层 bench N ≥ 30;browser 层 bench(单迭代秒级)
   N ≥ 15 并在结果 `parameters` 里记录原因。
2. **进程级重跑**(process reruns):run.sh 对每个 bench 默认重跑 R ≥ 3 次
   (browser churn 允许 R = 1,记录原因),每次一个独立结果文件
   (`<bench>.run-<k>.json`),run 间差异即进程级噪声的直观披露。

### 统计方法

- 报告 **min / p50 / p90 / p95 / p99 / max / mean / cv**(nearest-rank 百分位,
  与 `src/bao_engine/benches/evaluate_roundtrip.rs` 同一实现)
- **cv(coefficient of variation)> 5% ⇒ 结果标 `unstable: true`**,必须增大样本
  或换窗口重跑后才有资格进入对照;禁止悄悄丢样本让 cv 变好看
- **cold(进程首迭代)单列**,不混入 warm 统计;`n=1` 的 cold 指标只作趋势参考

### 不允许

- **best-case** 单次跑当结果
- **average** 单独出场(易受 outlier 扭曲,永远配 median/p50)
- 跑 < 10 次的"快速估"进正式对照
- cherry-pick 跑得高的一次;跨档位/跨 host 拿来对比
- **丢 outlier**:异常样本如实进 max/p99,归因写进 `notes`,不删除

### Warmup

每个 warm 指标前丢弃 W 次 warmup 迭代(默认 W=3~5,browser 层 W=2),
warmup 结果**不计入**统计,丢弃次数记入 `parameters`。

## 4. 测量什么:指标单位表

| 指标类 | 单位(SI 字符串,进 JSON `unit` 字段) | 测量手段 |
|--------|--------------------------------------|----------|
| latency | `us` / `ms`(按量级选一,全文件一致) | per-op `Instant::now()` delta;进程级 wall-clock 用 hyperfine |
| throughput | `ops_per_s`;带宽类 `MB_per_s` | `ops / elapsed_seconds` |
| RSS / 内存 | `KiB`(`/proc/<pid>/status` `VmRSS`/`VmHWM` 原生单位) | idle / churn / post 三相采样 + 峰值 + 斜率 |
| CPU | loadavg 快照(自动入 `environment`);CPU-time(`utime+stime`,未来) | `/proc/loadavg`;`/proc/<pid>/stat` |
| build-time | `s`(未来维度,本轮未测,单位先立) | hyperfine around `cargo build` |
| 计数(fd/thread/page) | `count` | `/proc/self/fd`、`/proc/<pid>/status Threads` |

### Wall-clock(进程级)

- 用 `hyperfine`:`hyperfine --runs 10 --warmup 2 '<cmd>'`,报告 median + min + max + cv

### In-process(吞吐 / 延迟)

- 吞吐(`req/s` / `nav/s`)= `ops / elapsed_seconds`
- 延迟 = per-op `Instant` delta,取 p50 / p95 / p99;**上尾包含 SM GC 停顿——
  这是诚实的往返延迟,不是噪声**,禁止把 p99 修掉

### RSS / Memory

- Linux:读 `/proc/<pid>/status` 的 `VmRSS` + `VmHWM`(harness 内置采样线程)
- 采样点:idle 稳态 / churn 期间 / churn 结束后(post,看回收)
- 报告:三相各自 min/p50/max + churn 斜率(`KiB_per_s`)+ 峰值 VmHWM;
  **单调增长必须可归因,不可解释增长不得放行**(issue #19 G 节原则,seed 只采集)

### Cargo test 测频(注意)

Rust 项目的 `cargo test` / `clippy` / `build` 在 CI 验收语境必须 `--jobs 4`
(见 CLAUDE.md),但这与 benchmark 无关:benchmark 独立进程单独跑,见 §6。

## 5. 噪声处理

| 噪声源 | 处理 |
|--------|------|
| JIT / cache 冷启动 | warmup 丢弃 + cold 单列(§3) |
| SM GC 停顿 | 不滤除:如实进 p95/p99/max;逐调用延迟分布天然重尾,cv 阈值针对**跑间 median**,逐调用 cv 仅作分布形状披露 |
| 机器调度 / 共享负载 | bench 独占进程跑(§6);`loadavg` 自动入档;loadavg 异常 → 换窗口重跑,旧结果标记不可用 |
| 调频 / 温度 | 推荐 governor=performance;共享开发机如实披露(`environment.notes`) |
| 单次抖动 | 回归判定禁用单次数字:必须 R ≥ 3 次重跑 + cv 门;单跑回归一律不 actionable |
| 跨环境差异 | 档位/host 锁定(§2),结果文件永久绑定 commit + host + 日期 |

## 6. Benchmark 与功能测试隔离(deadline 隔离)

**benchmark 永远不进功能测试管线。** 具体红线:

- bench 是独立进程(`cargo run -p bench-harness -- <bench>`),
  **不注册为 `#[test]`、不在 nextest/cargo test 下运行**——测试 fleet 的
  deadline/timeout/并发调度不得作用于性能测量,避免调度噪声伪装逻辑回归
- 每个 bench 自带 wall-clock 预算(`parameters.budget_*`),超预算即在该迭代边界
  停止采样并如实标注实际 n;**fail-closed**:任何验证失败(结果不对、服务器
  异常、页面不 ready)→ 进程非零退出 + 输出含 `error` 字段的结果 JSON,
  禁止部分成功冒充完整基线
- bench 运行窗口尽量避开测试 fleet / 大编译 concurrently(loadavg 入档供回溯);
- 回归门(未来 H 节)同样独立于测试门:重复样本 + cv 规则判 adjudication,
  不与功能测试的 pass/fail 混流

## 7. 机器可读结果格式

- **Schema**:`bench/schema/bench-result.schema.json`(JSON Schema 2020-12)。
  每个 bench 每次运行输出**恰好一个** JSON 文档(stdout),字段与 schema 一一对应。
- **结构**:`schema_version` / `benchmark` / `date_utc` / `environment`(六要素)
  / `parameters` / `metrics[]`(name、unit、kind、higher_is_better、n、
  min/p50/p90/p95/p99/max/mean/cv、unstable、可选 raw samples)/ `notes[]`
  / 可选 `error`(失败时,配非零退出码)
- **落盘**:`bench/results/<YYYY-MM-DD>-<commit-short>/` 目录,
  文件名 `<bench>.run-<k>.json`;seed 基线数据随仓库提交(基线是可复现对照的锚点,
  必须版本化)
- **校验**:任何消费方先按 schema 校验再读数;harness 输出即符合 schema,
  手工编辑过的结果文件作废
- **报表**(未来 I 节):`bench/REPORT.md` 从 results/ 自动生成,好/坏结果都公开

## 8. 对照基线

每个维度报告 Bao + 至少一个对照:

```
| Dimension       | Bao (median) | Bun (median) | Node (median) | Bao vs Bun | Bao vs Node |
|-----------------|--------------|--------------|---------------|------------|-------------|
| Cold startup    | TBD          | TBD          | TBD           | TBD        | TBD         |
```

**对照脚本必须可复现**:同硬件、同时间窗、同测量方法(hyperfine / in-process)。

### 允许的劣势

**"Bao slower -X%"** 是允许的——Bao 不是为赢 Bun/Node 设计的纯 JS runtime,
也不是为赢 Chromium 设计的浏览器。**禁止隐藏劣势**。

不允许的:
- 不跑对照只报 Bao
- 跑了对照但不报告
- 报告时只取 Bao 高的维度

## 9. 报告模板

每个 benchmark 报告必须含:

```markdown
### <dimension name>

- Hardware: <CPU> / <RAM> / <OS>
- Bao version: <commit> / profile: <cargo_profile>
- Counterpart: <Bun version> / <Node version> / <Chromium version>
- Date: <YYYY-MM-DD>
- Command:
  ```bash
  bench/run.sh <bench>
  ```

| Variant | Median | Min | Max | p95 | CV |
|---------|--------|-----|-----|-----|----|
| Bao     | TBD    | TBD | TBD | TBD | TBD |
| Bun     | TBD    | TBD | TBD | TBD | TBD |

Interpretation: <一句话总结,允许 "Bao slower -X%">
```

## 10. 运行方式(本地;CI 集成=规划)

- **GitHub Actions 已弃用(2026-08-22 用户裁决,费用)**:性能 bench 不建 workflow,
  全部本地跑:`bench/run.sh [bench...]`
- 回归检测进入发布门(未来 H 节):对照上次版本化基线,负偏移超阈值 →
  fail / require adjudication(重复样本 + cv 规则,不用单次数字)

## 11. 当前限制(2026-09-10,Phase A 交付时点)

- 首批 seed 基线 5 项(见 `bench/results/`):runtime create/drop、realm create/drop、
  page churn、fetch 小负载吞吐(Node 栈,`JsContext` + 本地 HTTP server)、RSS 三相采样
- seed 全部 test-ci 档 + 共享开发机(非独占空闲)——正式对照报告待 fat-LTO release
  档 + governor=performance 窗口
- 未覆盖(按 issue #19 分节推进):evaluate round-trip 已有
  `src/bao_engine/benches/evaluate_roundtrip.rs`(2026-08-21 首跑);fs/crypto/sqlite/
  spawn/bundler、CDP 双 transport、多页并发 RSS、soak、Node/Bun/Chromium 对照 = 后续 Phase
- 已知观察项:`bao -e` / `bao run *.mjs` 顶层 await fetch 在 CLI 自驱 drain 下挂起
  (2026-09-10 探测,详见 results 目录 notes 与 issue #19 评论)——bench ④ 因此走
  测试已证明的 embedder-pump 形态;该 CLI 缺陷本身待独立 issue 根治
