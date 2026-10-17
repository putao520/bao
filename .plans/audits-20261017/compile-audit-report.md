# bao 编译加速审计报告(2026-10-17,只读诊断)

审计人:编译加速审计执行体。方法:静态核证(.cargo/config.toml 双层/workspace Cargo.toml/sccache 统计/target 目录实测)+ 2 个有界热实验(touch+bio 增量构建,零冷编、零配置改动)。

---

## 1. 量化基线

### sccache 统计(审计起点,历史累计 8092 请求)

| 指标 | 值 |
|---|---|
| 总命中率 | **47.71%** |
| C/C++ 命中率 | **87.13%**(2085 hit / 308 miss) |
| Assembler 命中率 | 88.89% |
| **Rust 命中率** | **1.46%**(44 hit / **2965 miss**) |
| Non-cacheable 原因 top | `incremental` 689 / `crate-type` 361 / `-` 215 / missing input 131 |
| 缓存位置 | 本地磁盘 /srv/home-links/.cache/sccache,**101G 满载**(上限 100G,LRU 逐出中) |
| 分布式 | **未配置**(无 dist scheduler env/config;~/.cargo/config.toml 注释中的 buildlet 16.18.0.1:10501 已退役/从未启用) |

### 实验期间 sccache 增量 diff(实验 A+B 全程,~6 分钟窗口)

- Rust **hits +0**;Rust **misses +430**(syn/quote/aws-lc 等重编全 miss)
- C/C++ hits +260(缓存正常工作)
- Non-cacheable(incremental) +9(并发会话的 workspace 成员编译)

### 实验 B:一次增量构建的真实成本(touch 4 行的 bao_cli/src/lib.rs → `cargo build --timings -p bao_bin --jobs 4`)

**Wall 3m06s**。Top 耗时单元(timings HTML,unit 并行故总和 288s > wall):

| 单元 | 耗时 | 性质 |
|---|---|---|
| bao-servo-script 0.5.32 | **64.6s** | vendor servo script 组件(dev 重编,最大 Rust 单元) |
| aws-lc-sys 0.46.0 | **55.8s** | C 面自定义 build(版本 bump 后族全量) |
| bao-servo-script-bindings | **42.6s** | servo codegen bindings |
| bun_uws_sys | 12.4s | C++ uWS |
| bao-servo-layout / bun_runtime / bun_css / bun_install | 8.4/8.2/7.2/6.7s | Rust |
| **bao_bin(含 1.4G debug 链接)** | **5.6s** | **链接不是长杆** |

**链接器事实修正**:mold 已在位且生效(`/usr/bin/ld.mold` + 用户 config `-fuse-ld=mold --no-keep-memory`,实验命令行实测确认)。1.4G debug 二进制链接含在 5.6s unit 内。「链接 345MB 是长杆」的输入假设过时。

### target 目录全景(碎片化实测)

