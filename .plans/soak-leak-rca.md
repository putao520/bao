# soak malloc 泄漏 RCA(heaptrack 归因 · W6 · 2026-09-29)

> 复现车辆:`bench-harness soak --duration-mins 2`(xvfb-run;数据: 页 churn,794-805 cycles/run)。
> 工具:heaptrack 1.5.0(apt 安装);数据 `/tmp/w6_ht.zst`(30 MB zst,本机留存)。
> A/B knob:`BAO_SOAK_URL=<url>`(soak_bench 新增;默认未设=零行为变化)。

## ① A/B 斜率两数字(2 min/窗)
- **B 基线(data: 页,默认)**:794 cycles,`vm_rss_slope_over_soak` = **1498.8 KiB/min**,RSS after_close 464 MiB,forced-GC pre 485 MiB。
- **A(http+fetch 页)**:805 cycles,`vm_rss_slope_over_soak` = **1649.1 KiB/min**,RSS after_close 448 MiB,forced-GC pre 504 MiB。
- **判定:同泄**(差 +10% 在 run 方差内;fetch 页只多 ~10%,非数量级差)。→ 网络栈侧(uv/uws fetch 生命周期,嫌疑①)**排除为主因**;泄漏主体=DOM/pipeline 侧(每 cycle 页面生命周期本身)。

## ② 归因 top 表(heaptrack 存活字节,run 尾泄漏总量 8.42M / peak 117.34M / peak RSS 479.15M)
| # | 存活字节 | 调用数 | 分配点 | 归属 |
|---|---|---|---|---|
| 1 | **2.51M** | 518,172 | `js::ScriptSource` ← `CompilationInput::initScriptSource`(Stencil.cpp:1454,pod_arena_malloc) | SM per-eval ScriptSource(小件海量) |
| 2 | **2.43M** | 5,152 | `StencilXDR::codeSharedData`(StencilXdr.cpp:355)← DecodeStencil ← **`bao_engine::xdr_cache::load`(xdr_cache.rs:365)← `stencil_cache::evaluate_script_cached`(:263)← `bao_stealth::engine_props::inject_js_hooks`(:1488)← install_web_apis ← create_node_realm_native** | **XDR cache 链 SharedData(~484 B/realm-eval)** |
| 3 | 82.43K | 5,152 | 同 #2 链(XDR 次级分配) | 同 #2 |
| 4 | 1.10M | 31,060 | libglib-2.0 | glib |
| 5 | 234.04K | 4,236 | libglib + **libgstreamer-1.0** | gstreamer init |
| 6 | 188.05K | 46 | libglib + libgstreamer | gstreamer init(一次性) |
| 7 | 184.70K | 2,886 | libgstreamer | gstreamer |
| 8 | 183.93K | 16 | libglib + libgstreamer | gstreamer init(一次性) |
| 9 | 991.70K | 6,537 | (杂项面) | — |
| 10 | 69.26K | 2,886 | libglib + libgstreamer | gstreamer |
| — | 其余(425K/361K/82K/43K…) | — | 杂项 | — |

**环境注记**:peak 分节的最大 malloc 消费者=**libgallium-25.2.8(Mesa/llvmpipe 软渲染)41.85M+52.07M(未解析库)**——xvfb 软渲染环境的渲染堆,非 bao 逻辑面。

