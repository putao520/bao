#!/usr/bin/env bash
# wave-gate-20260921.sh — mozjs 升级波(2026-09-21)波末门集(§10.1 静态化)
# 五节各自独立退出码;支持单节调用(--only N);由 C 在波末执行,本脚本作者不执行。
#
# 用法:  bash /tmp/wave-gate-20260921.sh [--only 1|2|3|4|5]
# 环境变量:
#   REPO       仓库根(默认 /home/putao/code/rust/bao)
#   BASELINE   mozjs 上游基线目录(默认 /tmp/r1-prep/sm153/mozjs-153.3.0,SM153 pristine;
#              缺失 fail-loud 打印重物化配方,无网络下载回退)
#   NT         测试器覆盖("nextest"|"plain",默认 nextest,缺失自动降级 plain 并 WARN)
set -uo pipefail

REPO=${REPO:-/home/putao/code/rust/bao}
JOBS=4
declare -A GATE_RC
ONLY=""
[ "${1:-}" = "--only" ] && ONLY="${2:-}"

cd "$REPO" || { echo "FATAL: repo $REPO 不可达"; exit 97; }

nt_runner() { # 统一测试器选择:nextest 优先,缺失降级 plain(文档化豁免路径)
  if command -v cargo-nextest >/dev/null 2>&1 || cargo nextest --version >/dev/null 2>&1; then
    echo "nextest"
  else
    echo "plain"
  fi
}
NT="${NT:-$(nt_runner)}"

# ---------------- 门① B2:bao_cdp_client 全 suite + bao_browser bridge_channel/console 面 ----------------
# 产物:B2 D5(3df9b83b)/D3(06c76af6)/D2(d4eb7db1+调和在途)
# 判定:两命令 rc=0;bridge_channel/console filterset 须命中 ≥1 测试(零命中=门红,防 suite 空转假绿)
gate1_b2() {
  local rc=0 n
  if [ "$NT" = "nextest" ]; then
    cargo nextest run -p bao_cdp_client --cargo-profile test-ci -j "$JOBS" || rc=1
    n=$(cargo nextest list -p bao-browser -E 'test(bridge_channel) or test(console)' 2>/dev/null | grep -c ': test$')
    [ "${n:-0}" -ge 1 ] || { echo "G1-FAIL: bridge_channel/console filterset 零命中($n)——B2 面测试缺失"; rc=1; }
    cargo nextest run -p bao-browser -E 'test(bridge_channel) or test(console)' --cargo-profile test-ci -j "$JOBS" || rc=1
  else
    echo "[WARN] nextest 缺失,降级 plain cargo test(--test-threads=1,CLAUDE.md 测试纪律文档化路径)"
    cargo test -p bao_cdp_client --jobs "$JOBS" -- --test-threads=1 || rc=1
    cargo test -p bao-browser --jobs "$JOBS" -- --test-threads=1 bridge_channel || rc=1
    cargo test -p bao-browser --jobs "$JOBS" -- --test-threads=1 console || rc=1
  fi
  return $rc
}

# ---------------- 门② mozjs:check -p 双 crate + stencil_xdr smoke + bao_engine 零 fail 基线 ----------------
# 产物:mozjs 三连(c0aa4b61/569e709b/31db4d73)+ em1 余项(P7 EncodeStencil 已入 CLAUDE.md 清单第 7 项)
# 判定:check rc=0;smoke rc=0;bao_engine rc=0 且零 FAILED(383 基线数随并行波漂移——只断言零 fail,不断言绝对数)
gate2_mozjs() {
  local rc=0 log
  cargo check -p bao-mozjs-sys -p bao-mozjs --jobs "$JOBS" || rc=1
  if [ "$NT" = "nextest" ]; then
    cargo nextest run -p bao-mozjs -E 'test(stencil_xdr)' --cargo-profile test-ci -j "$JOBS" || rc=1
    log=$(cargo nextest run -p bao_engine --cargo-profile test-ci -j "$JOBS" 2>&1) || rc=1
  else
    cargo test -p bao-mozjs --jobs "$JOBS" --test stencil_xdr -- --test-threads=1 || rc=1
    log=$(cargo test -p bao_engine --jobs "$JOBS" -- --test-threads=1 2>&1) || rc=1
  fi
  echo "$log" | grep -E 'FAILED|panicked' >/dev/null && { echo "G2-FAIL: bao_engine 出现 FAILED/panicked"; rc=1; }
  echo "$log" | grep -oE '[0-9]+ passed' | head -1 || echo "G2-WARN: bao_engine 计数行未解析(人工复核)"
  return $rc
}

