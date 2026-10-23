# publish-prep-20260921 — 发布闭包预备案卷(只读,纯计划;执行轮另派)

窗口:`16d793e5..HEAD`(61 commits,2026-09-21 mozjs 升级波 + 单宇宙归一波 + B2 收口)。
方法:git log --name-only 聚类 + manifest 实读 + crates.io API 实查(2026-09-21,UA 头)。
前置:用户裁决"通过验收后发布"——本卷是发布波裁决点输入(f0ba4ce8 预记),**零执行**。

---

## ① 受影响集(波内 commits 触碰的已发布/将发布 crate)

| crate | 波内触碰 | 变更性质 |
|---|---|---|
| **bao-mozjs**(vendor/mozjs/mozjs) | SM153 源整体前移(7889 文件簇) | **breaking**(SM140→153 源代际替换) |
| **bao-mozjs-sys** | SM153 + 单宇宙吸收(8d4c8260)+ extracted deps(b3f6424e)+ 链接闭包(b9ddad2e) | **breaking** |
| **bao-mozjs-src-{js,intl,python}** | 153.3.0-0 源切片重切 | **breaking** |
| **12 extracted**(vendor/mozjs/mozjs-extracted-crates,upstream-literal 名 `mozjs_*`) | 新增 in-tree(wave-A upstream-literal path+pin 形) | **新增件,未发布** |
| **bun_sm**(src/bun_sm) | SM153 moduleloading 迁移,ModuleLoadHook 统一+JobQueue 153.3 陷阱(b5b3fa96) | **breaking**(公开 API) |
| **bao_engine** | mozjs req 翻转(^0.24)+ 3 文件 | **breaking**(依赖耦合泄漏) |
| **bun_runtime**(src/bao_runtime) | 24 文件:jsstr 全迁 + 153 era | **breaking**(mozjs ^0.24 req 泄漏) |
| **bao-browser**(src/bao_browser) | B2 三片 + jsstr 收尾 + mozjs 直接 pin ^0.24 | **breaking** |
| **bao_cdp_client** | D5 EventSubscriber Carrier-A(c76775a2,single sync_channel + try_send wire face) | **breaking**(事件 API) |
| **bao_workflow_host** | NonNull qualify fix(37f88dad) | internal → patch |
| **bao-core**(src/bao,伞) | 依赖面 +4 bao- 变化 | 随闭包 bump |
| vendor/servo 族 | 单宇宙 C1/C2(manifest inline+根删,5a2d85bc/a4a3b942);script/script_bindings `js` 已 in-tree ^0.24 | internal;**波内零版本漂移**(实测 diff 仅 0.5.2 literal 化,无 bump) |
| vendor/boringssl | 第五宇宙删除(375abb6b) | internal(发布面有无=执行轮核实点) |
| vendor/ipc-channel | 仅 lock 删除 | 无发布面 |
| .plans/.claude/.spec/bench/.cargo/CLAUDE.md | 文档/工具链 | 无发布面 |

## ② 版本面谱(published → manifest → 目标;face-transition:0.x breaking=minor)

| crate | crates.io 最新(实测) | manifest 现值 | 发布目标 | 判定 |
|---|---|---|---|---|
| bao-mozjs | 0.22.1 | **0.24.0** | **0.24.0** | breaking,minor 已预跳 |
| bao-mozjs-sys | 140.14.0-1 | **153.3.0-0** | **153.3.0-0** | SM 血统版本标识(上游自身 v153.0.0-N 同形) |
| bao-mozjs-src-js/intl/python | 140.14.0-0 ×3 | **153.3.0-0** ×3 | **153.3.0-0** | 同上 |
| 12 extracted(bao- 改名后) | —(FREE) | 153.3.0 | **bao-mozjs-* 153.3.0** | 新增首发布 |
| bun_sm | 0.2.0 | 0.2.0 | **0.3.0** | 同数不同物:API breaking → minor |
| bao_engine | 0.2.0 | 0.2.0 | **0.3.0** | req ^0.24 破坏 → minor |
| bun_runtime | 0.3.0 | 0.3.0 | **0.4.0** | 同上 |
| bao-browser | 0.2.1 | 0.2.1 | **0.3.0** | 同上 |
| bao_cdp_client | 0.2.0 | 0.2.0 | **0.3.0** | 事件 API 破坏 → minor |
| bao_workflow_host | 0.2.0 | 0.2.0 | **0.2.1** | internal fix → patch |
| bao-core | 0.2.0 | 0.2.0 | **0.3.0** | 伞依赖面更新 |
| servo 族(script/script_bindings/…) | 各现行 | 0.5.2 等(零漂移) | **不强制** | internal;若随波发布按 servo runbook(77cd78ef 已 remap) |

