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

### e188(insertParagraph flex/grid-item 原子分裂分支——e183 残留① 24 格清偿,2026-10-12,REQ-BRW-050)

**清偿**:e183 残留分类①(flex/grid 原子项 24)按 §e187 改判根因(命令层缺 Chrome 的 item 原子分裂分支)根治。24 格=4 容器型(flex/inline-flex/grid/inline-grid)× 3 caret 位([]abc/a[]bc/abc[])× 2 paragraphseparator,全在 run/insertparagraph.html `?7001-last` chunk,锚=editing/data/insertparagraph.js:2586-2710(Chrome-142 注记)。

**修复面**(vendor 2 文件,execcommand 域,§e187 判据一行谓词可达):
- `contenteditable/element.rs`:补 `resolved_display_inside_value()` 访问器(`style().get_box().display.inside()`,与 `resolved_display_value` 同一路径,DisplayInside import);
- `commands/insertparagraph.rs`:Step 7 归一化后(ws_styled 域门复算之后、inline_chain 采集之前)增 item 分裂分支——`flex_or_grid_item_ancestor`(caret 最近 block 祖先,其父元素 computed display inside∈{Flex,Grid,GridLanes}=原子 item)+ `split_flex_or_grid_item`:caret 后半段经 Range extractContents 迁入 item 的浅克隆(属性随行),插为 item 的紧邻兄弟,**容器零触碰**;空半侧 br 占位(`block_needs_placeholder`+`append_placeholder` 复用);caret collapse 到尾半段 0;ws_styled 域显式排除(e174 域门延续,e183 教训④)。现行为(容器克隆劈两个 div、style 属性随行,probe2 实证)被该分支前置短路。

**验收(RED→GREEN,基线=HEAD a5d1a604e 干净重建,私有 CARGO_TARGET_DIR=/var/tmp/e188-target,sccache 温)**:
- **24 格:RED 2/2 跑全 FAIL(24/24)→ GREEN 3/3 跑全 PASS(24/24),PASS 集逐字节恒等**——run/insertparagraph.html 66→42 unexpected-FAIL(fixed 24,与 flex/grid 族集合精确相等,零非 flex 旁效);caret 分布 []abc/a[]bc/abc[] 各 8;
- **全域(editing 857 tests,--processes 3,flock 端口互斥,载具 /var/tmp/e186-veh run_full.sh)**:red2_full vs green_full 逐文件对拍 **NEW-RED=0、WORSE=0**(improved 恰 24;unexpected-PASS 249→249 恒等;unexpected-FAIL 文件数 163→163 恒等);
- **insertlinebreak 同族保持**:34 flex/grid cells RED/GREEN 双侧全 PASS(0 unexpected 保持,§e187 回归面闭环);
- `cargo check --profile test-ci -p bao-servo-script` RC=0,触碰文件零警告(41 条全为存量他文件);
- 证据:/var/tmp/e188-veh/{red2_probe,red2_full,green_target1,green_full,green_target2,postdel_probe}.raw.log;二进制 bao.e188-red2(07:16,HEAD)/bao.e188-green(07:04,HEAD+2 文件)。

**基线污染事故(如实披露,red2 重取)**:首轮"RED"构建(06:50 启动)在其 script crate 编译窗(rlib 落盘 06:59:43)之内被本波源码编辑(mtime 06:57:16/06:57:39)穿透——"RED"二进制实际含修复(首轮 red_full 中 24 格已 PASS);判据=源 mtime vs script rlib 落盘 mtime 对照 + 目标 chunk 行为探针(污染 RED 全 PASS)。处置:污染日志改名 red_full.POLLUTED-green-behavior.raw.log 存证,源码回退重建真 RED(bao.e188-red2,探针 24/24 FAIL 实证)后全量重跑基线。green 二进制(07:04)编译于全部编辑落定之后、回退之前,树字节与恢复态一致,行为探针 24/24 PASS×3 实证,继续有效。**二次教训:①`cargo build` 的 unit 编译序≠拓扑直觉(script 在 link 前最后一刻编译),"后台 build 未到我的文件"必须以 deps rlib 落盘 mtime 实证,不得以 console 编译行位置推断;②绿色构建漏带 CARGO_TARGET_DIR 落入共享默认 target(/var/cargo-builds/…)——产物已取回 pin 至私有目录,共享 target 同期被他者使用未被覆盖(cargo 文件锁保证原子),未构成污染但已登记操作偏差。**

**deletion-manifest(已执行)**:~/code/tools/servo/tests/wpt/tests/editing/other/e187-probe.html + e187-probe2.html 删除(§e187 探针遗留,raw 证据 /tmp/e187-probe{,2}.raw.log 与 e186-veh 日志留存);/var/tmp/e185-veh/meta/MANIFEST.json 两项 items.testharness.editing.other 条目移除(备份 MANIFEST.json.e188-backup),postdel_probe 功能级复跑实证 manifest 可用。

**登记面**:无新增红;e186 残留(66−24=42)中 queryCommand 级联 13/白空间 6/双属性 4/散格等维持原登记归属;§e187 的「修复判据后续合同」清偿完毕,两探针资产按 manifest 删除。

### e190(forecolor/fontname 双层根修落地——font 属性 typed 存储+is_indeterminate 等值语义归一,2 意外格清偿+63 stale pin 自治;新暴露 face 合成 panic 类待裁,2026-10-12,REQ-BRW-050)

**清偿(两层,均 §e189 判据)**:
- **A 层(font 属性 untyped 存储)**:`force_the_value` Step 10.5/10.6(contenteditable/node.rs)color 臂 `set_string_attribute` → `set_attribute`+`AttrValue::from_legacy_color`、face 臂 → `set_atomic_attribute`(`AttrValue::Atom`)——镜像 `HTMLFontElement::parse_plain_attribute` typed 形态;presentational hint 合成(element.rs:1325/1353 只认 `AttrValue::Color`/`Atom` 变体)从此生效。
- **B 层(indeterm 等值语义归一)**:`is_indeterminate`(basecommand.rs)逐对裸 `!=` → `are_equivalent_values`(spec "distinct effective command values" 的等值语义);`are_equivalent_values` 的 BackColor/ForeColor/HiliteColor 臂双 `parse_legacy_color` → 新 helper `parse_color_for_value_equivalence`(CSS 全形态解析先行(canvas `parse_color` 复用,`pub(super)`→`pub(crate)`+2d/mod.rs re-export 一行)+ legacy 兜底)——`parse_legacy_color` 对 rgb() 函数形态落 legacy 兜底算法产出垃圾字节的格式漂移类根修。Step-17 font 短路**零触碰**(load-bearing 保持,探针实证 innerHTML 属性形态无 style 属性)。
- `cargo check --profile test-ci -p bao-servo-script` RC=0,触碰 4 文件零新警告(execcommand 2+canvas 2)。

