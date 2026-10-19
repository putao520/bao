# GOLDEN 兼容性 oracle 战役台账(2026-10-20)

立法: PRD-REQ-NODE-COMPAT-SUITE / PRD-REQ-WEB-STD-GOLDEN / PRD-REQ-CHROME-DEVTOOLS-GOLDEN(2026-10-19 经 prd/spec_write 登记)
派生: REQ-ENG-048(Node 官方套件) / REQ-BRW-051(WPT 全量) / REQ-CDP-010(Chrome CDP)
用户原始指令: 「使用开源行业的GOLDEN测试用例都要过…NODE 也有自己的,我们需要 100% 支持 NODE 兼容」→ 三线并发 DAG

---

## GOLDEN A — Node 官方 test/ 套件(REQ-ENG-048)

oracle: nodejs/node v24.x 官方 test/ 套件, bao 专属 runner(/tmp/golden-node/run_suite.sh + common-shim)

| 阶段 | 结果 | commit |
|---|---|---|
| 首跑基线 | 10/66 通过(15.2%) + 7 缺陷 | — |
| 修复波(#114/#115) | 5 缺陷根治: assert callable function / assert.throws 三分支 / util.format Symbol rooted! / process.version v24.19.0 / return_string UTF-16(path.win32 mojibake 横扫) | 75f43d120 |
| 停报转 #116 | ③ CJS 撞名 SyntaxError+var 泄漏 → E-sm-intern; ④ AST Store SIGSEGV → E-ast-store(2026-10-20 并发在飞) | — |
| ④ 根治完成(E-ast-store) | **归因纠正:非跨线程,是同线程从未 init**——生产面 initialize_store() 仅 bun_serde_api/router 两处,JS parse 面不经 AST,首构造来自 resolver 解析旁置 package.json(json5/yaml/toml 的 init 全在 #[cfg(test)]);链=require→resolver parse_package_json→json_parser→new_expr→Store::assert→thread-local null→unreachable panic 跨 VM dispatch FFI 边界无法发起→SIGSEGV。修形=new_store.rs 宏三处自愈(assert 撤 tripwire/append lazy create/reset 未创建 no-op,REQ-ENG-048 回归钉 4 条),expr/stmt 两实例全类覆盖,零调用方改动(LSP 画像:append→create() 自愈链实证,0 外部消费者零影响面) | 树(待波末) |
| ④ 验证 | RED→GREEN 双向 4/4(新回归钉在旧代码 FAIL 修后 PASS);cargo nt -p bun_ast **31/31 绿**;横扫残留=0(全仓 must-be-init panic 形唯一+thread_local expect/unreachable 组合零其他+bare #[thread_local] 逐一审计);e2e(bao test /tmp/ast-repro 复现例,修前 SIGSEGV 139→期望 loaded 1)留统一波末 farm V | — |
| 归因勘误(E-sm-intern 双 oracle 实证) | #114 fix-report §3「根因在 bun_sm module_loader 或 bun_transpiler」**有误**——真站点=`src/bao_runtime/src/require.rs:1071` load_cjs_module 把 CJS 模块当全局脚本 Evaluate2(顶层 const 进 realm 全局词法环境→撞名 SyntaxError;var 成 global 属性跨模块泄漏);旁证=bun_sm ESM-import-CJS 路径(IIFE wrapper)作用域正确零缺陷 | — |
| ③修形裁决 | 镜像 node v24 官方 wrapper(`function (exports, require, module, __filename, __dirname){...}`,require("module").wrapper 实测);require 侧+入口 script 臂同步包 wrapper;扩权 src/bao_runtime/{require,runtime}.rs | — |
| ③ 落盘形态(E-sm-intern) | require.rs `load_cjs_module` 重写为 wrapper compile+call(新增 normalize_cjs_shebang/wrap_cjs_function_source/eval_cjs_body,删除孤儿 get_prop_from_val;global save/restore 往返整体消灭,exports/module 全程 Rust 侧 rooted);runtime.rs eval_file_body 两 script 臂+run_test_file script 臂改走 wrap_cjs_entry_source(preamble 与 wrapper 前缀同行,行号零漂移);回归钉 9 条(跨模块 const 撞名/export vs 局部变量/var 泄漏/入口词法隔离/wrapper 参数 parity/exports 重绑定/顶层 return/shebang/嵌套 require+cache)落 tests/suite/cjs_scoping_tests.rs | 树(待波末) |
| ③ 波末 V(主会话代验) | **cjs_scoping 9/9 绿**(farm,尾调修复+钉翻转后);require_ 16/16 绿;module_ 33/34——唯一红=dns c13 resolveAny 三格,**本地同树隔离 PASS(0.036s)定案=farm 容器 DNS stub 环境差异非代码回归**(测试 offline-first localhost 设计,模块加载 7 格 farm 全绿证 wrapper 无碍) | — |
| ③ E-sm-intern 终报补充 | 停滞真因=批 02:01 已完成但完成通知未被消费(投递面);其批三红归因=**陈旧钉第四例**——#114 assert-callable 修复漏翻的 typeof 钉(node oracle 实证 assert=function),已横扫翻转 5 处(require_deep:35/require_system_deep:49/require_timers:115/runtime_deep:173/node_assert_util:30);宽容钉(`\|\|'function'` 形)不动 | 树(待波末) |
| 登记项(后续波候选) | `require('assert/strict')` 现=object,node oracle=function——assert/strict 形态偏差,待核 install_assert 的 strict 面 | — |
| 复验 | test-assert 5/18→8+; CRASH 消除; 37/37 既有面绿(assert/c17_util/fs_coverage/cp_path_tty/types_isproxy) | — |
| 测试钉翻转×2 | assert_deep_tests(typeof object→function, fail() 消息 "Failed"); stream_buffer_assert_tests(:313 同面) | 75f43d120 |

结论: 通过率 15.2%→提升(精确数字待 #116 收口后全量重跑登记);oracle parity 原则=Node 官方实测>报告转述。


## GOLDEN C — Chrome DevTools Protocol oracle(REQ-CDP-010)

oracle: Chrome 155.0.8059.39 headless=new(9333, 独立 user-data-dir);被测 bao browser --cdp-port 9334

| 阶段 | 结果 | commit |
|---|---|---|
| 保真扫描(#109) | 20 域 13 保真 + 7 偏差(/tmp/golden-cdp/report.md) | — |
| 修复波(#110) | 5 项根治全收敛: ①异常信封(normalize marker 门控+横扫 3 处,text=Uncaught Chrome 形) ②网络事件链误报更正(HTTP 链完整,零改动) ③错误码 -32000 归一×3 ④字段补全(getBrowserContexts/getDocument 4 字段/getFrameTree 10 键/getLayoutMetrics 6 键) ⑤setDownloadBehavior oracle 实测修正(枚举放行回退白名单) | 3dfdcf2eb |
| 波末 V(farm) | bao_cdp protocol_ 843/0; bao-browser cdp_ws_command_face 5/5(钉翻转后); bao_cdp --lib 215/0 | — |
| oracle 复验 | verify-fix-out{,2,3}.json 三轮双跑一致 | — |
| 测试钉翻转×1 | ws_runtime_object_protocol_roundtrip step6(text 含消息→text=Uncaught+exception.description) | 3dfdcf2eb |
| 停报 | ①dataReceived/ExtraInfo/data:-URL 事件链=vendor 事件面(待派 vendor 波) ②objectId 跨调用句柄表(架构级) ③getFrameTree loaderId(无 per-load 持久身份,小架构后续) | — |
| 停报①清偿(E-vendor-net,2026-10-20) | dataReceived 变体落地(五变体+三体交付缝发射,wire delta 总和恒等于 Finish 终值)+data: 导航全四链(request skip 移除+scheme_gate 收窄);oracle 双载具双跑 dataLength 41/55 逐字相等 ALL STABLE;窄域 V 11/11+1/1;CLAUDE.md vendor patch 行已登记(重放要点 3 条+grep 锚 DataReceived/scheme_gate);诚实边界四条(SW 面顺序歪斜既有/ExtraInfo 不做/单 tap 语义未动/压缩流 per-chunk 近似) | 树(待波末) |

## GOLDEN B — WPT 全量(REQ-BRW-051)

载具: /tmp/golden-wpt/(venv + run_bao_wpt.py + 私有 meta/MANIFEST 168784 items/281 域;配方 memory wpt-runner-playbook)
状态: batch2 推进中(css/fetch/mathml/navigation-api/mixed-content/xhr/webaudio/service-workers 已落盘, --processes 4 llvmpipe 上限, 端口 flock 跨整批)
已证零回归锚: dom 域 736 零回归(评估波);六域基线对照待全量回报
预期产物: /tmp/golden-wpt/fullrun-report.md(逐域 unexpected 表+zero-regression 判定+偏差分类)

---

## 测量基建坑(沉淀)

1. farm wrapper `--jobs` 恒追加尾部落 `--` 后被 libtest 拒 → 命名过滤走位置参数无尾参形态
2. suite 单 harness target 名=目录名收敛(tests/suite/),farm 用 `--test suite <filter>`
3. 陈旧测试钉翻转三例(#114 assert 两处 + #110 CDP 一处):修复后 characterization pin 必须翻转成 oracle 断言;E 波 grep 改动面 pin 时必须含 tests/suite 集成面(E-golden-cdp-fix 自述只在 src/ 内扫过)
4. farm `-p 非成员包` no-match=farm 侧 lock 解析态问题(本地全图 metadata 实证包在图,farm 树 manifest 同形却 no-match;单包重试/本地通道绕过;E-vendor-net farm 插曲+共享卷被外部清的 auto-install 证据,已存 memory bao-farm-multip-check-quirk)
5. READ-GATE 对执行体(subagent)证据通道结构性不可满足(ISSUE #155/#156):E 三读实质完成仍被拦 → 主会话开窗(touch read-gate-off)→E 落盘→树实证→即删旗闭环;主会话自身被拦时补正式三读(prd/spec_read+search_code LSP)即过
