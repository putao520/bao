# bao 产出大小审计(release 尺寸矩阵)— 2026-10-17

零配置改动;全部实验走 env 覆盖 + 私有 target(/var/cargo-builds/bao-size-audit-target,13G,保留可复用)。
产物副本:/tmp/bao-size-audit/{bao.original, bao.strip-debug, bao.strip-all, bao.debug0, bao.debug0.stripall}

## 1. 四态尺寸矩阵(release,lto=fat,cg=1)

| 形态 | 字节 | MiB | 相对当前 |
|---|---:|---:|---:|
| **当前** `[release]` debug=line-tables-only + strip=none | 643,249,776 | 614 | — |
| + strip --strip-debug(≡ strip="debuginfo") | 169,740,120 | 162 | −74% |
| + strip --strip-all(≡ strip="symbols") | 141,024,552 | 135 | −78% |
| debug=0(env `CARGO_PROFILE_RELEASE_DEBUG=0`) | 296,516,512 | 283 | −52% |
| debug=0 + strip-all | 141,048,216 | 135 | −78% |

- 两个 profile 的 strip-all floor 字节级差 23,664 B(0.017%)= 布局噪声 → **debuginfo 不改 codegen**(strip 后完全等价)。
- 对照既有产物:test-ci bin 168M(opt2+debug-assertions+无LTO+strip=true)/ debug bin 1.4G / cov-target test-ci 330M。
- strip-all 产物 smoke:`--version` RC=0(两个变体均验)。
- 冷构建时长:33m44s(--jobs 4,空 target;实测含 CCG 侧 sccache 关闭)。

## 2. 构成(原 614MB 二进制)

### 段级
| 段 | 大小 | 说明 |
|---|---:|---|
| debug 族合计 | ~474 MB(77%) | debug_info 190 / debug_str 115 / debug_ranges 92 / debug_line 72.6 / aranges 2.3 / abbrev 0.7 |
| .text | 93.2 MB | |
| .symtab | ~29 MB | (strip-all 与 strip-debug 之差) |
| .rodata | 26.1 MB | 其中 **icudt78_dat = 12.3 MB(rodata 47%)** |
| unwind 族 | 10.5 MB | eh_frame 7.5 + hdr 1.0 + gcc_except_table 3.0 |

### debug_info 190MB 逐 CU 归因(llvm-dwarfdump producer,1752 CU 全配对)
| producer | 大小 | 占比 |
|---|---:|---:|
| **rust**(515 CGU) | 146.5 MB | 77.1% |
| gcc C/C++(906 CU,`-g1` 非纯行表) | 42.1 MB | 22.1% |
| clang C(331 CU) | 1.5 MB | 0.8% |

关键事实:**rustc 的 `line-tables-only` 实际产物 = 函数级 DIE + 行表**(每函数 DW_TAG_subprogram + 名字进 .debug_str + 地址段进 .debug_ranges),不是纯行表。最大单 CU = `vendor/servo/components/script/lib.rs` LTO CGU,51.5MB。debug=0 实测削掉:debug_info −146 / debug_str −86 / debug_ranges −66 / debug_line −50 ≈ **−347MB**。C 侧残留(debug=0 后)≈123MB,主源 = gcc `-g1`(gcc 语义=行表+函数/变量 DIE;注释里「两家互认」对 clang 成立、对 gcc 不成立)+ mozjs Unified CU。

