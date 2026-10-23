# ev4 · vendor 四宇宙归一 — 变换设计与机械脚本(设计轮交付)

> 任务:vendor/{servo,stylo,ipc-channel,freetype-wrapper} 吸收进主 workspace 的完整变换设计与机械脚本。
> 用户书面裁决 2026-09-21「全部立即归一」。内部工程细节零产品语义(裁据类二)。
> 本轮只读产出;switch 执行待 eu1 mozjs 落地后另行派发。
> 机器可读全谱:`/tmp/ev4-design/inventory.json`;变换脚本:`/tmp/ev4-design/inline_vendor_workspace.py`;独立验证器:`/tmp/ev4-design/validate_inline.py`。

---

## 0. 总序 — 宇宙拓扑现状与归一定义

### 0.1 现状:一仓多宇宙

| 宇宙 | 根 | lock | patch 表 | 内容 |
|---|---|---|---|---|
| 主宇宙 | `Cargo.toml`(repo 根) | `Cargo.lock`(1040 pkgs,tracked) | `[patch.crates-io]` 1 条(freetype) | 全部 bao/bun crate |
| servo 宇宙 | `vendor/servo/Cargo.toml`(虚拟 manifest,`[workspace]` members=3) | `vendor/servo/Cargo.lock`(1252 pkgs,**tracked**) | `[patch.crates-io]` 1 条(freetype) | 38 组件目录 ~70 manifests |
| crown | `vendor/servo/support/crown/`(自带 lock + rust-toolchain,manifest 明言「not part of the Cargo workspace」) | `support/crown/Cargo.lock` | 无 | servo linter,零产品面 |
| stylo / ipc-channel / freetype-wrapper | **无** `[workspace]` 根(单 crate,`vendor/stylo` 含 bao-stylo + bao-stylo-derive 两 crate) | 仅 ipc-channel 有(lock tracked) | 无 | 非 member path dep / patch 目标 |
| mozjs 宇宙 | `vendor/mozjs/Cargo.toml`(`[workspace]` 根在位) | — | — | **eu1 在途**,本轮不触碰 |

两宇宙对同批共享包解析出**不同版本**(主锁 vs servo 锁 ~120 个 key 漂移,如 gstreamer 0.25.3 vs 0.25.0、hyper 1.10.0 vs 1.11.0、rand 0.8.6 vs 0.8.5)——这正是 2026-09-21 mozjs 双编译事故类:同一 crate 在两个 lock 下各编译一份、feature 归并各走各的。

### 0.2 归一定义(终态断言,五条)

1. `find vendor -maxdepth 2 -name Cargo.toml` 只剩 crate 级 manifest(无 `vendor/servo/Cargo.toml` 虚拟根;`vendor/mozjs` 按 eu1 终态);
2. `grep -rl "^\\[workspace\\]" vendor --include=Cargo.toml` 零命中;
3. 全仓只有一个 `Cargo.lock`(主根)+ 一个 `[patch.crates-io]`(主根,freetype 1 条);
4. `cargo metadata --frozen`(主根)RC=0 且 `git diff Cargo.lock` 为空(**switch 零解析变化**);
5. `cargo check -p bao_browser -p bao-servo-script --jobs 4` RC=0(产品面编译 parity)。

### 0.3 归一路线裁决:Option A「内联 + 删根 + 保 exclude」(推荐)

两个候选:

| | **Option A(推荐)**:内联继承 → 删 servo `[workspace]` 根 → 主根 exclude **保持不动**(servo 组件仍是非成员 path dep) | Option B:内联 → 删根 → 撤 exclude(组件自动成为主 workspace 成员) |
|---|---|---|
| 宇宙数 | **1**(单 lock/单 patch/单解析/单编译) | 1 |
| 主锁变化 | **零字节**(switch 后 `git diff Cargo.lock` 必须为空) | 新增 ~128 个纯新包名(winit/zbus/wayland-*/egui/wgpu/accesskit 族)——非成员的 dev-dep 本不进主锁(bao-servo 在 servo 锁有 winit dev 边、主锁无 winit 即铁证),成成员后 dev-dep 全量入锁,锁 diff 大且永久 |
| `--workspace`/`--all` 命令爆炸半径 | 零变化(`cargo fmt --all` 禁令照旧,`build --workspace` 不扫 servo) | +60 个重型成员(script/layout/webrender 链)进所有 workspace 级命令 |
| daily-ops 发布闭包 | 不变(闭包仍只算主成员;servo lockstep 线改从组件目录发) | 闭包计算必须新增「跳过 vendor 成员」规则,否则每波误发 60 个 servo crate |
| dev-surface 内部双编译 | 无 | 有(glib 0.22.4 新增 vs 现存 0.22.8 等 85 个同名异版包并行,`cargo test --workspace` 图内两份编译) |

