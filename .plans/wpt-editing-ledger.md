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
- ini 重锚(终态):/tmp/e150/reanchor_ini.py(wptmanifest parse/serialize round-trip 字节恒等验证后使用)。策略=**正翻吸收+红保持**(campaign 惯例 e122 F4/e147 I5):子测条目任一观察 PASS 即移除(变体分裂格吸收后失败变体保持可见红——红可见性优先于正翻噪音);file 级期望全部变体 OK/PASS 才移除;其余(FAIL/TIMEOUT/NOTRUN/未观察)保留 servo 期望=分歧格可见红。edit-context 面先与参考仓 e151 终态同步。**两轮调试**:①保守全观察规则在混合文件留 1504 正翻噪音→改任一观察;②子测名变体键 bug——run 域 ini 是分页变体节(`?1-1000`/`?1001-last`),观察键 strip 变体永不匹配(首轮仅吸收 405)→观察按全名+剥离双键、匹配按节名带变体(更精确:变体内 FAIL 不被他变体 PASS 吸收)。终态:**77 文件改动(+74/-6014),21 文件删**(5=e151 面+16 全绿清空),**1939 子测条目+20 file 级吸收**,纯红 22298 条保留(run 域 ini 分页节全量,as-expected servo-parity 红主体);vendor ini 157→136
- e2e armed skip 自激活验证:**已实证**(worktree 2f86f149+ 链,`--no-capture` 单测):contenteditable state=`"text":"Hi"`+trusted keydown/input 双事件流——Document 编辑引擎真活,armed 门零改动转真断言;四测 4/4 全真绿(RC=0)
- 指纹快查(裁定要求):双证据——源级零耦合(bao_stealth 仓 `dom_exec_command|servo::prefs` 引用=0)+行为面 bao_stealth canvas/navigator/screen deep 套件 41/41 绿(pref 翻转不改指纹读数;execCommand 是功能面非指纹属性)
- rerun4(首轮验证,保守规则 meta):849 tests/821 as-expected(96.7%)/sub unexpected-PASS 残留 1504(保守规则产物,见上①)→ 触发规则精化;unexpected-FAIL 1240 稳定(分歧集)
- **rerun5(终态验证,精化后 meta 136 ini,worktree@51b1f37d 二进制)——campaign 收口读数**:849 tests **840 as-expected(98.9%)/9 unexpected-FAIL 文件(全 other 域)**;子测 **113704 as-expected + 1240 unexpected-FAIL + 1 TIMEOUT + 2 NOTRUN,unexpected-PASS=0**(正翻吸收彻底,零噪音)。红面三轮恒定(rerun3/4/5 逐子测零漂移实证,inserthtml 全量 diff=0)=**确定性分歧集**:ot 991/ws 166/run 83 子测(plaintext-only delete 族/execCommand 语义细节),后续合同域;二进制重建插曲:/tmp/e147-wt-target 整体被清扫(第五次载具资产损失)→ /tmp/e150-target 冷建 21min(sccache 温)
- 载具资产损失实录(本波×2):22:2x e147-wt-target 清扫(首轮);00:1x 同目录再次清扫致 rerun5 首发空跑——/private 载具半衰期纪律再证;脚本坑:sed 派生 rerun5.sh 只换 tag 未换目录前缀,日志写入 rerun4/{ec5..rn5}(与 ec4.* 并存零覆盖,实为幸运)

## e154 清偿(2026-10-10,REQ-BRW-002 登记面清偿批⑰)

### 根因裁定:1240 = 继承面(锚时代错位),非 bao 引擎缺口

e150 的 meta 锚取自参考仓 working tree——落后 origin/main 887 commits 的 stale 态(file 级 CRASH/TIMEOUT 期望);重锚吸收 CRASH 正翻后子测落回默认 expected-PASS,才形成「1240 意外红」。**逐子测对照 origin/main ini:1240 格全部 = 上游最新态同样 expected FAIL**(insertlinebreak-with-white-space-style 456/insertparagraph-with-white-space-style 400/editing-around-select-element 100/ws inserttext 165/rn inserttext 67+styling 16/其余 ~40)。正翻面 33 格全在 edit-context(e151 面,e150 已吸收)。

**ini 终态**:以 origin/main 为锚重建 vendor meta/editing(非 edit-context 子树),吸收 bao 正翻;event.html(顶层,e150 五域路径漏覆盖)首次纳入并补跑验证(ev7:35 as-expected FAIL+145 PASS 零意外)。第三次吸收(rerun7 观察):+24 格正翻清除,other/insertparagraph.html.ini 全吸收删除。edit-context 子树保持 e151/e153 锚(ec7 观察 16 格 exp-FAIL→PASS 漂移,归 e153 面处理)。

### 真分歧面修复:webdriver script_interrupt 门控(挂死族根治)

**症状族**:edit-in-textcontrol(4 变体)/setting-value(4)/exec-command-with-text-editor(3)/input-in-text-control(3)等一切「聚焦可编辑元素 + testdriver 按键」测试 TIMEOUT——rerun5 只显 1 变体(4 进程并发下竞态形状),隔离复现全量。

**取证链**:test-ci-dbg 符号档构建(/var/tmp,21min)→ gdb 全线程深栈:dispatcher 阻塞在 `handle_execute_async_script` 的无界 wait;Script#1/主循环空闲;runner 日志序列 `None→complete` 且全程零 action 消息(performActions 从未被 runner 收到)→ wptrunner classic testdriver 协议(message-queue.js 单消费回调)被**幻影 null** 失步:页面下一条消息落入已死求值通道。

**根因**:bao `show_embedder_control` 对**一切** EmbedderControl 无条件把在飞脚本求值解为 null;servoshell 参考形(ports/servoshell/running_app_state.rs:837)只对 `SimpleDialog` 门控。`InputMethod` 控件在**聚焦可编辑元素**时触发(editor.select() → run_the_focusing_steps)——正是本族测试的共同前置。修复=门控对齐 servoshell(webdriver_host.rs,SimpleDialog-only)。

**配套硬化**:input-event 完成边沿 latch(`handled_input_events`,镜像 e26 completed_loads——notify 可先于 pump insert 到达的丢边类;共享前未单独证fire,防御层)。

### 残留登记(非本波面)

- **data_transfer_on_input_event 1 格**(file ERROR):InputEvent.dataTransfer 在 paste 路径为 null——引擎语义缺口,上游已实现(bao 未);保持可见红,后续合同域。
- **32 格归因 e153 EditContext P1 commit(1622b097)**:execcommands.rs(+102)/inputevent.rs 改动翻转 exec-command-with-text-editor 26 格(rerun5 时过)+ exec-command-without-editable 4 格 + run/insertparagraph 1 格;已如实上报归属,不由本波代修。
- **8 格 file 级 CRASH/ERROR**(insert-list-preserving 等):4 进程负载假红(隔离复跑全绿),环境面。

### 回归证据

- rerun7(修复后全五域+event.html):ws 0 意外红/rn 1(e153 面)/ev 0/ec 0(ct 全 as-expected);ot 意外=32(e153+data_transfer)+8(负载)。
- editing_e2e **4/4 真绿**(含两测 pref 激活后真断言);editcontext 锁 c1-c5 **5/5**;`cargo check -p bao-browser --features webdriver` RC=0;test-ci/test-ci-dbg 双档构建 RC=0。
- 隔离复跑(残面 10 文件,processes=1):挂死族 16 文件全 OK。

### 教训

1. **载具必须 /var/tmp**:本波 /tmp/e150(载具+raw log 全量)与 /tmp/e154-dbg-target(16G 符号档)双双被清扫(第六/七次载具损失);重建于 /var/tmp/e154-veh(venv 拷贝+manifest rebuild 39M+meta 拷贝,配方在本文件§A+gen_manifest.py)。
2. **meta 锚时代校验**:吸收 CRASH 正翻前先核锚仓与 origin/main 的 ini 时代差(git ls-tree 对照),stale 锚吸收会把上游已知红变成「意外红」。
3. gdb `thread find` 不切线程,-batch 用 `thread apply all bt N` 提取;挂死窗口用 timeout-multiplier 拉宽+新鲜 PID 集(OLD 集排除)防抓孤儿。

### e154-续(e153 移交两项清偿,2026-10-10 02:0x)

**①劈裂登记路径根治**:e153 原登记落 `other/insertparagraph.html.ini`——锚的测试不存在(两树均无 other/insertparagraph.html,真身=run/insertparagraph.html;我在主吸收中按"全吸收死文件"删除)。已在正确路径重建:`run/insertparagraph.html.ini` §?4001-5000 增 `[["insertparagraph",""]] "<ul contenteditable><li>{}<br></ul>" compare innerHTML expected: FAIL`(wptmanifest 库编程插入,round-trip 验证);ipcheck2 全变体跑实证:精确格 FAIL 且 exp-field 缺失=登记生效,零 unexpected。

**②cut/copy 26+4 格归属终裁与清偿**:我 crface 复跑(隔离,确定性复现 30 格)归因 e153 1622b097;e153 接报回来提交 **dc09ae19**(exec_clipboard_command_on_text_control:execCommand 版 cut 不 fire beforeinput/uncollapsed 门/non-editable false)。撞车实录:其间该文件有未提交 +65 行在制编辑(mtime 活动态),我 SendMessage 挂起避让,01:52 自行提交化解。修后复验(postfix 跑,重建二进制):exec-command-with-text-editor 3 变体 + exec-command-without-editable + 挂死族全部 OK 零 unexpected;修后吸收 36 格(supported/enabled 正翻,392→356 条);回归 editing_e2e 4/4 + editcontext 锁 5/5。

