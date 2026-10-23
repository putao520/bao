#!/usr/bin/env bash
# @tool REQ-CDP-010
# GOLDEN C 双载具编排(Chrome 155 oracle 9333 + bao --cdp-port 9334)→ cdp-oracle-verify.py 逐字段比对。
# 在 farm 容器内运行(golden-cdp 持久盘挂载到 /golden-cdp)。
set -euo pipefail

GC=/golden-cdp
CHROME="$GC/opt/opt/google/chrome/chrome"
BAO=/t/test-ci/bao
ORACLE_PORT=9333
BAO_PORT=9334
XVFB=${XVFB:-0}
VERIFY=/golden-cdp/cdp-oracle-verify.py
export PYTHONPATH=/golden-cdp/pylibs${PYTHONPATH:+:$PYTHONPATH}
export DISPLAY="${DISPLAY:-:99}"

cd "$GC"

if [ "$XVFB" = "1" ]; then
    XAUTH=$(mktemp -d)/Xauthority
    export XAUTHORITY="$XAUTH"
    xvfb-run -a -s "-screen 0 1920x1080x24" bash -c '
        exec "'"$0"'" --nested
    ' || true
    exit $?
fi

# 清理旧进程
pkill -f "remote-debugging-port=$ORACLE_PORT" 2>/dev/null || true
pkill -f "cdp-port=$BAO_PORT" 2>/dev/null || true
sleep 1

# 1) Chrome 155 oracle(headless=new,独立 user-data-dir)
"$CHROME" \
    --headless=new \
    --no-sandbox \
    --disable-gpu \
    --disable-dev-shm-usage \
    --remote-debugging-port=$ORACLE_PORT \
    --remote-allow-origins=* \
    --user-data-dir="$GC/chrome-profile-oracle" \
    about:blank > "$GC/reports/chrome-oracle.log" 2>&1 &
CHROME_PID=$!

# 2) bao browser --cdp-port
"$BAO" browser --cdp-port=$BAO_PORT > "$GC/reports/bao-cdp.log" 2>&1 &
BAO_PID=$!

cleanup() {
    kill $CHROME_PID $BAO_PID 2>/dev/null || true
    wait $CHROME_PID $BAO_PID 2>/dev/null || true
}
trap cleanup EXIT

# 等两端 /json/list 就绪
wait_port() {
    local port=$1 name=$2
    for i in $(seq 1 60); do
        if curl -sf --max-time 2 "http://127.0.0.1:$port/json/list" >/dev/null 2>&1; then
            echo "[golden-cdp] $name ready on port $port"
            return 0
        fi
        sleep 1
    done
    echo "[golden-cdp] ERROR: $name not ready on port $port" >&2
    return 1
}

wait_port $ORACLE_PORT "chrome-oracle"
wait_port $BAO_PORT "bao-cdp"

# 3) verify 探针逐字段比对
python3 "$VERIFY" --oracle-port $ORACLE_PORT --bao-port $BAO_PORT --out "$GC/reports/verify-smoke-$(date +%Y%m%d-%H%M%S).json"
RC=$?

echo "[golden-cdp] verify RC=$RC"
exit $RC