Option A 以最小差分命中「归一」的本义(消灭第二 [workspace] 根 = 消灭第二解析),`exclude` 四条目**原样保留**成为防再成员化的护栏。双编译病根(servo 根 + mozjs 根两个平行解析)被连根拔除,而主锁零漂移可被 `--frozen` 机械证明。

### 0.4 与 eu1(mozjs)的次序依赖

本设计对 `vendor/mozjs` 零触碰(主根 `Cargo.toml:107-108` 的 mozjs path wiring 与 `exclude` 条目只读引用)。**实现轮必须在 eu1 落地后执行**;eu1 对 mozjs 的吸收应采用同构变换(内联 mozjs 成员继承 → 删 `vendor/mozjs` 虚拟根 → exclude 保留),两轮共享同一套脚本与断言。servo 组件经 `js = { package = "bao-mozjs", path = "../mozjs/mozjs", ... }`(servo 根 deps 第 172 行)指向 mozjs——内联时 path 按消费方目录重算(脚本已处理),mozjs 吸收与否不改变本变换的正确性,只改变执行时序。

---

## 1. ① 继承面全谱(vendor/servo)

- 扫描:74 个 manifest(排除 `target/`、`.venv/`);**70 个使用继承,1727 个继承点**(依赖 1303 + package 字段 424);3 个无继承(crown、`python/tidy/tests/` 两夹具)。
- 逐 manifest 明细由脚本枚举(手抄即错):`inventory.json` → `trees["vendor/servo"]["manifests"]`(每条含 `inherited_package_fields`、`inherited_deps`(按 section 含 override 字面)、`own_lints_tables`);汇总表见本文件附录 A。
- **根值来源**(全部来自 `vendor/servo/Cargo.toml`,行号):
  - `[workspace.package]`(15-30 行):version="0.5.2"、authors、repository、description、license=MPL-2.0、edition=2024、publish=false、rust-version="1.88.0"。
  - `[workspace.dependencies]`(32-335 行):331 个 key 被消费;8 个孤儿(cipher/der/digest/generic-array/hyper-rustls/num-bigint-dig/sec1/tokio-stream)零消费,随根删除消亡,无需内联。
  - **workspace 内部 path 项**(344-413 行,~69 个 bao-servo-* 条目):内联时 `path` 必须按消费方目录重算(components/X → `../X`;components/shared/X → `../../X`;跨仓 `../ipc-channel`/`../stylo`/`../mozjs/mozjs` 同理)。脚本以「根相对 path → 绝对目标 → 消费方相对」机械重算,验证器逐点比对绝对目标不变。
- **继承形态闭集**(census 实测,任何闭集外形态脚本 fail-closed 拒改):
  - package 字段:`edition`(70)/`rust-version`(69)/`repository`(69)/`authors`(69)/`license`(67)/`description`(67)/`version`(11)/`publish`(2,仅 servoshell + media/examples 继承 `publish=false`);
  - 依赖 override 键:仅 `{ workspace, optional×70, features×83, default-features×5 }`;合并语义 = 根值 base + features 并集 + default-features/optional 取 override(package/path/version 禁改,出现即 FAIL);
  - 全部单行 inline table 之外存在 **3 处多行记录**(`components/media/backends/gstreamer/Cargo.toml:47-49`、`ports/servoshell/Cargo.toml:139-148`),脚本以字符串感知括号深度聚合,含注释即拒;
  - `[lints.rust]`/`[lints.clippy]` 出现于 7 个 manifest,全部是**本地表**(servo 根无 `[workspace.lints]`,无从继承)——零变换、零残留;归一后也不落主根 lints(行为不变)。
- 版本 11 个 `version.workspace` 继承者(=0.5.2)中 9 个在主锁(path-dep 可达);`media/examples`、`servoshell` 不可达(见 §4)。

### vendor/stylo / ipc-channel / freetype-wrapper

