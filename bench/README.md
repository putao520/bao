# Bao Benchmarks

> **诚实可复现基线 (Honest, reproducible baseline)。**
> 不追求赢 Chromium,不追求赢 Bun;追求**可复现**的性能数字。
> **允许"Bao slower -15%"**——不允许伪造或不测。

---

## 目标

公开 Bao 的性能数字,让任何人能:

1. 在自己机器上复现同样的数字
2. 与 Chromium / Bun / Node 做横向比较(我们提供跑法,不替我们填数字)
3. 跟踪 Bao 自身版本间的回归 / 改进

## 当前状态

**Phase A 基建已落地(issue #19,2026-09-10)**:方法论(`METHODOLOGY.md`)、
机器可读结果格式(`schema/bench-result.schema.json`)、独立进程 harness
(`bench/harness` + `bench/run.sh`,deadline 与测试 fleet 隔离)、首批 5 项 seed
基线(`results/`):runtime create/drop、realm create/drop、page churn
(create/navigate/evaluate/close + RSS/fd/thread 探针)、fetch 小负载吞吐
(Node 栈,本地 server)、RSS 三相采样。

**更早的 harness:evaluate() 往返延迟**(#2),位于
`src/bao_engine/benches/evaluate_roundtrip.rs`(cargo bench 标准 `[[bench]]` target,
仓库 bench 先例 `src/js_parser/benches/`)。首跑数据见 §3.1。

其余维度(§4 全部、CDP 双 transport、soak、对照)仍 TBD。

## 测量维度(规划)

### 1. Runtime(纯 JS / Node API 层)

| 维度 | 指标 | 目标 |
|------|------|------|
| Cold startup | `bao --version` wall-clock | 端到端冷启 |
| Warm startup | 第 2 次 `bao run -e '...'` | warm cache 后启动 |
| JS execution | fixed-loop arithmetic / string / object churn | SpiderMonkey JIT 路径 |
| Memory (idle) | RSS after `bao -e '1+1'` 退出前 | baseline RSS |
| Memory (churn) | 1000 次 `require` / `eval` 后 RSS delta | leak 检测 |

### 2. Browser(servo 集成层)

| 维度 | 指标 | 目标 |
|------|------|------|
| First page | `BaoRuntime::new()` → `PageHandle` ready | 引擎初始化 + 首页成本 |
| RSS / page | 1 / 10 / 50 / 100 页时 RSS | 多页面伸缩性 |
| Page churn | 创建/销毁 1000 次 RSS delta | 内存泄漏 |
| Cold navigation | `navigate(url)` 到 load event | 单页导航成本 |
| Render | first paint / steady state 像素 ready | 渲染管线成本 |

### 3. Automation(CDP / 自动化层)

| 维度 | 指标 | 目标 |
|------|------|------|
| Navigation throughput | `nav/s` 连续 navigate | CDP + 引擎协同 |
| Evaluate throughput | `evaluate/s` 连续 eval | **已测**(首跑 ~417k/s,见 §3.1) |
| CDP roundtrip | single command roundtrip latency | WS / memory transport 延迟 |
| Screenshot latency | `Page.screenshot` end-to-end | 截图管线 |
| Multi-page parallel | 10 并发 page 各自 navigate | 并行调度 |

### 3.1 Evaluate throughput — 首跑数据(2026-08-21)

Harness:`src/bao_engine/benches/evaluate_roundtrip.rs` — 引擎直驱
`JsContext::eval` 往返(compile + execute + RunJobs + 值转 native)。CDP
`Runtime.evaluate` / `Runtime.callFunctionOn` 经 servo bridge 落到同一
SpiderMonkey 求值面;本数字覆盖 **JS↔native 环路**,不含 WS/CDP 传输跳
(那是独立的 "CDP roundtrip" 维度)。测量方法见 `METHODOLOGY.md` §4
in-process 延迟:per-op `Instant` delta,p50/p95/p99,warmup 2000 次剔除,
每 iteration 结果校验(fail-closed)。

- **硬件**:Intel i9-10900KF(20c,3.70GHz)/ 125GB RAM / Linux 7.0.0-28-generic x86_64
- **Bao**:commit `87b1e5eb`,rustc 1.99.0-nightly(2026-07-19)
- **Profile**:`test-ci`(opt-level 2,无 LTO)→ **数字为下界**;正式基线待 fat-LTO release 跑
- **环境备注**:共享开发机,非独占空闲
- **跑法**:`cargo bench -p bao_engine --bench evaluate_roundtrip --profile test-ci`(全保真:`cargo bench -p bao_engine --bench evaluate_roundtrip`)
- **日期**:2026-08-21

| case | n | min | p50 | p95 | p99 | max | mean | cv | ops/s(p50) |
|------|---|-----|-----|-----|-----|-----|------|-----|-----------|
| warm_simple_1p1(`1+1`) | 20000 | 2.27µs | 2.40µs | 4.46µs | 6.13µs | 24.9µs | 2.70µs | 32.1% | ~417k |
| warm_fn_call(预定义函数,callFunctionOn 形态) | 20000 | 3.11µs | 3.36µs | 5.89µs | 7.99µs | 34.9µs | 3.79µs | 29.4% | ~298k |
| warm_json_stringify(中等:alloc + 序列化 + 字符串回传) | 20000 | 11.8µs | 12.2µs | 17.2µs | 24.0µs | 55.8µs | 13.3µs | 21.4% | ~82k |
| cold_first_eval(进程首个 eval) | 1 | 236µs | — | — | — | — | — | — | — |
| fresh_realm_first_eval(新 realm 首 eval,≈新 CDP page 首次 evaluate) | 1 | 155µs | — | — | — | — | — | — | — |
| engine_init(`JsContext::for_test`) | 1 | 16.7ms | — | — | — | — | — | — | — |

解读:evaluate(`1+1`)热态往返 ≈ **2.4µs 中位(~417k calls/s)**。cv
21–33% 为逐调用延迟分布的尾部形态(SM GC 停顿落在 p95+/max),非跑间
抖动(`METHODOLOGY.md` 的 cv>5% 规则针对跑间 median)。

### 4. Node / Bun API(I/O 层)

| 维度 | 指标 | 目标 |
|------|------|------|
| `fs` | read/write 吞吐(MB/s) | 文件 I/O |
| `http` | req/s 本地 loopback | HTTP 客户端 + 服务端 |
| Compression | deflate/gzip 吞吐 | zlib / Bun.deflate |
| Crypto | hash 吞吐(MB/s) | SHA / HMAC |

## 对照基线(规划)

每个维度跑 Bao + 至少一个对照:

- **JS / Runtime**:Bun / Node
- **Browser / Automation**:Chromium + Playwright(同 CDP 调用面)
- **Node API**:Node / Bun 原生

**对照脚本与 Bao 跑法同源**,我们提供 harness,不替对照项目填数字。

## 报告原则

- 每个数字必须配:**硬件**(CPU / RAM / OS kernel)、**Bao 版本**、**对照版本**、**跑法**、**日期**
- 中位数 ≥ 多次跑的 median(默认 ≥10 次),非 best-case
- 允许**负值**("Bao slower -X%"),不允许隐藏或跳过劣势维度
- 跑不出数字的维度标 `TBD`,不填零、不填估

## 当前文件

```
bench/
  README.md            (本文件)
  METHODOLOGY.md       (测量方法 — issue #19 Phase A 六要素/噪声/deadline 隔离)
  schema/
    bench-result.schema.json   (机器可读结果格式,JSON Schema 2020-12)
  harness/             (cargo workspace member `bench-harness`:独立进程 bench driver)
  run.sh               (编排:env 绑定(commit/host/日期)→ 构建 → 跑 → results/ 落盘)
  results/             (seed 基线数据,随仓库版本化;<date>-<commit>/<bench>.run-<k>.json)
  REPORT.md            (未来:自动生成的完整报告,issue #19 I 节)
```

后续维度(harness subcommand 扩展):fs / crypto / sqlite / spawn / bundler
(§4)、CDP 双 transport、多页并发 RSS、soak(#19 F/G)。
