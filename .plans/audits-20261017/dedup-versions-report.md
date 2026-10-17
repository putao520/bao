# 依赖多版本共存去重 campaign 报告(e-dedup-versions,2026-10-17)

用户定性:「多 crates 项目,同库多版本=二进制里多份代码」。基线:cargo tree -d --edges normal = **43 族 / 105 版本**;cargo metadata(x86_64-linux,normal 边)精确反向图 = **119 个重复版本节点**。执行两阶段:阶段 1 定量画像(只读)+ 阶段 2 安全统一执行 + 方案。**未 commit/push(按约束)**。

数据源:`/tmp/dedup-tree-d.txt`(tree -d 全量)、`/tmp/dedup-metadata.json`(单次 cargo metadata 精确图)、`/tmp/dedup-precise.txt`(版本级反向图)、`/tmp/dedup-class.tsv`(proc-macro/体积分类)。

---

## 一、阶段 1 定量画像结论

### 分类口径
- **runtime 链接**(进二进制,有尺寸代价)vs **proc-macro/build-only**(只耗编译时间):逐版本读 registry Cargo.toml 的 `proc-macro = true` + 反向图判断可达性
- 引入方归属:WS=工作区成员(bao/bun 自有,可控)/ VENDOR=vendor servo|stylo|usvg(path dep,本 campaign 禁改)/ EXT=crates.io 外部(不可改 manifest)
- 同版本双节点(syn/serde/smallvec/time-core/proc-macro2 ×2)= host/target 双构建或 build-vs-normal feature 不统一,**cargo 结构性行为,非版本去重对象**

### 核心画像:绝大多数重复是 EXT/VENDOR 锚定的不可约面
重复存在的机理=不同消费者声明互斥 req。逐族反查后,**42/43 族的最旧版本引入方全部是 EXT 或 VENDOR**;唯一整族 bao 可控面是 bun_* crate 的 strum(48 manifests)与 bun_sys 的 rustix。关键锚定链(示例):

| 族 | 旧版引入锚 | 新版引入锚 | 判定 |
|---|---|---|---|
| base64 0.22/0.23 | bcrypt、hyper-util、headers、serde_with、webdriver、content-security-policy、swc_react、rust-argon2、pem(全 EXT)+ bao-browser/bun_base64 | servo 6 组件 + usvg | EXT 锚,不可约 |
| thiserror 1/2 | webdriver 0.54(EXT) | 全图 | EXT 锚 |
| cookie 0.16/0.18 | webdriver 0.54(EXT) | servo + bao-browser | EXT 锚 |
| cssparser 0.36/0.38 | lol_html 3.0 + selectors 0.37(EXT) | stylo + selectors fork | EXT/VENDOR 锚 |
| phf 0.11/0.13/0.14 | bun_* 与 swc **共占 0.11.3**(已统一);0.13=lol_html;0.14=stylo | — | 锚定,不可约 |
| rand 0.8/0.9/0.10 | 0.8=stylo(VENDOR)+phf_generator(build-only);0.9=tungstenite 0.26+twox-hash;0.10=servo net(VENDOR) | — | 0.9 族 bao 可断 |
| digest/sha1/sha2 0.10 线 | headers、content-security-policy、swc_react、tungstenite 0.26(EXT) | RustCrypto 0.11 全家 | EXT 锚 |
| getrandom 0.2/0.3/0.4 | 0.2=rand 0.8+ring;0.3=rand 0.9;0.4=主流 | — | 0.3 随 rand 0.9 清除 |
| hashbrown 0.12/0.14/0.15/0.16/0.17 | indexmap 1.9(serde_with)、swc、petgraph、hashlink、stylo(全 EXT/VENDOR) | — | 锚定 |
| skrifa 0.44/0.46、read-fonts 0.41/0.43 | glifo(vello_cpu 链,EXT) | servo fonts | EXT 锚 |
| roxmltree 0.20/0.21 | fontdb→fontconfig-parser(EXT) | usvg | EXT 锚 |
| rustix 0.38/1.1 | **仅 bun_sys(WS)** | tempfile/wayland/x11 等 | **bao 可统一 ✓** |
| strum 0.26/0.28 | **48 个 bun_* manifest(WS,workspace pin)** | servo + stylo | **bao 可统一 ✓** |
| compact_str 0.7/0.8 | swc_ecma_codegen(EXT) | bao_cdp/bun_runtime | EXT 锚 |
| bitflags 1.3/2.x | webrender、glib、rusqlite、wayland、lol_html(全 EXT) | — | EXT 锁(生态常态) |
| darling 0.20/0.21/0.23 | stylo-derive+to-shmem-derive(VENDOR)+derive_builder/sea-query/enumset/serde_with(EXT) | — | proc-macro,上游协调 |