三树 **零继承、零 patch、零 [workspace] 根**(inventory.json 逐树验证):manifest 均为自洽字面值(stylo/ipc-channel 为 cargo normalize 形态)。`vendor/stylo` = bao-stylo 0.20.2 + bao-stylo-derive 0.20.1 两 crate(后者为前者 `[dependencies.style_derive]` path dep)。**本轮零 manifest 变换**;其「吸收」= 随 servo 根删除自动并入单一解析(它们早已是主锁内 path 包:`bao-stylo` 0.20.2 / `bao-stylo-derive` 0.20.1 / `bao-ipc-channel` 0.22.0 / `freetype` 0.8.0 全部 `source=(path)` 在主锁,复用 eu1 mozjs 判据链「主锁已含传递解析=吸收零解析变化」)。

### 跨宇宙既有 wiring(内联后逐字保持)

- 主→servo:`src/bao_browser/Cargo.toml:27-28,66`(bao-servo =0.5.15 / bao-servo-url =0.5.6 / bao-servo-net =0.5.17 path dep,零改动);
- servo→主:`components/net` 等以 `path = "../../../../src/bao_boringssl_bridge"` 等字面 path dep 反向依赖 bao crate(零改动);
- servo→ipc-channel:`ipc-channel = { package = "bao-ipc-channel", version="0.22.0", path="../ipc-channel" }`(根 168 行)经 8+ 组件消费;
- servo→stylo:`style = { package = "bao-stylo", version="=0.20.2", path="../stylo" }`(根 281 行,dep key=`style` 保 `use style::` 形态 A)经 6+ 组件消费;
- servo→freetype:唯一通道是 `[patch.crates-io]`(见 §2),非 path dep;
- servo→mozjs:根 172 行 `js = { package = "bao-mozjs", ... path = "../mozjs/mozjs" }` 经 script/script_bindings/script_webgpu 消费。

## 2. ② patch 链 parity(#43 链语义逐字保持)

| 表 | 条目 | 渲染值 |
|---|---|---|
| 主根 `Cargo.toml:433-434` | `freetype` | `path = "vendor/freetype-wrapper"` |
| `vendor/servo/Cargo.toml:475-476` | `freetype` | `path = "../freetype-wrapper"` |

