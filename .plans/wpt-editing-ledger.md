# WPT editing 域 campaign 台账(e150,REQ-BRW-002,C1 验证清偿)

- 日期:2026-10-08(首跑);载具 /tmp/e150;状态:**停等裁定**(见 §C)
- 合同:e150——editing campaign 首跑 + 三账分账 + bao e2e pin + ini 落 vendor + 本台账
- 输入:e149 侦察报告(/tmp/e149-editingcontext-recon.md);载具配方 e121/e137/e147 台账

## A. 载具与二进制

| 项 | 值 |
|---|---|
| 二进制 | /tmp/e147-wt-target/test-ci/bao;worktree /tmp/e147-wt 钉 HEAD 06050b70 自建(增量 2m05s;e147 worktree 复用灭菌路线——其脏 diff 9 文件已核实全部 = commit 13a5c4a1 内容,零信息丢失) |
| 载具 | /tmp/e150:venv(拷自 /tmp/e147-wpt-veh + `python -m pip install pyyaml`;注意 bin/* shebang 指回原 venv,须以 `python -m` 形态调用)+ 私有 manifest(meta/MANIFEST.json 39M,`manifest.load_and_update(rebuild=True, cache_root=私有)`,v9 层级树格式)+ launcher run_e150.py + 分析器 analyze.py |
| manifest 条目 | editing 域 testharness 182 + crashtest 145 + reftest 2 + manual 9(排除)= 自动化 329 |
| 端口门 | 全端口集 8000-8003/8443-8446/8888/8889/9000,跑前 ss 查、跑后清 bao/serve 孤儿 |
| meta | /tmp/e150/meta/editing = servo 参考仓 meta/editing 快照(157 ini,零多值期望零 known_intermittent——e147 压平坑不适用) |

## B. 载具级根因发现(本 campaign 主产出):WPT pref 管道结构性死亡

**现象**:other 域 508 测试中 90 文件 ERROR(exp OK),87x 消息 `document.execCommand is not a function`;crashtests 9 个 unexpected TIMEOUT 同根因(init 脚本 execCommand 调用抛 TypeError → `test-wait` class 永不移除)。

**证据链**:
1. servo WPT 跑法 = wptrunner servo browser **无条件传 `--enable-experimental-web-platform-features`**(servo.py:104)→ servoshell EXPERIMENTAL_PREFS(20 项)激活,含 `dom_exec_command_enabled`;
2. vendor 门面:`Document.webidl` execCommand/queryCommand* 全 `[Pref="dom_exec_command_enabled"]`(默认 **false**);`Document::perform_editing_action`(editing.rs:386)同 pref 早退——**Document 编辑上下文(contenteditable/designMode)的一切编辑动作**(含普通字符插入:Character 无修饰 → `EditingAction::InsertText` → Document 臂)+ execCommand API 族全暗;
3. **不受门面**(实测):Clipboard 臂(Ctrl+C/X/V→handle_clipboard_action 直达)、SelectAll、TextControl 臂(input/textarea 键入,text_input 引擎)——`EditingContext::perform_editing_action`(editing.rs:738)无 pref 门;
4. **bao 缺陷链**:`--enable-experimental-web-platform-features` CLI 接受但零消费(仅入口分派,cli.rs:235);`--pref=K=V` 管道正确执行(布尔安全;f64 强转 bug 只落数字 pref,e137 G5-3 登记)但 `run_browser → BrowserRuntime::new` 构造 `ServoBuilder::preferences(default+8 项精选翻转)` → `Servo::new` 的 `prefs::set(builder)` **必然覆写全局**(lib.rs:230-238 自注:"the only durable injection point is ServoBuilder::preferences")。实测:launcher 传 20 条 --pref 全带上命令行,execCommand 仍缺(verify1.raw.log)。

**影响**:servo meta 锚(157 ini,期望文件级 OK+子测语义红)结构性不可达——execCommand 驱动的 run/other 域全域读数空心(pref 暗失败 ≠ servo 语义分歧);as-expected 面的 9572 子测 FAIL 同为错因失败。此缺陷对一切 pref-gated 域 campaign 都是地雷(webaudio/css/dom-encoding 此前恰好无 pref 门)。

**修面(待裁定)**:BrowserRuntime::new 的 preferences 构造面消费 CLI pref 面(--pref 覆写 + enable-experimental 旗标),或 WPT 入口把已 apply 的全局面并回 builder preferences。启用 execCommand 是页可观测面变化(Chromium/Firefox 都有,正向对齐)——指纹宪法域,用户裁定。

## C. 停等裁定(A/B)

- **A(建议)**:批准 pref 合同修订 → 重建二进制全量重跑 5 域 → servo meta 锚正常分账 + ini 终态落地;
- **B**:pref-dark 现状读数交付(分账+根因登记,ini 落 servo meta 快照锚,unexpected 面作为登记缺陷类可见)。

裁定已报 Commander(两条消息:停报原文 + 爆炸半径修正)。

## D. campaign 分账(pref-dark 读数,裁定前中间态;二进制=06050b70 基线,e151 EditContext 提交 4c41ecd6 在后——edit-context 13 文件 expected-FAIL 与该二进制精确配对)

| 域 | 跑数 | as-expected | unexpected(单根因=execCommand 暗面,§B) |
|---|---|---|---|
| edit-context | 13 | 全部(4 ERROR+7 OK+2 PASS;子测 41 FAIL+1 PASS) | **0** —— API 缺失性 FAIL 结构性合法(06050b70 无 EditContext;e151 波已落 4c41ecd6,后续轮将翻) |
| crashtests | 144 | 135(PASS 132+TIMEOUT 3) | **TIMEOUT 9** |
| other | 508 | 391 | **117**(ERROR exp-OK 90/ERROR exp-CRASH 12/TIMEOUT exp-OK 9/OK exp-ERROR 1/OK exp-TIMEOUT 5) |
| whitespaces+plaintext-only | 64 | 61 | **3**(ERROR 2+OK exp-CRASH 1 正翻);子测 unexpected-FAIL 978(plaintext-only delete/forwardDelete 族 ×12 变体) |
| run(重参数化) | 120 展开 | 119 | **1**(OK exp-CRASH 正翻);子测 unexpected-FAIL **77796**(tests.js 重参数化,execCommand 暗错因红,harness 文件级照常收官)+ unexpected-PASS 39 |
| **合计(五块全)** | **849** | **719(84.7%)** | **130**;子测 as-expected **30679** / unexpected **79271**(单根因 §B;run 域占 77796) |

正翻信号(unexpected-PASS 面):OK-exp-CRASH 2(crashtests 域跑者存活性超 servo 预期)+ OK-exp-TIMEOUT 5(other 域)+ run 域子测 39——预执行后重跑时按 e122 F4 惯例处置。

## E. bao e2e pin(已完成,worktree 灭菌验证 4/4 PASS RC=0)

`src/bao_browser/tests/suite/editing_e2e_tests.rs`(新文件)+ main.rs 注册:

| 测 | 状态 | 覆盖 |
|---|---|---|
| `editing_e2e_insert_text_carrier_input_and_textarea` | **真绿** | insertText 载体(Character Down/Up 对=cdp_handler::cmd_insert_text 同形)真实更新 input/textarea .value;含非 ASCII("编辑");无关字段零污染 |
| `editing_e2e_password_copy_cut_guard` | **真绿** | 真键 Ctrl+C/X 路径:文本框正控(clipboardchange≥1+deleteByCut=1)+ password 守卫阻断双零(copying_enabled/cutting_enabled,57a307695) |
| `editing_e2e_contenteditable_real_key_insertion` | armed skip | pref-dark 门(typeof document.execCommand 探针);合同落地零改动自激活 |
| `editing_e2e_enter_key_paragraph_in_contenteditable` | armed skip | 同上;Enter→InsertParagraph 断言 |

## F. vendor 同步(已落,裁定无关)

- `vendor/servo/tests/wpt/tests/editing/`:389 文件,与参考仓字节一致(389/389 cmp 验证);
- `vendor/servo/tests/wpt/meta/editing/`:157 ini,与 servo 参考仓 meta/editing 字节一致(edit-context 14 文件 expected-FAIL 基线在内;e151 EditContext 波落地后由该波翻转,时序正确)。

## G. 教训沉淀

1. venv 拷贝陷阱:bin/* shebang 指回原 venv——`pip install` 必须用 `python -m pip` 形态(裸 pip 装进原 venv,e150 实录);
2. `manifest.load_and_update(write_manifest=True)` 自写盘,手动 `m.write(f)` 会 AttributeError(MANIFEST v9 层级树格式,条目按 type→目录→文件组织,旧扁平解析读数为零);
3. gen_manifest 依赖 pyyaml(multiprocessing worker 内 import,venv 必装);
4. campaign 前置检查项新增:**pref 门域扫描**——目标域的关键 API 若有 `[Pref=]` webidl 门或 `pref!` 调用,先核实 servo WPT 跑法的 pref 激活路径与 bao 消费面,否则读数整体空心。


## 裁定 A 执行态(2026-10-08,Commander 接管段;e150 429 限流体假)

- A1 pref 管道修复:2886b1c1(BrowserConfig/BaoConfig pref_overrides 字段)+lib.rs:368 apply_pref_overrides_to 消费面(四文件)已 commit
- A2 重跑读数:rerun3 五域 raw log 归档(/tmp/e150/rerun3/)。形态=**pref 激活大面积活化双向洗牌**:subtest unexpected-PASS(exp FAIL)≈1935 正向(vs unexpected-FAIL≈1240 负);file 级 ec2/ct2 unexpected-FAIL=0(crashtests 9 TIMEOUT 全消);rn2 85152 子测 PASS(语义红 10546=servo-parity 正常态)
- ini 重锚:manifest_update=True kwargs 通路未生效(ot3 stdout 零 update 痕迹,待 e150 恢复后调通);edit-context 面 e151 ini 已在 vendor(差集仅该面 10 条);其余 149 ini 重锚待 update 通路调通或 analyze 驱动手改
- e2e armed skip 自激活验证:pref 落地后重跑确认(待 e150)
- 残留移交:e150(23:09 限流重置后)完成 ini 重锚+e2e 自激活确认+终报