**验证**:
- 探针(/tmp/e190-probe.py,ba.e190-green):t0_brCS=rgb(0,0,255)(RED rgb(0,0,0))、t0_indeterm=false(RED true,零 JS 修复)、fontname face 面 sans-serif 全通、innerHTML `<p><font color="#0000ff">…` Chrome 形。
- 目标 2 格 2/2 跑分布(green_target1/2 逐字节恒等)+ e188 green_full 日志二源 RED 双跑一致(expected:PASS status:FAIL);`<p>[foo<p><br><p>bar]` indeterm 双格 FAIL→PASS。
- **B 放宽正向连带 61 格**(全部 exp:FAIL→PASS 朝 Chrome parity,WPT 数据=Chrome 数据):forecolor 15(含 §e189 预测的 brown 双格 indeterm before——`parse_legacy_color("brown")` 与 computed `rgb(165, 42, 42)` 归一后等值)、backcolor 21、hilitecolor 23、fontname 3(stale pin 类);共 63 stale pin(双跑背书:full+run3 的 4802 格逐格恒等 + chunk1 三跑)经 wptmanifest AST 通道删除(vendor ini 4 文件 −189 行,round-trip 稳定断言),post-ini 确认跑 4 文件 unexpected-PASS **63→0**。载具 meta 副本(/var/tmp/e185-veh/meta)已同步。
- 基线 provenance:HEAD(e538dc98a)≡65983b025 代码态零差(git diff 实证)→ e188-green 二进制+green_full 日志为有效基线;green 二进制私有 CARGO_TARGET_DIR=/var/tmp/e190-target。

