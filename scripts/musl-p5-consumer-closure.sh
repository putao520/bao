#!/usr/bin/env bash
# musl 消费者闭包配方 — 可执行形态(issue #10 musl-closure p5,三段 gate 复现)
#
# 配方真源:docs/musl-p5-consumer-closure.md(§1-8,由 daily-ops musl-p5 recipe-draft 忠实转写)
# 形态裁决(§4):动态 musl 二进制(interpreter /lib/ld-musl-x86_64.so.1,Alpine 原生形态,
#   合法 musl target 产物);纯静态在 Alpine 发布闭包架构性不可达。
# gate 判据(§6,2026-09-10 全 PASS):build exit=0 / link dynamically linked / run exit=0。
#
# 用法:
#   scripts/musl-p5-consumer-closure.sh                  # 复用既有容器 bao-musl-p4(热缓存),缺则全新拉起
#   CONTAINER=my-name scripts/musl-p5-consumer-closure.sh
#
# 零树写纪律(§2):repo 以 :ro 挂 /src,构建目录 /tmp/p5,一律 --locked。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="alpine:3.20"                       # §1
CONTAINER="${CONTAINER:-bao-musl-p4}"     # §1(热缓存容器,保留勿 GC)
TOOLCHAIN="nightly-2026-07-20"            # §1(musl-native;minimal+rustfmt+clippy)
TARGET="x86_64-unknown-linux-musl"
TARGET_US="x86_64_unknown_linux_musl"
CARGO="/usr/local/cargo/bin/cargo"
CONS_DIR="/tmp/p5c"                       # registry-only 消费项目(§6)
TGT_DIR="/tmp/p5"                         # §2(独立防污染)
BIN="$TGT_DIR/$TARGET/release/p5-consumer"

# §1 apk 累积全量(p4 基底 + p5 增量 + p5 终链补;curl 仅为 rustup 安装期取回器)
APK_SET="curl build-base clang zlib-dev libdeflate-dev linux-headers \
python3 bash zip llvm17 clang17-dev cmake perl freetype-dev fontconfig-dev harfbuzz-dev \
glib-dev gstreamer-dev gst-plugins-base-dev gst-plugins-bad-dev mesa-dev \
libarchive-dev c-ares-dev"

# §3 env(容器真实 env 胜 config [env] 注入)+ §4 target 侧动态化(env 通道恰好只达 target 单元)
GCC_INSTALL_DIR="/usr/lib/gcc/x86_64-alpine-linux-musl/13.2.1"  # cc-rs 恒注 triple 与 Alpine 原生不符的修复
BUILD_ENV=(
  -e "CC=clang" -e "CXX=clang++"
  -e "CC_$TARGET_US=clang" -e "CXX_$TARGET_US=clang++"
  -e "AR=ar" -e "AR_$TARGET_US=ar"
  -e "CFLAGS=-g1 --gcc-install-dir=$GCC_INSTALL_DIR"
  -e "CXXFLAGS=-g1 --gcc-install-dir=$GCC_INSTALL_DIR"
  -e "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=clang++"
  -e "RUSTFLAGS=-C target-feature=-crt-static"
  -e "CARGO_TARGET_DIR=$TGT_DIR"
)

cexec() { docker exec -i "${BUILD_ENV[@]}" "$CONTAINER" sh -c "$*"; }

ensure_container() {
  if docker container inspect "$CONTAINER" >/dev/null 2>&1; then
    docker start "$CONTAINER" >/dev/null 2>&1 || true
    echo "[musl-p5] reuse container $CONTAINER (hot cache; §1 — 保留勿 GC)"
    return
  fi
  echo "[musl-p5] creating container $CONTAINER from $IMAGE (§1)"
  # §2:repo 只读挂载 /src,零树写
  docker run -d --name "$CONTAINER" -v "$REPO_ROOT:/src:ro" "$IMAGE" tail -f /dev/null >/dev/null
  docker exec "$CONTAINER" apk add $APK_SET
  # §1:rustup 钉版(musl-native host 工具链;minimal+rustfmt+clippy)
  docker exec "$CONTAINER" sh -c '
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
      --default-toolchain nightly-2026-07-20 --profile minimal -c rustfmt -c clippy'
}

setup_consumer() {
  echo "[musl-p5] setting up registry-only consumer at $CONS_DIR (§6)"
  # p5-consumer:BaoConfig::default()+StealthProfile::firefox_default() 强制真链接
  # (dead-dependency 防护:codegen 必须引用 bao 符号,终链必须解析完整 native 闭包)
  cexec "mkdir -p $CONS_DIR/src $CONS_DIR/.cargo"
  cexec "cat > $CONS_DIR/Cargo.toml" <<'EOF'
[package]
name = "p5-consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
bao-core = "0.1.12"
EOF
  cexec "cat > $CONS_DIR/src/main.rs" <<'EOF'
fn main() {
    // Force real linkage of the bao-core rlib (not a dead dependency edge):
    // construct library-level values so codegen references bao symbols and
    // the final link must resolve the full native closure (mozjs/boringssl/
    // uws/freetype/fontconfig/gstreamer static+dynamic libs).
    let _cfg = bao::BaoConfig::default();
    let _profile = bao::StealthProfile::firefox_default();
    println!("p5-consumer: bao-core musl consumer gate run stage OK");
}
EOF
  # §4:host 单元动态化(bindgen dlopen 断点根治:rustup musl-native 把 build script
  # 编成 static-pie,静态 musl 无 dlopen;RUSTFLAGS env 通道不达 host 单元)
  cexec "cat > $CONS_DIR/.cargo/config.toml" <<'EOF'
[host]
rustflags = ["-C", "target-feature=-crt-static"]
EOF
  # §5:消费面对齐——CSP 钉 0.8.1 对齐 repo 权威 lock(0.8.3 E0004 Destination::Text)
  cexec "cd $CONS_DIR && $CARGO generate-lockfile"
  cexec "cd $CONS_DIR && $CARGO update -p content-security-policy --precise 0.8.1"
}

gate_three_stages() {
  echo "[musl-p5] stage 1/3 build (§4 -Zhost-config -Ztarget-applies-to-host; §2 --locked)"
  cexec "cd $CONS_DIR && $CARGO +$TOOLCHAIN build --release --locked \
    -Zhost-config -Ztarget-applies-to-host --target $TARGET"
  echo "BUILD_EXIT=0"

  echo "[musl-p5] stage 2/3 link (§6 dynamically linked, interpreter /lib/ld-musl-x86_64.so.1)"
  docker exec "$CONTAINER" sh -c "readelf -l $BIN" | grep -q "ld-musl-x86_64.so.1" \
    || { echo "GATE FAIL: not a dynamic musl binary"; exit 1; }
  docker exec "$CONTAINER" sh -c "readelf -d $BIN" | grep NEEDED

  echo "[musl-p5] stage 3/3 run (§6 容器执行)"
  docker exec "$CONTAINER" "$BIN"
  echo "RUN_EXIT=0"

  docker exec "$CONTAINER" sha256sum "$BIN"
  echo "[musl-p5] three-stage gate PASS (§6)"
}

ensure_container
setup_consumer
gate_three_stages
