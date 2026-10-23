# Target 路由波(立项设计,2026-10-02)

## 目标终态
CDP 事件路由从「W49 广播回退」升级为 **per-target 定向**:delegate 事件携带真实 WebView 身份 → broadcast_for_target 精确命中 attach 该 target 的 sessions → 解锁 per-target frameId(v7 路径 B,幻影帧风险消除)。

## 现状底账(主会话读码,2026-10-02)
- `delegate.rs` 57 个 ServoEvent 发射点,至少 5 处 `target_id: "0".to_string()` 占位(v7 波收编 MAIN_FRAME_ID 时保留——targetId 命名空间,刻意未动)
- `ws_registry.rs broadcast_for_target`:attached_sessions 表按 target 过滤;**miss 即 untagged 全广播**(W49 回退)——事件路由的真缺口在此:target 是占位 "0",任何 attach 过真 target 的 session 都 miss → 广播污染其它页的 FrameManager(v7 波证明:per-target frame id 先行切 B 会幻影帧替换错页主 frame,故 A/B 必须同批)
- servo `WebViewDelegate` trait 无 frame id 但 **notify_* 回调有 webview 参数**(`notify_url_changed(&self, _webview, url)`——v7 波已核)——真实身份可从回调参数取

## 波形(三件同批)
1. **身份贯通**:delegate 的 notify_* 回调从 webview 参数提取真实 WebViewId → ServoEvent.target_id 换真值(5 占位点 + 全 57 点审计)
2. **路由收紧**:broadcast_for_target 的 miss 分支从「untagged 全广播」改「drop + debug log」(Chrome 语义:无 session 订阅即不投)——**必须与 ① 同 commit**,否则占位 target 的旧事件全丢
3. **frameId 切 B**(v7 预授权):MAIN_FRAME_ID → per-target 主 frame id(事件发射点派生);三重钉(mini 断言/ws-command-face 断言/cdp_handler 源断言)同步升

## 验收 oracle
- 双真客户端锚(puppeteer/playwright)绿;**多页并发探针**(两 page 各 attach,导航 A 页,B 页 FrameManager 不收 A 的 frameNavigated——幻影帧回归的机械判据)
- WPT 40 全量不回归;soak 一轮(cdp 面事件量变,ratchet 剖面对比)

## 风险底账
- 事件双发(synth/real)e14 已裁维持现状——本波只动 real 路径的 target 定向,不触碰 synth 面
- W49 回退是为 flat-session 事件可达立的(历史因果在 ws_registry 注释);收紧前须确认双锚不依赖回退路径(探针:attach 后开 debug log 观察 miss)