**新暴露缺陷类(全域 gate 唯一阻塞,scope 裁决待 lead)**:
- 全域对拍(857→855 tests):NEW-RED=1——event.html OK→**CRASH**,`vendor/stylo/values/computed/font.rs:573`「Family name with leading, trailing, or consecutive white spaces should have been marked quoted by the parser」(Script#10 panic)。
- **A/B 归因**:GREEN 二进制 `execCommand("fontName","","")` 即 panic(/tmp/e190-arm2-probe.py),基线同脚本正常(`<font face="">` innerHTML+value=serif)——命令路径新触发=A 层 face typed 化开启(face="" Atom → presentational hint → 合成声明 → to_css 断言);RED 二进制同 panic 面零暴露(face untyped String → 无 hint)。
- **类预存实证**:基线二进制在纯静态 `<font face="">`+`getComputedStyle().fontFamily` 下**同样 panic**(stderr 实证)——§e189「27 调用点」普查未覆盖 face-family 合成面;类缺陷=`parse_single_face_value_from_string`(htmlfontelement.rs)对含空白名恒标 `Identifiers`(stylo to_css 对 Identifiers 的空段 debug_assert)+ 空段不过滤(`get_face` 对空 Atom 返回 `Some("")`,而 basecommand.rs FontFamily 短路早有 `!face.is_empty()` 约定——两处约定漂移)。
- 待裁差分(1 文件 3 hunk,parse 侧根治非缓解):①`get_face` 空原子→None(event.html 命令面回归直修);②`parse_face_attribute` 过滤空段;③空白违约名标 `Quoted`(Chrome 对 `<font face="a  b">` 的 computed 序列化即引号形)。已上报 lead 3 次(含补丁终形),未落树(合同白名单=execcommand/+,htmlfontelement.rs=vendor 其他域)。
- 上游候选(stylo vendor):`parse_legacy_color` Step 9 `input.as_bytes()[0]` 对 trim 后空串(如 `" "`)越界 panic——本波暴露面≤旧路径(B 层 CSS 解析先行),独立登记不本波修。

**deletion-manifest**:ini 63 条 stale expected:FAIL 删除(双跑 PASS 实证逐条 replacement,见上);业务代码零删除。留证:/var/tmp/e190-veh(raw log ×5:red_target1/green_target1/2/3/green_full/green_post_ini)+/var/tmp/e190-target(bao.e190-green)+/tmp/e190-{probe,arm2-probe,cellset,flip-cells.txt}。

**教训**:①typed 属性存储的 producer 侧修复(执行器路径设属性)必须同步审计 consumer 侧合成路径的 parse 不变量——layout-typed 变体是「producer 必须走 typed parse」与「consumer 假设 typed 值满足引擎不变量」的双向契约,e189 的普查只看了 producer 侧;②「B 层放宽语义」的红利面(61 格)远超目标 2 格,等值比较归一是多读数路径浏览器的系统性修复,e189 预测的「可能翻转既有 expected:FAIL indeterm 格」以 63 格兑现;③CRASH 归因必做基线同路径复现(arm2 probe 双二进制)——「静态路径同 panic」一条就划清了「本波引入类」与「本波新触发预存类」的归属线。

### e191(face-parse 三 hunk 授权落地收口——e190 接管,2026-10-12,REQ-BRW-050)

**落地面**(主会话 4 次授权+接管裁定后,e190 入向通道故障无接收证据;开工前树态核验与 §e190 记载一致,htmlfontelement.rs 零改动):
- `htmlfontelement.rs` 三 hunk(§e190 终形,零设计改动):①`get_face` 空原子→None(镜像 basecommand `!face.is_empty()` 约定)②`parse_face_attribute` 空段过滤 ③空白违约名标 `Quoted`(`name.split(' ')` 逐段非空才 Identifiers,判据镜像 stylo to_css 断言)。
- commit 分面:① e190 九文件(A/B 层+canvas widening+ini −189)= `4541ee75adc5fd673b6645602420544673028672`;② face-parse 三 hunk = `64088911c6b70629b5f1a1045523c6aa0c0d5862`;③ 本台账补记。禁 push。

**复验账**(二进制 bao.e191-green,私有 CARGO_TARGET_DIR=/var/tmp/e190-target 复用 e190 暖缓存;provenance:源 mtime 12:14:53 < libscript rlib 12:18:11 < 二进制 12:18:14,构建窗清洁):
- `cargo check --profile test-ci -p bao-servo-script` RC=0,触碰文件零警告。
- RED 侧(前态二进制 bao.e190-green):facecrash 探针静态臂即崩(返回 null)+arm2 stderr 直证 font.rs:573 panic 复现。
- GREEN 侧:facecrash `static_empty`="serif"(face="" 无声明回默认)、`static_dblspace`=`"a  b"` 引号形(Chrome parity);arm2 基线形 `<font face="">foo</font>`+value=serif+indeterm=false;两探针 stderr 零 panic。
- event.html 定向:**OK**(CRASH→OK 翻转)。
- 全域 post-sync 跑(855/855,载具 attempt 1 CLEAN):NEW-RED 唯一余项=other/data_transfer_on_input_event_with_insertfrompaste_type.html 整文件 ERROR 格——**3/3 隔离复跑全绿=③类并行负载 flake**(harness RemoteDisconnected teardown 形,§e167 已登记类,与 face-parse 域零交集);improved=1(event.html);uP 306→243 恰=63 pin 清除;forecolor/fontname `<p>[foo<p><br><p>bar]` 目标格全 PASS(expected:None 干净态)。

**载具 meta 副本 stale 事故(如实登记)**:e190 §e190「载具 meta 副本已同步」声明与现状不符——green_post_ini(08:50,uP 63→0 确认)跑时 meta 为同步态,**10:50 被覆写回 pre-deletion 内容**(归因已闭环 2026-10-12:e190 自述=其本人防污染回滚——当时 commit 持有、HEAD 二进制对 63 删除格会记 NEW-RED,同步态 meta 对同伴波是污染源,其终报有载「载具 meta 副本已回滚 HEAD 形态」;非未知来源,channel 事故期消息交错致 e191 误记「未知操作」);e191 首轮全域跑(12:31)消费 stale meta,4 文件 63 格以 uP 形态记账(行为面 PASS 不变,NEW-RED=0 不受影响)。处置:4 文件 ini 从 vendor 重同步(md5 逐文件一致)+全域 post-sync 净跑(12:45)取 canonical 账。**遗留**:载具 meta 副本与 vendor 在另 4 文件(delete/forwarddelete/insertparagraph/inserttext)存在 e190 范围外历史 drift(方向同为 meta 多 pin,早期波 ini 删除未同步载具;e190 基线 green_full 与 e191 跑共享同副本,对拍有效性不受影响)——后续波若全域对拍,建议先全树同步 meta 副本再取基线。

**误跑事故(如实登记)**:e191 首轮全域调用漏传 `/editing` 路径(run_e183.py 空 test_list=整个 WPT manifest),发现于 test_start 计数(1804≠855)后立即终止清日志重跑;载具 run_full.sh 不强制测试面参数,为已知操作陷阱。

### e194(face-parse 类根修持久回归面落地——e190 条件 3 裁量成立,bao-browser suite live e2e 三断言,2026-10-12,REQ-BRW-050)

- 落点:`src/bao_browser/tests/suite/face_parse_regression_live_tests.rs`(main.rs 注册)——§e190 条件 3「立持久回归面」裁量成立的载体(vendor 单测不可执行=登记判例,bao-browser suite live 测试=在位形态,镜像 audioworklet/editing_e2e live 先例:BAO_TEST_NETWORK+DISPLAY 双门+serializer)。三断言镜像 /tmp/e190-facecrash-probe.py 判定语义(data: URL 同文档形):①静态 `<font face="">` computed fontFamily=`serif`(无声明,零 panic——panic 类经 eval Err/15s 有界 spin 显形)②静态 `<font face="a  b">` 引号形 `"a  b"`(JSON 转义断言)③命令面 execCommand fontName "" value=`serif`+indeterm=false;另 err_* 捕获字段零出现断言(JS 侧异常非静默)。
- 复验:`cargo check -p bao-browser --features webdriver --tests` RC=0 零新警告;`BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser -E 'test(face_parse)'` **1 passed / 0 failed**(2021 skipped),探针实测值 `{"static_empty":"serif","static_dblspace":"\"a  b\"","cmd_innerHTML":"<font face=\"\">foo</font>","cmd_value":"serif","cmd_indeterm":false}` ≡ §e191 green 账逐字段一致;RED 侧对照引用 §e191(前态二进制静态臂即崩+font.rs:573 stderr),本面不重复 A/B。日志留证 /tmp/e194-face-parse-nt.log。

### e193(meta stale-pin 机械清扫——vendor canonical uP 155 格双跑背书全清,e192 面 4 格留,2026-10-12,REQ-BRW-050)

**范围校正(重要,e191 账口径修正)**:e191 全域账 uP=243 消费的是**部分 drift 载具 meta**;本波开工按 §e191 教训先全树同步载具 meta←vendor(`rsync -a --delete`,diff -rq 恒等,md5 快照留证),同步即剥掉 delete/forwarddelete/insertparagraph/inserttext 4 文件 88 个「vendor 本就没有的 drift pin」→ **vendor canonical stale-pin 宇宙=155 格/18 文件**(runA 实测,uF 面 20910/273 文件与 e191-green_full 逐字节同值,交叉验证成立)。

**方法(纯 meta 面,零代码改动)**:
- 干净重建 opt 二进制 bao.e193(`CARGO_TARGET_DIR=/var/tmp/e193-target cargo build --profile test-ci -p bao_bin`,10m44s;provenance:构建窗内 vendor+src 零源文件 mtime 晚于 libscript rlib 13:06:48 < 二进制 13:06:51,e188 判据;HEAD 6a020cc0f)。
- 全域双跑 runA/runB(855 tests × 2,attempt 1 CLEAN × 2):**cell 宇宙逐格恒等**(116824=116824,only-A=0 only-B=0,唯一 changed 格=edit-context-paste-html exp:PASS 格 FAIL→PASS=A/B flake,非删除集)。
- 155 格(PASS+exp:FAIL 双跑背书)经 **wptmanifest AST 通道**删除(/tmp/e193/astdel.py:parse→round-trip 字节恒等门→定位 KeyValueNode(单 ValueNode 'FAIL' 纯形,条件/List 形 REFUSE 拒触)→删 pin→空 subtest 壳连带删→空 section 连带删→serialize→re-parse 稳定断言;not_found 任一即整文件拒绝=fail-closed):**17 文件 −477 行,零插入行**(16 M+1 全空删=insertparagraph-in-child-of-html.tentative.html.ini 48 格即全部内容;每 pin 3 行形=§e190 形),vendor diff --numstat 全列 0/39 形。
- **e192 邻接面隔离**:delete-without-unwrapping-first-line-of-child-block.html.ini 4 格全程跳过(vendor+载具两份零触碰,git diff 空)。

**post-sweep 确认账(runC,attempt 1 CLEAN)**:
- **uP 155→4,残留恰=e192 在途面 4 格**(delete-without-unwrapping),本波目标格 151 格全清归零。
- FAIL 面恒等:uF 20910→20911;cell 级 diff(celldiff)= NEW-RED 仅 2 格——`plaintext-only/paste.https.html?white-space=pre-line`「Pasting 2 paragraphs…」×2,runC 单跑红;**3/3 隔离复跑全绿=③类并行负载 flake**(§e167 已登记类,非本波回归:meta-only 改动不可能改行为,同二进制 A/B 双跑均 PASS);另有 1 格 FIXED=即上述 A/B flake 格(edit-context-paste-html,C 跑转绿)。**确定性 NEW-RED=0**。
- 载具 meta 全程受控:md5 快照三查(同步态≡runA/runB 消费态;post-sweep 态≡runC 消费态;runC 后零变化)。

**flake 登记(不修,载具负载时序类)**:①`/editing/edit-context/edit-context-paste-html.tentative.html`「Test that HTML is pasted into EditContext as plain text」(A 红 B/C 绿);②`/editing/plaintext-only/paste.https.html?white-space=pre-line`「Pasting 2 paragraphs」「Pasting 2 paragraphs into <b>」pasted result 双格(C 红 A/B 绿+3/3 隔离绿)。

**证据**:/tmp/e193/{runA,runB,runC,iso_paste_1..3}.raw.log+console/attempts、delset.json、astreport.json、veh-meta-*.md5;二进制 /var/tmp/e193-target/test-ci/bao;工具 /tmp/e193/{passset.py,astdel.py}。ini 载具副本已同步 post-sweep 态。

### e192(键盘驱动 beforeinput 携带 targetRanges——InputEvents L2 contenteditable 功能缺口清偿,9 格双跑翻转+全域对拍零差,2026-10-12,REQ-BRW-050)

**归因(静态全集+运行时探针双证)**:键盘 Backspace/Delete/InsertText 在 contenteditable 的 beforeinput 唯一派发点=execcommands.rs Step 4.2(exec_command_for_command_id;链:run_default_keydown_handler:2067→EditingContext::Document::perform_editing_action(editing.rs:1058)→Document::perform_editing_action(editing.rs:640)→Step 4.2 构造 InputEvent 后无 set_target_ranges)。beforeinput 派发点全集恰 3 处:execcommands.rs(缺口面)/editcontext.rs(EditContext 路径,insertText 已填)/text_control.rs(spec Empty=正确,未动)。上游对照:origin/main inputevent.rs GetTargetRanges=硬编码 Vec::new()+TODO(fork 自研成立)。语义锚=InputEvents L2 "content that the event will modify if it is not canceled";text 控件+historyUndo/Redo=Empty。

**实装(execcommands.rs +175/−9;delete.rs 仅可见性 token,已按授权条件预报告)**:`target_ranges_for_command`(Step 4.2 派发前调用,非塌缩=active range;塌缩+Delete=前兄弟 removable line-break/shell 臂(复用 delete.rs `is_removable_line_break_or_shell` pub(crate) 化,零语义 delta;caret 归一化落 text 节点 offset 0 时沿 inline 链爬升——运行时 eprintln 探针实证,已移除)+文本臂 (offset-1,offset);塌缩+ForwardDelete=文本臂 (offset,offset+1)+经 walk 内联下降(child at offset 为 Text),删除单元恰好清空 inline 壳时加宽到壳元素父级坐标(`` `<b>\n</b>[]{}` `` → (host,0)-(host,1),Chrome 唯一接受形));塌缩+其他命令与深 walk 形(隐形节点移除/块合并)=空(残留登记)。**cons 壳内快照陷阱教训**:首版加宽把坐标写进壳内 (b,0)-(b,1),Chrome 报父级坐标 (host,0)-(host,1)——元素单元 range 的坐标级=壳的父级。

**验收**:RED=HEAD 6a020cc0f 干净重建(私有 CARGO_TARGET_DIR=/var/tmp/e192-target,binary 13:16:32;vendor script 零 diff、无源文件晚于二进制、并行 commit 986888d0c test-only 后置)。终态二进制对目标文件 **5 跑**(green3/green4 终版×2+green5/green6 ini 后×2+green_full)恒等:9 格全 PASS、population 209、ini 后 0 unexpected。**全域 editing 对拍(red_full vs green_full,冻结 meta 快照 /var/tmp/e192-meta-snap,flock CLEAN×1)**:population 115969=115969、**NEW-RED=0、WORSE=0、GONE=0、IMPROVED=恰 9**。9 格账=合同 2(getTargetRanges Backspace `<br><div id=child>[\]def`/`<b><br></b>[\]def`)+同族 br 变体 4(abc<br><br>、<br><br> 裸暴露格)+DeleteKey&preformat 5(前向文本臂 4+壳加宽 1)。

**ini 自治(双跑背书,repo+载具副本双写,字节恒等门)**:11 条 expected:FAIL 删除——7 条 getTargetRanges(BackspaceKey&br 2+DeleteKey&preformat 5)+e193 留给本波的 4 条同文件外层 stale pin(`Backspace/document.execCommand("delete") at abc<br><br>/<br><br> []def` 外层删除断言,红绿双侧多跑恒 PASS);green6 终验 0 unexpected。

**登记(未清偿残留,禁顺手扩面)**:①clipboard insertFromPaste/insertFromDrop 的 contenteditable beforeinput targetRanges(spec Non-empty,现空)——editing.rs fire_beforeinput_on_element paste 面,姊妹缺口独立波;②塌缩+非 delete 命令(insertText/insertParagraph/format* 等)与深 walk 形(隐形节点移除/块合并)的 targetRanges 仍空——Chrome 形非空,同属后续增量;③CDP Input.dispatchKeyEvent 面:与 testdriver 键投同收敛 run_default_keydown_handler,本修复一覆盖两载体(e194 形态登记,未单独立探针)。

**证据**:二进制 /var/tmp/e192-target/test-ci/{bao.e192-red,bao};raw log /var/tmp/e186-veh/e192_{red,green1..6,red_full,green_full,probe}.{raw,console}.log+attempts;build log /tmp/e192-{red,green}-build.log;冻结 meta 快照 /var/tmp/e192-meta-snap;载具副本前态备份 /tmp/e192-veh-meta-before.ini。

### §e195-strand③ 定性(主会话预取证,2026-10-12 13:5x,e192 二进制 1aea01c07 只读复跑)

- **paste.https.html:98/98 全 PASS 零钉零红**——e161 登记残留「paste.https 12」已被后续波(e167 clipboard 块分隔序列化/e186 谓词/e188 flex 分裂/e192 targetRanges 链)顺带全量清偿,该 strand 面清零。
- **paste-when-nested-with-contenteditable-true.https.html:94 PASS+4 裸暴露 FAIL(无 ini 钉)**——嵌套粘贴 beforeinput text/html 语义(`pasting in contenteditable=…` 系),即 e161「nested 4 beforeinput」的存留面;与 strand② paste-clipboard-change/strand④ insertFromPaste targetRanges 同属 clipboard text/html 语义簇,e195 恢复后按此证据直接归并处置(证据 raw log /tmp/v_e195_s3_paste.raw.log)。

### e195(editing 尾批收官:strand④ paste targetRanges 实装 + strand② paste-clipboard-change 引擎半边根治 + ①③⑤ 定性/普查,2026-10-12,REQ-BRW-050)

**分面 commit**:① strand④ = `3b257255e`(editing.rs paste 面 + editcontext.rs EditContext paste 面);② strand② 引擎半边 = `312d7f119`(execcommands.rs canceled-copy 写回)。禁 push。

**基线**:HEAD 1aea01c07 干净重建 bao.e195-red(私有 CARGO_TARGET_DIR=/var/tmp/e195-target;provenance:源 mtime < binary 15:01:29,零触碰文件警告);全域 855 tests attempt 1 CLEAN:uF 273 文件/20903 cell、uP 0、population 116824(与 §e193 cell 宇宙逐格同值,交叉验证成立)。冻结 meta 快照 /var/tmp/e195-meta-snap(vendor canonical rsync 恒等门 + md5)。

**strand④(insertFromPaste targetRanges 姊妹缺口)——清偿**:
- 实装双面:`paste_into_current_editing_context`(contenteditable host)与 `EditContext::handle_paste` 的 beforeinput 由空改携当前选择(派发前捕获 active range 为单一 StaticRange;spec 锚=input-events getTargetRanges「the content that the event will modify if it is not canceled」,Chrome 生成数据钉死非删除类 inputType=当前选择、塌缩含——input-events-get-target-ranges.html「Actions other than deletion should have current selection as target ranges」,`test2-5-test2-5` 格=塌缩 caret 亦报告)。EditContext 面把 `target_ranges_for_insert` 的选择段抽为 `edit_context_selection_target_ranges` 单源(canvas host 排除保持),insertText 与 insertFromPaste 共用——零语义 delta。
- **insertFromDrop 定性**:全仓(vendor script)零 drop 编辑插入路径,该 inputType 无派发点——e192 残留①的 insertFromDrop 半边=不存在面,非缺口。
- oracle:无 /editing 测试钉 paste targetRanges(editing 域 grep 全集=delete-without-unwrapping 9 格+edit-context-input insertText 格),临时探针 `/input-events/e195-paste-target-ranges-probe.html`(reference repo+冻结 MANIFEST 条目,deletion-manifest 已于收口删除):RED=assert length 1 got 0(1 跑);GREEN=contenteditable 面+EditContext 面 3/3 subtest ×2 跑(green5/green6)全 PASS+早期单面 2 跑 PASS。**EditContext 探针首版自身缺陷**(监听器漏挂 echost)致 2 跑 FAIL——探针 bug 非引擎,修后全绿。
- ini 面:零改动(strand④ 无 /editing cell 翻转,加性 payload 不改既有行为——全域对拍实证)。

**strand②(paste-clipboard-change 2)——引擎半边根治+双残面定性**:
- **根因一(引擎缺陷,已修)**:`exec_clipboard_command_on_text_control` 在 copy/cut 事件被 canceled 时直接 return false,监听器 setData 的载荷被静默丢弃(取证二段:修前探针 `textctl-copy prevented=true` 且 `write_content_to_the_clipboard` 零调用;修后 `write t=… v=New text`+pump `set-recv v=New text`)。修复= canceled 分支补 `write_content_to_the_clipboard` 写回(镜像 handle_clipboard_action Step 4 canceled-Copy 臂;clipboard-apis canceled 分支义务);`write_content_to_the_clipboard` pub(crate) 化;返回值 false 保持(无 WPT 钉;exec-command-with-text-editor 零 preventDefault cell)。与 paste 臂既有的后置重读(e182)闭合:修复后链路=写回→重读采纳「New text」→插入。
- **根因二(残面,scope 外登记)**:?id=text 修复后 1/5 跑 PASS,其余=**arboard/X11 后端微秒窗 set→get 竞态读空**——二代带时间戳探针四跑实证:pump 侧处理序 write→set-recv→get-recv 恒正确而读回为空(run1 fresh="" 于 set-recv 同毫秒;run2/4 snapshot="" 于自身 setup set 后 124ms-4s),叠加并行浏览器进程争用同一 X11 CLIPBOARD。后端定性:`cargo tree -i arboard` 实证 arboard 3.6.1 已链接,clipboard_delegate.rs linux+feature=arboard 模块活跃。**连带修正 §e167 判据**:「clipboard feature off→fallback 进程本地、跨进程污染不可能」不成立于 WPT 二进制(feature 统一后 arboard 活跃)——e167/e193 登记③类 stale-clipboard flake 的「同进程 fallback」排除柱塌,跨进程 X11 共享即其更简归因。修复域=servo 剪贴板后端面(clipboard_delegate.rs/servo.rs,scope 外):候选=进程内事务读写(镜像 Chromium in-process clipboard)或写读序号化。**缓解禁令**:未加重试/超时。
- **根因三(?id=contenteditable,非本域)**:空 contenteditable 宿主零高→`test_driver.click` 报「element click intercepted」→async 在 click 处 abort,**paste 管线从未执行**(complete payload 仅含 click rejection,无 assert 记录;paste-arm 探针缺席旁证)。命中/布局域(空编辑宿主 caret 行盒),非 clipboard 域。
- ini 面:零改动(?id=text 现为 flaky-pass,expected:FAIL pin 保留如实可见;无双跑 PASS 背书不删)。

**strand①(qCV 族)——普查升级+设计登记,未实施(合同 stop 条款)**:§e182「8 格」登记对现实严重低估——当轮基线普查=**173 cell 四家族**:stylewithcss-qcs 71(before 态 expected false got true)、dps-qcv 49(expected "p" got "div" 跨 cell 状态泄漏形)、forecolor-qcv-delete 32+backcolor-qcv-multitest 21(value 序列化形:expected computed "rgb(165,42,42)" got 属性 raw "brown"=§e189 第二层 font 短路在 queryCommandValue 路径的存留,e190 B 层只归一了 indeterm 等值比较)。multitest cell 模型=同文档顺序执行(tests.js doSpecCell 每 cell 新 div 但 document 级命令状态共享),Chrome 期望值含其自身跨 cell 泄漏序。设计登记(后续独立合同,先取证 tests.js 状态模型再动代码):a) forecolor/backcolor 53 格走 §e189 已登记的「value 路径归一化为 computed 形态」候选(force Step-17 短路 load-bearing 两臂联动);b) dps/stylewithcss 120 格=命令状态生命周期取证(e182 override 试错负证据仍有效,禁轻动)。合同 stop「连续 2 轮零新证据 → 登记」:本轮产出=普查+家族形态+根因分型(新证据),实施面让位独立波。

