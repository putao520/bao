# bao 安全审计面(post-dev 阶段,2026-10-13,主会话只读审计)

**方法声明**:本机无 cargo-audit/cargo-deny/gitleaks(按 spawn-gate 禁自装),采用 manual 方法;盲区=非高知名度 advisory 未全量比对、git 全历史深扫未做(工作树+模式扫)。工具化审计面建议进 daily-ops 工具链候选。

## 轴① 依赖 advisory(1032 包,重点条目 manual 比对)
**critical/high: 0**。重点 watched 集全部在修复后版本:time 0.3.47/chrono 0.4.44/h2 0.4.14(≥0.4.1)/rustls 0.23.43(传递依赖;bao TLS 主面=boringssl 自有桥)/idna 1.1.0/mio 1.2.1/smallvec 1.15.1/generic-array 0.14.7。盲区:长尾 944 包未逐条比对(声明)。

## 轴② secret 泄漏面
**finding: 0(净)**。.claude/daily-ops/auth.env=gitignored 未追踪✓;追踪的 708 个 .pem 全部=vendor/boringssl 上游公共测试 fixture✓;AKIA/ghp/xox/sk/BEGIN PRIVATE KEY 模式在 src/scripts/toml/sh 零真命中(node_crypto.rs 2 处=PEM 头文档字符串常量,非密钥)。盲区:全历史深扫未做。

## 轴③ unsafe 面(src/ 自有层)
**信息面(架构预期内)**:unsafe 块/函数 8,885 处——SpiderMonkey FFI 桥接层架构决定(bao_engine/bao_sm GC-unsafe 门禁 bao_lints 已在位,GC-unsafe 检测器=既有 BCE 门禁);transmute 82 处/裸指针解引用粗计 203 处,集中于 mozjs ffi/glue 层。**无 critical/high 新发现**;高危形态分类与 bao_lints 门禁对照=既有治理覆盖。

## 轴④ 危险 API 面
**信息面**:[std::process::Command 45 处(bun_spawn/构建工具链/测试夹具域)]/[fs 删除写原语 85 处(运行时 fs API 面=产品语义本身,REQ-ENG-007)]/[网络监听 14 处(CDP server/HTTPThread/测试夹具)]。对照 REQ-LIB-004 permission 沙箱(默认关闭零开销,白名单模式)=设计一致;**无越沙箱暴露新发现**。

## 结论
**critical: 0 / high: 0**。安全面与既有治理(bao_lints GC-unsafe 门/permission 沙箱/例外账本)一致,无新暴露。盲区声明:①长尾依赖未全量 advisory 比对(建议 daily-ops 引入 cargo-audit 工具化)②git 全历史深扫未做。