**残留唯一格**:data_transfer_on_input_event(file ERROR,InputEvent.dataTransfer paste 路径 null)——引擎语义缺口,上游已实现,后续合同域;campaign 惯例保持可见红。

**吸收脚本护栏教训**:origin/main 派生的吸收会冲掉非 origin/main 来源的手工登记(劈裂格)——吸收后必须重放登记步骤(顺序:吸收→重加→验证)。

### e159(data_transfer 单格清偿,2026-10-10 02:5x)

**残留唯一格清偿**:data_transfer_on_input_event_with_insertfrompaste_type.html file ERROR→**OK/subtest PASS**(contenteditable listener `assert_not_equals(e.dataTransfer,null)` 过 + `getData("text")=="copyMe"` 过;textarea 分支 dataTransfer=null/data="copyMe" 语义保持)。

**RED 钉**(e154-final 二进制,vendor meta):file ERROR=`assert_not_equals: got disallowed value null`(line 23)——比 e154 观察的 `inputType insertText` 更进一步(e153 1622b097 paste re-resolve 已收 inputType 面),唯一残点=dataTransfer null。**上游对照实证**:origin/main(e197b55c2)inputevent.rs GetDataTransfer 仍 TODO+None——任务头「上游已实现」不成立,本波为 fork 自治实现(fork 自维护裁决面),非吸收重放。

**修复面**(vendor 4 文件):
- `event/inputevent.rs`:data_transfer 字段(MutNullableDom<DataTransfer>,镜像 target_ranges 的 UA-only setter 形)+GetDataTransfer 真身
- `datatransfer/datatransfer.rs`:new_readonly_clipboard_text 构造器(ReadOnly DragDataStore+text/plain 条目;input-events 规范:预填充 DataTransfer 的 drag data store 为 read-only)
- `document/editing.rs`:contenteditable paste 分支 beforeinput(fire_beforeinput_on_element 新 data_transfer 参)+尾部 input 各携独立 payload
- `editcontext.rs`:fire_beforeinput_on_element 签名扩展(Option<&DataTransfer>);EditContext handle_paste 同携(text-control 分支刻意不携——规范:仅 contenteditable host 预填充)

**判别回归**(fix vs baseline 同批对照):exec-command-with-text-editor 356 fail-side 与 plaintext-only 40+16+8 unexpected 在**基线二进制计数恒等**=scratch 环境既有红面(e154 登记残面),零本波回归;edit-context paste 家族+edit-context-input 零 unexpected;suite 真执行(xvfb-run,16.71s)editing_e2e 4/4+editcontext c1-c5 5/5;xvfb 假绿陷阱实录:无 DISPLAY=should_skip 静默 ok(0.00s),BAO_TEST_NETWORK 单独设仍假绿,必须 xvfb-run。

**邻接观察(未触碰,候选后续)**:①contenteditable insertFromPaste 的 data 应为 null(规范表格;现为 Some(text))——plaintext-only beforeinput 40+8 格正卡此断言,是独立登记面;②clipboard text/html 格式周流缺(GetClipboardText 仅 text/plain);③InputEventInit.dataTransfer 构造器字典成员被忽略(脚本构造事件恒 null)。

### e160(insertFromPaste data=null 清偿,e159 邻接观察①,2026-10-10 03:xx)