**strand③(text-html 残留定性)——主会话预取证段确认+钉态勘误**:paste.https.html 98/98 全绿(本波全域账零红,与 lead 预取证一致)=e161 登记 12 格已被 e167(块分隔)顺带全量清偿。paste-when-nested 4 格=嵌套粘贴 text/html 语义,**钉态勘误:非「无 ini 钉裸暴露」——vendor ini 有 4 条 expected:FAIL(raw log exp 字段缺省=as-expected 符合预期语义)**;根因双缺陷=paste 管线对 contenteditable 只插 text(忽略 text/html payload,`paste_into_current_editing_context` 的 html 参仅进 dataTransfer)+不替换非塌缩选择(e160 时代已登记的 replace-selection 缺口),4 格 got 形(`ABCabc` 未替换+纯文本)与该双因精确一致。归并处置:clipboard text/html 语义簇(与 strand② 数据通路残面同域),独立合同域。

**strand⑤(塌缩非 delete 命令与深 walk 形)——定性登记**:语义锚已由 Chrome 生成数据钉死(input-events-get-target-ranges.html 第二 promise「Actions other than deletion should have current selection as target ranges」:insertText 塌缩=collapsed caret 本身 `test2-5-test2-5`、全选替换=全选段、bold=选段)——**e192 的「塌缩+非 delete=空」残留的 Chrome 正形=当前选择,与 strand④ paste 面同一谓词**(active range 直报,无 walk 需求);实现落点=execcommands.rs `target_ranges_for_command` 塌缩臂对非删除命令回当前选择(一行级),深 walk 形(隐形节点移除/块合并)仍需 walk 前解析=独立增量。本波未实施(合同=定性账),设计已可直落。

