#!/usr/bin/env bash
# build-musl-sysroot.sh — musl sysroot 生产:freetype 静态库 + 头 + freetype2.pc(幂等可重放)
#
# 固化来源:/tmp/musl-freetype-poc/build-sysroot-freetype.sh(E14 PoC 全绿物,2026-09-09;
# 端到端证据见该目录 README.md:musl static-pie 产物 0 GLIBC 字符串,FT 渲染 POC_OK)。
# criterion:REQ-DEPLOY-1 "freetype-sys 等 pkg-config 类交叉阻塞根治(交叉环境变量/sysroot
# 方案,禁 PKG_CONFIG_ALLOW_CROSS 裸开关文档化收尾)"。背景 issue #10。
#
# 用法:
#   scripts/build-musl-sysroot.sh <sysroot-dir>
#
# 环境变量(均可覆盖,默认 = PoC 原样):
#   MUSL_CC     交叉 C 编译器(默认 musl-gcc;musl.cc 全量工具链可用
#               ~/musl-cross/x86_64-linux-musl-cross/bin/x86_64-linux-musl-gcc)
#   MUSL_AR     归档器(默认 ar;ar 只管归档格式,宿主 binutils 即可)
#   FT_SYS_SRC  freetype-sys registry 源目录(默认 glob freetype-sys-0.23.*;
#               只借用其 vendored freetype2/ 源树,零网络下载)
#
# 产物(全部落在 <sysroot-dir> 下,目录本身即 PKG_CONFIG_SYSROOT_DIR 值):
#   <sysroot>/usr/lib/libfreetype.a
#   <sysroot>/usr/include/freetype/
#   <sysroot>/usr/lib/pkgconfig/freetype2.pc   (Version: 26.1.20 = freetype 2.13.2
#                                               的 libtool 版,≥ build.rs 门 24.3.18)
# 编译中间物在 <sysroot>.build/ 下(与 sysroot 分离,可整目录删除重放)。
set -euo pipefail

usage() { echo "usage: $0 <sysroot-dir>" >&2; exit 2; }
[ $# -eq 1 ] || usage
SYSROOT=$1
case $SYSROOT in /*) ;; *) SYSROOT=$PWD/$SYSROOT ;; esac

MUSL_CC=${MUSL_CC:-musl-gcc}
MUSL_AR=${MUSL_AR:-ar}

BUILD_DIR=${SYSROOT}.build
OBJ_DIR=$BUILD_DIR/obj
FT=$BUILD_DIR/freetype2

command -v "$MUSL_CC" >/dev/null || { echo "FATAL: $MUSL_CC not found (set MUSL_CC)" >&2; exit 1; }
command -v "$MUSL_AR" >/dev/null || { echo "FATAL: $MUSL_AR not found (set MUSL_AR)" >&2; exit 1; }

# freetype-sys registry 源:只读借用其 vendored freetype2/,复制后 chmod u+w
if [ -z "${FT_SYS_SRC:-}" ]; then
  FT_SYS_SRC=$(echo ~/.cargo/registry/src/*/freetype-sys-0.23.*)
fi
[ -d "$FT_SYS_SRC/freetype2" ] || { echo "FATAL: no freetype2/ under $FT_SYS_SRC (set FT_SYS_SRC)" >&2; exit 1; }

mkdir -p "$OBJ_DIR" "$SYSROOT/usr/lib/pkgconfig" "$SYSROOT/usr/include"
[ -d "$FT" ] || { cp -r "$FT_SYS_SRC/freetype2" "$FT"; chmod -R u+w "$FT"; }

# 42 模块清单 = freetype-sys 0.23.0 build.rs bundled 清单原样(去 PNG define:
# 默认 ftoption.h 走内置 zlib,免外部 zlib 依赖)
MODULES="autofit/autofit base/ftbase base/ftbbox base/ftbdf base/ftbitmap base/ftcid \
base/ftdebug base/ftfstype base/ftgasp base/ftglyph base/ftgxval base/ftinit base/ftmm \
base/ftotval base/ftpatent base/ftpfr base/ftstroke base/ftsynth base/ftsystem base/fttype1 \
base/ftwinfnt bdf/bdf bzip2/ftbzip2 cache/ftcache cff/cff cid/type1cid gzip/ftgzip lzw/ftlzw \
pcf/pcf pfr/pfr psaux/psaux pshinter/pshinter psnames/psnames raster/raster sdf/sdf svg/svg \
sfnt/sfnt smooth/smooth truetype/truetype type1/type1 type42/type42 winfonts/winfnt"

INCS="-I$FT/include"
for m in $MODULES; do INCS="$INCS -I$FT/src/$(dirname "$m")"; done
INCS=$(echo $INCS | tr ' ' '\n' | awk '!seen[$0]++' | tr '\n' ' ')

for m in $MODULES; do
  obj="$OBJ_DIR/$(echo "$m" | tr '/' '_').o"
  "$MUSL_CC" -O2 -fPIC -DFT2_BUILD_LIBRARY -w $INCS -c "$FT/src/$m.c" -o "$obj"
done

# 先删后建:重放不留陈旧成员(幂等)
rm -f "$SYSROOT/usr/lib/libfreetype.a"
"$MUSL_AR" rcs "$SYSROOT/usr/lib/libfreetype.a" "$OBJ_DIR"/*.o

rm -rf "$SYSROOT/usr/include/freetype"
cp -r "$FT/include/freetype" "$SYSROOT/usr/include/"

cat > "$SYSROOT/usr/lib/pkgconfig/freetype2.pc" <<'PC'
prefix=/usr
exec_prefix=${prefix}
libdir=${prefix}/lib
includedir=${prefix}/include

Name: FreeType 2
URL: https://freetype.org
Description: A free, high-quality, and portable font engine.
Version: 26.1.20
Requires:
Requires.private:
Libs: -L${libdir} -lfreetype
Libs.private: -lm
Cflags: -I${includedir}
PC

echo "sysroot ready: $SYSROOT"