## ③ topo 序与依赖链(边=实测 grep,合同给定序的一处实测修正)

```
 0. 验收门全绿(wave-gate 20260921 五节)——用户裁决前置
 1. 12 × bao-mozjs-*(153.3.0)        [publish 前置:名占 FREE✓;manifest package 改名或 publish 舞步]
 2. bao-mozjs-src-{js,intl,python}(153.3.0-0)  [无上游前置]
 3. bao-mozjs-sys(153.3.0-0)          [前置 1+2;publish 时 path 剥除,=153.3.0 须在库]
 4. bao-mozjs(0.24.0)                 [前置 3]
 5. bun_sm(0.3.0)                     [前置 4]
 6. bao_engine(0.3.0)                 [前置 4+5;实测 bao_engine←bun_sm]
 7. bao_workflow_host(0.2.1)          [实测 bun_runtime 依赖它 → 必须先于 bun_runtime;
                                        ⚠ 合同给定序把它放在 bun_runtime 后,此处实测修正]
 8. bun_runtime(0.4.0)                [前置 6+7]
 9. bao_cdp_client(0.3.0)             [实测 bao-browser 依赖它 → 先于 browser]
10. bao-browser(0.3.0)                [前置 6+8+9]
11. bao-core(0.3.0,伞)               [前置全部]
12. closure 扩集(机械扫):url(←browser)/uws(←cdp_client)/bun_core(←bun_runtime)/
    bao_cli/bao_bundler/bao_cdp——若其 req 对上述 crate 带版本 pin 则随波重发
    (58-crate loop 先例,33min 形态);servo 族独立 runbook 可并行
```

## ④ 阻断面盘点

| 项 | 实测 | 判定 |
|---|---|---|
| **CARGO_REGISTRY_TOKEN** | **在位**(35 字符;值未读取) | 无阻断 |
| **12 名占(bao-mozjs-unicode-bidi-ffi … bao-mozjs-utf16-iter)** | **全部 FREE**(逐个 crates.io API 实查 2026-09-21) | 无阻断;bao- 改名法可行 |
| **同名反证**:`mozjs_*`(upstream-literal 名) | servo 占有,最新 **153.0.0**(ev4 实查) | **same-name publish 被阻**(非 maintainer)→ bao- 改名法为唯一自主路径;或等 servo 发 153.3.x(节奏不在 bao 手) |
| **版本对齐缺口** | registry mozjs_* 153.0.0 ≠ 153.3.0 源(icudt77l→78l 内容差实测,f0ba4ce8) | registry 消费维持否决;in-tree path(=153.3.0 同源)为源,发布面 pin 与改名绑定 |
| **泄漏耦合传递闭包** | mozjs ^0.24 已贯通:bun_sm/bao_engine/bun_runtime(workspace dep)+ bao-browser(直接 pin)+ servo script/script_bindings(in-tree path ^0.24) | published 闭包 = 5-10 层新 minor 各带 ^0.24;下游拿新闭包无版本真空 |
| **publish 舞步项(执行轮清单)** | ①12 extracted 的 package 改名(key=mozjs_* 保持供 features/DEP_* 元数据,package=bao-mozjs-*,fork-rename-rule)②dev-dep strip(历史 runbook 形态)③0045 patch 引入的 cbindgen build-dep 版本须 registry 可解析 ④12 件发布顺序=无互依可并行(icu_data 依赖 icu_* 同批) ⑤closure 扫描(③-12) | 执行轮合同要点 |

## 结论(一句话)

发布闭包 = **12(bao- 改名,FREE 实测)+ 3 卫星 + mozjs-sys + mozjs + 6 src crate(minor/patch bump,workflow_host 先于 bun_runtime 的实测修正)+ 伞**;token 在位、名占全 FREE、无未决阻断面;唯二执行轮要点 = extracted 的改名舞步与 closure 机械扫。