**全域对拍(red_full vs green_full,同冻结 meta,flock attempt 1 CLEAN)**:uF cell **20903=20903 恒等**、uP 0=0;celldiff 仅 2 格反向 flake 翻转——NEW-RED 1(data_transfer_on_input_event_with_insertfrompaste_type 整文件 ERROR)=§e191 同格已登记③类负载 flake,**3/3 隔离复跑全绿**;FIXED 1(edit-context-paste-handler-changes-active「redirects focus」格)=同登记类反向。**确定性 NEW-RED=0、WORSE=0**。

**deletion-manifest(已执行)**:/home/putao/code/tools/servo/tests/wpt/tests/input-events/e195-paste-target-ranges-probe.html 删除(探针收口,raw log 留证);冻结快照 MANIFEST 同步移除条目。**载具事故登记**:MANIFEST.json 曾以 json.dump 紧凑形重写(键序保持、JSON 等价;red_full 消费原始形、green_full 消费重写形——对 /editing 测试宇宙零差,celldiff population 恒等实证)。

**证据**:二进制 /var/tmp/e195-target/test-ci/{bao.e195-red,bao.e195-green,bao.e195-probe,bao.e195-probe2};raw log /var/tmp/e195-veh/(red_full/green_full/ptr_probe_{red,green1..6}/pcc_green{1..3}/pcc_probe2_{1..4}/dt_iso_{1..3});build log /tmp/e195-{red,green,probe,probe2}-build.log;冻结 meta 快照 /var/tmp/e195-meta-snap。

