# gsc-spec 插件缺陷档案(五缺陷 consolidated)

- 日期:2026-08-22 | 档案性质:维护方(putao520/gsc)交付件,源码级 consolidated
- 版本锚:源码 `~/.claude/plugins/cache/gsc-spec/gsc-spec/6.8.1582`(v6.8.1582,commit 19e9372b)
- 证据来源:probe-link-canonical 侦察(links 族四缺陷,file:line 源码级)+ spec-debt-wave/#34 处置实证(bug-create 路由缺陷,含 2026-08-22 半写入污染回滚事件)
- 关联档:`docs/audits/2026-08-22-taskstore-bce.md`(spec_write 工具缺陷记录行即缺陷 5 首次登记)

## 缺陷 1:appendReqXref 空目标文件 → 字面 broken link

- **位置**:`mcp/src/spec/crud/html-gen.mjs:419-423`(调 L411 `createXrefElement` 时 targetFile 传 `''`);调用方 `crud/test.mjs:147`、`crud/register.mjs:1167`
- **机制**:xref 生成时 targetFile 恒为空串,产出字面 `href=".html#req-xxx-n"`;从不解析目标 REQ 的实际归属文件
- **触发条件**:任何跨文件 REQ xref 生成路径(TEST 注册/REQ 注册时挂链)
- **仓内影响**:66 条 broken links(`spec_govern check` SPEC LINKS 段 66 error(s))的直接来源
- **仓内规避**:不自动修(等修复);登记在案以 distinguishing 存量格式债
- **修复建议**:createXrefElement 前先经 idMap/registry 解析目标元素归属文件填入 targetFile;或生成后置校验 href 不得以 `.html#` 裸前缀开头

## 缺陷 2:idMap 键大小写双态 → 合法 fragment 判 broken

- **位置**:`build-index.mjs:131`(`item.id || item.htmlId`);`validate/links.mjs:73`(fragment 精确匹配)
- **机制**:REQ 在部分文件以大写 `data-req` 值注册、02-SYSTEM 的 section 额外以小写 html id 注册 → idMap 同一编号存在大小写双键;links 校验 fragment 精确匹配零归一化 → 指向合法小写 fragment 的链接被误判 broken
- **触发条件**:目标 REQ 位于仅大写注册的文件(10-REQUIREMENTS 注册大写/02-SYSTEM 双注册)
- **仓内影响**:SPEC LINKS 段部分 error 为假阳性(与缺陷 1 真坏链混叠,需甄别)
- **仓内规避**:读 check 报告时对 `.html#req-xxx-n` 小写形态逐条人工甄别真坏/假阳性
- **修复建议**:links 校验前对 fragment 与 idMap 键统一 lowercase 归一化(与缺陷 4 的 canonical 归一合并修复)

## 缺陷 3:removeDanglingXrefs 破坏性删除 → 数据丢失放大器

- **位置**:`fix/fix-links.mjs:161-177`
- **机制**:fixLinks 修不动的 xref 直接删除 `<a>` 节点——无 dry-run、无备份、无删除清单输出
- **触发条件**:任何 `spec_govern check fixLinks=true` 执行路径
- **仓内影响**:与缺陷 2 组合 = 假阳性链接也被判 dangling 而删除(本仓推演实测会误删 60 节点 + 6 条改错向);数据丢失不可逆
- **仓内规避**:**禁跑 fixLinks 直到缺陷 1-3 修复**(硬规则);如必须修链,先 git commit 快照再人工定点
- **修复建议**:removeDanglingXrefs 改为默认 dry-run 报清单、显式确认才执行;删除前校验目标确属缺陷 1 真坏链(排除大小写假阳性)

## 缺陷 4:fixAnchorHref 大小写不对称 → canonical 漂移

- **位置**:`fix/fix-links.mjs:118-125`
- **机制**:查找键 lowercase 化、写回值保留原大小写 → 修复产出的 canonical 形态随目标注册路径漂移(10-REQ 侧大写/02-SYS 侧小写),同一目标两种 canonical 并存
- **触发条件**:fixLinks 对跨文件 REQ 链执行 anchor 修复
- **仓内影响**:FORMATTING AUDIT 的 CONFLICTS/GAPS 段部分条目源于双形态并存(如 `REQ-BRW-4` vs `REQ-BRW-004`、TEST 双形态 22 条 DUPLICATES)
- **仓内规避**:同缺陷 3,禁跑 fixLinks
- **修复建议**:canonical 形态单一化(建议以 registry 数字 ID 形态为唯一 canonical),fixAnchorHref 写回与校验共用同一归一化函数

## 缺陷 5:spec_write bug-create 默认路由 → 本体静默丢弃

- **位置**:spec_write bug 元素 create 的默认宿主路由(默认路由到 00-INDEX——bug 元素非法宿主)
- **机制**:create 默认路由 file=00-INDEX → 返回 ok=true 但元素本体静默丢弃,仅残留 `.id-registry.json` 编号分配 + 00-INDEX 统计副作用(半写入态);用户声明服务已好但 2026-08-22 两会话确定性复现(#34 首试 + 前序半写入污染事件)
- **触发条件**:不带显式 `file=` 参数的 bug create
- **仓内影响**:曾产生 370 编号污染 + 回滚处置(见 taskstore-bce 档);#34 retry 显式 `file=11-TESTING` 成功绕行
- **仓内规避**:**bug-create 必须显式 file= 合法宿主**(本仓先例:11-TESTING);create 后强制 spec_read 复读本体验身
- **修复建议**:bug 元素默认路由改为合法宿主文件(或按类型路由表);create 响应增加「写入文件内元素计数 +1」自校验,不匹配即返回 failed 而非 ok=true

## 仓内硬规则(直到 1-3 修复)

1. 禁跑 `spec_govern fixLinks`(含任何 destructive fix 开关)
2. bug-create 一律显式 `file=` + 复读验身
3. check 报告 links 段逐条人工甄别(缺陷 1 真坏 / 缺陷 2 假阳性)
