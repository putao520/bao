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

**Bench harness:TBD(尚未实现)**。本目录目前只有方法论 + 维度规划,
不包含可运行的 bench 代码。这是后续的工程项。

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
| Evaluate throughput | `evaluate/s` 连续 eval | JS 执行 + CDP 往返 |
| CDP roundtrip | single command roundtrip latency | WS / memory transport 延迟 |
| Screenshot latency | `Page.screenshot` end-to-end | 截图管线 |
| Multi-page parallel | 10 并发 page 各自 navigate | 并行调度 |

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
  README.md       (本文件)
  METHODLOGY.md   (测量方法)
```

后续 harness 落地后会扩展:

```
bench/
  runtime/        # 启动 / JS exec / memory
  browser/        # 首页 / RSS / page churn
  automation/     # nav/s / evaluate/s / CDP roundtrip
  node_api/       # fs / http / compression / crypto
  REPORT.md       # 最新一次完整报告(版本绑定)
```