**教训**:①「登记 N 格」的台账数字会随波演进而失真——strand① 按 §e182 字面立项(8 格)实测 173 格,后续合同必须先跑当轮普查再定方案粒度;②「flaky 1/N」的格子必须先分型再定 ini 策略(本波 text 格=引擎缺陷已修+后端竞态残面,pin 保留=诚实可见;若误判纯 flake 删 pin 会把残面藏进 as-expected);③ WPT raw log `expected` 字段缺省=as-expected(ini 钉消费态),「无 expected 字段=意外」的朴素口径会把 ini 钉格误计——celldiff 的集合差分两侧同口径故相对结论有效,绝对数必须按 §e185 判读律核对;④ GenericCallback 阻塞往返的「FIFO 直觉」在跨后端(arboard/X11)边界不成立——写读时序类归因必须带时间戳探针到 pump/后端两侧,单侧脚本线程探针会得出「顺序正确但读回错误」的不可解矛盾。

### e197(嵌套粘贴双缺陷根修——paste 管线消费 text/html + 非塌缩选择删后插,4 格清偿+全域 NEW-RED=0,2026-10-12,REQ-BRW-050)

**双缺陷与修法**(§e195 strand③ 勘误定性清偿;单文件 `document/editing.rs` paste_into_current_editing_context contenteditable 臂):
1. **text/html 消费**:此前 html 形只进 beforeinput/input 的 dataTransfer 载体,插入恒走 text-only(4 格 got `ABCabc` 的 abc 半边)。新增 `parse_paste_fragment`(createContextualFragment 语义:`Element::fragment_parsing_context`+`Element::parse_fragment`,caret 容器或其父元素为上下文)+ **script 元素剥离**(Chromium paste sanitizer 的可执行内容子集——旧 text-only 插入永不可能引入 script 节点,rich paste 不得成为注入向量;parse 先于任何 DOM 突变,失败降级 text 路径)。**外层载体规则** `paste_destination_preserves_format`:自插入点上溯,`contenteditable="true"/"plaintext-only"` 属性载体逐个覆盖 plain/format 决策(最外层胜出),ce="false" 或非可编辑边界(body/html/document)即停——嵌套属性在此即边界,尽管(e153 继承规则)它不成事件宿主:plain/format 是整段可编辑区的属性,非事件归属。无载体默认保格式(designMode/纯 true 链)。WPT 锚=paste-when-nested-with-contenteditable-true.https.html 四格期望(plain 嵌 true→保格式;true 嵌 plain→纯文本;false 孤岛恒边界)。
2. **非塌缩选择删后插**:此前 !collapsed_inside 直接 collapse 到 focus(got `ABCabc` 的 ABC 半边)。改为 beforeinput 取消门之后 `selection.DeleteFromDocument`(canceled paste 零突变保留;targetRanges 维持真实预贴选择,Chrome-true)。focus 在宿主之外时回退臂保持原语义 collapse(focused,0)。

**中途回归一次(诚实记录)**:v1 重排 collapse 块时把 focus-外回退臂错改为 collapse 到遗留 focus 节点(promise_setup 里 input 的选区)→ paste 落进上一编辑器,edit-context-paste-handler-changes-active 后两格 v1 隔离 ×3 稳定 FAIL(got "")。red-iso ×3 全 PASS 钉死真回归 → v2 恢复原回退臂语义,5/5 ×3 全绿。教训:collapse 块的三臂语义(inside-collapsed/inside-noncollapsed/outside)各自承载不同契约,重排只能动 inside-noncollapsed 臂。

**验证**(RED=e197-red(afdaeac79 干净重建,private CARGO_TARGET_DIR,cp -al 增量 2m13s,script crate 真重建级联,md5 e28c…/绿 01b4…;树 provenance:构建窗内 scope 文件仅本波 editing.rs,e196 basecommand.rs 19:42 落盘晚于 v2 构建 19:23 零混入);载具 /var/tmp/e197-veh(meta=e195-meta-snap 恒等拷贝+目标 ini 载具侧先行删除;flock+ss 门)):
- 目标 4 格:RED ×2 全 FAIL(got `ABCabc` ×4,双因精确一致)→ GREEN v2 ×2 全 PASS(14/14)。
- paste.https.html:RED 84 PASS/0 意外 ≡ GREEN v2 84/84 恒等(零回归;任务头「98/98」系旧口径,本波实测基线=84 格)。
- 全域对拍 red_full↔green2_full:113,384 格,**NEW-RED=0**,status 差 6 格全正向:4 目标格 + anchor 格(html-text-copy-paste-of-anchor「Attribute href is missing」rich paste 直接受益,red-iso F×3→green-iso P×2 真改进)+ paste-html 格(经 red-iso P×3 证伪改进归因=red_full 负载 flake,双侧隔离恒 PASS;handler 格同族 flake 已在 e160 登记)。
- uP 账:anchor 格 ini 钉 expected:FAIL 在 other/ 域=本波 scope 外,frozen-meta 账面留 uP 1 待该域合同清钉(双跑 PASS 背书已留:/var/tmp/e197-veh/iso_anchor_{red_1..3,g_1..2})。removing-inline-style 两格=单文件载入内 3 重复自变(双侧交错 P/F),内在 flaky 定性,与本波无关。
- cargo check -p bao-servo-script RC=0,editing.rs 零警告。

**deletion-manifest**:vendor ini 目标文件整删(4 格双跑 PASS 背书,循 e160 paste.https.ini 先例);无业务删除面。

**证据**:二进制 /var/tmp/e197-target/test-ci/{bao.e197-red,bao.e197-green};raw log /var/tmp/e197-veh/(red_n1/n2、red_paste、red_full、green_n1/n2、green_paste、green_full(v1 回归谱系)、iso_handler_{1..3,_red_1..3,_g2_1..3}、iso_pastehtml_{1..3,_red_1..3}、iso_anchor_{red_1..3,g_1..2}、iso_removing_{1..3}、green2_n1/n2/paste/full);celldiff/fulldiff /var/tmp/e197-veh/{celldiff,fulldiff}.py;build log /tmp/e197-{red,green,green-build2,green-build3}.log;载具配方继承 e185(run_e183.py+E197_META_ROOT)。

**教训**:①共享树并发期(E196 同仓在途)构建 provenance 必须 mtime 三点对照(我的源/他人源/构建时刻)——本次 e196 49 行 WIP 晚构建 19 分钟,账目清白靠的是核对而非假设;②重排多臂条件块前先给每臂标注其契约(此处 outside 臂承载 edit-context-paste-handler-changes-active),否则回退臂语义漂移成真回归;③正向翻转格的改进归因必须红侧隔离复跑钉稳定性——paste-html 格若未做 red-iso 会被误记为本波改进。

### e196(qCV/qCS face 收官——§e195 普查勘误(口径倒置)+B 面状态生命周期 Chrome-shared 判死+A 面 value 路径/BackColor override 双根修,2026-10-12,REQ-BRW-050)

**普查勘误(重要,推翻 §e195 strand① 的 173 格口径)**:§e195 的四家族普查消费了**倒置的 raw-log expected 语义**——WPT raw log 中 `expected` 字段**缺省=as-expected(ini 钉消费的 FAIL)**、**`expected:"PASS"` 在场=真意外 FAIL**(§e195 教训③文字在档但其普查代码未遵守)。按正确语义对本波冻结基线(HEAD afdaeac79 干净重建 bao.e196-red;载具 meta←vendor 全同步 md5≡e195 冻结快照)重普查:**四家族 162 格 = 148 ini 钉消费(as-expected FAIL,诚实可见红)+ 14 真意外**(dps-qcv 8 + stylewithcss-qcs 6,全部落在 insertparagraph.html 13 格 + insertlinebreak.html 1 格);「forecolor-qcv-delete 32 / backcolor-qcv-multitest 21」实为 16/26 且 **100% 钉消费**。本波全域账:population=115969、uF=51 cell/12 文件、uP=1(paste-clipboard-change flaky 类),与 §e195 red_full population 恒等交叉验证成立。

