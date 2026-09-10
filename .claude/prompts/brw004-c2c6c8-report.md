# REQ-BRW-004 C2/C6/C8 live 覆盖补齐报告

- 执行角色:E(测试覆盖,tester 域),基线 HEAD = `f77faf8b` + 本任务测试增量。
- 任务:V 批放行项——C2/C6/C8 的 SPEC criterion 文本含页→Worker postMessage/结构化克隆/importScripts+crypto+performance+location 语义,但全树零对应 live 断言(v47 ③节)。
- 本任务**零产品源码改动**。全部增量 = 新测试文件 `src/bao_browser/tests/suite/worker_realm_api_tests.rs`(4 个 live e2e 测试,`@trace TEST-BRW-004 [req:REQ-BRW-004] [criterion:2,6,8] [level:integration]`)+ `main.rs` 一行注册。
- 测试范式:`BAO_TEST_NETWORK=1 xvfb-run -a cargo nextest run --cargo-profile test-ci -p bao-browser -E 'test(worker_realm_api)'` → **7/7 绿,退出码 0**(4 e2e + 3 common 单元)。每测试经 `common::run_isolated` 自隔离子进程真跑。

## 断言车辆(为何与既有 worker 族不同)

既有 live worker 车辆全部用 data: URL worker 且只传 worker→page 字符串。本文件改用 **http-served worker 脚本**(本机 H1 fixture,per-path 路由,JS MIME)三点原因:

1. `importScripts` 相对解析基于 worker 自身 URL(workerglobalscope.rs `join(worker_url)`),data: base 无法承载同源相对导入;
2. WorkerLocation href/origin 断言需要真实 http origin 才有意义(Rust 侧与 fixture URL 精确比对);
3. 页面与 worker 同 fixture origin,满足 classic worker 同源规则。

## 三 criterion 结论(每 criterion 一行)

### C2 — worker.postMessage(msg) 页→Worker,Worker onmessage 接收:**GREEN(补齐)**

- 测试:`worker_realm_api_c2_page_to_worker_postmessage_echo`
- 证据:`[c2] rx=C2ECHO|ok=1|x=1 hits=["/", "/wk_msg.js"]`
- 断言语义:页面在 `new Worker(url)` 返回后**立即** post(SPEC 原话 `{x:1}` 目标形态——早发消息依赖 port message queue 排队到 worker 脚本装好 `self.onmessage` 为止);worker 侧深检收到的是**对象**(`payload.x === 1`、tag 完整、非字符串化);echo 回程 verdict 同回合证明 worker→page 腿。fixture 命中 `"/wk_msg.js"` 证明 worker 脚本真实网络加载。

### C6 — Structured Clone(对象/数组/Buffer/ArrayBuffer/Transferable)+ importScripts:**GREEN(补齐)**

- 测试 1:`worker_realm_api_c6_structured_clone_roundtrip_five_types`(超任务下限:六型、双向、深比较)
  - page→worker 证据:`[c6-clone-in] CLONEIN|plain=1|arr=1|nested=1|ab=1|map=1|u8=1`
  - worker→page 证据:`[c6-clone-out] OUT|plain=1|arr=1|nested=1|ab=1|map=1|u8=1`
  - 六型 = plain object(含 null 字段)/ array / 嵌套对象+数组图 / ArrayBuffer(8B **逐字节内容**比对)/ Map(size+逐 entry)/ Uint8Array(SPEC "Buffer" 面)。worker→page 方向由 worker **构造**同形异值复合对象 post 回,页面侧深检——双向都不是"发个字符串回来"。
- 测试 2:`worker_realm_api_c6_import_scripts_helper_globals`
  - 证据:`[c6-import] C6IMPORT|ok=1|fn=1|call=1|mark=1|second=1 hits=["/", "/wk_import.js", "/helper.js"]`
  - 断言语义:`importScripts('/helper.js')` 相对解析→同源 fetch(**server 侧命中 `/helper.js`** = 双侧证明:传输+效果);helper 的**函数声明**在 worker 全局可调用(`helperTriple(21)===63`)、imported 脚本写入的 `self.HELPER_MARK` 可见(证明脚本在 worker scope 真实执行,非仅 fetch)。