### 体量级(重复源码面,wc -l registry src)
执行前重复面 top(runtime 类):rustix 0.38.44 **65,294 行**、linux-raw-sys 0.4.15 **330,430 行**(const 表,编译时间大头)、hashbrown ×5(~23k 行/份)、read-fonts+skrifa 双份(~45k+37k 行/份)、rustix 1.1.4(71,857 行)、digest 双线 ~16.5k 行、base64 双份 ~15.9k 行、itertools 4 版(2 版 dev/build-only)、phf 三线、tungstenite 双份 ~10.6k 行。

---

## 二、阶段 2 执行结果(全部落地,7 crate check 绿 + 波末门通过)

### E1 rustix 0.38.44 → 1.1.4 ✓
- 变更:根 `Cargo.toml` workspace pin `0.38`→`1.1`(唯一消费者 bun_sys);`src/sys/linux_syscall.rs` 两处适配——rustix 1.x `read/pread` 改取 `Buf: Buffer<u8>` by-value,`retry(FnMut)` 闭包内改 `&mut buf[..]` 逐次 reborrow(`&mut [T]: Buffer<T>` Output=usize,返回语义零变化)
- 收益:lock 移除 **rustix 0.38.44(65,294 行)+ linux-raw-sys 0.4.15(330,430 行,编译面)**;linux-raw-sys 0.12.1 与 rustix 1.1.4 保持单份
- 验证:`cargo check -p bun_sys` RC=0

### E2 strum 0.26.3 → 0.28.0 ✓
- 变更:根 workspace pin `0.26`→`0.28`(48 manifests 经 `strum.workspace = true` 单点换版)
- 使用面核实:全树仅 8 文件 4 类用法(`IntoStaticStr` derive ×6 文件、`FromRepr` ×1、`strum(serialize_all/serialize)` 属性 ×2)——0.28 全部保留原语义;另发现 **~44 个 manifest 声明 strum 但零使用(死依赖,继承自 bun 上游)**,留作后续 hygiene(不在本 campaign 动 44 个 manifest)
- 收益:lock 移除 **strum 0.26.3(352 行 runtime)+ strum_macros 0.26.4(3,544 行 proc-macro)**
- 验证:bun_hash/bun_shell_parser/bun_core/bun_ast/bun_base64/bun_threading(6 crate 覆盖全部使用面)check RC=0(14m20s,strum 换版强制 47 消费 crate 重编,一次性成本)

### E3 twox-hash 关 `random` feature ✓(库的使用减少面)
- 变更:workspace pin 改 `default-features = false, features = ["xxhash32","xxhash64","xxhash3_64","xxhash3_128","std"]`
- 依据:bun_hash 全部调用点显式传 seed(`oneshot(seed,..)`/`with_seed(seed)`),**零 `Default()` 随机种子用法**;`random=["dep:rand"]` 是纯死边
- 收益:断 twox-hash→rand 0.9 边;rand 0.9 全族清除待 tungstenite 方案落地后自动达成
- 验证:bun_hash check RC=0

### lock diff 审计(逐族一验一滚,零意外)
`git diff Cargo.lock` = **恰好 4 包移除**(linux-raw-sys 0.4.15、rustix 0.38.44、strum 0.26.3、strum_macros 0.26.4)+ 对应依赖行删减,+68/−112 行;零版本漂移、零其他族扰动。无回滚族(三改一次通过,唯一编译适配=rustix reborrow 2 处)。

### 波末门
- `cargo check -p bao_bin --jobs 4`:见文末运行记录(结果行)
- scoped 测试面(sys 18/hash 28/threading 6/base64 20/shell_parser 1/ast 27 个 `#[test]`):见文末 cargo nt 记录
- 并发协作:根 Cargo.toml 期间被 teammate 追加 rustix `"system"` feature(加法,已保留);e-size-audit 已预告+收尾双通报

---

## 三、major 分裂方案表(不执行,只出方案)