**B 面(stylewithcss-qcs/dps-qcv 状态生命周期)——取证→判死,零引擎改动(合同 stop 条款落地)**:
- **tests.js 状态模型取证( replica 形态检验)**:①run/*.html 每页加载时「silly hack」`execCommand("styleWithCSS",false,"true")` 置 swc=true(Chrome 150 实测生效);②数据文件的 before/after 期望=生成期**全序列命令链**的泄漏值;③**variant 页(?N-M)把 execCommand 藏在 subsetTest 门内**——out-of-range 格只跑 setup+before 查询、**跳过命令**,故早段 in-range 格的 before 读到页初态(hack true/div 默认)而非生成链值;④after 查询无条件跑但仅 in-range 格有断言。
- **引擎状态机实证正确**:replica 页(execCommand spy)逐命令读回——每个 set 落地、每格链值≡数据记录链;bao 全文件跑只有与 Chrome 相同的 1 格 hack 分歧格。
- **Chrome 150 逐格 A/B(replica 同页对拍)**:bold.html?3001-last 双双字节级同失败(swc after expected false got true + bold state after expected true got false);insertparagraph.html?4001-5000 五格 F 行字节恒等;**其余 5 个 census variant 补全对拍 = 13/13 ipg census 格两引擎同失败**;insertlinebreak?1001-last 代表格同。**结论:14 真意外格=variant 页载具语义,Chrome 同判,任何「修复」都会偏离真 Chrome(反指纹目标反向)**→ 终止实施,登记在案。148 钉格保留(与 Chrome 同判的诚实可见红)。
- 工具沉淀:/tmp/e196/replica-template.html(harness 复刻:真 tests.js runConformanceTest + 真 data 文件内联 + subsetTest range 门 + execCommand spy);**内联 data 必须 escape `</script`**(insertparagraph/delete/multitest 数据含 script 格字面量,不 escape=整页 JS 静默截断,两个引擎同症状)。

**A 面(forecolor/backcolor qCV)——三根因分型+双修**:
1. **值路径 raw 属性短路(4 格,§e189 第二层判据落实施)**:`CssPropertyName::Color.resolved_value_for_node` 的 font 属性短路对 **in-DOM** font 恒返 raw attr("brown"/"blue")——e190 A 层 typed 存储落地后短路对在档 font 已属多余且有害(computed 经 presentational hint 正确解析)。修复=短路**守卫化下沉**到 `element.style()==None` 分支(detached/未样式化 font 回退属性形态,保 Step-17 load-bearing),在档 font 返 computed(Chrome 形)。FontFamily 臂零触碰(§e189:face 串==CSS 形)。
2. **BackColor override 生命周期(26 格 multitest 全清)**:spec record_current_overrides Step 4 列表(foreName/fontSize/foreColor/hiliteColor)**缺 backColor**——preserving 命令(delete/ipg/justify*/fd/indent/hr/image/ilb)record→动作内 caret 移动触发 `selection_boundaries_changed` 清 override→restore 回不来→after 值 transparent;inserttext 类格(19)同理(inserttext 自带 record/restore 但记录列表同缺)。Chrome 数据(multitest cell[198][199][200]:[backcolor][delete] 后值=cyan、[delete][inserttext] 后值=cyan 且 DOM 含 cyan span)实证 **Chrome 会跨 preserving 命令恢复 backColor override**。修复=record 列表追加 BackColor(fork 偏差,注释锚数据格)。**连带发现:typing-applies-override 机制引擎已存在**(inserttext Step 15 restore 后 value override 经 restore_states_and_values 的 execute 臂包裹插入文本为 cyan span,cell[198] 形 DOM 命中)——26 格 qCV+7 格 compare-innerHTML 一并翻转。
3. **登记未修(独立波域)**:①**delete 后 caret 落点类(12 格,forecolor-delete after-shape)**——innerHTML 与 Chrome 恒等但 caret 落在合并 span 外(computed black),delete.rs 范围恢复/落点域;②**backColor/hiliteColor 双槽类(2 格)**——Chrome 两命令共享一个 background override 槽(数据:[hilitecolor aqua][backcolor tan] 后 hilitecolor 读 tan),我们是两独立槽;③paste 族 8 格 flake 类(§e167/e193/e195 在档)。

**验收(RED=HEAD afdaeac79 干净重建 bao.e196-red;GREEN=双 hunk 重建 bao.e196-green;私有 CARGO_TARGET_DIR=/var/tmp/e196-target;provenance:两轮源 mtime<rlib<binary,构建窗零触碰文件)**:
- `cargo check --profile test-ci -p bao-servo-script` RC=0 零新警告;
- 目标三跑(green_target1/target2/green_full)**45 格三源背书全 PASS**(delete?7001-last 4 值路径格 + multitest 33 [26 qCV+7 innerHTML] + forecolor.html?1001-2000 4 + forwarddelete?6001-7000/?7001-last 4 [后 8 格=Fix1 直连受益 bonus]);
- **全域对拍(red_full vs green_full,855 tests,RC 噪声同形,coverage 855/855+pop 115969 双侧恒等)**:**NEW-RED=0、WORSE=0、uF 回归=0**;uF 51→48(−3=paste.https 方向);uP 1→50。**green 侧 provenance 归因修正(共享树并发,E197 并行波)**:green 二进制(rlib 19:56)编译于 e197 嵌套粘贴根修(d525f719d,document/editing.rs)入树之后——green_full 的 50 uP = **本波 45 格(delete/multitest/forecolor/forwarddelete,execcommand 域,三源背书)** + **e197 的 5 格**(paste-when-nested 4=e197 合同目标格+html-text-copy-paste 1,document/editing.rs paste 域,其 ini 自治 d525f719d 已随波执行);uF −3 与 GONE-uP 1(paste.https/paste-clipboard-change)同样归 e197 域/flake,不计本波。两波 commit 文件域零交集(13a071f7e vs d525f719d diff 实证),45 格翻转为本波两 hunk 的直接域内效果(值路径/override 生命周期,旁波 diff 零触碰)。
- **ini 自治**:45 钉经 wptmanifest AST 通道删除(e193 astdel.py 复用;round-trip 门+非纯 FAIL 形拒触+空壳连带删;4 文件 −123 行全 0 插入形:delete/forecolor/forwarddelete 各 0/12、multitest 0/99);载具 meta 同步(md5 恒等门)后 post-ini 确认跑:**residual uP=0、NEW-RED=0、RC=0**。

**deletion-manifest**:ini 45 条 stale expected:FAIL 删除(双跑+全域三源 PASS 实证逐条 replacement);业务代码零删除。