### C8 — DedicatedWorkerGlobalScope API(crypto/performance/location):**GREEN(补齐)**

- 测试:`worker_realm_api_c8_crypto_performance_location`
- 证据:`[c8] C8|href=http://127.0.0.1:7551/wk_c8.js|origin=http://127.0.0.1:7551|proto=http:|host=127.0.0.1:7551|uuid=1|uuid4=1|grv=1|pnow=1|pnowdt=11.190`
  - `location.href`/`origin`:与 Rust 侧 fixture 计算的 worker URL/origin **精确相等**(`proto=http:`、host 含端口);非仅"存在 location 对象"。
  - `crypto.randomUUID`:两次结果均匹配 8-4-4-4-12 UUID 形状且互异;**附证(report-only 标记)**:`uuid4=1` — 严格 v4(版本 4 + variant 8/9/a/b)亦符合,超 SPEC 断言面仅记录不门禁。
  - `crypto.getRandomValues`:**恒等返回**(返回所填同一 view)、16 字节宽、两次抽取互异(真实熵,非零 buffer)。
  - `performance.now()`:数字、非负、跨真实计算(2M 迭代)严格递增,实测 delta 11.190ms。

## ④ 既有 worker 族零回归

| 族 | 结果 | 判定 |
|---|---|---|
| worker 名义族(worker_tests / concurrent / fingerprint / multi_injection / onerror / stealth_worker_audio / mediation / controller + 本文件 4 新) | 147 绿,1 红 | 红 = `serviceworker_fetchevent_tests::c19_sw_realm_exposes_fetchevent_pipeline_live` — **v47 R1 已知确定性存量红**(引入即红,同签名),非本任务引入 |
| stealth_offscreencanvas_tests | 15/15 绿 | 与 v47 基线一致 |
| fetch_axis_probe_tests(HEAD 基线四测 b_abs/b_rel/t_settimeout/c_worker_realm) | 4/4 绿 + 3 单元绿 | 基线面零回归 |
| fetch_axis_probe_urlshape_double_slash | 1 红 | **非基线、非本任务**:`git show HEAD … | grep -c urlshape` = 0,该测是工作树中他 agent 的未提交 +55 行在途新增(裁定 fetchevent R1 的对照探针),从未在任何 commit 验证绿;失败形态为 `//dbl_probe` 被出口规整为 `/dbl_probe`(fixture 只见单斜杠) |

## 执行环境备注(共享脏工作树归属事实)

- 本任务两次运行之间,工作树被他人 in-flight vendor 编辑瞬时打断编译(`vendor/servo/components/net/fetch/bun_bridge.rs:2060` E0599 `BunHttpResponse.status/headers` — S2b net-intercept 域在途改动,非本任务文件)。family 回归改用 **run-1 已构建且含本任务最终测试源码的二进制**(`/var/cargo-builds/3c/6184ceb77072ba/test-ci/deps/suite-1c63a1c1c843a986`,构建于本任务测试文件定稿后、源码未再变)直接执行,evidence 真退出码取得,未受 vendor churn 影响。
- 工作树归属审计:`git status` 中本任务 diff 仅 `worker_realm_api_tests.rs`(新)+ `main.rs` +1 行(`mod worker_realm_api_tests;`);`main.rs` 的 `mod sw_stealth_profile_tests;`、fetch_axis +55 行、serviceworker_fetchevent ±6 行、vendor 5 文件、Cargo.toml 均为他人 in-flight,未触碰。

## 结论

- **C2 / C6 / C8 三 criterion 全部 GREEN**——v47 ③节所记三个"browser-realm live 覆盖缺口"已由本文件 4 个 live 测试补齐,含双向结构化克隆六型深比较、importScripts 双侧证明、location 精确相等与 crypto/performance 真值断言。
- 既有 worker 族零回归(唯一红 = v47 已知存量 fetchevent 红 + 一条 HEAD 不存在的他人在途探针红,均如实归属,与本任务无关)。
- REQ-BRW-004 criterion 矩阵(v47 口径)由此:C2/C6/C8 red→green;放行条件中「C2/C6/C8 缺口补测」一项完成。