## ③ 根因结论(三选一裁定的证据链)
1. **代码缺陷(修复合同草案)**——malloc 侧泄漏主体=bao face:
   - **XDR cache 链(#2/#3,合计 2.51M/2min)**:每 realm boot 经 inject_js_hooks 走 `evaluate_script_cached` 的 xdr 磁盘命中分支,`DecodeStencil` 产出的 stencil/SharedData(~484 B×6.4 hook evals/realm)在实例化后未释放。初判:stencil_cache 的 refcount 纪律(AddRef/Release/insert 覆写/LRU 驱逐/reset)源码面平衡——残余嫌疑在 SM SharedData 引用链(DecodeStencil 产物的 SharedData 引用随 Instantiate 后未断)或 xdr_cache::load 返回面。**修复合同**:bao_engine 域,以 SM ShareDECODER refcount 审计(Instantiate 后 `stencil->sharedData` 引用断言)+ 泄漏回归测试(N realm boots 后 codeSharedData 存活字节归零)。
   - **ScriptSource 海量小件(#1,2.51M/518K 调用)**:每 eval 的 ScriptSource 结构残留——与 #2 同族(CompilationInput 生命周期),随同一修复波审计。
   - 72h 可行性:线性部 #2/#3 ≈ 2.5 MiB/min → 72h ≈ **10.4 GiB,不可行 unless 修复**。
2. **分配器/环境语义(量化上界)**:
   - glib/gstreamer init 泄漏:一次性(#6/#8,16-46 调用)+ 常驻小流(#5/#7/…每 cycle 次级)——上界 ≈ 1-2 MiB 总量(有界)✓ 72h 可行。
   - Mesa/llvmpipe 软渲染堆(peak 41.85M+52.07M 主消费):xvfb 环境语义,非 bao 面;生产 GPU 路径不适用。
   - **SM GC chunk 驻留(W5 STOP 项,非 malloc 非 heaptrack 可见)**:RSS 主段(~2.1 MB/realm 线性)——与 malloc 侧独立,归 W5 裁决。72h:30K cycles × 2.1 MB ≈ **60+ GiB 线性,unless chunk 池裁决放行复用**。
3. **需更长跑分辨的项**:无——本窗数据已可分辨全部 top 嫌疑(嫌疑④ glibc arena:threads 恒 88+malloc 残留有界,非主因)。

## ④ 72h soak 入场前置判定
- **先决**:①修复 XDR cache 链泄漏(本报告修复合同)→ malloc 侧余量(glib 有界+Mesa 环境)可行;
- **W5 chunk 驻留裁决独立前置**(RSS 主段,~60 GiB/72h 线性 unless 池裁决)——两项同过方可入场。

## 复现命令
```bash
# B 基线(默认 data: churn)
xvfb-run -a bench-harness soak --duration-mins 2 --out /tmp/soak_b.json
# A fetch 侧(BAO_SOAK_URL knob,本波新增;外部静态服务器自备)
BAO_SOAK_URL=http://127.0.0.1:8731/page.html xvfb-run -a bench-harness soak --duration-mins 2 --out /tmp/soak_a.json
# 归因
xvfb-run -a heaptrack -o /tmp/w6_ht bench-harness soak --duration-mins 2 --out /tmp/soak_ht.json
heaptrack_print -f /tmp/w6_ht.zst -p 0 -a 0 -T 0 -l 1 -n 40
```


## W7 修复尝试记录(2026-09-29)——**修复无效,已回退;根因上移 STOP**
- **尝试**:stencil-cache drain 桥(RED-1 同点 exit-processing 调用 + worker clear_js_runtime 同调,bao_engine `drain_thread_cache` 释放 cache-held stencils)。
- **结果**:heaptrack 复测 2min 窗 → codeSharedData 泄漏 **3.17M/6736 调用(每 cycle 反而升)**——drain 只对真正退出的线程生效;churn 的 ScriptThread 是**池化复用不退出**(threads 恒 88),exit-processing 根本不触达。已按审计诚实原则回退全部接线(vendor 桥/install/worker 调用/stencil_cache drain fn)。
- **根因统一(W5+W6 同一缺陷的两个投影)**:死 realm 的 zone 从不 GC——
  - W5 投影:zone chunk 内存驻留(~2.1 MB/realm 线性,GC 后不回落,decommitted=0);
  - W6 投影:死 realm 内实例化的 JSScript 持 XDR SharedData 引用(malloc 侧可见部分,~484 B×6.4 hook evals/realm)+ ScriptSource 小件。
  - W5 zone-eval 的 zone_count 回落(102→2)只证明 zone **结构体**在 SM 内部重整;**chunk 字节与其中 malloc 对象不还**。
- **处置(停止条款)**:修复需 SM realm-discard/zone-GC 面(每页 close 后对死 realm 触发 zone GC/discard,或 SM chunk 池上限)——vendor/SM 域 → **上游 issue 候选 + known-limitation 记录**(本文件+gc-leak-ledger W5 段)。72h 入场前置保持:该缺陷修复前,线性驻留 ~22 KiB/cycle 不可入场。
- **W12-B 追记(2026-09-29,console SIGSEGV 双红,非本 RCA 主体但同域)**:7045fa57 把 console.rs build_message 的 caller 探测从 `describe_scripted_caller_safe` 换成裸 `describe_scripted_caller`——opt 档 FrameIter::settleOnActivation SIGSEGV。裁定=patch-replay 丢失(safe 包装仍在 rust.rs,只是调用点被置换),恢复 safe 调用点+三件登记,battery 双红转绿(test-ci 档)。
- **W6 归因表其余结论不变**:glib/gstreamer init 一次性有界 ✓;Mesa 软渲染堆=xvfb 环境语义;glibc arena 非主因。
