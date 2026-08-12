# Bao — local CI & developer tasks (just).
#
# CI runs locally: mozjs compiles SpiderMonkey from source (slow on CI runners),
# so the canonical build/test/lint pipeline is this justfile, run on a dev box.
# GitHub Actions (.github/workflows/) mirrors a subset for public signal, but the
# source of truth for "does it pass" is `just ci` here.
#
# 铁律:所有 cargo 命令 --jobs 1(mozjs EBUSY patch,见 CLAUDE.md)。
#
# 常用:
#   just              # 列出所有 recipe
#   just ci           # 本地 CI 全流程(fmt + lint + check + test + bce)
#   just test         # cargo test --jobs 1
#   just bce          # BCE 门禁(等价 make bce-check)
#   just doctor       # bao doctor 环境自检
#   just smoke        # 本地 browser smoke(需 xvfb)

# 默认:打印帮助。`just` 无参时进入。
default:
    @just --list

# ─── 基础变量 ────────────────────────────────────────────────────────────────
# mozjs EBUSY patch:libtest 线程池线程在 TLS teardown 时持锁,多线程编译/测试触发
# pthread_mutex_destroy SIGSEGV。全仓 cargo 必须 --jobs 1 / --test-threads=1。
jobs := "1"
bin := "target/debug/bao"
release_bin := "target/release/bao"

# ─── 构建 ────────────────────────────────────────────────────────────────────

# 构建 CLI binary(debug)。
build:
    cargo build --jobs {{jobs}} -p bao_bin

# 构建 CLI binary(release,LTO,首次很慢)。
build-release:
    cargo build --release --jobs {{jobs}} -p bao_bin

# 仅 check(不产 binary,比 build 快一点)。
check:
    cargo check --jobs {{jobs}} -p bao_bin

# check 整个 workspace(慢,含 mozjs 全栈)。
check-all:
    cargo check --jobs {{jobs}} --workspace

# ─── 测试 ────────────────────────────────────────────────────────────────────

# 跑全仓测试(--jobs 1 / --test-threads=1 是 mozjs EBUSY 铁律)。
test:
    cargo test --jobs {{jobs}} -- --test-threads=1

# 跑单个 crate 的测试。用法:just test-crate bao_browser
test-crate crate:
    cargo test --jobs {{jobs}} -p {{crate}} -- --test-threads=1

# 只跑 bao 库测试(快速回归)。
test-lib:
    cargo test --jobs {{jobs}} -p bao --lib -- --test-threads=1

# ─── 代码质量 ────────────────────────────────────────────────────────────────

# 格式检查(不修改)。
fmt-check:
    cargo fmt --all --check

# 格式化(修改源码)。
fmt:
    cargo fmt --all

# clippy(默认开 warnings,核心 bao_* crate)。
lint:
    cargo clippy --jobs {{jobs}} -p bao_bin -p bao_browser -p bao_cdp -p bao_runtime

# clippy 整个 workspace 并拒绝 warnings(严格,慢)。
lint-strict:
    cargo clippy --jobs {{jobs}} --workspace --all-targets -- -D warnings

# ─── BCE 门禁(复用 Makefile,不重复造) ───────────────────────────────────────
# BCE = Bug-Class Eradication。每类 BUG 根治后留下确定性扫描器,残留>0 阻断。
# 详细:make bce-check / src/BUG-KNOWLEDGE.md。

# 全部 BCE 门禁(gc-unsafe AST+py / ast-catch-fallback / spec-id)。
bce:
    make bce-check

# 只跑 GC-unsafe Handle 门禁(BCE-20260619-012,最常踩)。
bce-gc:
    make bce-gc-unsafe

# ─── 工具入口 ────────────────────────────────────────────────────────────────

# 环境自检(bao doctor:Rust nightly / clang / mozjs / DISPLAY / CDP)。
doctor: build
    ./{{bin}} doctor

# 顶层 eval(等价 bao run -e)。用法:just run 'console.log(1+1)'
run code: build
    ./{{bin}} run -e '{{code}}'

# 一键 bootstrap(工具链 + 构建 + doctor)。
bootstrap:
    ./scripts/bootstrap.sh

# ─── 本地 CI 全流程 ──────────────────────────────────────────────────────────

# 本地 CI:fmt + lint + check + test + bce。这是"是否通过"的权威判定。
ci: fmt-check lint check test bce
    @echo "✓ 本地 CI 全流程通过 (fmt + lint + check + test + bce)"

