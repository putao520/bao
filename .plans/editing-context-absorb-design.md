# 立项设计件:上游 EditingContext 基建吸收波(document/editing.rs)

- 日期:2026-10-03
- 评估来源:daily-ops DEFERRED 链(2026-09-17 登记债 → 2026-09-30 ABSORB-DEFERRED 57a307695 → 2026-10-02 DEFERRED 3b07503bc → 本窗口评估)
- 上游基线:d735c4566(2026-10-02,`.claude/upstream-baseline.json`);被 stop 的上游 commit:3b07503bc(#48497)
- 裁定:**大波(立项),非窄吸收;短期维持分叉 + 触发条件,吸收执行建议并入 vendor re-baseline 批或等上游迁移沉降**
- 依据:依赖闭包 12-13 文件(>5,停条件①)+ 上游编辑面 3 周 8 连击结构迁移仍在演化(停条件②)

## 1. 上游波拓扑(8 commits,2026-09-10 → 2026-10-01,全部为 bao 基线祖先)

| commit | 日期 | 内容 | 结构重量 |
|---|---|---|---|
| 8d24a4368 #47964 | 09-10 | editing 代码从 DocumentEventHandler 提取到 **document/editing.rs(新建)** | 高(基建创立) |
| 0d552726d #48068 | 09-14 | editing 相关改名 | 低 |
| d01629900 #48076 | 09-15 | ClipboardEvent 接口友好化 | 低 |
| ca0a16314 #48101 | 09-16 | Document selection copy/select-all 右键菜单项 | 中 |
| fd4d5d0e3 #48324 | 09-24 | key event handling 全面整合(embedder EditingAction/Direction/Motion/ModifySelection 面) | 高 |
| b6a229634 #48165 | 09-28 | 无 layout box 的 EditingContext 不执行 EditingAction | 低(语义守卫) |
| 57a307695 #48404 | 09-29 | password input 禁 copy/cut(copying_enabled/cutting_enabled/pasting_enabled 拆分) | 中 |
| 3b07503bc #48497 | 10-01 | handle_mousedown_event 从 VirtualMethods 移入 EditingContext | 高(结构迁移) |

上游 editing.rs @d735c4566 = 925 行,核心结构:
- `EditingContext` enum(Document | TextControl 双变体)+ `TryFrom<&Node>` 分派
- `TextControlElementEditingContext`
- `SelectionGranularity`(鼠标选区粒度)
- `Document::{editing_context, handle_clipboard_action, fire_clipboard_event, write_content_to_the_clipboard, perform_editing_action, handle_mousedown_event}`
- EditingContext 方法组:event_target / selection_content / has_uncollapsed_selection / has_selectable_text / has_selection_or_cursor / **copying_enabled / cutting_enabled / pasting_enabled** / remove_the_contents_of_the_selection / fire_cut_events / insert_content / fire_paste_events / select_all / perform_editing_action / handle_mousedown_event
- `editing_action_from_keyboard_event`(ShortcutMatcher 全键位→EditingAction 映射:SelectAll/Copy/Cut/Paste/InsertNewline/InsertParagraph/MoveCursor×多粒度)
- 已知未完成残留:`script_triggered=false` 硬编码、Option+Enter TODO —— 上游仍在演化中

## 2. bao 分叉面现状(三方位 diff 表,行数)

| 文件 | bao(6834e4f4) | 上游 pre-wave(8d24a4368^) | 上游 now(d735c4566) | 判定 |
|---|---|---|---|---|
| document/**editing.rs** | **缺失** | — | 925 | **全缺口(基建本体)** |
| document/document_event_handler.rs | 3230 | 3200 | 2946 | bao≈pre-wave(+30 吸收残差);波净变 -254 |
| document/document_embedder_controls.rs | 588 | 588 | 529 | bao=pre-wave 精确 |
| htmlinputelement.rs | 2741 | 2716 | 2685 | bao≈pre-wave |
| htmltextareaelement.rs | 970 | 953 | 900 | bao≈pre-wave |
| text_input.rs | 1209 | 1232 | 835 | bao≈pre-wave;波净变 **-397** |
| text_control.rs | 435 | 421 | 477 | bao≈pre-wave + 1 BAO patch(2026-09-28 window-end trait face,as_element——吸收时须重放) |
| input_type/color_input_type.rs、text_input_widget.rs | pre-wave 形态 | — | 小改 | 随波 |
| node/node.rs | 4665 | 4711 | 4668 | 窗口其他变更已吸收;**mousedown 块仍在**(4570 行 handle_mousedown_event) |
| node/virtualmethods.rs | 467 | 443 | 451 | mousedown 虚方法声明仍在(182 行);波删 16 行 |
| document/mod.rs | 25 | 23 | 24 | 缺 `mod editing;` 一行 |
| **selection.rs** | **949** | 1633 | 1820 | **独立旧分叉**(bao 基底=2026-08-26 "selection-mouse",上游窗口内 +511;EditingContext Document 臂消费 Selection 语义) |
| document/focus.rs | 1275 | 1234 | 1275 | 已吸收(与上游 now 字节一致) |
| clipboard/clipboardevent.rs | 172 | 121 | 166 | 已吸收(+1 bao accessor get_clipboard_data) |
| event/keyboardevent.rs | 366 | 361 | 357 | ≈已吸收 |
| shared/embedder/input_events.rs | — | — | — | **已吸收**(EditingAction/EditingDirection/EditingMotion/ModifySelection/ClipboardAction 全 enum 面与上游 now 一致) |

bao 对波文件的自有锚极少:document_event_handler.rs 仅 1 处注释锚(1992 行)、text_control.rs 1 处 trait-face patch —— **重放成本低,但结构迁移体量大**。

## 3. 语义吸收状态

已吸收(经 8e9a235a wave-20260930 与后续波的适配落点):
- embedder EditingAction 全 enum 面(input_events.rs 与上游 now 等价)
- #48165 layout-box 守卫语义(inline 落点,注释明示"bao's equivalent landing face for EditingContext::perform_editing_action")
- ClipboardEvent 友好化(d01629900)+ focus.rs 键盘迁移 hunk(fd4d5d0e3 局部)
- ContextMenuAction::SelectAll(document_embedder_controls.rs,但走 TextControlElement 旧路)

未吸收(真实缺口):
- **EditingContext 结构本体**(Document|TextControl 统一分派)——925 行 editing.rs 零落地,document/mod.rs 无声明
- EditingAction 分派只覆盖 ClipboardAction 三臂(`handle_editing_action(cx, None, …)` 恒传 None);SelectAll/MoveCursor/InsertNewline/InsertParagraph 的 EditingAction 路径、键盘→动作全映射(editing_action_from_keyboard_event)未接
- password copy/cut 守卫(57a307695 的 copying_enabled/cutting_enabled)未落
- mousedown 处理仍在 VirtualMethods(pre-3b07503bc 形态)
- Document 层选区(非文本控件)copy/select-all 语义缺口

## 4. 吸收路径设计(三选一裁决)

| 路径 | 内容 | 判定 |
|---|---|---|
| A. 窄吸收(≤5 文件) | 仅落 editing.rs + mod.rs + mousedown 迁移 | **不可行**:EditingContext 消费面(document_event_handler/embedder_controls/text_input/两 form_controls/node 双文件)不可切分;闭包 12-13 文件,超停条件① |
| B. **大波吸收(推荐形态)** | 8 commits 依序重放于 d735c4566 基,EditingContext 整体落地 | 成立,但闭包 12-13 文件 + selection.rs 邻接旧分叉(949 vs 1820)需先行定性 |
| C. 维持分叉 + 触发条件 | 保持现 inline 形态,登记跟踪 | **短期姿态成立**(已有 09-17/09-30 债登记);不能永久,触发条件见 §6 |

**合成裁定:B 为目标终态,C 为当下姿态;执行时机取 §6 触发条件(上游沉降或 vendor re-baseline 批并车),不单独立即开波。**

## 5. 大波执行设计(路径 B 展开时)

### 5.1 波次序(依上游 commit 序重放,保持可二分)
1. 8d24a4368 重放:新建 document/editing.rs(925 行@d735c4566 终态一次成型,不逐 commit 中间态)+ mod.rs 声明 + document_event_handler.rs 提取面
2. 0d552726d/d01629900:与 1 合并落地(改名/ClipboardEvent——后者已吸收,零 diff 预期)
3. ca0a16314:embedder_controls 右键菜单切 EditingContext(compile/cut/paste enablement flags)
4. fd4d5d0e3:document_event_handler 键盘整合(focus.rs/keyboardevent/input_events 已吸收,只补 consumer 侧)
5. b6a229634:撤 bao inline 守卫,换 editing.rs 原生守卫(删 1992 注释锚)
6. 57a307695:password 守卫(copying_enabled/cutting_enabled)
7. 3b07503bc:node.rs/virtualmethods.rs mousedown 迁移(删 bao node.rs:4570 块与 virtualmethods.rs:182 声明)

### 5.2 邻接风险(吸收前必须先裁)
- **selection.rs 旧分叉**:bao 949 行(2026-08-26 基)vs 上游 1820 行。EditingContext::Document 臂经 codegen SelectionMethods 消费 Selection;若窗口内 selection.rs 其他变更(如 ca0a16314 触碰)依赖新内部 API,须把 selection.rs re-baseline 并入本波或先行独立波定性。**未裁前不开工**。
- W28(font-ready discard 守卫)/C15(worker 注入)锚不在波文件内,预期零交互;text_control.rs 2026-09-28 trait-face patch(29 行注释处)需重放。
- text_input.rs 净变 -397(613 行 churn 窗口),为最大单文件风险点;bao 形态≈pre-wave,重放映射干净。

### 5.3 验收面
- 新 WPT:input-type-password-copy-and-cut.html(mozilla)、edit-in-textcontrol-immediately-after-hidden.tentative(+.ini)、onselectstart-on-key-in-contenteditable
- servo 测试:components/servo/tests/context_menu.rs(上游新增)
- 单测:tests/unit/script/textinput.rs(随波)
- 回归:`cargo nt -p bao-servo-script` 聚焦 selection/clipboard/editing filterset + 浏览器 e2e 冒泡(键盘编辑/右键菜单/password)

## 6. 触发条件(C→B 转换)

1. **上游沉降信号**:editing.rs 连续 ≥2 周无结构迁移 commit(或连续 2 个 daily-ops 窗口无该文件 ABSORB 项)→ 可开 B 波;
2. **vendor re-baseline 批并车**:9-30 triage 已裁本债"归 vendor re-baseline 批同族"——该批立项时本波并入,不单独开窗;
3. **行为需求触发**:password 泄密面(57a307695)或 Document 选区 copy 缺口成为用户可见缺陷报告时,提级单独立波。

## 7. 台账登记(本评估的即时产物)

差异登记已完成于本文件 §2/§3;`.claude/upstream-baseline.json` servo notes 的 3b07503bc DEFERRED 项由主会话补注指针 → 本文件(评估波无写盘权限变更基线文件)。