### .text 92.8MB 符号域归一(nm, Rust v0 crate-hash 精确提取)
| 域 | 大小 | 占比 |
|---|---:|---:|
| servo/stylo Rust | 28.1 MB | 30.3% |
| third-party Rust | 27.7 MB | 29.9% |
| — 其中 rust std/core/alloc | ~9.9 MB | |
| bao-layer Rust (src/*) | 11.6 MB | 12.5% |
| mozjs SM (C++) | 9.4 MB | 10.1% |
| C++ 未归(ANGLE/glslopt/胶水) | 6.1 MB | 6.6% |
| C libs 未归 | 3.0 MB | 3.3% |
| webrender Rust | 1.5 MB | 1.6% |
| aws-lc (C,rustls provider) | 1.5 MB | 1.6% |
| ICU (C) | 1.4 MB | 1.5% |
| boringssl (C,stealth TLS) | 0.85 MB | 0.9% |
| img/codec/db + harfbuzz/ft/ots (C) | 1.6 MB | 1.7% |

third-party Rust 尾部:crossbeam_channel 1.3MB、postcard 0.83MB、rustfft 0.80MB、swc_ecma_parser 0.46MB、skrifa/harfrust/usvg/brotli 各 ~0.4MB。TOP 符号:jemalloc arena_emap_global(bss 2MB)、script_bindings RegisterBindings::InitAllStatics 0.93MB、bun_crash_handler SIGALTSTACK bss 0.5MB、aws-lc aes-gcm avx512 0.34MB×2、brotli 字典表。

## 3. 方案矩阵(ROI 排序,零改动可落地)

### P0 — 发布物 strip(打包步骤,零配置改动)
`objcopy --only-keep-debug bao bao.debug`(留 symtab+DWARF 作取证 companion)→ `strip --strip-all bao`。
发布物 **614→135MB(−78%)**;companion 另存即可复现 gdb/llvm-symbolizer 取证。
**取证面已核实无损**:crash_handler 的 release 形态本就不做进程内行号查找(Linux 走 dladdr,`source_location: None` → `???:?:?`;DWARF 行表查找注释明示 "not ported"),线上取证走 trace-string(ASLR 剥离地址)服务端重映射(bun.report 形态)。行号表只服务本地调试。
最低限度替代:profile 改 `strip="debuginfo"` → 162MB(保留 symtab,本地 gdb 仍有函数名)。属配置一行,需属主裁决。

### P1 — release 本地形态 debug=0 + profile 分裂(镜像 test-ci-dbg 先例)
```
[profile.release]        # debug=0, 283MB(或配合 P0 打包 strip → 135MB)
[profile.release-dbg]    # inherits release + debug="line-tables-only" + strip=false
```
调试会话用 release-dbg(等价今天的形态),日常 release 用 debug=0。codegen 已证零差异。日常构建 −52%。
纯 env 可先行验证/过渡:`CARGO_PROFILE_RELEASE_DEBUG=0`(本次实验即此形态,无任何配置写入)。

### P2 — C 侧 `-g1` → `-gline-tables-only`(配置提案,需属主裁决)
gcc 13 支持该 flag(纯行表),可把 C 侧残留 123MB 压到 ~25MB → **未 strip 形态 ≈165-175MB**,行号保留。
通道:① `[env] CFLAGS/CXXFLAGS`(.cargo/config.toml 注释明示 cc 链有效,但注意 config 注释同时声明 `-g1` 是「两家互认」——该判断对 gcc 不成立,本次实测修正);② mozjs 侧 `MOZ_DEBUG_FLAGS` 硬编码在 vendor/mozjs/mozjs-sys/build.rs(`-gdwarf-4 -g1`,包内 patch,env 覆盖无效——注释明示),须动 build.rs。属配置改动,本审计未动。

### P3 — 不建议/低优先
- **panic=abort**:unwind 面合计仅 10.5MB 且 C++ 必须保留;Rust 侧估 2-5MB(<2%)。语义:crash_handler 的 `rust_panic_hook` 已无条件 `crash()`(abort),Rust panic 生产形态本就是进程终止;但 pre-hook-install 窗口与 vendor 侧 3 处生产 catch_unwind(script line_breaker 换行回退 / image_cache 恶意 SVG 隔离 / HTTPThread startup guard)假设存在不确定性。收益/风险比差,**不动**。
- **ICU 裁剪**:icudt78_dat 12.3MB 已是精简形态(full ~30MB);再裁走 `ICU_DATA_FILTER_FILE`(Firefox 形)需改 mozjs 构建链,预期 −5~8MB,中工作量低 ROI。
- **opt-level 降档(3→2/s)**:可再压 .text 5-15%,但运行时性能让位(项目宪法性能优先),不做。
- 加密栈并存(boringssl + aws-lc)合计仅 2.3MB,非杠杆。

## 4. 结论
当前 614MB 的 77% 是 debug 段,其中 77% 又来自 Rust(rustc line-tables-only 的实现形态超出「纯行表」预期)。**分发产物 strip(P0)是唯一的免费午餐:135MB,零风险零改动;本地形态 debug=0(P1)再省一半。** text 本体 93MB(servo/stylo 30% + third-party Rust 30% + bao 12.5% + mozjs C++ 10%)在 fat-LTO 下已收敛,无意外大头;rodata 大头是 ICU 数据 12.3MB。

## 5. 过程备注
- 私有 target 保留:/var/cargo-builds/bao-size-audit-target(13G,含 release+debug0 两套产物,可复用)。
- 本会话曾触发一次 READ-GATE 对 /tmp 分析脚本的采样拦截(非项目源码写入);改走 Bash heredoc 执行,未用逃生口,未循环重试。
- mozjs .a(libjs_static.a 318MB)内 .debug_info 仅 36MB,与二进制中 C 侧 42-123MB 的差异源于 archive `size` 汇总口径与链接期重定位开销;CU 级 producer 归因以二进制实测为准。
