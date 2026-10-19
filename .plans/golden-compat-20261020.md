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
| 归因勘误(E-sm-intern 双 oracle 实证) | #114 fix-report §3「根因在 bun_sm module_loader 或 bun_transpiler」**有误**——真站点=`src/bao_runtime/src/require.rs:1071` load_cjs_module 把 CJS 模块当全局脚本 Evaluate2(顶层 const 进 realm 全局词法环境→撞名 SyntaxError;var 成 global 属性跨模块泄漏);旁证=bun_sm ESM-import-CJS 路径(IIFE wrapper)作用域正确零缺陷 | — |
| ③修形裁决 | 镜像 node v24 官方 wrapper(`function (exports, require, module, __filename, __dirname){...}`,require("module").wrapper 实测);require 侧+入口 script 臂同步包 wrapper;扩权 src/bao_runtime/{require,runtime}.rs | — |
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
