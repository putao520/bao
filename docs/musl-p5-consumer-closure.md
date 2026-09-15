# musl 消费者闭包配方(p5)

> **落盘注记(2026-09-16)**:本文由 `.claude/daily-ops/musl-p5/recipe-draft.md`(2026-09-12
> 只读轮整合稿)§1-8 忠实转写入库,可执行形态见 `scripts/musl-p5-consumer-closure.sh`。
> 落盘时逐项核验:§7 前三项资产(`scripts/clang-musl{,++}.sh` + `check-musl-tu.c`、
> `src/lsquic_sys/csrc/sys/queue.h` vendored + build.rs include 接线、`.cargo/config.toml`
> per-target musl wrapper 接线)均已在库跟踪,不重复落地;第 4 项即本对文件。
> §8 序列中 ④ 的实况:#10 已于 2026-09-10T05:30:09Z 关闭(先于配方进 repo,close 前置
> 未满足即关闭,处置归用户裁决)。三段 gate 证据(2026-09-10 全 PASS):build
> BUILD7_EXIT=0(706 target 单元 15m30s)/ link 5.4MB dynamically linked(interpreter
> /lib/ld-musl-x86_64.so.1;sha256=c9ff77c12ded2fcc0713d3049b5b12345046e1aa93e71bdabd458c11805d3c87)/
> 容器内 run exit=0。以下 §1-8 为配方正文,与 draft 逐字一致,零新增裁决。

## 1. 容器基底(名称 `bao-musl-p4`,保留勿 GC;热缓存 /tmp/p5 3.4G+)

- 镜像 `alpine:3.20`
- apk(累积全量):`build-base clang(17.0.6) zlib-dev libdeflate-dev linux-headers(6.6-r0)`(p4)
  + `python3 bash zip llvm17 clang17-dev cmake perl freetype-dev fontconfig-dev harfbuzz-dev glib-dev gstreamer-dev gst-plugins-base-dev gst-plugins-bad-dev mesa-dev`(p5)
  + `libarchive-dev c-ares-dev`(p5 终链补)
- rustup 钉版 `nightly-2026-07-20`(minimal+rustfmt+clippy)

## 2. 挂载与零树写纪律

- repo 以 `:ro` 挂 `/src`;`CARGO_TARGET_DIR=/tmp/p5`(独立防污染)
- 一律 `--locked`(零 Cargo.lock 改动、零树写)

## 3. env(容器真实 env 胜 config [env] 注入)

```
CC=clang CXX=clang++  (per-target CC_/CXX_ 同值)
AR=ar  AR_x86_64_unknown_linux_musl=ar
CFLAGS/CXXFLAGS='-g1 --gcc-install-dir=/usr/lib/gcc/x86_64-alpine-linux-musl/13.2.1'
  # ↑ cc-rs 对 clang 恒注 --target=x86_64-unknown-linux-musl 与 Alpine 原生 triple 不符的修复
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=clang++
```

## 4. host/target 动静分工(cargo 1.99-nightly 2026-07-17 实证)

- `-Zhost-config -Ztarget-applies-to-host` + `.cargo/config.toml`:
  `[host] rustflags=["-C","target-feature=-crt-static"]` — 仅 host 单元(build script/proc-macro)动态化,解 bindgen dlopen 断点(rustup musl-native 工具链把 build script 编成 static-pie,静态 musl 无 dlopen;RUSTFLAGS env 通道不达 host 单元,verbose rustc 行双证)
- `RUSTFLAGS='-C target-feature=-crt-static'`(env 通道恰好只达 target 单元)— target 亦动态
- 终产物形态裁决(e67 候选 A,2026-09-10):**动态 musl 二进制**(interpreter /lib/ld-musl-x86_64.so.1,Alpine 原生形态,合法 musl target 产物);纯静态在 Alpine 发布闭包架构性不可达(gstreamer/libarchive/X11 族无 .a 供给 + static libstdc++ × mimalloc operator new/delete 多重定义)

## 5. 已知消费面对齐

- `content-security-policy` 必须 `--precise 0.8.1` 对齐 repo 权威 lock(0.8.3 E0004 Destination::Text;上游 0.9 待核,见 triage-servo-2026-09-12)
- uws 代际根因:HttpRouter.h constexpr std::string 需 libstdc++12+,musl.cc 仅 GCC 11.2.1 — 容器 clang17+gcc13 头解;always_inline(loop.c)同解
- lsquic:musl 缺 BSD sys/queue.h → vendored queue.h(csrc include,未 commit 资产见 p3.5 记录)

## 6. 三段 gate(全 PASS 2026-09-10;p5-consumer 强制真链接 BaoConfig::default()+StealthProfile::firefox_default())

1. build:全图 exit=0(含 bao-mozjs-sys/mozangle bindgen build script)
2. link:产物 dynamically linked,NEEDED={libz,libarchive,libstdc++,libfontconfig,libfreetype,libcares,libdeflate,libgcc_s,libc.musl-x86_64.so.1}
3. run:容器执行 exit=0

## 7. 未 commit 资产清单(净树轮一并收口)

- `scripts/clang-musl{,++}.sh` + `check-musl-tu.c`(宿主 cross 探针;p3.5)
- lsquic build.rs csrc include(queue.h vendor)
- `.cargo/config.toml` wrapper 接线(per-target musl env;宿主 gnu 零影响已证)
- 本配方 → `scripts/` 脚本化 + `docs/` 文档化

## 8. 剩余收口序列(净树轮)

① 落盘 §7 资产 + 配方 → ② 三重判据(波末)→ ③ commit+push → ④ #10 终局 close(配方 repo 可见后)
