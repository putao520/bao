#!/usr/bin/env bash
# soak-daily.sh — nightly soak launcher behind systemd user timer bao-soak.timer.
#
# 72h ladder step 2 of the ledger soak plan (.plans/spidermonkey-evolution.md,
# 2026-09-10 soak section): 60min × N nightly runs whose zero-hang streak feeds
# the 72h entry condition ("60min 连续 ≥3 次零挂死"). The unit's
# TimeoutStartSec=90min is the hard kill run-1 lacked (unbounded 2h45m hang).
#
# Responsibilities (nothing else — bench/run.sh stays the single soak front):
#   - overlap guard (flock),
#   - detect a previously killed night via the pending marker → honest streak
#     reset (TimeoutStartSec SIGTERMs this whole script; the marker is the only
#     witness that survives the kill),
#   - archive same date+commit leftovers (run.sh overwrites soak.run-1.json but
#     the segments sidecar APPENDS — a rerun would mix two runs into one series),
#   - run `bench/run.sh soak` (results land in bench/results/<date>-<commit>/),
#   - record the night in bench/soak-state/{runs.jsonl,last-run.json} and update
#     the streak in bench/soak-state/entry-progress.json.
#
# daily-ops (06:07) collects the state files next morning — see the ledger
# entry "soak 调度基建落地"; daily-ops itself is NOT modified.
#
# Environment:
#   BAO_SOAK_MINS  soak duration (default 60). Any value != 60 is a verification
#                  run: recorded with counts_toward_entry=false, never touches
#                  the streak (the entry condition is 60min runs).
# Dates are UTC throughout (run.sh convention for bench/results/<date>-<commit>/).
set -euo pipefail

REPO="/home/putao/code/rust/bao"
STATE="$REPO/bench/soak-state"
mkdir -p "$STATE"

# systemd user services carry no nvm/cargo PATH (same root cause daily-ops hit
# 2026-08-24) — cargo/rustc are the load-bearing entries here.
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/usr/local/sbin:/usr/sbin"

exec 9>"$STATE/lock"
flock -n 9 || { echo "[soak-daily] busy, skip"; exit 0; }

MINS="${BAO_SOAK_MINS:-60}"
TODAY="$(date -u +%F)"
NOW_ISO="$(date -u +%FT%TZ)"
RUNLOG="$STATE/night-$TODAY.log"

append_run() { # verdict zero_hang counts streak_after note
  local verdict="$1" zero_hang="$2" counts="$3" streak_after="$4" note="$5"
  printf '{"date":"%s","started_at":"%s","duration_mins":%s,"verdict":"%s","zero_hang":%s,"counts_toward_entry":%s,"streak_after":%s,"note":"%s"}\n' \
    "$TODAY" "$NOW_ISO" "$MINS" "$verdict" "$zero_hang" "$counts" "$streak_after" "$note" \
    >> "$STATE/runs.jsonl"
}

streak_read() { jq -r '.streak // 0' "$STATE/entry-progress.json"; }

streak_write() { # new_streak last_run_ref
  jq --argjson streak "$1" --arg last "$2" --arg date "$TODAY" \
    '.streak = $streak | .last_run = $last | .last_update = $date' \
    "$STATE/entry-progress.json" > "$STATE/entry-progress.json.tmp" \
    && mv "$STATE/entry-progress.json.tmp" "$STATE/entry-progress.json"
}

# ── A previous night that never completed (killed / power loss) ──────────────
PENDING="$STATE/pending"
if [ -f "$PENDING" ]; then
  P_INFO="$(cat "$PENDING")"
  append_run "killed-no-completion" false true 0 \
    "previous run left pending marker and never completed (TimeoutStartSec kill or power loss): $P_INFO — streak reset"
  streak_write 0 "killed:$P_INFO"
  rm -f "$PENDING"
fi

# ── Same date+commit rerun guard (sidecar is append-mode) ────────────────────
SHORT="$(git -C "$REPO" rev-parse --short HEAD)"
DIR="$REPO/bench/results/$TODAY-$SHORT"
if [ -f "$DIR/soak.run-1.segments.jsonl" ]; then
  for f in "$DIR"/soak.run-1.*; do
    [ -e "$f" ] || continue
    mv "$f" "${f%.*}.prev-$(date -u +%H%M).${f##*.}"
  done
fi

echo "[soak-daily] $NOW_ISO start: soak ${MINS}min (head $SHORT)" | tee -a "$RUNLOG"
printf '%s pid=%s mins=%s head=%s\n' "$NOW_ISO" "$$" "$MINS" "$SHORT" > "$PENDING"

RC=0
SOAK_DURATION_MINS="$MINS" bash "$REPO/bench/run.sh" soak 2>&1 | tee -a "$RUNLOG" || RC=$?

rm -f "$PENDING"

# ── Verdict ──────────────────────────────────────────────────────────────────
# run.sh exit 0 + result doc without an error field is the only clean shape.
# Cycle failures cannot hide: BAO_SOAK_CONTINUE_ON_FAIL stays off, so a churn
# cycle error fails the run itself (fail-closed, per run.sh default).
VERDICT="failed"
ZERO_HANG=false
NOTE="run.sh rc=$RC"
JSON="$DIR/soak.run-1.json"
if [ "$RC" -eq 0 ] && [ -f "$JSON" ]; then
  ERR="$(jq -r '.error // empty' "$JSON")"
  if [ -z "$ERR" ]; then
    VERDICT="completed"
    ZERO_HANG=true
    NOTE="clean run"
  else
    NOTE="harness error: $ERR"
  fi
elif [ "$RC" -ne 0 ]; then
  NOTE="run.sh rc=$RC (see $DIR/soak.run-1.log)"
fi

S="$(streak_read)"
if [ "$MINS" -eq 60 ]; then
  if [ "$ZERO_HANG" = true ]; then
    S=$((S + 1))
    append_run "$VERDICT" true true "$S" "zero-hang 60min — entry condition $S/3"
  else
    if [ "$S" -ne 0 ]; then NOTE="$NOTE; streak reset $S→0"; fi
    S=0
    append_run "$VERDICT" false true 0 "$NOTE"
  fi
  streak_write "$S" "$TODAY run ($VERDICT)"
else
  append_run "$VERDICT" "$ZERO_HANG" false "$S" \
    "verification run (BAO_SOAK_MINS=$MINS), not counted toward the 60min entry streak; $NOTE"
fi

tail -n 1 "$STATE/runs.jsonl" > "$STATE/last-run.json"
echo "[soak-daily] done: verdict=$VERDICT streak=$S/$MINS min" | tee -a "$RUNLOG"
exit "$RC"