# ---------------- 门③ 单宇宙:vendor 零 workspace 根 + metadata 单解析 + 主锁 --frozen 零 diff ----------------
# 产物:ev4 C2(用户裁决 2026-09-21 单一编译宇宙;memory:single-compile-universe-ruling)
# 判定:vendor 下无 [workspace] 根/无 rust-toolchain.toml/无 Cargo.lock/无 target/;metadata --frozen 成功且锁零 diff
# 注:ev4 C2 落地前本门恒红(预期中的中途红);波末必须绿。
gate3_single_universe() {
  local rc=0 bad lockbak; bad=""
  # 豁免(C 裁定 2026-09-21):vendor/servo/support/crown/ 为 servo 自辖 linter 宇宙,
  # 上游 Cargo.toml 明言 "crown is not part of the Cargo workspace"——其 lock/rust-toolchain 不属 bao 构建面。
  # find 不限深:复活点可能在任意深度(如 vendor/mozjs/mozjs-sys),crown 路径显式排除。
  while IFS= read -r f; do
    case "$f" in vendor/servo/support/crown/*) continue ;; esac
    grep -q '^\[workspace\]' "$f" && { bad="$bad $f"; }
  done < <(find vendor -name Cargo.toml 2>/dev/null)
  [ -z "$bad" ] || { echo "G3-FAIL: vendor 下发现 [workspace] 根:$bad"; rc=1; }
  for f in $(find vendor \( -name 'rust-toolchain.toml' -o -name 'Cargo.lock' \) 2>/dev/null); do
    case "$f" in vendor/servo/support/crown/*) continue ;; esac
    echo "G3-FAIL: vendor 下残留 $f"; rc=1
  done
  for d in $(find vendor -type d -name target 2>/dev/null); do
    case "$d" in vendor/servo/support/crown/*) continue ;; esac
    echo "G3-FAIL: vendor 下残留构建目录 $d"; rc=1
  done
  lockbak=$(mktemp)
  cp Cargo.lock "$lockbak"
  cargo metadata --frozen --format-version 1 >/dev/null 2>&1 || { echo "G3-FAIL: cargo metadata --frozen 失败(锁与 manifest 不同步)"; rc=1; }
  cmp -s Cargo.lock "$lockbak" || { echo "G3-FAIL: 主锁被 metadata 触碰(diff 非零)"; rc=1; }
  rm -f "$lockbak"
  return $rc
}

# ---------------- 门④ parity:bao-*.patch apply --check 对物化基线 + CLAUDE.md 清单计数 7 项 ----------------
# 基线:SM153 pristine(/tmp/r1-prep/sm153/mozjs-153.3.0;BASELINE 可覆盖);上游 0044 层(P5 层序前提)
# 产物:R0(b9c47230, bao-0001/0002)+ SM153 patch 化条目;判据复用 Round0/R1-prep 对账法
# 判定:CLAUDE.md fork patch 清单表恰 7 行;每个 bao-*.patch 对基线 apply --check rc=0
gate4_parity() {
  local rc=0 base n cnt
  # 清单区间:标题行 → "另:" 行(表尾);terminator 必须在表后,否则区间为空计数 0
  cnt=$(grep -cE '^\| [0-9]+ \|' <(sed -n '/mozjs fork BAO patch 清单/,/^另:/p' CLAUDE.md))
  [ "$cnt" = "7" ] || { echo "G4-FAIL: CLAUDE.md mozjs fork patch 清单 = $cnt 行,断言 7"; rc=1; }
  ls vendor/mozjs/mozjs-sys/etc/patches/bao-*.patch >/dev/null 2>&1 || { echo "G4-FAIL: etc/patches 无 bao-*.patch"; return 1; }
  base="${BASELINE:-}"
  if [ -z "$base" ]; then
    # SM153 切换(2026-09-21):基线=SM153.3.0 pristine(servo release tag
    # mozjs-source-3b49a449…,根 mozjs-153.3.0/);140 物化树已按用户"旧版本删除"裁决清除,
    # 网络下载路径移除——基线必须已在盘(重物化用 R1-prep 方法:release tarball 抽三文件)。
    for c in /tmp/r1-prep/sm153/mozjs-153.3.0; do
      [ -d "$c" ] && base="$c" && break
    done
  fi
  if [ -z "$base" ]; then
    echo "G4-FAIL: SM153 基线缺失(/tmp/r1-prep/sm153/mozjs-153.3.0 不在)——重物化:下载 release tag mozjs-source-3b49a44994fb7ff6902f7df5a3bd5f689a4fb4e9 的 mozjs.tar.xz 后抽 js/src/{jsapi.cpp,jsapi.h,jit/BaselineFrame.cpp}"
    return 1
  fi
  local w=/tmp/wave-gate-parity-$$; rm -rf "$w"; mkdir -p "$w/js/src/jit"
  for f in js/src/jsapi.cpp js/src/jsapi.h js/src/jit/BaselineFrame.cpp; do cp "$base/$f" "$w/$f"; done
  git -C "$w" init -q . && git -C "$w" add -A && git -C "$w" -c user.email=gate@bao -c user.name=gate commit -qm baseline
  # 注意:git -C 切目录后 patch 路径必须绝对(相对路径会相对 scratch 解析→not found)
  git -C "$w" apply --include='js/src/jsapi.cpp' "$REPO/vendor/mozjs/mozjs-sys/etc/patches/0044-CurrentGlobalHandle.patch" || { echo "G4-FAIL: 上游 0044 层无法应用(P5 层序前提)"; rm -rf "$w"; return 1; }
  for p in "$REPO"/vendor/mozjs/mozjs-sys/etc/patches/bao-*.patch; do
    git -C "$w" apply --check "$p" || { echo "G4-FAIL: $(basename "$p") apply --check 失败"; rc=1; }
  done
  rm -rf "$w"
  echo "[G4] CLAUDE.md 清单=7 行;bao patch 集:$(ls vendor/mozjs/mozjs-sys/etc/patches/bao-*.patch | wc -l) 个(≤7 合规)"
  return $rc
}

# ---------------- 门⑤ publish 抽样:cargo package --list(零网络) ----------------
# 产物:W0b 发布面(根 Cargo.toml :104-107 规则)
# 判定:bao-mozjs --list rc=0 且非空;bao-servo = 文档化豁免(主根 exclude 非成员,
#       Option A 单宇宙裁决;其发布面验证归 servo 发布 runbook 的波门,不在本波门内强制)
gate5_publish() {
  local rc=0 out
  out=$(cargo package --list --no-verify -p bao-mozjs 2>&1) || { echo "G5-FAIL: bao-mozjs package --list 失败:$(echo "$out" | tail -2)"; rc=1; }
  [ -n "$out" ] || { echo "G5-FAIL: bao-mozjs --list 空"; rc=1; }
  echo "$out" | grep -q 'src/lib.rs' || echo "G5-WARN: bao-mozjs 清单未见 src/lib.rs(人工复核)"
  if out=$(cargo package --list --no-verify -p bao-servo 2>&1); then
    echo "[G5] bao-servo --list OK(非成员也可解析)"
  else
    echo "[G5-EXEMPT] bao-servo 非主根成员(vendor/servo exclude,单宇宙 Option A)——发布面门归 servo 发布 runbook;本门记录豁免不判红"
  fi
  return $rc
}

# ---------------- 调度 ----------------
declare -A NAMES=( [1]=B2 [2]=mozjs [3]=single-universe [4]=parity [5]=publish )
declare -A FNS=( [1]=gate1_b2 [2]=gate2_mozjs [3]=gate3_single_universe [4]=gate4_parity [5]=gate5_publish )
overall=0
for i in 1 2 3 4 5; do
  if [ -n "$ONLY" ] && [ "$ONLY" != "$i" ]; then continue; fi
  echo "======== 门$i(${NAMES[$i]}) ========"
  case $i in
    1) gate1_b2; r=$? ;;
    2) gate2_mozjs; r=$? ;;
    3) gate3_single_universe; r=$? ;;
    4) gate4_parity; r=$? ;;
    5) gate5_publish; r=$? ;;
  esac
  GATE_RC[$i]=$r
  [ "$r" = 0 ] && echo "门$i PASS" || { echo "门$i FAIL(rc=$r)"; overall=1; }
done
echo "======== 波末门汇总 ========"
for i in 1 2 3 4 5; do
  [ -n "$ONLY" ] && [ "$ONLY" != "$i" ] && continue
  echo "门$i(${NAMES[$i]}): rc=${GATE_RC[$i]:-skipped}"
done
exit $overall