**教训**:①「普查」必须绑定「判读律代码化」——§e195 教训③文字在档、其普查代码仍倒置了 expected 语义,口径勘误让 173 格 face 塌缩成 148 钉+14 真意外,方案粒度随之重定(普查逻辑必须直接复用 §e185 判读律实现,禁止手写过滤);②「expected 字段缺省=钉消费」与「FAIL 且 expected='PASS'=意外」是同一律的两面,celldiff/普查/验收三处必须同源;③variant 页语义(out-of-range 格跳命令)是 editing 域 A/B 对拍的系统性前提——单页全跑 replica 与真 variant 页行为不同,复刻 harness 必须连 subsetTest 门一起复刻;④fork 自研命令面读 Chrome 数据时,spec 列表缺项(record_current_overrides 无 backColor)=数据可直接实证的 fork 偏差源,「spec 说啥」先于「数据说啥」核一遍再动手;⑤replica 内联数据 `</script` 截断是静默失败(页 JS 整体死,两引擎同症状易误判引擎 hang)。

**证据**:二进制 /var/tmp/e196-target/test-ci/{bao.e196-red,bao.e196-green};raw log /var/tmp/e196-veh/(e196_red_full/green_full/green_target1/green_target2/green_postini);载具 /var/tmp/e196-veh(run_e196.py+meta);build log /tmp/e196-{red,green}-build.log;replica 页+spy+探针 /tmp/e196/(replica-template.html、mkreplica.py、bao-run-replica2.py、cells.py、census173.py、green-probe.py、delset.json、astreport.json);Chrome A/B raw /tmp/e196/chrome-{bold,ipg}-*.out。

### e198(编辑生态末批四小项——anchor 钉④/命名色+单槽③/delete 合并 caret 落点②/塌缩非删除 targetRanges①,56 钉清偿+全域 NEW-RED=0,2026-10-12,REQ-BRW-050)

**四项账**:

- **④ anchor 钉清(1 格)**:§e197 遗留 uP=1(html-text-copy-paste-of-anchor-with-href-in-content-editable.html「Attribute href is missing」钉)。e197 三跑背书(iso_anchor_red_1..3+g_1..2)与本波 e198-red 全域(uP=1)+e198-green5 复证 PASS 逐层在档;wptmanifest AST 通道删钉(纯 FAIL 形),文件随之空删。commit 8628e5084。
- **③ backColor/hiliteColor(12 格)**:**登记根因修正**——§e196 的「两独立槽」系 replica 探针定性,真引擎数据(e198-red multitest aqua/tan 族)显示 execCommand 直接 `return false`(got false),零应用零覆盖:「aqua」/「tan」被 simple-color-string 校验拒收→「#aqua」二次失败。双根修:①值校验改 CSS <color> 语法(parse_color 通道,is_valid_css_color,forecolor 同形面未动=登记相邻观察);②override_slot 单槽化(HiliteColor→BackColor 槽位规范化,读写五点全走共享槽,Chrome 数据 multitest.js:2074/2079 双向互读实证)。翻清 multitest aqua/tan 10 cell+event.html 输入事件 2 格。commit 11e519a22。
- **② delete 块合并 caret 落点(30 格)**:§e196 登记 12 格(foreColor-after)+连带族 18 格。根修=delete_the_selection 块合并路径终态 caret 重定位:Backward 门内塌缩到删除区间记录起点的**最深 inline 壳内表达**(沿非空 inline 子元素下潜,止于 inline 元素层;(p,1)→(font/span,1)),替代 step 34.1 的块界地址——walk 起点 last_equivalent_point 已把等价点类爬到块界,壳内地址才计算壳的有效样式。**绿侧三轮迭代**(green2 全量下潜→8 格 typing-around-link anchor 回归:Chrome 打字落 `<a>foo</a>XY` 外,anchor 即屏障;green3→2 格 fontSize-after:文本端点无 effectively-contained formattable,qCV 解析空串,止于 inline 元素;green4→连带根修 size-less font 元素祖先遍历被空属性串截断=既有 quirk,透明化(Chrome 计算继承值);green5 全清)。typing-around 8 格+forwarddelete fontSize-before 2 格+multitest delete-then-type 13 格顺带翻清。commit 8188dda17。
- **① 塌缩非删除 targetRanges(定性直落+深 walk 终止登记)**:execcommands.rs `target_ranges_for_command` `_` 臂塌缩 caret 回当前选择(塌缩 StaticRange;Chrome input-events getTargetRanges 方法表+test2 `test2-5-test2-5` 形);undo/redo 早退与 delete/forwarddelete 只读 peek 臂零触碰。临时探针(Tools 树+冻结 MANIFEST 条目,e195 先例,收口已删)RED=空数组 2 跑/GREEN=4/4×3 跑 PASS。**深 walk 形(隐形节点移除/块合并)登记终止**:其删除内容在 delete_the_selection 变异过程中方可解析,只读预解析需 walk 算法级重构(违反 stop 条款,不硬做);Chrome 侧该面为键盘驱动 carrier(input-events-get-target-ranges-*.tentative),与 execCommand 面无共同测试格。commit 230d7ffd8。

**回归账**:全域对拍 e198-red vs e198-green5(855 tests,population 116824 双侧恒等):**NEW-RED=0、WORSE=0**,uF 文件 159→158,improved 2;uP 1→56(55 真翻转:④1+③12+②30+字体解析连带 12,逐格结构性归因,零 flake 混入)。paste.https/plaintext-only 2 格绿侧真意外 FAIL=并行负载剪贴板 flake(§e167/e193/e195 在档类):双二进制隔离复跑(ppaste_green2/ppaste_red)12/12 全 PASS 判非本波。ini 自治 56 钉 AST 通道删除(6 文件,anchor 文件空删;round-trip 门+纯 FAIL 形,not_found=0),post-ini 确认跑(6 文件+双探针)RC=0/uP=0/trueFail=0。

**教训**:①replica 探针定性必须经真引擎数据复核后才可当根因——§e196「双槽 2 格」实为「命名色拒收+零应用」,登记措辞与引擎行为差一层;②等价点链方向:last_equivalent_point 是**向上**爬(块界),first_equivalent_point 才是最深壳内——「caret 应在合并点」的直觉地址在 delete_the_selection 的 start 变量里已被归一化抹掉,eprintln 单点探针(HIT off=1 len=2)是打破推演僵局的判据;③qCV 在「塌缩 caret 落文本端点」处解析空串(无 effectively-contained formattable)是引擎解析面事实,壳内元素级地址与 Chrome 观察恒等——carrier 地址选择必须绑定本引擎解析行为逐格验证;④Chrome 的 anchor 落点与 style-壳落点分叉(打字不入链)=typing-around 族格的存在理由,统一「最深壳」规则必回归;⑤flake 判别必须双二进制同页隔离复跑(ppaste 双绿),单侧复跑不构成归属判定。

**证据**:二进制 /var/tmp/e198-target/test-ci/{bao.e198-red,bao.e198-green,green2..green5}(红侧 provenance:源 mtime 21:13:41<libscript rlib<binary 21:20:38,构建窗 scope 文件零触碰);raw log /var/tmp/e198-veh/(red_full/green_full/green2_full/green3_full/green5_full/postini_green5/probe_red2/probes_green5b/delete_green3..5/dcp_green3..5/tal_green3/ppaste_*/paste_*/trp_green2_*);delset198.json+astreport198.json;载具 /var/tmp/e198-veh(run_e198.py,E197_META_ROOT 同构配方,冻结快照 /var/tmp/e198-meta-snap=MANIFEST db107c76 与 e195/e196 同源);deletion-manifest:双探针文件(e198-target-ranges-probe.html/e198-delete-caret-probe.html)+冻结 MANIFEST 两条目已删(Tools 树,raw log 留证)。