# 快速 CI(跳过慢的 test,适合改完代码快速验证)。
ci-fast: fmt-check lint check bce
    @echo "✓ 快速 CI 通过 (fmt + lint + check + bce)"

# ─── 本地 smoke(需 xvfb + DISPLAY) ──────────────────────────────────────────
# servo WebRender 需要 DISPLAY,headless 也需要 Xvfb。

# 本地 browser smoke:启动 bao browser + 验证 CDP up + evaluate。
smoke: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== 本地 browser smoke (xvfb) ==="
    xvfb-run -a bash -c '
        set -euo pipefail
        ./{{release_bin}} browser --cdp-port 9222 --url https://example.com &
        BAO_PID=$!
        trap "kill $BAO_PID 2>/dev/null || true" EXIT
        # 等 CDP up
        for i in $(seq 1 30); do
            if curl -sf http://127.0.0.1:9222/json/version >/dev/null 2>&1; then
                echo "✓ CDP up after ${i}s"
                break
            fi
            sleep 1
        done
        curl -sf http://127.0.0.1:9222/json/version | head -c 200; echo
        echo "=== smoke done ==="
    '

# 本地 CDP smoke:用 bao 自身 evaluate 验证(不依赖 Playwright 安装)。
smoke-eval: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    xvfb-run -a ./{{release_bin}} run -e 'console.log("bao eval ok:", 1+1)'

# ─── 本地 release 打包(对应 .github/workflows/release.yml 的本地版) ─────────

# 本地打包 release tar.gz + SHA256(不发布,只产产物)。用法:just release v0.1.0-alpha.1
release version: build-release
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION="{{version}}"
    STAGE="bao-${VERSION}-linux-x86_64"
    rm -rf "$STAGE" "${STAGE}.tar.gz"
    mkdir -p "$STAGE/bin"
    cp {{release_bin}} "$STAGE/bin/bao"
    cp README.md CHANGELOG.md LICENSE "$STAGE/" 2>/dev/null || true
    cp scripts/bootstrap.sh "$STAGE/" 2>/dev/null || true
    tar -czf "${STAGE}.tar.gz" "$STAGE"
    sha256sum "${STAGE}.tar.gz" > SHA256SUMS
    echo "✓ 打包完成: ${STAGE}.tar.gz"
    cat SHA256SUMS

# ─── 本地跑 GitHub Actions workflow(via `act`) ─────────────────────────────
# `just` 不直接解析 GHA;底层用 `act`(nektos/act,已装)在 Docker 里复现 GHA runner
# 跑 .github/workflows/*.yml。这样 GitHub Actions 和本地 just 共用同一套 workflow
# 真源,不重复维护。依赖:act + docker daemon 活着。
#
# 注意:首次很慢 — 拉取 act runner 镜像(catthehacker/ubuntu,~2GB)+ 容器内从零
# 编译 mozjs。workflow 里的 actions/cache 在 act 下也生效(挂到宿主 ~/.cache/act)。

# 列出 act 识别的所有 workflow job(--list,不执行)。
gha-list:
    act --list

# 跑 CI workflow(push 等价:fmt + check + clippy + bce-gate)。
# 用法:just gha-ci [--job check]  (额外参数透传给 act)
gha-ci *ARGS:
    act push -W .github/workflows/ci.yml {{ARGS}}

# 跑 browser-smoke workflow(manual/schedule 等价:bao browser + Xvfb + CDP 验证)。
gha-smoke *ARGS:
    act workflow_dispatch -W .github/workflows/browser-smoke.yml {{ARGS}}

# 跑 cdp-smoke workflow(Playwright → Bao CDP 端到端)。
gha-cdp *ARGS:
    act workflow_dispatch -W .github/workflows/cdp-smoke.yml {{ARGS}}

# 跑 release workflow(本地构建 release binary + 打包,不真正发 GitHub Release)。
# act 默认无 GITHUB_TOKEN 写权限,所以 softprops/action-gh-release 会失败 —
# 用 just gha-release 只验证 build + stage 步骤;真正发布走 git tag push 或手动。
gha-release *ARGS:
    act workflow_dispatch -W .github/workflows/release.yml \
        -f tag=v0.1.0-alpha.1 {{ARGS}}

# 跑全部 workflow(push 等价事件)。
gha-all *ARGS:
    act push {{ARGS}}

# ─── 清理 ────────────────────────────────────────────────────────────────────

# 清理 cargo 构建产物(注意:会删 mozjs 编译缓存,下次很慢)。
clean:
    cargo clean

# 只清理测试产物(保留 mozjs 编译缓存)。
clean-tests:
    rm -rf target/tmp