- 两侧各**恰好 1 条**、同名同 key、绝对目标同一目录(`vendor/freetype-wrapper/`)——**parity 完整,主根无需预补任何条目**(#43 链不会断);渲染文本不同仅是相对路径基准不同。
- 全 vendor 树无其他 `[patch.*]` 表(grep -F 实证:仅上述两处 + 叙述性注释);无 URL patch。
- `ipc-channel`/`js`/`style` 是 path+package wiring,**不是** patch(W0b 裁决:[patch] 不能承载 rename 包,主根 413-421 行注释记录)——内联后 wiring 逐字下放,语义不变。
- **单侧化后**:servo 根删除 → servo 侧 patch 表随之消亡;主根 1 条 freetype patch 成为全宇宙唯一真源,对非成员 path dep 同样生效(patch 作用于当前 workspace 的整个解析,这正是今天主锁把 `freetype 0.8.0` 解析为 path 包的机制)。
- 证据不足项(如实标注):「单侧 patch 下 `cargo publish` 组件目录的 publish-verify 解析」无法静态判定——组件目录发布时主根 patch 不在场,freetype 链会解析到 **registry** freetype 0.8.0(freetype-sys ^0.23)。E17 符号对照(77 native fn 双版全导出)支持可编译,但需实现轮以一次 `cargo publish --dry-run -p bao-servo-net`(组件目录)实测收口。

## 3. ③ 工具链 / target / lock 现状

| 项 | vendor/servo | vendor/stylo | vendor/ipc-channel | vendor/freetype-wrapper |
|---|---|---|---|---|
| rust-toolchain | `rust-toolchain.toml`(**channel 1.95.0 stable** + clippy/llvm-tools/rustc-dev/rustfmt/rust-src,tracked);`support/crown/` 另有 1 份 | 无 | 无 | 无 |
| 自带 Cargo.lock | 有(1252 pkgs,**tracked**) | 无 | 有(134 pkgs,tracked) | 无 |
| target/ | 有(未 tracked;debug/doc/flycheck1/package) | 有(debug/doc/package——publish/package 验证痕迹) | 有(CACHEDIR/package) | 无 |
| 主仓工具链 | 主根 `rust-toolchain.toml` = **nightly-2026-07-20**(components: rustfmt, clippy) | 同左 | 同左 | 同左 |

处置(进入 switch commit):
- **删** `vendor/servo/rust-toolchain.toml`:1.95.0 stable 钉与主宇宙 nightly-2026-07-20 冲突;servo 组件今天就是在主 nightly 下编译的(主锁 RC=0 日常构建即证)。保留它会让任何按 CWD 解析工具链的载体(rust-analyzer 打开 vendor/servo 下文件、子目录内 cargo 调用)拿错工具链。
- **删** `vendor/servo/Cargo.lock`(宇宙消亡,lock 成无根死文件);
- `vendor/servo/.cargo/config.toml`(android/windows linker 表):保留为惰性文件。主根 `cargo` 调用不读它(cargo 只从调用 CWD 向上合并 config);仅当有人未来在 vendor/servo 子树内交叉编译 android 时才有意义。与 E18/E19 的 android 腿删除裁决不冲突(该裁决动的是 fonts manifest,不是本 config)。
- `vendor/servo/.clippy.toml`、`rustfmt.toml`:保留(rustfmt/clippy 配置按被处理文件就近查找,对 vendor 内文件的格式化纪律仍生效;`cargo fmt --all` 禁令照旧,scripts/local-ci.sh:9 注释语义不变)。
- `support/crown/`(自带 lock/toolchain/manifest)与 `python/tidy/tests/` 夹具:零触碰(crown 自我声明非 workspace 成员;两夹具无继承)。
- `vendor/ipc-channel/Cargo.lock` + `target/`:**保留**(单 crate 独立 dev loop 的复现资产,对主宇宙零交互;主锁恒以 path 语义解析它,不读这份 lock)。同理由保留 `vendor/stylo/target/`(未 tracked 的本地产物)。
- servo 根 `[profile.dev.package.num-bigint-dig] opt-level=3` 与 `[profile.dev.package.tikv-jemalloc-sys] opt-level=1`(421-428 行):**随根消亡,无需搬运**——profile 按 workspace 生效,主宇宙构建从来不曾使用它们(这两个 override 只作用于 servo 宇宙的 dev 构建);`num-bigint-dig` 在主锁根本不存在,`tikv-jemalloc-sys` 在主宇宙本就走主 profile。零行为变化。

## 4. ④ standalone 消费方横扫(谁会 cd 进 vendor 跑 cargo / 依赖该宇宙)

全仓 grep(`*.sh/*.py/*.md/*.toml/*.yml/Makefile/.github/.claude`),排除 vendor 内部与 target:

**活消费方(实现轮必须更新或重映射):**
1. `.claude/daily-ops/reports/2026-09-04.md:52`:`(cd vendor/servo && cargo nextest run -p bao-servo-net --lib)`(历史报告,佐证该工作流真实存在);
2. `.claude/daily-ops/reports/2026-09-19.md:38`:`cd vendor/servo && cargo check -p bao-servo-script --jobs 4` → RC=0;
3. daily-ops 发布线:`.claude/daily-ops/reports/2026-09-17.md:40`、`2026-09-08.md:45`、`2026-09-19.md:50` 等——「双 workspace 反向闭包 = 主线 78 + bao-servo lockstep 16~18」是日常现实,servo crate 发布从 servo 宇宙出发(`cargo publish -p bao-servo-*`,含 STRIP→publish→RESTORE 自指 dev-dep 解法)。**根删除后 remap**:验证/check/test 类改主根 `cargo check|nt -p bao-servo-*`(-p 匹配全图包,非成员同样可选中);**publish 改组件目录** `cargo publish --manifest-path vendor/servo/components/<c>/Cargo.toml`(或 cd 进组件目录)——自含 manifest 使其成为合法单 crate 发布。副作用(如实标注):从主根跑 `cargo test -p bao-servo-*` 会按需解析该 crate dev-deps 并**一次性增长主锁**(预测闭包名单:`/tmp/ev4-design/predicted_lock_additions.json`,~128 纯新名 + 85 同名现有包 req 复核;`cargo test -p bao-servo` 最重,会把 wgpu/webxr/winit 族 dev 闭包拉进主锁)。建议协议写明:.servo crate 测试按需逐 crate 跑,跑前预期锁增长、跑后 `git diff Cargo.lock` 审计。
4. `.claude/upstream-baseline.json`(SSOT):servo 条目 notes 引用 `vendor/servo 快照裁剪源`——吸收流程持续向 vendor/servo 投放文件,**不依赖** servo workspace 根(逐文件重放),无需变更;实现轮在 notes 追加一行「workspace 根已删,验证一律主根 -p」。
5. `.claude/skills/daily-ops/references/upstream-daily.md:42-43`、`upstream-absorb/SKILL.md`、项目 `CLAUDE.md`(servo 定制文件清单/双 workspace patch 叙述):叙述性引用,实现轮补一段「2026-09-2x 归一后的宇宙拓扑」。
6. `scripts/local-ci.sh:9`:`cargo fmt` 禁 `--all`(递归 vendor/servo)——**禁令保持**(Option A 下 servo 组件非成员,`--all` 本不会碰它们,注释语义升级为双保险)。
7. `docs/build-macos.md`(219/265/288-292 行)、`docs/CLUSTER-IPC-DESIGN.md:133`、`src/BUG-KNOWLEDGE.md`、`THIRD_PARTY_LICENSES.md`(30-51,191-209 行):路径/叙述引用,零动作;THIRD_PARTY_LICENSES 第 33 行「exclude 显式排除」叙述在 Option A 下仍为真。
8. `.github/workflows/platform-matrix.yml:108`:`cargo check -p bao_uloop -p bun_uws_sys -p bun_libuv_sys --locked`——全是主成员,零影响。
9. `vendor/servo/mach`/`python/`(servo 自建体系):根删除后 `mach` 失效。**无任何 bao 工具链调用 mach**(grep 零命中)——接受失去,无需迁移。
10. `scripts/sm-audit/*.py`、`.claude/sm-capability-ledger.json`、`scripts/bootstrap.sh` 等:仅引用 `vendor/mozjs`(eu1 域),零 servo/ipc/stylo/freetype 引用。

**零消费方**(横扫阴性):Makefile(无 vendor 引用)、`.github/`(无 vendor 引用)、`bench/`、Rust 源码注释外零路径依赖。

## 5. ⑤ 机械内联变换脚本 + 实现轮执行序

### 5.1 脚本(已交付、已在 rehearsal 实测)

| 文件 | 作用 |
|---|---|
| `/tmp/ev4-design/inline_vendor_workspace.py` | 行级保注释内联:解析树根 `[workspace.package]/[workspace.dependencies]` → 逐 member manifest 把继承键替换为字面值(path 按消费方目录重算;features 并集;多行 inline table 字符串感知聚合;注释全保留;`--dry-run` 默认输出 unified diff,`--apply` 写盘)。**fail-closed**:闭集外 override 键 / 多行内注释 / 根缺 key / 改写后仍有 `workspace = true` 残留 → 立即 FAIL,零部分提交。**幂等**:第二轮 0 rewrite(实测)。 |
| `/tmp/ev4-design/validate_inline.py` | 独立验证器(不同代码路径):从干净树(worktree 取 HEAD)重算全部继承点期望值逐点对照。实测 `checked 1727 inheritance sites: ERR=0; unchanged manifests: 4`。 |
| `/tmp/ev4-design/analyze_vendor_workspaces.py` | 全谱分析器(本轮产出 inventory.json/summary.md 的真源)。 |
| `/tmp/ev4-design/inventory.json` · `predicted_lock_additions.json` | 机器可读全谱;on-demand dev-dep 增长预测名单。 |

脚本通过条件(已在 `/tmp/ev4-design/_rehearsal/` 对整树副本实测):70 manifest 重写、1727/1727 继承点语义相等、非继承键零改动、注释保留、绝对 path 目标不变、二次运行零 diff、逐 manifest tomllib 可解析。

### 5.2 实现轮执行序(每仓/每阶段一个原子 commit)

前置:`eu1` mozjs 吸收已落地(或明确排队);全程遵守 §10(worktree 验证)与波末一次测。

1. **C1(adapter,零行为变化)**:`python3 inline_vendor_workspace.py --apply --tree vendor/servo`。
   验收:`validate_inline.py`(对 worktree 干净树)ERR=0;`cargo metadata --frozen`(主根)RC=0 且 `git diff Cargo.lock` **空**;`cargo metadata --frozen --manifest-path vendor/servo/Cargo.toml` RC=0 且 servo lock 亦零 diff(adapter 态两宇宙仍各自自洽)。stylo/ipc-channel/freetype-wrapper 无继承,本步零文件变化。
2. **C2(switch·servo)**:`git rm vendor/servo/Cargo.toml vendor/servo/Cargo.lock vendor/servo/rust-toolchain.toml`;主根 exclude **不动**。
   验收(=总序 §0.2 断言):`cargo metadata --frozen` RC=0 + `git diff Cargo.lock` 空(零解析变化,最强门);`cargo check -p bao_browser -p bao-servo-script --jobs 4` RC=0;`find vendor -maxdepth 2 -name Cargo.toml` 无 servo 根。
3. **C3(switch·stylo/ipc-channel/freetype-wrapper)**:零 manifest 变更的确认性 commit——跑总序断言脚本并把三树终态(无 [workspace] 根/无 patch/主锁 path 包在位)写进 commit message;ipc-channel lock/target 保留(§3)。
4. **C4(协议收口)**:更新 §4 清单 1-5 的活消费方(daily-ops 协议 remap + upstream-baseline notes + CLAUDE.md 拓扑段);`publish --dry-run` 一次 bao-servo-net(组件目录)实测补上 §2 唯一「证据不足」项,结论写回本 dossier 尾注。
5. **终态断言脚本(收口门,一次性内联执行)**:

```bash
set -e
[ -z "$(git status --porcelain Cargo.lock)" ] && echo "A5 lock-zero-diff OK" || echo "A5 FAIL"
cargo metadata --frozen --format-version 1 >/dev/null && echo "A5 metadata-frozen OK"
find vendor -maxdepth 2 -name Cargo.toml | sort   # 期望: 仅 crate 级(servo 根缺席;mozjs 按 eu1)
! grep -rl "^\[workspace\]" vendor --include=Cargo.toml --exclude-dir=target | grep -q . && echo "A5 no-ws-root OK"
[ "$(grep -c "^\[patch.crates-io\]" Cargo.toml)" = "1" ] && echo "A5 single-patch OK"
cargo check -p bao_browser -p bao-servo-script --jobs 4
```

回滚:C1/C2 均为独立 commit,任一门红灯 `git revert` 该 commit 即回原宇宙拓扑(C1 后两宇宙仍各自可解析,回滚无残留)。

---

## 附录 A · vendor/servo 继承面逐 manifest 汇总(脚本枚举)

(由 inventory.json 生成,机器真源;`sites`=该 manifest 继承点总数,`fields`=继承的 package 字段)

| manifest | sites | 继承的 package 字段 | 依赖继承分布 |
|---|---|---|---|
| `vendor/servo/Cargo.toml` | 0 | — | 仅 package 字段 |
| `vendor/servo/components/allocator/Cargo.toml` | 15 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×3 dependencies(target)×3 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/background_hang_monitor/Cargo.toml` | 18 | authors,description,edition,license,repository,rust-version | deps:dependencies×9 dependencies(target)×1 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/bluetooth/Cargo.toml` | 18 | authors,description,edition,license,repository,rust-version | deps:dependencies×12 |
| `vendor/servo/components/canvas/Cargo.toml` | 29 | authors,description,edition,license,repository,rust-version | deps:dependencies×23 |
| `vendor/servo/components/config/Cargo.toml` | 13 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×6 |
| `vendor/servo/components/config/macro/Cargo.toml` | 11 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×4 |
| `vendor/servo/components/constellation/Cargo.toml` | 47 | authors,description,edition,license,repository,rust-version | deps:dependencies×40 dependencies(target)×1 |
| `vendor/servo/components/default-resources/Cargo.toml` | 7 | authors,description,edition,license,repository,rust-version | deps:dependencies×1 |
| `vendor/servo/components/deny_public_fields/Cargo.toml` | 10 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×3 |
| `vendor/servo/components/devtools/Cargo.toml` | 30 | authors,description,edition,license,repository,rust-version | deps:build-dependencies×1 dependencies×23 |
| `vendor/servo/components/dom_struct/Cargo.toml` | 11 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×4 |
| `vendor/servo/components/fonts/Cargo.toml` | 62 | authors,description,edition,license,repository,rust-version | deps:dependencies×42 dependencies(target)×1 dependencies(target)×3 dependencies(target)×1 dependencies(target)×6 dependencies(target)×3 |
| `vendor/servo/components/geometry/Cargo.toml` | 12 | authors,description,edition,license,repository,rust-version | deps:dependencies×6 |
| `vendor/servo/components/hyper_serde/Cargo.toml` | 13 | edition,version | deps:dependencies×7 dev-dependencies×4 |
| `vendor/servo/components/jstraceable_derive/Cargo.toml` | 10 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×3 |
| `vendor/servo/components/layout/Cargo.toml` | 61 | authors,description,edition,license,repository,rust-version | deps:dependencies×54 dev-dependencies×1 |
| `vendor/servo/components/malloc_size_of/Cargo.toml` | 42 | authors,description,edition,repository,rust-version | deps:dependencies×37 |
| `vendor/servo/components/media/audio/Cargo.toml` | 23 | authors,description,edition,license,repository,rust-version | deps:dependencies×17 |
| `vendor/servo/components/media/backends/auto/Cargo.toml` | 8 | authors,description,edition,license,repository,rust-version | deps:dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/media/backends/dummy/Cargo.toml` | 13 | authors,description,edition,license,repository,rust-version | deps:dependencies×7 |
| `vendor/servo/components/media/backends/gstreamer/Cargo.toml` | 31 | authors,description,edition,license,repository,rust-version | deps:dependencies×23 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/media/backends/gstreamer/render/Cargo.toml` | 8 | authors,description,edition,license,repository,rust-version | deps:dependencies×2 |
| `vendor/servo/components/media/backends/gstreamer/render-android/Cargo.toml` | 13 | authors,description,edition,license,repository,rust-version | deps:dependencies×7 |
| `vendor/servo/components/media/backends/gstreamer/render-unix/Cargo.toml` | 15 | authors,description,edition,license,repository,rust-version | deps:dependencies×9 |
| `vendor/servo/components/media/backends/ohos/Cargo.toml` | 22 | authors,description,edition,license,repository,rust-version | deps:build-dependencies×1 dependencies×15 |
| `vendor/servo/components/media/examples/Cargo.toml` | 15 | authors,description,edition,license,publish,repository,rust-version,version | deps:dependencies×7 |
| `vendor/servo/components/media/media-thread/Cargo.toml` | 18 | authors,description,edition,license,repository,rust-version | deps:dependencies×12 |
| `vendor/servo/components/media/player/Cargo.toml` | 12 | authors,description,edition,license,repository,rust-version | deps:dependencies×6 |
| `vendor/servo/components/media/servo-media/Cargo.toml` | 12 | authors,description,edition,license,repository,rust-version | deps:dependencies×6 |
| `vendor/servo/components/media/servo-media-derive/Cargo.toml` | 10 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×3 |
| `vendor/servo/components/media/streams/Cargo.toml` | 9 | authors,description,edition,license,repository,rust-version | deps:dependencies×3 |
| `vendor/servo/components/media/traits/Cargo.toml` | 8 | authors,description,edition,license,repository,rust-version | deps:dependencies×2 |
| `vendor/servo/components/media/webrtc/Cargo.toml` | 9 | authors,description,edition,license,repository,rust-version | deps:dependencies×3 |
| `vendor/servo/components/metrics/Cargo.toml` | 14 | authors,description,edition,license,repository,rust-version | deps:dependencies×8 |
| `vendor/servo/components/net/Cargo.toml` | 82 | authors,description,edition,license,repository,rust-version | deps:dependencies×70 dev-dependencies×6 |
| `vendor/servo/components/paint/Cargo.toml` | 40 | authors,description,edition,license,repository,rust-version | deps:dependencies×33 dev-dependencies×1 |
| `vendor/servo/components/pixels/Cargo.toml` | 15 | authors,description,edition,license,repository,rust-version | deps:dependencies×8 dev-dependencies×1 |
| `vendor/servo/components/profile/Cargo.toml` | 17 | authors,description,edition,license,repository,rust-version | deps:dependencies×8 dependencies(target)×1 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/script/Cargo.toml` | 149 | authors,description,edition,license,repository,rust-version | deps:dependencies×142 dependencies(target)×1 |
| `vendor/servo/components/script_bindings/Cargo.toml` | 43 | authors,description,edition,repository,rust-version | deps:build-dependencies×3 dependencies×34 dev-dependencies×1 |
| `vendor/servo/components/script_webgpu/Cargo.toml` | 21 | authors,description,edition,license,repository,rust-version | deps:dependencies×15 |
| `vendor/servo/components/servo/Cargo.toml` | 85 | authors,description,edition,license,repository,rust-version | deps:dependencies×61 dev-dependencies×10 dependencies(target)×1 dependencies(target)×1 dependencies(target)×2 dependencies(target)×1 dependencies(target)×2 dependencies(target)×1 |
| `vendor/servo/components/servo_tracing/Cargo.toml` | 11 | authors,description,edition,license,repository,rust-version,version | deps:dependencies×3 dev-dependencies×1 |
| `vendor/servo/components/shared/background_hang_monitor/Cargo.toml` | 8 | authors,description,edition,license,repository,rust-version | deps:dependencies×2 |
| `vendor/servo/components/shared/base/Cargo.toml` | 24 | authors,description,edition,license,repository,rust-version | deps:dependencies×15 dependencies(target)×1 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/shared/bluetooth/Cargo.toml` | 10 | authors,description,edition,license,repository,rust-version | deps:dependencies×4 |
| `vendor/servo/components/shared/canvas/Cargo.toml` | 22 | authors,description,edition,license,repository,rust-version | deps:dependencies×16 |
| `vendor/servo/components/shared/constellation/Cargo.toml` | 36 | authors,description,edition,license,repository,rust-version | deps:dependencies×30 |
| `vendor/servo/components/shared/devtools/Cargo.toml` | 16 | authors,description,edition,license,repository,rust-version | deps:dependencies×10 |
| `vendor/servo/components/shared/embedder/Cargo.toml` | 33 | authors,description,edition,license,repository,rust-version | deps:dependencies×27 |
| `vendor/servo/components/shared/fonts/Cargo.toml` | 24 | authors,description,edition,license,repository,rust-version | deps:dependencies×17 dependencies(target)×1 |
| `vendor/servo/components/shared/layout/Cargo.toml` | 33 | authors,description,edition,license,repository,rust-version | deps:dependencies×27 |
| `vendor/servo/components/shared/net/Cargo.toml` | 42 | authors,description,edition,license,repository,rust-version | deps:dependencies×35 dev-dependencies×1 |
| `vendor/servo/components/shared/paint/Cargo.toml` | 35 | authors,description,edition,license,repository,rust-version | deps:dependencies×29 |
| `vendor/servo/components/shared/profile/Cargo.toml` | 16 | authors,description,edition,license,repository,rust-version | deps:dependencies×10 |
| `vendor/servo/components/shared/script/Cargo.toml` | 37 | authors,description,edition,license,repository,rust-version | deps:dependencies×31 |
| `vendor/servo/components/shared/storage/Cargo.toml` | 13 | authors,description,edition,license,repository,rust-version | deps:dependencies×7 |
| `vendor/servo/components/shared/webgpu/Cargo.toml` | 15 | authors,description,edition,license,repository,rust-version | deps:dependencies×9 |
| `vendor/servo/components/shared/webxr/Cargo.toml` | 14 | authors,edition,license,repository,rust-version | deps:dependencies×9 |
| `vendor/servo/components/storage/Cargo.toml` | 27 | authors,description,edition,license,repository,rust-version | deps:dependencies×18 dev-dependencies×3 |
| `vendor/servo/components/timers/Cargo.toml` | 9 | authors,description,edition,license,repository,rust-version | deps:dependencies×3 |
| `vendor/servo/components/url/Cargo.toml` | 13 | authors,description,edition,license,repository,rust-version | deps:dependencies×7 |
| `vendor/servo/components/wakelock/Cargo.toml` | 8 | authors,description,edition,license,repository,rust-version | deps:dependencies×2 |
| `vendor/servo/components/webdriver_server/Cargo.toml` | 31 | authors,description,edition,license,repository,rust-version | deps:dependencies×25 |
| `vendor/servo/components/webgl/Cargo.toml` | 25 | authors,description,edition,license,repository,rust-version | deps:dependencies×19 |
| `vendor/servo/components/webgpu/Cargo.toml` | 21 | authors,description,edition,license,repository,rust-version | deps:dependencies×12 dependencies(target)×1 dependencies(target)×1 dependencies(target)×1 |
| `vendor/servo/components/webvtt/Cargo.toml` | 9 | authors,description,edition,license,repository,rust-version | deps:dependencies×3 |
| `vendor/servo/components/webxr/Cargo.toml` | 17 | authors,edition,license,repository,rust-version | deps:dependencies×10 dependencies(target)×2 |
| `vendor/servo/components/xpath/Cargo.toml` | 10 | authors,description,edition,license,repository,rust-version | deps:dependencies×4 |
| `vendor/servo/ports/servoshell/Cargo.toml` | 67 | authors,description,edition,license,publish,repository,rust-version,version | deps:build-dependencies×1 dependencies×20 dependencies(target)×1 dependencies(target)×4 dependencies(target)×12 dependencies(target)×1 dependencies(target)×13 dependencies(target)×2 dependencies(target)×2 dependencies(target)×2 build-dependencies(target)×1 |
| `vendor/servo/python/tidy/tests/Cargo.toml` | 0 | — | tidy 夹具 |
| `vendor/servo/python/tidy/tests/path_dependency/Cargo.toml` | 0 | — | tidy 夹具 |
| `vendor/servo/support/crown/Cargo.toml` | 0 | — | 非成员(crown 自带 lock/toolchain) |