| 族 | 现状 | 升级路径 | 破坏面 | 工作量 | 建议 |
|---|---|---|---|---|---|
| **tungstenite 0.26/0.30** | 0.26←cdp-server(bao 唯一持有者);0.30←servo net+async-tungstenite | cdp-server manifest `0.26`→`0.30`+3 文件移植 | 小:`accept`/`WebSocket::from_raw_socket`/`Role::Server`/`Error::Io` 不变;`Message::Text(String)`→`Utf8Bytes`(`data.into()` 仍编译,`From<String>` 在);`Message::Binary(Vec<u8>)`→`Bytes`(`&data` deref 兼容 `from_utf8_lossy`);`Message::Frame(_)` 臂不变 | ~0.5-1 天(含 session/event/server 三文件测试) | **升**。连带清 rand 0.9.4+rand_core 0.9.5+rand_chacha 0.9.0+getrandom 0.3.4(与 E3 合力)+sha1 0.10 一个消费边;净移除 ~5.1k 行+4 包 |
| **thiserror 1/2** | 1.0.69←webdriver 0.54(EXT,servo webdriver_server 消费) | 等 webdriver 上游发 thiserror-2 版,或 vendor servo 自提 webdriver 0.5x | webdriver crate API(EXT,不可控) | 上游依赖 | **留**(1 份 runtime ~400 行,等待上游) |
| **rand 0.8/0.9/0.10** | 0.8=stylo pin `0.8.6+small_rng`(VENDOR 锁);0.10=servo net `0.10`(VENDOR 锁);0.9=tungstenite 0.26(方案 1 后消亡) | 0.9 随 tungstenite;0.8/0.10 需 vendor manifest 变更(本 campaign 禁改) | rand 0.9→0.10 对 cdp-server 透传面=tungstenite 内部 | 方案 1 覆盖 | 0.9 **清**(随方案 1);0.8/0.10 **留**(stylo/servo 锁定面,跟随上游 rand 1.0 迁移) |
| **cookie 0.16/0.18** | 0.16.2←webdriver 0.54(EXT) | 同 thiserror 族 | webdriver API | 上游依赖 | **留** |
| **cssparser 0.36/0.38** | 0.36←lol_html 3.0+selectors 0.37(EXT,bun_lolhtml_sys 消费链);0.38=stylo/selectors fork | lol_html 新版若升 cssparser 则吸收;否则 fork selectors 0.37 线对齐(重) | cssparser 0.x minor=breaking;lol_html 升级是独立上游吸收战役 | 大(lol_html 吸收)或中(fork 对齐) | **留**;登记 lol_html 吸收波的对齐检查项 |

附带登记(非任务点名族,同方法学):
- **base64 0.22/0.23**:bao 侧(0.22,workspace pin)升 0.23 无收益——9 个 EXT 消费者(bcrypt/hyper-util/headers/serde_with/webdriver/csp/argon2/swc_react/pem)钉 0.22,0.22 必然共存。**留**
- **digest 0.10 线**(block-buffer/crypto-common/sha1/sha2/cpufeatures 0.2):headers 0.4.1+content-security-policy 0.9+swc_react(EXT)锚定。tungstenite 方案后 sha1 0.10 少一个消费者但族不灭。**留**
- **criterion 0.5.1/0.6.0**:纯 dev-dep(bench);统一需动 vendor/mozjs icu 三 manifest+bun_js_parser,收益仅 bench 编译时间。**留**(可选低成本跟进)
- **hashbrown 0.12.3**(serde_with→indexmap 1.9)/0.14.5(swc)/0.15.5(petgraph)/0.16.1(hashlink):全 EXT 锚。**留**
- **skrifa 0.44/read-fonts 0.41/roxmltree 0.20**:glifo+fontdb(EXT)锚;usvg 虽 bao-vendored 但单独升不灭族。**留**

### proc-macro 重复(编译时间项,如实定级)
darling ×3、phf_macros ×3、phf_generator ×3、cssparser-macros ×2、thiserror-impl ×2、syn 2/3、heck 0.4/0.5、proc-macro2 同版×2:引入方全部是不同上游 proc-macro crate(serde_with_macros/enumset_derive/derive_builder/sea-query/stylo-derive/to-shmem-derive/phf/cssparser-macros/webdriver/ref-cast)**各自钉不同 darling/syn/heck 线**。统一=上游协调(等各 derive crate 升级),**本地零可行杠杆,定级:上游协调项**。唯一本地可清的 proc-macro 重复(strum_macros 0.26)已由 E2 完成。

### 库的使用减少面(feature 泄漏,初判)
| 项 | 状态 |
|---|---|
| twox-hash `random` feature | **已清(E3)** |
| ~44 个 bun_* manifest 声明 strum 零使用 | 死依赖,登记(动 44 manifest 属独立 hygiene 波) |
| serde 全图 `rc` feature(非默认)被某消费者开启 | 影响=Rc<T> impls,极小;定位属后续抽样 |
| regex 1.12.3 全 unicode-* 表(vendor servo+bun_runtime/bun_js_parser/bun_sm 消费) | unicode feature 影响 regex **语义**(case folding 等),trim=语义变更非纯瘦身,不建议动 |
| tokio 全家(fs/net/signal/macros/rt-multi-thread) | vendor servo 消费,锁 |
| rand 0.8 `small_rng` / rand 0.10 `std_rng+thread_rng` | stylo/servo pin,锁 |
| linux-raw-sys 0.12.1(479k 行)| const 表,链接期 DCE,编译时间成本;随 rustix 1.1.4 保留,无版本级动作 |
| **双 TLS 栈定性**: boringssl(bao_boringssl_bridge/boringssl_sys,**stealth 铁律必需**)+ rustls 0.23.45(aws-lc-rs provider,**vendor servo net/script 直依 aws-lc-rs**,锁)+ ring 0.17.14(rustls-webpki 内部依赖,锁)。三栈共存=功能各自必需+vendor/EXT 锁定,**不做虚假去重**;aws-lc 消除需 servo manifest 变更(本 campaign 禁改),唯一 bao 侧边=rcgen 0.13(bao_boringssl_bridge 消费)但 aws-lc 由 vendor 直依锚死,rcgen feature 调整无收益 |