| 路径 | 体积 | 状态 |
|---|---|---|
| /var/cargo-builds/3c/6184ceb77072ba/**debug** | 308G(deps 143G + **incremental 133G** + build 40G) | 活跃 |
| …/**test-ci** | 121G(**incremental 88G**,1147 crate 会话目录) | 活跃 |
| …/**llvm-cov-target** | 140G(内嵌套一层 llvm-cov-target/,套娃) | 今日 08:17 活跃 |
| /var/cargo-builds/**bao-cli-cov-target** | 117G(第二份插桩全图) | 今日 13:04 活跃 |
| /tmp/{clippy-fix,p0-users,t5ca,v254-manifest}-target | 各数 G~数十G | clippy-fix-target 审计期间活跃(并发 agent) |
| **dev-dist**(用户 config 设计的加速 profile) | **不存在** | **零使用** |

合计编译产物 **~790G+**;/var 盘 72%(余 511G)。

---

## 2. 六条假设逐条核证

### ① sccache 对 incremental 编译静默不缓存 → dev 命中≈0 —— **证实,但需一处重要细化**

机械链(全部实证):
- `~/.cargo/config.toml` `[profile.dev] incremental = true`(用户级,注释明确该取舍是有意的)
- cargo 对 **workspace 成员**编译传 `-C incremental=<target>/debug/incremental/<crate>-<hash>`(bao_lints 等 session 目录实测在位)
- sccache 见 incremental flag → non-cacheable(stats `incremental` 689 次居首)→ **不缓存**

**细化(修正任务输入)**:外部依赖(registry ~800 crate)的 rustc 命令行**不带** incremental(实验 A 抓取的 quote/syn 完整命令行为证)→ 外部依赖**走 sccache 且有效**。bypass 精确限定在 **116 个 workspace 成员**——恰是改动最频繁、重编最频繁的一层。这是"日常迭代快(增量)vs 冷场景无缓存救回"的刻意取舍,但冷场景一侧的保护(dev-dist)从未被使用(见④)。

### ② target 目录碎片化 —— **证实,且比输入假设严重**

并存碎片 ≥7 个:主 target 内 debug/test-ci/llvm-cov-target 三树 + 独立 bao-cli-cov-target + /tmp 4 个(1 个审计期间仍在产生)。两份插桩全图(bao-cli-cov-target 117G 与 llvm-cov-target 140G)是双份重复投资;llvm-cov-target 内还有嵌套套娃层。任何一次落到冷 target 的构建 = 全图冷编,sccache 只能救回 C/C++(87% 命中),Rust 面(workspace 成员 bypass + 外部依赖缓存被逐出)大量真重编。

### ③ profile 三套来回切换各自重编 —— **证实 + 一处意外发现**

dev/test-ci/instrument-coverage 三树并存且今日全部活跃,切换=冷树全量,插桩 RUSTFLAGS 变化=同树全 miss(物理必然)。

**意外发现(高价值缺陷)**:`[profile.test-ci] inherits = "test"`(inherits dev)→ **incremental=true 沿继承链传播到 CI profile**。铁证:test-ci/incremental/ 有 1147 个 crate 会话目录、787 个在 10-16 后写入、926 个在 10-15 后写入——**跑批 profile 正在做增量编译**:88G incremental 工件对跑批零收益(无迭代编辑),同时让 test-ci 的全部 workspace 成员编译 bypass sccache。

### ④ dev-dist 加速 profile 空转 —— **新发现(输入假设之外)**

`~/.cargo/config.toml` 已配置完整双轨体系:`[profile.dev-dist] incremental=false`(让 workspace 成员也进 sccache)+ alias `bd`/`ntd`。但 dev-dist target 目录**不存在 = 零使用**。冷场景的缓存救回通道建好了,从未走过车。

### ⑤ sccache 大请求死锁 —— 采信主会话历史实证(本次不复现);当前 server 健康(stats 正常推进,无 timeout/error)

### ⑥ C++ 有效但族升级全量重编 + dep 图体量 —— **证实**

C/C++ 87.13% 命中;实测 aws-lc-sys 0.46.0(近期 0.41→0.46 bump)重编 55.8s。dep 图:116 workspace 成员,timings 图可见 1129+ 编译单元;CFLAGS/CXXFLAGS=-g1 与 MOZ_DEBUG_FLAGS 已在 .cargo/config.toml 锚定(任何改动触发 C 全量,config 注释自带重编警告)。

---

## 3. 「为什么每次都要全量编译」真因排序

1. **树切换**:7+ 碎片 target,任一冷树=全图;插桩双份加剧
2. **test-ci 意外增量继承**:跑批也不进 sccache(本应最可缓存的 profile)
3. **dev-dist 空转**:冷场景无 Rust 缓存救回通道
4. **101G 缓存满载逐出**:外部依赖 Rust 缓存被挤(实验中 syn/quote 都 miss 的诱因之一)
5. 工具链 pin 变化 / vendor 族升级(C++ 侧,物理必然,已有 -g1 缓解)

---

## 4. 改进方案矩阵(按 ROI 排序,均待用户裁决,本次零改动)

### ROI-1:`[profile.test-ci] incremental = false`(一行,收益最大)

- **改法**:workspace Cargo.toml test-ci 段加一行(config.toml 或 Cargo.toml 任一,含 `cargo clean -p` 类 fingerprint 影响需一次重编过渡)
- **预期收益**:test-ci 全部编译进 sccache → 切分支/重跑批的 Rust 面从全重编变命中恢复;释放 88G incremental 工件;跑批 profile 无增量需求,单次性能损失≈0
- **风险**:低。增量工件消失后首次全量 test-ci 慢一轮;无语义影响
- **验证**:跑一次 `cargo nt -p <crate>`(test-ci),sccache stats 出现 Rust hits;test-ci/incremental 停止增长

### ROI-2:target 目录收敛政策(治理)

- **改法**:①插桩收敛单树(裁 bao-cli-cov-target 或 llvm-cov-target 二选一,需用户裁决保留哪个——两者今日都活跃,涉及 coverage 工作流);②清 llvm-cov-target 内层套娃;③/tmp scratch target 纳入会话收尾即焚清单
- **预期收益**:**~120-250G 磁盘**;消除最大冷编触发面
- **风险**:中低。coverage 脚本路径有耦合(c9-binmode/cov-audit 系列产物),需按引用清单迁移
- **验证**:du 对账 + 一次插桩跑批全绿

### ROI-3:dev-dist 双轨纪律落地(配置零改动,纯工作流)

- **改法**:切分支/大改/冷树场景统一走 `bd`/`ntd`(dev-dist),日常单 crate 迭代保持 `cargo nt`(dev 增量)。可选:把 dev-dist 也纳入 make/脚本入口降低遗忘率
- **预期收益**:冷场景外部依赖+workspace 成员双命中(sccache 唯一能救 Rust 的通道);与 ROI-1 叠加后 test-ci/dev-dist 双 profile 全部可缓存
- **风险**:无配置风险;纪律依赖是弱点
- **验证**:跑一次 `bd`,sccache Rust hits 首次出现批量增长,二次 `bd` 命中率跃升

### ROI-4:sccache 缓存扩容 100G→200G

- **改法**:`SCCACHE_CACHE_SIZE` env(现 SCCACHE_CACHE_SIZE=100G 在环境注入;/srv/sda1 余 2.4T 充裕)
- **预期收益**:降低外部依赖 Rust 缓存逐出率(实验中 syn/quote miss 的诱因);C/C++ 命中维持
- **风险**:极低,磁盘换命中率
- **验证**:扩容后一周观察 `--show-stats` Rust 命中率从 1.46% 的爬升

### ROI-5:不建议动 dev 的 incremental

dev 增量是正确取舍(改一行秒级 vs 全量分钟级),关掉它日常迭代显著变慢。保持现状;冷场景由 ROI-1/3 覆盖。

### ROI-6:wild 链接器 —— 低优先,搁置

`ld.wild` 软链在位但 wild 二进制已丢(config 注释自述"lost with /tmp cleanup")。链接已由 mold 承担且实测非长杆(bao_bin 1.4G 链接含在 5.6s unit)。仅当未来 debug 链接 >30s 再评估。

### 范围外登记(体量问题非缓存问题)

单次增量最重三单元 bao-servo-script 64.6s / bindings 42.6s / aws-lc-sys 55.8s 是体量固有成本,缓存只救冷场景不救必重编;根治方向(servo script 组件拆分/aws-lc 预编译产物缓存)超出本次编译配置范畴,登记不动。

---

## 5. 结论一句话

sccache 没有失效——**它从未被允许管 Rust**:116 个 workspace 成员被 dev 增量挡在门外(有意取舍),本应补位的 test-ci 被继承链意外带进增量(88G 白扔),本应救场的 dev-dist 建好未用,缓存本体还被 7 树碎片与 100G 逐出挤压。四个修复(ROI-1/2/3/4)都不改产品代码,合计预期把"每次全量"的主要触发面消除,磁盘回收 200G+。
