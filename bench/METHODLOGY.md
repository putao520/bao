# Benchmark Methodology

> **可复现是第一原则**。本文档定义如何跑、跑几次、如何报告。
> 任何性能数字必须能由独立第三方在同样硬件上复现。

---

## 1. 硬件与环境要求

报告每个 benchmark 时**必须**列出以下环境信息(缺一不可):

### 必填

- **CPU**:型号 + 核数 + 主频(如 `AMD Ryzen 9 7950X, 16c/32t, base 4.5GHz`)
- **RAM**:总量 + 类型(如 `64GB DDR5-6000`)
- **OS / Kernel**:`uname -a` 完整输出
- **Bao version**:`bao --version` 输出(commit hash)
- **Bao build mode**:`debug` / `release`(`--release` 默认)
- **对照版本**(若对照跑):Node `node --version`、Bun `bun --version`、Chromium `--version`
- **跑法**:可粘贴的完整命令
- **日期**:ISO 8601

### 推荐

- **CPU governor**:`performance`(避免动态调频抖动)
- **温度**:跑前 / 跑后(避免 thermal throttle)
- **其他后台进程**:列出或确认空闲
- **Rust toolchain**:`rustc --version` + nightly date

## 2. 构建模式

**所有报告数字默认 `--release`。**

```bash
cargo build --release -p bao_bin
# 产物 target/release/bao
```

debug build 数字可作开发期参考,但**不计入正式报告**。

## 3. 跑几次 / 取什么

### 默认策略

- **每个 benchmark 跑 ≥ 10 次**
- 报告 **median**(中位数)
- 同时列出 **min / max / p95**(让读者看分布)
- 报告 **cv (coefficient of variation)**:若 `cv > 5%` 标注 unstable,建议增加样本数

### 不允许

- **best-case** 单次跑当结果
- **average**(易受 outlier 扭曲,用 median)
- 跑 < 10 次的"快速估"
- cherry-pick 跑得高的一次

### Warmup

每次 benchmark **前**跑 1-2 次 warmup(warm cache / JIT),warmup 结果**不计入**统计。

## 4. 测量什么

### Wall-clock

- 用 `hyperfine`(Rust 工具,apt / cargo 可装)跑 CLI 命令
- 默认 `hyperfine --runs 10 --warmup 2 '<cmd>'`
- 报告 hyperfine 输出的 median + min + max + cv

### In-process(吞吐 / 延迟)

- 用 `std::time::Instant` 或 `criterion` crate
- 吞吐(`req/s` / `nav/s`)= `ops / elapsed_seconds`
- 延迟(latency)= per-op `Instant::now()` delta,取 p50 / p95 / p99

### RSS / Memory

- Linux:`/usr/bin/time -v` 或读 `/proc/<pid>/status` 的 `VmRSS`
- 采样点:启动后 idle / 峰值 / 退出前
- 报告:**idle RSS** + **peak RSS** + **delta**(泄漏指标)

### Cargo test 测频(注意)

Rust 项目的 `cargo test` / `clippy` / `build` 必须用 `--jobs 1`(见 CLAUDE.md),
但**这仅适用于 CI 验收**,不适用于性能 benchmark。benchmark 用 `--release` 单独跑。

## 5. 对照基线

每个维度报告 Bao + 至少一个对照:

```
| Dimension       | Bao (median) | Bun (median) | Node (median) | Bao vs Bun | Bao vs Node |
|-----------------|--------------|--------------|---------------|------------|-------------|
| Cold startup    | TBD          | TBD          | TBD           | TBD        | TBD         |
| ...             | ...          | ...          | ...           | ...        | ...        |
```

**对照脚本必须可复现**:同硬件、同时间窗、同测量方法(hyperfine / criterion)。

### 允许的劣势

**"Bao slower -X%"** 是允许的——Bao 不是为赢 Bun/Node 设计的纯 JS runtime,
也不是为赢 Chromium 设计的浏览器。**禁止隐藏劣势**。

不允许的:
- 不跑对照只报 Bao
- 跑了对照但不报告
- 报告时只取 Bao 高的维度

## 6. 报告模板

每个 benchmark 报告必须含:

```markdown
### <dimension name>

- Hardware: <CPU> / <RAM> / <OS>
- Bao version: <version>
- Counterpart: <Bun version> / <Node version> / <Chromium version>
- Date: <YYYY-MM-DD>
- Command:
  ```bash
  hyperfine --runs 10 --warmup 2 'bao run -e "..."'
  ```

| Variant | Median | Min | Max | p95 | CV |
|---------|--------|-----|-----|-----|----|
| Bao     | TBD    | TBD | TBD | TBD | TBD |
| Bun     | TBD    | TBD | TBD | TBD | TBD |

Interpretation: <一句话总结,允许 "Bao slower -X%">
```

## 7. CI 集成(规划)

- **benchmark 不在常规 CI 跑**(慢 + 数字会因 CI 机器抖动失真)
- 单独的 `bench.yml` workflow,**手动触发**(workflow_dispatch)
- 跑完写入 `bench/REPORT.md`(version-bound + date-stamped)
- 性能回归 alert:**TBD**(对照上次报告,负偏移 >X% 标 regression)

## 8. 当前限制

- **Bench harness:TBD**(尚未实现)
- **首版 REPORT.md:TBD**(尚未有任何 benchmark 跑出来)
- 所有 dimension 的 Bao 数字 / 对照数字:**TBD**,禁止填写