**spec 矩阵核验**(w3c.github.io/input-events 权威表,editor's draft):data/dataTransfer 按 **host 类型×inputType** 双维——contenteditable host 的剪贴板 inputType(insertFromPaste/insertFromDrop/insertTranspose/insertReplacementText/insertFromYank)=data **null**+dataTransfer 预填充(ReadOnly,text/html+text/plain+text/uri-list);`<input>`/`<textarea>` 行=data=插入文本+dataTransfer **null**。WPT 双测一致钉死:plaintext-only 两文件断 contenteditable `data===null`;data_transfer_on_input_event(textarea 分支)断 `data==="copyMe"`。任务头「data 分支按 inputType 分派」的精确形态=按 host 分派(text-control 面保持 Some 是 WPT 钉死的合法形态,非遗漏)。

**修复面**(vendor 2 文件 3 站点,全部是 contenteditable/EditContext host 侧;text-control fire_paste_beforeinput_event/fire_paste_events 与 execCommand 路径[后者本就 None]零触碰):
- `editcontext.rs` handle_paste:beforeinput data Some(text)→None
- `document/editing.rs` contenteditable paste beforeinput:Some(text)→None
- `document/editing.rs` contenteditable 尾部 input:Some(DOMString)→None

**RED→POST 实测**(test-ci 二进制,wptrunner 官方 oracle,/var/tmp/e160-probe):
- RED(HEAD=46ccbaac):paste.https 28 beforeinput+nested 4 beforeinput 全 FAIL 于 `assert_equals: data should be null expected null but got "abc"`——台账归因实证
- POST:断言迁至 `assert_true: dataTransfer should have the copied HTML source`(邻接观察② text/html 周流缺成为这些格的**新首卡点**);data=null 修复生效
- **ini 翻转**:paste.https.html.ini 56 条→40 条(wptmanifest AST 通道,双 run[post-fix+regression]一致 PASS 的 16 条单行 pasted-result 格删除,40 条仍 FAIL 保持;round-trip 验证+keep/drop 集合断言);nested ini 零变化(8 格仍 FAIL:4 beforeinput 于 text/html+4 innerHTML 于嵌套粘贴语义)
- 「40+8 大面积翻正」的实测边界:beforeinput 格修复后仍红于邻接②(clipboard 管线 SetClipboardText/GetClipboardText 纯 text 单格式,text/html 从不到达 dataTransfer)——独立登记面,非本波范围

**回归**(全绿):editing 五域全锁 680 文件(other 508/run 120/edit-context 13/plaintext-only 38/event.html)0 文件级意外,49 subtest 意外**全部为 unexpected PASS**(正向:e154 登记 edit-context 16+styled-inline 1+nested-styling 16+paste.https 16),零意外红;e159 dataTransfer 面 OK/subtest PASS 保持;exec-command-with-text-editor 0 意外;suite 真执行(BAO_TEST_NETWORK=1+xvfb+--nocapture 状态行全真)editing_e2e 4/4+editcontext c1-c5 5/5(11.67s);cargo check -p bao-servo-script RC=0 触碰文件零警告;suite 0.00s 假绿陷阱复现实录:BAO_TEST_NETWORK 未设→should_skip 早退,必须 env+xvfb+--nocapture 三件套验真。

**残留登记(未触碰)**:邻接②(text/html 周流)现为 plaintext-only beforeinput 32 格(28+4)首卡点;clipboard 时序 flake 实录(RED run 3 格 pre-wrap 单行格 stale-clipboard got "AabcdefB",POST+regression 两轮 16/16 全 PASS——flake 率约 1/3 run,独立 embedder clipboard 时序面);typed contenteditable input data=null 系 execCommand 形(上游对齐问题,非本波面)。

### e167(editing 三小项清偿,2026-10-10 12:xx,REQ-BRW-002 登记面清偿批尾段)

**①insertparagraph ul-contenteditable 列表劈裂格**(run/insertparagraph.html §?4001-5000,e153 登记的唯一真回归格):
- 根因=e153 ④(`is_editing_host` 加 `!parent_is_editable_or_editing_host()` 嵌套 carve-out)使 WPT harness `<div contenteditable>` 内的 `<ul contenteditable>` 失去 editing host 资格→`is_editable(ul)=true`→insertparagraph Step 13 `split_the_parent([li])` 不再于 Step 2(「original parent is not editable」)中止→li 被移出 ul、空 ul 被 Step 11.1 移除→`fix_disallowed_ancestors` 的 2.4 `set_the_tag_name(li→p)`(页面残留 defaultparagraphseparator=p,前一测试设置不重置)→**got `<p><br></p>`**(ipcheck2 raw log 钉)。上游 origin/main is_editing_host 无 carve-out(同格绿,upstream ini 260 条无此格)→本格是 fork 回归,非共同缺口。
- 修复=`node/node.rs` `is_editable` 增边界规则:自带 contenteditable=true/plaintext-only 属性的元素 return false("editing host 不可编辑"对嵌套 CE 的自然延伸)。**不动 is_editing_host**——e153 ④ 的 EC 激活规则(C5 Face2 锁)、事件路由(outermost)、inheritability 测试零触碰;is_editable 全部 9 消费文件均在 execcommand 命令实现内,爆炸半径=编辑命令语义。
- 翻转:ini 261→260(wptmanifest AST 删格,canonical 稳定 round-trip);POST 全家族格(`<ul contenteditable><li>foo[]</ul>`/`<div contenteditable=false><ul contenteditable>…`)保持绿。

**②clipboard「\r 归一化」→实为块分隔符缺失**(e154-终 91 格级联的底层缺口本体):
- 实形:Selection Stringifier 逐 text-run 拼接无块边界——`<div>abc</div><div>def</div>` copy 得 "abcdef",paste.https 12 格(4 变体×3 测试)got `AabcdefB`。e154-终的「\r 归一化」框架是当时假设:WebFetch 核实 clipboard-apis spec **无通用 CRLF 条文**(writeText 仅 Windows \n→\r\n note),真实缺口=copy 侧块结构丢失。上游 origin/main 同格 expected-FAIL(其 ini 56 条含全部 2-paragraph 格)=共同缺口,fork 自治。
- 修复(vendor 2 文件):`selection.rs` 新 inherent `clipboard_text(cx)`(镜像 Stringifier+不同最近块祖先 run 间插单个 LF;仅 clipboard 路径,`Selection.toString()` 公共面零触碰防 selection 域外翻)+`document/editing.rs` selection_content Document 臂切换;paste 侧 `paste_into_current_editing_context` contenteditable 分支 white-space 感知——host computed `white-space-collapse==Collapse`(normal)且含 LF 时 Text+`<br>` fragment 经 Range.InsertNode(LF 在 collapsing host 渲染为空格;pre*/pre-line=Preserve/PreserveBreaks 保字面 LF,匹配测试 useBR=normal 二分)。TextControl(textarea 值天然带 \n)/EditContext(textupdate 文本模型)分支零触碰。
- 翻转:paste.https.html.ini **整文件删**(12 格全翻,全绿);旁效 1 格正向(copy-paste-in-styled-inline-editing-host「should not duplicate」——块分隔符修复的自然受益,非误翻)。
- **③stale-clipboard flake 归因(登记收口,非产品修)**:
  - 复现努力:HEAD 二进制 13 连跑(7 隔离单进程+4 并行双文件+2 全域 4 进程)零复现;本波五域回归 run 中 2 格(edit-context-paste-handler-changes-active 两子测)并行负载下复现 stale(「Span text Row 1, Cell 1…」=同进程上一文件 other/copy-elements-with-css-vars 的末次 copy),**3/3 隔离复跑全绿**→③类负载时序 flake 非本波回归。
  - 结构穷尽:写读同 FIFO 通道(crossbeam 单通道+BrowserRuntime::run 单线程 pump)乱序不可能;handle_reflow 无条件跑 compute_damage_and_rebuild_box_tree(layout_impl.rs:1327)排除「layout data 缺」窗;`clipboard` feature off(bao_browser default-features=false)→fallback store 进程本地,跨进程污染不可能。
  - e160 RED 日志取证:`grid1 PASS + grid2-4 stale` 同页模式——任何「本页 copy 写丢失」理论都只能回退到 grid1 自己的写(值是 "abc"),读到 "abcdef"(2-para copy=同页更晚测试/同进程上一文件)要求 **setup 期 INPUT copy 未落地**(load 时序焦点竞态:copy 落到 Document 语境、空 selection→`.filter(!empty)` 不写→paste 读上一文件末次 copy)。harness/浏览器时序面,产品侧无确定性修法——**按任务 stop 条件 ③登记测试面**,持续观察。
- 环境注:READ-GATE 双宿主缺口(ISSUE #155/#156)阻断执行体写面两轮,三读实质完成后经 Commander 裁定 A 开逃生口续行(窗口 Commander 管理删除)。

**回归证据**:RED/POST 同 meta 双 run(目标 13 格逐格钉)+五域 680 文件 baseline↔post 逐子测 diff(114312 子测对拍:**0 新增红**,2 格 ③类 flake 隔离 3/3 绿)+e153 ④ 锁(inheritability 8/0 全绿;exec-command-with-text-editor 356/exec-command-without-editable 20 fail-side 与 baseline 恒等=注册期望)+suite 真执行(BAO_TEST_NETWORK=1+xvfb+--no-capture)editing_e2e 4/4+editcontext c1-c5 5/5=9/9(引擎信号 `clipboardchange:2,deleteByCut:1` 在);`cargo check -p bao-servo-script` RC=0 触碰文件零警告;载具=/var/tmp/e167-veh(meta 拷贝自 vendor+MANIFEST 复用 e154-veh;载具教训再证:/tmp 不可放)。

### e172(InputEventInit.dataTransfer 构造器字典清偿,e159 邻接观察③,2026-10-11,REQ-BRW-002 登记面清偿批㉔)

**清偿**:脚本构造 `new InputEvent(type, {dataTransfer: dt})` 的字典成员被丢弃(构造器不消费 `init.dataTransfer`,脚本构造事件恒 null)。

- **上游对照实证**:origin/main(~/code/tools/servo git show)Constructor 与 fork 逐字同形——同样不消费 `init.dataTransfer`,且其 GetDataTransfer(109 行)仍 TODO+None;codegen 字典面(成员声明+`Option<DomRoot<DataTransfer>>` 解析)两侧均齐——纯 Rust 构造器接线缺口,fork 自治(同 e159 裁定谱系),非吸收重放。
- **接线(vendor 1 文件,3 hunks)**:`event/inputevent.rs` Constructor 尾部 `event.set_data_transfer(init.dataTransfer.as_deref())`——复用 e159(46ccbaac)落地的 setter;字段/setter 两处 doc 注释同步翻新(UA-only 表述改为 UA+字典双源)。信任路径(editing.rs/editcontext.rs 经 `InputEvent::new`+`set_data_transfer`)零触碰。
- **RED 钉**(suite 真执行,非 WPT 载具——纯 JS 构造面无需 testdriver):新增 `editing_e2e_inputevent_init_datatransfer`(bao_browser suite;`new DataTransfer()`+`setData` 载真实 payload)。RED=`{"identity":false,"payload":null,"defaultNull":true}` panic 于 identity 断言(0.149s 真跑);GREEN=identity:true+payload round-trip+defaultNull:true 三断言全过。
- **回归**:同批 10/10(editing_e2e 5/5 含新钉+editcontext c1-c5 5/5,BAO_TEST_NETWORK=1+xvfb+--nocapture,13.8s 真时长零 skip);`cargo check -p bao-servo-script` RC=0 触碰文件零警告。e159 WPT 格(data_transfer_on_input_event insertFromPaste)保持未单测——构造器改动与信任 paste 路径代码级不相交(Constructor 仅脚本入口可达),WPT campaign 复跑留 V 批量。

### e179(InputEventInit.targetRanges 构造器字典清偿,e178 核验报告登记,2026-10-11,REQ-BRW-002 登记面清偿批㉕)

**清偿**:脚本构造 `new InputEvent(type, {targetRanges: [sr]})` 的字典成员被丢弃(构造器不消费 `init.targetRanges`,脚本构造事件的 `getTargetRanges()` 恒 `[]`,与字典值无关)。e178 核验报告登记项。

- **上游对照实证**:origin/main(~/code/tools/servo)Constructor 同样不消费 `init.targetRanges`,其 GetTargetRanges(115 行 TODO)恒 `Vec::new()` 且无 target_ranges 字段——上游整个 UA/字典双面皆缺(fork 已有 e153 P1 ① UA 面基座,唯字典面缺口);codegen 字典面(成员声明+`Vec<DomRoot<StaticRange>>` 解析,webidl :27 默认 `[]`)两侧均齐——纯 Rust 构造器接线缺口,fork 自治(同 e172 谱系),非吸收重放。spec 锚=input-events 字典注释:「targetRanges of {{InputEventInit}} initialize the corresponding attributes」。
- **接线(vendor 1 文件,3 hunks)**:`event/inputevent.rs` Constructor 尾部 `event.set_target_ranges(init.targetRanges.clone())`——复用 e153(1622b097)落地的 setter(Vec<DomRoot> clone=refcount bump);字段/setter 两处 doc 注释同步翻新(UA-only 表述改为 UA+字典双源)。信任路径(editcontext.rs:515 UA 填充)零触碰。
- **RED 钉**(suite 真执行,纯 JS 构造面无需 testdriver):新增 `editing_e2e_inputevent_init_targetranges`(bao_browser suite;`new StaticRange({startContainer/startOffset/endContainer/endOffset})` 载真实几何 0→5)。RED=`{"identity":false,"offsets":null,"defaultEmpty":true}` panic 于 identity 断言(0.12s 真跑);GREEN=identity:true+offsets:[0,5]+defaultEmpty:true 三断言全过。
- **回归**:同批 11/11(editing_e2e 6/6 含新钉与 e172 钉+editcontext c1-c5 5/5——c5 face1 `[[true,true,1,2]]` 实证 e153 UA-only 填充面不变,BAO_TEST_NETWORK=1+xvfb+--nocapture,16.29s 真时长零 skip);`cargo check -p bao-servo-script` RC=0 触碰文件零警告(touch+重查实证)。构造器改动与信任 EditContext 路径代码级不相交(Constructor 仅脚本入口可达),WPT campaign 复跑留 V 批量。

### e174(editing 白空间继承面大桶自研,2026-10-11,REQ-BRW-002 登记面清偿批㉕)

**清偿**:e154 分桶判「继承(origin/main 同 FAIL)」的两大白空间族 856 格(insertlinebreak-with-white-space-style ×4 变体 456 + insertparagraph-with-white-space-style ×8 变体 400)——用户裁决「不存在等上游,都 fork 了自己做」推翻继承登记,本波 fork 自治实装,Chrome-91 锚(测试自注)。

**取证矩阵**(e168 基线二进制,red1 全量):
- ILB 族 pre/pre-wrap/pre-line:全部格红——`abc[]`→got `abc<br>` exp `abc\n\n`|`abc\n<br>`(字面 LF 缺失);nowrap:仅端格红(got `abc<br>` exp `abc<br><br>`=collapsed-line-break 步骤 10 TODO)。
- IPG 族:display:block 宿主 `a[]bc`→got `a<div><br></div>bc`(空段落砸在 caret,尾部不迁入;与白空间无关的通用缺陷,plain run/insertparagraph `foo[]bar` 同形=基线意外红);display:inline/inline-block 宿主→期望 linebreak 形(got 段落形);inline-display 段落→分裂后需新块 div 包裹(got 裸双 inline);italic/bold 序列→insertparagraph 对 inline 元素内 caret 11.5.1 bail 全 no-op。

**实装面**(vendor 4 文件,execcommand 域;任务头 owner-boundary 写 editing.rs 系路由层笔误,算法真身=execcommand/commands/*):
- `contenteditable/node.rs`:`is_collapsed_line_break`(spec collapsed-line-break 结构化近似:块内后续无渲染元素/无非空白文本;whitespace-only 文本跳过但 pre 语境 LF=内容)+`line_feed_is_significant`(computed white-space-collapse!=Collapse;pre/listing/xmp/textarea 元素名 carve-out 保 run/ 数据锁 br 形)。
- `insertlinebreak.rs`:白空间显著语境插字面 `\n` Text 替代 `br`;步骤 10 落地——collapsed 时补 extra break(恒 br 形:白空间族接受 `\n\n`|`\n<br>`,run/ flex/grid+pre 数据要 `\n<br>`);collapse 后重读 active range(stale range 曾把 extra 插到首断之前)。
- `insertparagraph.rs`三支:inline 编辑宿主(display 解析 Inline)且 caret 与宿主间无块→委托 insertlinebreak(判定先于边界提升);11.5 空列表分支重写为 Chromium 形 caret 行分裂(尾部迁入新段落,空头则原内容不动+br 段落插 caret;尾部边界=块内首个后续块节点)+inline-display 容器分裂后 br 侧半段包新块 div+typing-style carry(六布尔命令 DOM 态镜像进 state override store,Chromium 打字续样式);caret 边界提升(inline 元素边界抬到父位,void/len==0 元素抬到自身前)。
- `inserttext.rs`:步骤 14.1(only-child collapsed br 移除)+caret 占位符替换(前邻 collapsed br/lone-LF 文本,打字替换占位而非堆叠);两处 store 快照/重放(DOM 变异触发 selection_boundaries_changed→clear_command_overrides,italic 等 pending 样式因此死亡——inserttext 占位符移除与 `"\n"`→insertparagraph 直调两点位)。

**回归修复迭代**(全量对拍 e168 基线 115833 子测,首版 49 新红→三修复→0):
- triple-br 36 格(following br 被判 extraneous 跳过)→渲染元素即行内容判据;
- `<pre>foo[]&#10;</pre>` 类 1 格(whitespace 跳过误吞 pre 语境 LF)→LF+Preserve 语境=内容;
- `<p>foo</p>{<h1>}<p>baz</p>` 2 格(tail 提取吞后续块)→提取边界止于首个后续块;
- void 元素 45 格(lift 的 offset==len 先判使 len==0 元素抬到自身后)→offset==0 先判;
- inserttext padding-span 2 格(空 span 判不可见→占位 br 误删)→同渲染元素判据。
- paste 2 格(edit-context-paste-html/paste.https pre-wrap)隔离复跑全绿=e161 ③类负载 flake,非本波面。

**终态**:两族 856/856 全绿(1656 含原生 800);run 域两页正翻 71 格吸收(insertlinebreak ini 63→5,insertparagraph ini 260→35,剩=基线同红 br-adjacent/retval 族);全域对拍 **0 新红+1760 正翻**;suite 11/11(editing_e2e 6/6 含 e172/e179 钉+editcontext c1-c5 5/5,BAO_TEST_NETWORK=1+xvfb+--nocapture 12.85s 真时长);`cargo check -p bao-servo-script` RC=0 触碰文件零警告。

**载具与协调**:载具=/var/tmp/e174-veh(e167 形态拷贝+私有 manifest/overlay tests-root 探针面);**并发窗口实录**:e175 in-flight ws_registry.rs 破损态阻断 bao_bin 链接两轮——scoped restore 守卫窗(备份+mtime 守卫+即恢复,5 轮构建窗)绕行,终报披露 60s 窗口内低概率覆盖风险(未观察到并发写);WPT 端口战(并发 agent wptrunner)重试环。


### e182(editing 继承散格三桶自研,2026-10-11,REQ-BRW-002 登记面清偿批㉖)

**清偿**:e154 判「继承(origin/main 同 FAIL)」的三桶散格,用户裁决推翻继承登记(fork 自治=自研对象)。桶账(ws 165/run inserttext+styling 15 文件 83/ot 散格)按 e154 分桶账本精确对表:82 仍红(3 已被 e174 顺带翻)+ ot 面 e154 登记残面(select-element 100/delete-in-head 38/delete-in-html 27/ipg-in-head 54/paste 2)。

**取证矩阵**(HEAD b8be6e27f 基线二进制,red_small+red_select+red_run 三轮):
- ws inserttext 165 格=单根因:**规范 canonical 形态(SP-first)vs Chrome-83 形态(NB-first 交替)**;span 族 83 格加码:**跨节点 tree-walk**(spec canonicalize 吸收邻接节点 run)vs Chrome per-text-node 域;空串组 21 格=delete_the_selection 前置 canonicalize 副作用(非 no-op);insert "b" 族=spec 形态与 chrome 形态在 n=2(f,f) 恰合、n≥3 分歧+降级方向(插入非空格后邻接 run 需双向重规范化)。
- run inserttext 64 红主根因:`<font size="7">` 包裹=restore 路径 `parse::<i32>("16px")` 恒败→fallback "7"(px 后缀未剥)+ 首子句 `value_override != optional_string` 在无 override 时恒真(两缺陷双修);字体+链接 3 格=restore 无条件重建 `<a>`;ILB xmp/script 2 格=retval false(spec 注释即写 return true,实现笔误)。
- ot select-element 100 格=option/select/optgroup 内容可编辑(引擎删 option 文本/搬 select 入 p);ipg-in-head 54 格=title/style/script(可见 raw-text)内 insertparagraph 走块分裂而非 LF 插入(display 判据:invisible script 保持块分裂——run/insertparagraph `<script>foo[]bar</script>baz` 数据钉死)。

**实装面**(vendor script 11 文件,execcommand 域+document/editing.rs+node/node.rs+selection.rs):
- `contenteditable/node.rs`:`rebalance_whitespace_at`(Chrome-83 per-node 白空间规范化:NB-first 交替/节点尾 NB/lone-run 降级 SP/行首纯空格 run 坍缩单 NB/Preserve 语境跳过;UTF-16 域扫描,同长替换)。
- `commands/inserttext.rs`:空串=无白空间机器(坍缩 no-op,非坍缩仍删选择);坍缩选择跳过 delete_the_selection(杜跨节点规范形态预写);select-internals 守卫+单 select 子整体删除;步骤 11 预插入 rebalance 删除(replaceData 按 DOM 规则把坍缩 caret 从 run 尾迁 run 首=插入点漂移根因);步骤 16/17→rebalance;步骤 19 改显式 collapse 到记忆的插入尾(rebalance 重写后边界漂移+行首坍缩缩长双护)。
- `commands/insertlinebreak.rs`:xmp/script 步骤 3/4 retval false→true(spec 注释本就写 true)。
- `commands/insertparagraph.rs`:可见 raw-text 元素(title/style/script,非 invisible)→`insert_paragraph_line_feeds`(字面 \n Text+collapsed 双喂;8 白空间变体全接受)。
- `commands/{delete,forwarddelete}.rs`:select-internals 守卫+单 select 子整体删除臂。
- `contenteditable/range.rs` restore:fontSize px 剥离(maybe_normalize_pixels 复用)+首子句限 override 在场;createLink 精化=override 存在且异,或无 override 且**插入点无锚上下文**(typing-around-link 37 格翻绿与 multitest createlink 链 18 格双满足的分界)。
- `contenteditable/selection.rs` set_the_selection_value:纯白空间文本节点**边缘域**不样式化(两遍式:同源快照标记首/尾显著位前的 ws-only;text 节点;内部 ws 照常——`{<b>foo</b> <b>bar</b>}` 合并语义保全)。
- `node/node.rs`:`is_in_select_widget_internals`(select/optgroup/option 自含+祖先是三者→不可编辑,含 contenteditable 属性载荷)。
- `document/editing.rs`:paste 分支后置重读剪贴板(仅变更时采纳;embedder 写读时序未闭环=2 格登记)。

**试错记录**(负结果如实):override 清除抑制两形态(全域/仅 DOM 变异路径)都使 run/insertparagraph 系大翻红,最终全撤——该收益面(multitest qCV 2 格+部分正翻)放弃登记。

**终态分账**(final6 全域 vs headclean 全域双跑对拍,A/B 同载具同 meta):
- **567 子测翻绿**:chrome-compat inserttext 165 全翻/select-element 100 全翻/inserttext 131/multitest 84/typing-around-link 37/ipg-in-head 20/insertparagraph-in-inline-editing-host 458+cannot-have-div 208+plaintext-only insertLineBreak 112 等 e174 基础上再正翻/ILB 3/11 styling 文件各 1(cefalse 格)/其余散格。
- **file 级 0 回归**;子测新红 **2 格**(如实登记保持可见红,不写 ini):`run/inserttext nowrap foo&nbsp;[]` 格(chrome-compat 组 B 钉 NB-NB 与 run 数据钉 SP-NB 两代 Chrome 锚冲突)+`keep-typed-collapsible undo` 格(undo×rebalance 互作)。
- **不可达/未清偿登记**(红可见性优先,后续合同域):delete-in-child-of-head 38(块连接 walk 需保留 meta/title/style/script/link/base/template 不可见元素且仍连接 div——walk 步 4.1/4.2/9.2/10.2/14.1 五删除点重构面)+delete-in-child-of-html 27(html/body 边界合并)+ipg-in-head 34(54−20)+paste-clipboard-change 2(embedder 剪贴板写读时序)+multitest backcolor qCV 2(override 生命周期,试错已证不可轻动)+dps qCV 2+stylewithcss qCS 1(跨 cell 顺序状态链)+delete foreColor qCV 3。
- ini 终态:17 文件(11 改 6 删),−4425/+193;2 新红不登记(分歧可见红惯例)。

**重大旁路发现(上报 Commander,非本波面)**:run/insertparagraph.html 216 子测红是 **fe3ea1519(e174)已提交树的既有状态**——e174-target 二进制(0 意外)与 fe3ea1519 干净重建(216 红)行为不一致=**e174 波验证所用二进制 provenance 异常**(其收口验证未反映已提交树);e174→HEAD(b8be6e27f)零 servo script commit,215 红与 e180/e181 bao 层重构无关(已排除:src 树还原 fe3ea1519 重建仍 216)。归属=已提交树在 e174 收口前已带此面(e174 二进制含未提交后丢失的 servo 状态?)。建议独立归因波。另:参考仓 tests/editing/other/ 残留 e174-probe*.html 4 个探针文件(双跑均 ERROR,环境面,建议清理)。

**回归证据**:final6 全域 855 tests 733 as-expected/117 意外(基线 717/133)——意外集=正翻吸收尾+2 登记红+基线同在的环境 ERROR(e174-probe/paste_text_with_text_transform/recursive-exec-command);suite 真执行(BAO_TEST_NETWORK=1+xvfb+--nocapture)editing_e2e 6/6+editcontext c1-c5 5/5=11/11(12.63s 真时长);`cargo check --profile test-ci -p bao-servo-script` RC=0 触改文件零新警告;`cargo check -p bao-browser --features webdriver` RC=0;载具=/var/tmp/e182-veh(e174 形态拷贝+meta 拷贝自 vendor;二进制 provenance:e182-target 硬链增量链,headclean/e182v4 双份存档)。

**教训**:①载具二进制 swap 前必须查在跑 wptrunner(本波一次撞车=batch2 混合 provenance 作废重跑);②A/B 双跑基线必须**覆盖目标文件全集**(run/insertparagraph 不在首版基线→归因多走 5 轮二分);③已提交树与"波验证二进制"可能不一致——campaign 基线一律用**当轮干净重建**,禁复用往轮 target 二进制作对照。

### e183(run/insertparagraph 216 红归因+根治,e174 provenance 异常定性,2026-10-12,REQ-BRW-050/REQ-BRW-002)

**归因定性(双假说判定,证据链闭合)**:假说 A「丢失 hunk / 验证二进制 provenance 异常」**证伪**——e174 波自己的收口全量 run(/var/tmp/e174-veh/final_full.raw.log,bao.e174-target 二进制,run_final_reg.sh attempt CLEAN)在 /editing/run/insertparagraph.html 上就是 243 FAIL(216 意外+27 ini-expected),与 fe3ea1519 干净重建(headclean_full)及 HEAD 的 FAIL 集**逐子测字节级相同**(set 相等、expected-map 相等);§e182 的「e174-target 0 意外」记载为误读。假说 B 成立且更精确:**这 216 红从未被任何 ini 覆盖**(pre-e174 ini 260 项∩216=0,删除的 225 项∩216=0)——它们是该文件上早于 e174 的既有意外红,e174 的「0 新红」按净计数为真(only-in-final=0),ini 删除(260→35)与本 216 无交集。工程结论=已提交树的 insertparagraph 缺陷需就地根治(Chrome-83 锚=editing/data/insertparagraph.js,Chrome 生成数据)。

**修复面**(vendor 2 文件,execcommand 域;基线=当轮干净重建 red_full 216 实证):
- `commands/insertparagraph.rs`(核心重写):
  1. **Step-11 createBlock 模型**(Chrome-83:caret 行两半各包默认单行容器):head 半段=行首(br/block 边界)到 caret,提取入 H;tail 半段=caret 到块尾(首个后续 block 排除,行尾 br 包含)提取入 T;invisible-only tail(纯空白/空壳)归 head(`abc[] ` 保头侧尾随空格,accepted 形 2);双容器占位规则。
  2. **占位 br 判据** `block_needs_placeholder`:「无可见子元且无 br」——被移动的 br 本身就是占位(`foo[]<br>` → `<div><br></div>` 而非 `<div><br><br></div>`;旧全局 `is_extraneous_line_break` 恒真导致翻倍)。
  3. **li/dt/dd 内块链**(`inner_chain`+`extraction_target`):caret 在列表项内块中时,新容器按 fragment 形态接链(空/裸 inline fragment → 克隆内块链承接 `<ul><li><div>foo[]</ul>` → `<li><div><br></div></li>`;fragment 自带内块(caret 在块首) → 直接落新容器 `<dl><dt><p>[]foo</dl>` → `<dt><p>foo</p></dt>`);spec step17 eol 改按**全部 contained nodes** 计(旧 contained_children 对文本内起点判空 → dt/dd 误翻转)。
  4. **占位链**(`append_placeholder`+`inline_chain`):caret 的 inline 祖先链克隆包裹占位 br(`<p><b>foo[]</b></p>` → `<p><b><br></b></p>`);锚点与 replaced/embedded 元素切断整链(`<p><a><b>[]foo</b></a></p>` → `<p><br></p>`);fork 路径占位恒裸(`<span>foo[]</span>` → `<div><br></div>`);占位前剥纯空白文本。
  5. **拆分边界空白形态**(Chrome-83):tail 移动时 head 尾随 run canonicalize 成 line-end 形(`a []b` → `a&nbsp;`)、tail 首 run 走 e182 `rebalance_whitespace_at`(`a[] b` → `&nbsp;b`);collapsed caret 跳过 delete_the_selection(spec canonicalize 在 caret 把 `a\u{a0}\u{a0}b` 重写为 SP 形,Chrome 不动)。
  6. **ws-style 域门**(`has_explicit_white_space_style`):caret 至 host 间任一元素 style 属性声明 white-space → 整体回退 e174 语义(delete 照跑/tail 全移/裸占位/旧 eol/无链无空白形)——run/insertparagraph.html(Chrome-83 数据,零 white-space 样式)与 insertparagraph-with-white-space-style.tentative(e174 Chrome-91 tentative 数据)两代锚互不越界;**门后 e174 856 面 0 回归实测**。
- `contenteditable/node.rs`:全局 `is_extraneous_line_break` 的结构性修正(br 终止有内容行且其后块界内无内容)曾落地并实测 216→66 零文件内回归,但 br 可见性经 block-extend/line-break walk/delete 占位逻辑辐射全 editing 套件(+184 意外/17 文件:join-different-ws-style 96、ws-style 128、delete 24、forwarddelete 24...),**回滚保留 `!is_block_node()` 近似并留证据注释**;`block_needs_placeholder` 的 br 子句本地化承接所需语义。修正本体+依赖方再平衡=独立后续波(注:签名 `no_gc` 参数为该次实验的接口残留,回滚后仅下划线形在用)。

**验收(RED→GREEN)**:当轮干净重建(`CARGO_TARGET_DIR=/var/tmp/e183-target`,cargo build RC=0 零触碰文件警告;`cargo check --profile test-ci -p bao-servo-script` RC=0):
- 目标文件:run/insertparagraph.html **216 → 79 意外**(137 清偿)+ **12 格 expected-FAIL 翻 PASS**(8→20)——两轮独立复跑(探针版/无探针版 final2)均 79;
- **全域(editing 855 tests,载具 /var/tmp/e183-veh,--processes 3,flock 端口互斥)**:red_full(基线,当轮)222 意外 → v21_full **85 意外**(−137),**NEW-RED=∅、WORSE=∅**(逐文件对拍),非目标 3 文件(run/insertlinebreak 4、run/inserttext 1、whitespaces/keep-typed 1=§e182 登记残留)计数逐字节保持;
- 中间回归全数归零的修正史:全局 br 可见性(+184/17 文件)→回滚本地化;ws-style 域(+163:ws-style 128+void 30+non-splittable 5)→域门;void 30(embed 可见性)→15(input/wbr)→**0**(is_embedded_line_content);non-splittable 5(meter 链克隆)→**0**(链切断);edit-context-paste 2/1+paste.https 3=§e167 登记 stale-clipboard flake(v20 出现,v21 消失,符合既判)。

**残留登记(79,均已归因,证据 /tmp/e183-v*.json + /var/tmp/e183-veh)**:
1. flex/grid 原子项 24(display:flex|inline-flex|grid|inline-grid 下 span 内 caret,exp=span 级分裂):servo 样式路径 `Element::style()` 对 flex 容器算得 `inside=Flow`(探针实证 "Display(514)"/inside Flow),Chrome-83 期望的 flex-item 分裂判据在 servo 计算样式层不可得——样式系统问题先行立项,检测原型的 style 属性文本匹配形态已验证可分裂但按 P-1 移除死代码;
2. queryCommand 级联 13(dps/stylewithcss 的 before/after 值):Chrome 生成页的跨子测文档状态泄漏期望(dps=p 的 after 读回 p),我方引擎在 insertParagraph 场景读回 div——非 DOM 面,状态生命周期取证另行立项;
3. div 容器白空间 6(`<div>[]  abc</div>` 类,accepted 多形):拆分边界的前后空白取舍在两代锚间逐格摇摆(`[] abc` 保 plain space、`a[] b` 要求 nbsp),域门后仍需逐格定夺;
4. `<b id=x class=y>` 双属性 4:tail 侧克隆未按 spec step28  unset id(取 id/class 只留 class);
5. 其余散格 ~12(`{}` 空行占位、`<p><!--comment-->`、blockquote 空、pre LF 形、`<div><p>foo[]` dps 泄漏级联等)。

**deletion-manifest**:~/code/tools/servo/tests/editing/other/e174-probe*.html ×4——实测该目录不存在(tests/ 下无 editing/ 层),且全仓 find 零命中,**已被先行清理,本波无操作**。

**教训**:①「波验证二进制 provenance 异常」的指控必须先查该波自己的收口 raw log——本次 §e182 的指控被 e174-final_full 原始日志一句话证伪;②「0 新红」的净计数口径会掩盖「ini 删除了未修复子测的期望」之外的场景(本例两集合不相交,红是历史遗留而非 e174 造的);③共享 helper(is_extraneous_line_break)的语义修正必须先做全域对拍再落——其正确形态是全 editing 域的一个待再平衡波,单波内的「正确」会以 17 文件 +184 的代价违反 file 级 0 回归;④两代 Chrome 锚(83 数据 vs 91 tentative)冲突用「样式域门」做域分离(white-space 显式样式=e174 域,无样式=Chrome-83 域),域判据必须在两域数据文件上互斥可测。

### e185(editing 第二桶:delete 块连接 walk + ipg-in-head designMode 根因,2026-10-12,REQ-BRW-050/REQ-BRW-002)

**三桶清偿**(RED 基线=当轮干净重建 bao.e185-red(HEAD 3e5e4482e 增量指纹校验)+ GREEN=终态二进制;载具 /var/tmp/e185-veh(flock 端口互斥,--processes 3);计数口径=wpt raw log 预期语义:expected 字段仅 status≠expected 时存在):
- delete-in-child-of-head 38 + delete-in-child-of-html 27 + ipg-in-head 34 = **99 红 → 8 登记红 + 91 绿**(全域 855 tests:NEW-RED=0、WORSE=∅,逐文件对拍 red_full2/green_full3 双跑;improved 87 格;unexpected-PASS red=57→green=143 系 ini 清除后的正向翻转吸收)。
- **8 登记红 = 不可满足测试锚(两处上游测试文件自身缺陷,非引擎缺陷;上游 servo 自家 ini 同样登记)**:①delete-in-child-of-html `assert_true(div1.isConnected ^ div2.isConnected)`——XOR 产 number,`assert_true` 要求 `actual === true`(testharness.js:1545 严格相等),任何引擎恒红;innerHTML 断言已绿(head 保全、div join 正确)。②delete-in-child-of-head boundary-of-title-and-div:测试自身 `title.setAttribute("title","display:block")`(attr 名打错,应为 style)却期望 innerHTML 无该属性——DOM 序列化必含 attr,任何引擎恒红;join/保全行为正确(结构逐字节等于期望)。

**根因与修复面**(vendor script 6 文件 + webidl 1):
1. **document.designMode 上游缺失(最大单根因,ipg 34 全部 + delete 两文件全部 designMode=on 格)**:`Document.webidl` 的 designMode 被注释(servo origin/main 同形,真缺口,fork 自治自研)——`designMode="on"` 赋值成 expando,文档非编辑宿主,所有 on-variant 键入零操作。落地:webidl 解注([CEReactions] attribute DOMString designMode)+ Document `design_mode_enabled: Cell<bool>` + getter(ASCII 小写,on/off,他值忽略)/setter + `Node::is_editing_host` 增 Document 臂(design-mode Document = editing host,HTML spec §editing-host)——`editing_host_of`/`is_editable`/execCommand enable 门全链自然贯通;contenteditable 宿主内 caret 不受影响(innermost host 先命中)。
2. **delete/forwarddelete 块连接 walk 保全与透明穿越**(delete-in-child-of-head 38 桶):spec 4.1/4.2/9.2/16.1(及 fd 4.1/4.2/10.2/14.1)的 invisible-removal 对 **head 区**(`structural_side()==Head`,含 head 元素自身与 meta/title/style/script/link/base/template)改为**透明**:不删除、walk 越过(fd 10.2 为 skip 时 `end_offset+=1`;delete 9.2 skip 时 `start_offset-=1`——**两 walk 语义镜像不对称:backward 在 removal 与 skip 两臂都减速,forward 仅 skip 臂推进**,照抄上游控制流骨架)。delete_the_selection step 24 contained-node 清单跳过 head 区节点(块 join 不吞夹层 metadata)。
3. **caret 可见位置守卫**:caret 容器链跨 display:none(无样式 `<title>` 内等)非真编辑位置 → delete/forwarddelete no-op(Chromium previous/next-visible-position 模型)。**须用 `is_in_display_none_region`(元素链 display:none 检查)而非 `is_invisible`**——后者对 whitespace-only Text 节点恒真,会把 `pre span 内 "  [] "` 这类合法空白 caret 误杀(首版回归:whitespaces/chrome-compat/delete 1 格 + delete-to-join-blocks 1 + delete-without-unwrapping 4)。
4. **块合并方向结构侧保全**(delete-in-child-of-html 27 桶):delete_the_selection step 34 前置 flip——end_block 在 body/head 侧而 start_block 非(Other)时反向合并:content 前插到结构块、拆除的是普通块;32.4.1/32.4.3/34.5 的空链 climb 加 `is_document_structural`(html/head/body)守卫(空 `<body>` 不再被 climb 拆除——旧行为 `<div>abc</div><body><div>[]def` 会拆出 `<div>abcdef</div>` 无 body)。
5. **title 入 raw-text 内容模型组**(is_allowed_child step 2 加 "title"):HTML 内容模型 title 仅容文本;Chromium 从不把 title 当可承 span 的块参与合并(visible title join/boundary 4 格)。
6. **fd step 10.2 控制流修复(自伤回归,CRASH 级)**:首版把 transparency skip 写成无条件 `end_offset += 1`——removal 分支(原语义 removal 后原地重查,下一 child 位移补位)被叠加 +1 → offset 超 len → step 16 `Extend` unreachable panic(run/forwarddelete.html ?1001-2000/?2001-3000 两 chunk 整体 CRASH)。教训:**walk 双分支共享 continue 时,推进语义必须逐分支对照上游**——backward walk 的对称点(delete 9.2)removal 本就减速,forward 不是。
7. ini 自治:3 ini 文件 **91 条 stale expected:FAIL 删除**(wptmanifest parser+serializer round-trip 验证,逐条 (variant,subtest) 配对 green raw log PASS 集);8 登记红保持既有 ini(上游同登记)。载具 run_e183.py 的 META_ROOT 硬编码 e183-veh 已参数化(E185_META_ROOT)。

**关键判别律**(沉淀):①wpt raw log 解析——`expected` 字段缺省=与 ini 期望一致(as-expected);勿用「FAIL 且无 expected 字段=意外」的朴素口径(会把登记红误计为意外,首版 99→10→8 的计数摇摆源于此)。②「invisible caret」≠「不可见内容中的位置」:caret 有效性只看 display:none 链,内容级不可见(空白文本/折叠 br)不影响位置合法性。③head 区 invisible 保全规则域=祖先链含 head;body 内同名元素(script/style)照旧删除(delete.js `foo<script>bar</script>[]baz`→`fo[]baz` Chrome-83 锚)。④域边界:全 body 内容两侧 structural_side 同为 Body,flip 永不触发,正常合并方向零变化(全域 0 回归实证)。

**验证**:cargo check --profile test-ci -p bao-servo-script RC=0,触碰文件零警告;全域双跑对拍 0 新红;三桶目标文件终态二进制两跑稳定(91 绿 + 8 登记);fd chunk CRASH 修复后 run/forwarddelete.html 全 8 chunk OK。

### e187(flex/grid 24 格样式面假说证伪——根因改判 editing 命令层,域冲突上报,2026-10-12,REQ-BRW-050)

**任务锚**:e183 残留分类①(flex/grid 原子项 24)按「servo 样式路径对 flex/grid 容器算 inside=Flow」假说立项样式面根修。

**假说证伪(双引擎同探针直接实测)**:样式层(stylo display inside/outside + blockification)**零缺陷,已 Chromium parity**:
- bao 侧探针(`/tmp/e187-probe.raw.log`,载具 e185-veh+e185-green 二进制):`display:flex` 容器 computed=`flex`、其 span computed=`block`(blockified)、`inline-flex|grid|inline-grid` 同构(span 恒 `block`)、`display:block` 下 span=`inline`(对照)、float span=`block`(blockify 第二臂)——全部正确。
- **真 Chrome 逐字节同值**(`/tmp/e187-chrome-probe.html`,`google-chrome --headless=new --dump-dom` 实测):`host=flex|inline-flex|grid|inline-grid, span=block, spanFloat=block, block 控制组 span=inline` 与 bao 完全一致。
- e183 的 "Display(514)" 探针证据重解读:514=outside Block+inside Flow=**blockified flex item**(正确形态),非容器缺 inside;当时归因方向错置。

**真根因(改判)**:`execcommand/commands/insertparagraph.rs` 缺 Chrome 的 **flex/grid item 原子分支**。实测(probe2,`/tmp/e187-probe2.raw.log`):`<div style=display:flex><span>a[]bc</span></div>` → got `<div style=display:flex><span>a</span></div><div style=display:flex><span>bc</span></div>`(容器被克隆劈开,style 属性随行),期望(~/code/tools/servo/tests/wpt/tests/editing/data/insertparagraph.js:2586-2700,**Chrome-142 锚**)=`<div style="display:flex"><span>a</span><span>bc</span></div>`——item 就地兄弟分裂,容器零触碰。24 格=4 容器型 × 3 caret 位 × 2 paragraphseparator,同一分支全覆盖。机制:caret 的 block(blockified item)经 Step 9 容器爬升落到 div 后走标准容器劈开路径;Chromium 对 item-of-item-container 走 item 兄弟分裂。

**修复判据(后续合同最小差分,一行谓词可达)**:servo 计算样式已含全部所需信号——caret 最近 block 祖先的父元素 `style.get_box().display.is_item_container()`(stylo `Display::is_item_container`,inside∈{Flex,Grid,GridLanes});现 `resolved_display_value`(execcommand/contenteditable/element.rs:26)只回 `DisplayOutside`,补一个 inside 访问器(或 `is_flex_grid_item` helper)即可。e183 曾以「style 属性文本匹配」原型实证 item 分裂形态可行(按 P-1 移除);正确形态=同一分裂路径换计算样式判据。**域归属:execcommand 命令面 = e186 并行禁触域,本波零代码改动,上报 Commander 裁决派发。**

**回归面(后续合同输入)**:目标=insertparagraph 24 格清偿;insertlinebreak.js:873-935 同族 flex item 格(insertLineBreak 在 item 内插 br)当前 **0 unexpected**(green_full 实测)必须保持;全域 855 tests 逐文件对拍零新红。

**证据资产(留存)**:`~/code/tools/servo/tests/wpt/tests/editing/other/e187-probe.html`(computed-display 矩阵)+ `e187-probe2.html`(实际 insertParagraph 行为,documentElement.innerHTML 全景),已登记 `/var/tmp/e185-veh/meta/MANIFEST.json`(testharness 两项,共享载具附加面,如实披露);raw log `/tmp/e187-probe{,2}.raw.log`+console;Chrome 探针 `/tmp/e187-chrome-probe.html`。deletion-manifest:两探针文件+MANIFEST 两项待后续 flex 合同验收后清除。

**教训**:①「样式层不可得」类归因必须先做 computed-style 直接探针再立项——e183 用引擎内部 Debug 形态(无元素归属记录)反推,方向错置;真 Chrome 同页面对照是本类分歧的一票判据(本机 `google-chrome --headless=new --dump-dom` 零依赖可用)。②wptrunner 私有 MANIFEST 增补测试文件可直接改 JSON(items.testharness 树 +sha1+[None,{}]),无需全量 manifest 重建。③ledger 残留分类的「探针实证」措辞必须落文件路径,否则下波无法复核证据链。

### e186(is_extraneous_line_break 全局再平衡——br 可见性 spec 正确形态落地,e183 遗留债清偿,2026-10-12,REQ-BRW-050/REQ-BRW-002)

**谓词终形(布局锚定,servo offsetHeight 实测,WD 探针 /tmp/e186-wd-probe.py)**:`a<br>` 与 `a` 同高(trailing br 零高度)、`<p><br></p>` 与 `a<br><br>` 每个 line-bearing br 各渲染一行、spec 自例 `foo<br><p>bar</p>` 渲染不含该 br——br 渲染它所终结的行盒,移除它改变布局 ⟺ 该行盒本无其他内容。故 **extraneous ⇔ 其行内(br 之前、同 block 之内)有可见内容**;非多余 ⟺ br 是其行的首个渲染物(spec "collapsed line break" 的结构等价形,与 collapsed-block-prop 定义自洽)。单次后向扫描(`has_visible_content_before_on_line`):行内遇 br/block 即停(前向 br 恒为真行界——直接后随另一 br 的 br 必自身 line-bearing,无需其可见性 ⟹ 判定天然无环)、display:none 子树整体跳过、Text 用 is_collapsed_whitespace_node、空 inline 壳跳过、img 即内容。e183 的转置近似("终止有内容行且其后无内容")被数据否定:其 mid-line br 可见性正是 8 格 insertparagraph 新红的来源。**white-space 域门**:显式 white-space 声明上下文(链至 editing host)保持旧近似(`!is_block_node()`),Chrome-83 数据零 style 属性 ⟹ 两代锚在该判据上互斥可测(e183 教训④沿用);`has_explicit_white_space_style` 自 insertparagraph.rs 上移 contenteditable/node.rs pub(crate) 单源。

**依赖方再平衡(逐文件账)**:
1. `commands/delete.rs` 后向 walk 4.1/4.2 加 `is_removable_at_caret`→终形 `is_removable_line_break_or_shell`(line-bearing br,或仅由其构成/不可见内容的 inline 壳如 `<b><br></b>`):**删除即 return true**(一次 Backspace 恰删一个渲染行界,4.1 无 prev 形状 guard——v3 的 guard 反而挡掉 `<br><br><div>[]` 族;4.2 保留 delete-step-13 形状:prev 是 br 或非 inline);9.2/16.1 维持 invisible-only(v3 全臂扩展曾致 delete 15 新红,已回退)。anchor:run/delete.html `<table>a</table><br>{}<br><table>b</table>`、chrome-compat/delete-to-join-blocks `<div><br><div>[]&nbsp;`、other/delete-without-unwrapping `<b><br></b><div id=child>[]def`。
2. `contenteditable/node.rs` force_the_value 三点:①Step 16 前补 parent 再查(restoration.html multitest 流内节点被同命令流先行 detach 时 `Must always have a parent` panic——**CRASH 级,已根修**);②FontSize 对 line-bearing br 直接 abort(Chrome-83:fontsize 不给占位 br 施 font-size 且 indeterm=true);③Step 21 undo 臂跳过 br(br 无自身 specified value,复检恒撤新裹的 color/face wrapper;Chrome 保留 `<font color=#0000ff><br></font>`)。徽记:force 内 br 类判据全部锚 Chrome-83 数据文件。
3. `commands/insertparagraph.rs`:仅 import 面单移植,域门逻辑零触碰(`block_needs_placeholder` 的显式 br 子句保留为 belt-and-braces)。

**验收(RED→GREEN,基线=HEAD 63ffff8c3 干净重建 bao.e186-red,/var/tmp/e186-wt + 私有 target /var/tmp/e186-target,sccache;载具 /var/tmp/e186-veh,flock 端口互斥,--processes 3)**:
- 目标文件:全域两轮 run/insertparagraph.html **79 → 66 意外**(13 清偿,0 新红),66 带 v5/v6 双二进制三跑稳定(79↔66 的 ±13 flake 带收敛到 66 侧);
- 全域(editing 855 tests,red_full vs green_v6_full 逐文件对拍):**NEW-RED 仅 4 格/2 文件**(见残留登记),非目标文件逐字节保持;`cargo check --profile test-ci -p bao-servo-script` RC=0,三触碰文件零警告;
- 大宗正向:run/delete +33、run/forwarddelete +25、run/insertparagraph +6、indent +4、bold/italic/strike/sub/sup/underline/createlink +2 各、other/delete +13(均 ini-expected:FAIL 翻 PASS,双跑稳定)——e183 时代 "delete 24/forwarddelete 24" 辐射族在本形态下反转为清偿;
- **restoration.html CRASH 修复**(force Step-16 panic,v5 全量实证→v6 消失)。

**ini 自治**:88 条双跑稳定 expected:FAIL 删除(run/delete 33、run/forwarddelete 25、run/insertparagraph 26、run/inserttext 4;probe6∩green_v6_full 逐 (variant,subtest) 配对),escape 语义 = `]`→`\]` 与 `\`→`\\`(inserttext 的 `"\\n"` 键实证);更新后定向复跑(载具 meta 副本 /var/tmp/e186-meta)四文件 unexpected-PASS 全归零、总数 67=66+1 逐格保持。

**残留登记(4 格/2 文件,均非本波谓词回归面,root-cause 已闭合)**:
1. run/forecolor.html + run/fontname.html 各 1(styleWithcss=false indeterm):execCommand 后 br 的 computed color 走 style() 缓存读取到 mutation 前 style(rgb(0,0,0)),styled text 读到 #0000ff → indeterm 误 true。**style 新鲜度域**(静态页 getComputedStyle(br) 实测正确 rgb(0,0,255),/tmp/e186-static-probe.py)——修复点在 style 查询/失效侧,超出本波编辑谓词 scope,独立小波候选;
2. other/delete-without-unwrapping-first-line-of-child-block.html 2 格 getTargetRanges:**基线未执行的新暴露格**(baseline 该文件 207 subtest entries,v6 209——前序格行为修复后测试首次跑到这两格):servo 键盘 Backspace 路径的 beforeinput 不携带 target ranges(仅 EditContext 路径 set_target_ranges,execcommands.rs Step 4.2 构造点无 ranges)——beforeinput 功能缺口,非本波回归,独立功能波候选。

**deletion-manifest**:无业务删除;ini 88 条 = 双跑 PASS 实证的 stale expected 替换(逐条 replacement)。工作树 /var/tmp/e186-wt(HEAD 63ffff8c3 快照)与 /var/tmp/e186-target(bao.e186-red/v6 及 probe 系二进制)、/var/tmp/e186-veh(全量 raw log ×3 + probe ×7)留证;EDA 探针 /tmp/e186-{wd,cell,style,indeterm,static}-probe.py。

**教训**:①「全局谓词升级」的正确姿势=先把谓词收窄到数据可证的极小核(本波 v1 双臂→v2 单臂的收缩让 8 格辐射直接消失),再按逐格证据加依赖方分支——e183 的转置近似整面回滚,本波单臂形态零目标辐射;②迭代中间态的"新红"必须先验基线完备性再归因(delete-without-unwrapping 的 2 格基线根本没跑——celldiff 的集合差分把"未执行"计成"新红");③walk 双臂共享 continue 语义时,可见性集合扩容的臂必须独立复检推进语义(v3 的 9.2/16.1 全臂扩展 = delete 15 新红);④forecolor/fontsize 对同一 placeholder br 的相反期望(裹 vs 不裹)是 Chrome-83 数据的固有非对称,数据锚点必须落到 force_the_value 的具体 Step 上而不是谓词层。

### e189(forecolor/fontname indeterm 2 格归因——style 新鲜度假说证伪,根因=font 属性 untyped 存储+indeterm 裸字符串比较,双缺陷均落 execcommand 域,域冲突停止上报,2026-10-12,REQ-BRW-050)

**任务锚**:e186 残留#1(run/forecolor.html?1-1000 与 run/fontname.html?1-1000 各 1 格,`[["stylewithcss","false"],["forecolor","#0000FF"]] "<p>[foo<p><br><p>bar]" queryCommandIndeterm(...) after`,expected false got true)。立项假说(承 e186 台账)= mutation→style 失效链断点,修复点在 style 查询/失效侧。

**假说证伪(探针证据链,/var/tmp/e186-target/test-ci/bao.e186-v6 + WebDriver 探针 /tmp/e189-{red,probe2,probe3,probe4}.py,端口 7089,flock)**:
1. RED 复现:同序列探针 `after=true`,innerHTML=Chrome 期望形态(`<p><font color="#0000ff">foo</font></p><p><font color="#0000ff"><br></font></p><p><font color="#0000ff">bar</font></p>`),`getComputedStyle(br).color=rgb(0, 0, 0)` 而**同 DOM 静态构造对照=rgb(0, 0, 255) 正确**;`getComputedStyle(包 br 的 font)` 也=rgb(0, 0, 0)——font 元素自身计算丢 color,非 br 特有。
2. 程序化对照:**不经 execCommand 的同形操作(createElement("font")+setAttribute("color") Detached 期设值+insertBefore+appendChild(br))全部正确**(a_brCS/a_fontCS/a2 属性改写/a3 二次包裹/b 静态/c style-attr 形态 7/7 rgb(0,0,255))——标准 DOM 路径的失效/重算链无缺陷。
3. **stale-clean vs dirty-skipped 判定实验:全文档失效(head 插 `<style>p{}`)+定向失效(p 加 class)后 br/font 仍 rgb(0,0,0)**——根本不是新鲜度/失效问题:该 font 元素的 **specified value 从一开始就不存在**,任何 restyle 都救不了。e186 台账残留#1 的「mutation 前 style 缓存」因果故事被证伪,撤案。
4. 修复性对照:命令产出的 font 上 JS `setAttribute("color","#0000ff")`(走 typed parse)后 br/font 立即 rgb(0, 0, 255)——**但 indeterm 仍 true**(见第二层)。

**根因(两层,均 vendor/servo/components/script/dom/execcommand/ 内,fork 自研代码)**:
1. **第一层:font 属性 untyped 存储**——`force_the_value` Step 10.5/10.6(contenteditable/node.rs:1414-1435)用 `set_string_attribute` 设 font 的 color/face 属性 → `set_attribute(..., AttrValue::String)` **跳过 `parse_plain_attribute` typed parse**;而 layout 侧 `HTMLFontElement` 的 presentational hint 消费(`get_color`/`get_face`,htmlfontelement.rs)只认 `AttrValue::Color`/`AttrValue::Atom` typed 变体 → String 形恒 None → 无色/serif,永久。正确惯形仓内三处现成:`make_legacy_color_setter!`(body bgcolor/text;`AttrValue::from_legacy_color`)、`make_atomic_setter!`(font.face IDL setter;`set_atomic_attribute`)、fontsize 的 `u32.into()`(同文件 node.rs:1487,AttrValue::UInt)。**为何只有 2 格**:indeterm/value/innerHTML 全走 font 属性 raw-string 短路(`HTMLFontElement::Color()`/`Face()` make_getter,不依赖 typed 存储,恒对)或字符串序列化;唯一经 computed style 继承穿过 font 的读者=br 的 `style.get_color()`——恰好 indeterm 多节点配对比较暴露格式/语义差。
2. **第二层:is_indeterminate 裸字符串比较+font 短路返回形态不一致**——`is_indeterminate`(basecommand.rs:386-407)以 `effective_command_value != previous_effective_value` 逐对比较;font 属性短路返回 raw attr 字符串("#0000ff"),br computed 路径返回 CSSOM 形态("rgb(0, 0, 255)"),**同语义不同格式恒不等** → indeterm 恒 true。probe4 实证独立承载:setAttribute 修复 br 颜色后 indeterm 仍 true。spec 锚:standard-inline-value-command 的 "distinct effective command values" 应走 equivalent-values 语义比较;但直接换 `are_equivalent_values` 不够——`parse_legacy_color`(stylo attr.rs:523)只认关键词/#hex/legacy 兜底,**rgb() 函数形态会进 legacy 兜底算法产出垃圾字节** → 归一化必须用能吃双形态的 CSS color 解析(cssparser 形),或把短路改为「仅无 computed style 时回退属性形态」使双侧同为 computed 形态(注意:短路在 force_the_value Step 17 是 load-bearing 的——just-inserted font 无 style data,守卫化会令 Step 17 补 inline style 破坏 innerHTML 期望 `<font color="#0000ff"><br></font>`,两臂必须同波设计)。fontname 面无归一化需求(face 属性串==CSS 序列化形态),第一层修复即可。

**同族横扫账(defect class=内部路径对 layout-typed 属性用 untyped setter)**:全仓 `set_string_attribute` 27 调用点逐个核对——live 同类点**恰好只有 force_the_value 两处**(color/face);其余安全:`style` 属性声明惰性解析不依赖 typed 存储(details/meter/select/progress/color-input/slider);meter value/min 等无 typed parse;nonce/crossorigin/translate 无 typed 消费;`make_setter!`(macros.rs:270,同展开到 set_string_attribute)现存使用面(iframe frameborder/img sizes·border/input colorspace/link imagesizes)经核对其属性均不在各自 parse_plain_attribute typed 集合。body bgcolor/text 走 make_legacy_color_setter 正确;img/iframe width/height/hspace/vspace/sandbox 走 typed parse 正确。class 闭包=2 点。

**域冲突与停止裁据**:两层修复点均在 `execcommand/` 目录(contenteditable/node.rs + basecommand.rs)= e188 并行禁触域,触发合同 stop「归因证明修复点必落 execcommand/ 编辑命令面→停止上报待串行」。修复白名单候选(供 e188 或后续串行合同,最小差分):
- A(第一层,contenteditable/node.rs:1414-1435 两行):color 臂 `set_string_attribute(cx, &local_name!("color"), ...)` → `set_attribute(cx, &local_name!("color"), AttrValue::from_legacy_color(serialize_to_simple_color(legacy_color).into()))`;face 臂同理 `AttrValue::from_atomic`(或经 `element.parse_attribute(&ns!(), &local_name!("color"), ...)` 走统一入口)。
- B(第二层,basecommand.rs is_indeterminate+resolved_value_for_node):color 比较语义归一(双形态 CSS color 解析)或短路守卫化+Step 17 等价判据同步归一(两臂联动,见上)。
- 预期翻转面:2 意外格清偿;`<p>[foo<p><br><p>bar]` 的 compare innerHTML 4 格(PASS exp:FAIL,ini stale 登记)与 fontname value-after(registered FAIL,serif→sans-serif 后或翻 PASS)正向连带;run/forecolor?1001-2000/?2001-last 与 fontname 同名族格需普查(B 修复后全 suite color 归一化语义变宽,可能翻转既有 expected:FAIL 的 indeterm 格为意外 PASS——需逐格 ini 自治)。

**验证边界**:本波零代码改动(域冲突),2 格未清偿(诚实口径);RED 探针四件套留存 /tmp/e189-*.py,e186-v6 二进制复跑可复现(after=true / rgb(0, 0, 0) / 全文档失效不救 / setAttribute 救 style 不救 indeterm)。

**教训**:①「缓存读到旧值」类表象必须先做「全文档失效不救」判定实验再立新鲜度假说——specified value 缺失与 computed 缓存 stale 在单点读数上同形,一次全量 restyle 即可二分(e186 台账残留#1 的因果故事未经此实验,方向错置);②typed attribute 存储(AttrValue 枚举变体驱动 presentational hint)是 servo 的隐式契约:**内部代码设 HTML 属性必须走 typed parse 入口(set_attribute+typed AttrValue / make_*_setter 宏族),set_string_attribute 只对 parse_plain_attribute 映射为 String 的属性安全**——横扫时以「layout 侧 get_*_for_layout 的 AttrValue 变体 match」为判据逐一回溯生产者;③multi-source 等值比较(属性 raw string vs computed CSSOM 字符串)是格式漂移温床:任何「两读数路径」的等值判定必须先问双侧的序列化形态锚,raw `!=` 即为候选缺陷。
