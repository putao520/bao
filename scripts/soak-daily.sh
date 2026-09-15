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
# Artifact handoff (2026-09-16 user ruling — soak→daily-ops net-tree deadlock
# fix, A+B leg A): this script commits its own bench/ artifacts (soak-state ×3 +
# results/<date>-<short>/) before exiting, so the tree is clean again by
# morning. daily-ops (06:07) starts its wave on that clean tree; its SKILL.md
# machine-artifact whitelist clause is only the fallback for a failed
# self-collect. See the ledger entry "soak 调度基建落地".
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

# ── Artifact self-collect (soak→daily-ops net-tree deadlock fix, leg A) ──────
# 2026-09-16 user ruling (A+B double cover): soak commits its own bench/
# artifacts (soak-state ×3 + results/<date>-<short>/) so the tree is clean by
# morning. Without this, daily-ops' `git diff --quiet` pre-flight saw the
# overnight soak dirt every day → SKIPPED_BUSY read-only yield → 6-day deadlock
# (2026-09-11..15). The pathspec-scoped add+commit only ever touches bench/,
# never out-of-domain in-flight work (even if someone already staged other
# files). If any git step fails we warn loudly and still exit with the soak RC —
# daily-ops' SKILL.md machine-artifact whitelist clause stays as the fallback.
# Transients (soak-state/lock, night-*.log) are .gitignore'd; the pending
# marker is already gone by this point (removed right after run.sh returns).
collect_rc=0
collect_status=""
collect_status="$(git -C "$REPO" status --porcelain -- bench/)" || collect_rc=$?
if [ "$collect_rc" -eq 0 ] && [ -n "$collect_status" ]; then
  git -C "$REPO" add -- bench/ || collect_rc=$?
  if [ "$collect_rc" -eq 0 ]; then
    git -C "$REPO" commit \
      -m "ops(soak): $TODAY nightly artifacts — verdict=$VERDICT streak=$S ${MINS}min (head $SHORT)" \
      -- bench/ || collect_rc=$?
  fi
  if [ "$collect_rc" -eq 0 ]; then
    echo "[soak-daily] artifact self-collect: bench/ committed" | tee -a "$RUNLOG"
  fi
fi
if [ "$collect_rc" -ne 0 ]; then
  echo "[soak-daily] WARN: artifact self-collect failed (rc=$collect_rc) — daily-ops machine-artifact clause will re-collect next morning" >&2
fi

exit "$RC"