---

## 四、量化收益小结(执行部分,含 #70 追加波)
- lock 包数:−5(rustix 0.38.44、linux-raw-sys 0.4.15、strum 0.26.3、strum_macros 0.26.4、tungstenite 0.26.2)
- **linux 解析图额外 −4**:rand 0.9.4、rand_core 0.9.5、rand_chacha 0.9.0、getrandom 0.3.4 在 `--filter-platform x86_64-unknown-linux-gnu` 的 resolve 图中已零消费者(不出现在 linux 构建/二进制);lock 条目保留是跨平台 union(bao-ipc-channel 非 linux 面 + rav1e dev 面 + jobserver build 面),非 linux 二进制残留
- 重复面(host=linux 口径,唯一 name@ver key):**94 节点/44 族 → 79 节点/38 族**(E1-E3 后 86/40,#70 后 79/38);comm diff 逐步证明零意外增减
- 重复源码面:−~406k 行编译面(runtime 侧 rustix 0.38 65k 行 + strum 0.26 352 行 + tungstenite 0.26 5.1k 行三份实现消失;编译面大头 linux-raw-sys 330k)
- 二进制字节净变化:runtime 双份消失面≈rustix+strum+tungstenite 三份;精确字节数待 release 构建(归 e-size-audit 域)
- 不可约底座:34 族由 EXT/VENDOR req 锚定,消除路径=上游吸收(各 EXT crate 新版)或 vendor manifest 变更(本 campaign 禁改)

## 五、运行记录(终态)
- 七 crate check(bun_sys/bun_hash/bun_shell_parser/bun_core/bun_ast/bun_base64/bun_threading):RC=0
- 波末门 `cargo check -p bao_bin --jobs 4`:**Finished,RC=0**(/tmp/dedup-wave-check3.log;`^error`=0,294 处 'error' 子串全为 bun_uws_sys 上游 C 头文件 warning 噪声)
- scoped nextest(-p bun_sys -p bun_hash -p bun_threading -p bun_base64 -p bun_shell_parser -p bun_ast):**99 tests run: 99 passed, 0 skipped**(/tmp/dedup-scoped-nt.log)
- after-state 重复面:唯一 key(name@ver)口径 **94 节点/44 族 → 86 节点/40 族**;comm diff 证明恰好 4 族脱离重复列表(rustix、linux-raw-sys、strum、strum_macros——旧版移除且幸存版单化),零意外增减
- 插曲:首次波末门红在 bun_api.rs `::std::fs::umask`——归属 teammate task #67(薄库替换)在飞 WIP 的手术中间态,owner 已自行落修复(libc::umask+E0635 探针注释),与本 campaign 变更零关联,重跑即绿
- 禁 commit/push(按任务约束);工作树遗留:Cargo.toml(3 pin 改动 + teammate 的 rustix "system" feature)、Cargo.lock、src/sys/linux_syscall.rs(2 处 reborrow+注释)

## 六、追加波:#70 tungstenite 0.30 升级(已执行,同 campaign 方法学)
- 变更:`src/cdp-server/Cargo.toml` req `0.26`→`0.30`(handshake feature 保留);**零代码改动**——cdp-server 既有代码已是 0.27+ API 风格(`ws.read()`/`send()`/`from_raw_socket(stream, Role::Server, None)`),0.30 的 `Option<WebSocketConfig>` 签名一致,`Message::Text(Utf8Bytes)/Message::Binary(Bytes)` 与现有 `.into()`/deref 用法兼容
- lock:`tungstenite 0.26.2` 移除(lock 累计 −5);linux 解析图额外 −4(rand 0.9.4/rand_core 0.9.5/rand_chacha 0.9.0/getrandom 0.3.4 零消费者,lock 残留=非 linux/dev 面 union)
- 验证:`cargo check -p cdp-server` RC=0(零代码改动);`cargo nextest run -p cdp-server`:**1322/1322 PASS, 0 skipped**(测试真构造 WebSocket over loopback);波末门 `cargo check -p bao_bin` RC=0(3m25s)
- 方案表中 tungstenite 行状态:**已升**;rand 0.9 族状态:**linux 二进制已清**(lock 跨平台残留如实登记)
