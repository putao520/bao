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
