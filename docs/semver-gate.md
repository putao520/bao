# semver-gate — W4 #17-3 Gate C(API break-detection)

发布前自动 API break 检测:对发布面 crate 逐个跑
[cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks),
baseline = 该 crate 在 crates.io 的最新已发布版。

## 用法

```bash
cargo install cargo-semver-checks --locked      # 一次性;~/.cargo/bin
scripts/semver-gate.sh --run                    # 全量(发布面 crate 集)
scripts/semver-gate.sh --run --crate bao_cdp    # 单 crate
scripts/semver-gate.sh --run --report P         # 指定报告路径
```

报告默认落 `.claude/semver-gate-report-<date>.md`(逐 crate 结论表 + 汇总)。

## crate 集

workspace members 中**已发布到 crates.io** 者:脚本用 `cargo metadata`
取 member 包名,逐个 `curl -A bao-check` 探测 crates.io;离线/探测失败
回退固定清单(bao-core / bao-browser / bao_cdp / bao_engine / bao_stealth /
bun_runtime / bun_sm / cdp-server / bao_cdp_client)。

## 判定语义(face-transition-publish-discipline)

| verdict | 含义 | 处置 |
|---|---|---|
| PASS | 与 baseline 无 API break | — |
| FAIL (major) | required bump: major —— **0.x 语义 = minor bump 未做** | 硬 fail(退出码 1,阻断) |
| ADVISORY | minor/patch-required 或其他 break | 报告,不阻断 |
| SKIP (timeout) | 单 crate 超 `BAO_SEMVER_GATE_TIMEOUT`(默认 900s) | 标注跳过,不静默 |

0.x 纪律:破坏性变更 = minor bump(禁 patch);W2 运行时重命名以
deprecated alias 保持外部源兼容(semver-clean),故 gate 预期全 PASS——
出现 major-required 即 alias 边界破洞或未按纪律 bump。

## CI 挂接

`scripts/local-ci.sh` 第 5 段(可选):**默认 skip**(逐 crate 检查
分钟-十分钟级,避免日常 CI 长时);`BAO_SEMVER_GATE=1 ./scripts/local-ci.sh`
启用,启用时硬 fail 计入 local-ci 退出码。

## 工具

- cargo-semver-checks ≥ 0.50(`cargo install cargo-semver-checks --locked`)
- 首次接入证据:2026-09-29 全量实跑,报告见
  `.claude/semver-gate-report-2026-09-29.md`
