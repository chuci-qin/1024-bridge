#!/usr/bin/env bash
# stop-relayers.sh
# Stop all relayer processes started by start-relayers.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PID_FILE="$SCRIPT_DIR/relayers.pid"

log() { echo "[stop][$(date '+%H:%M:%S')] $*"; }

if [ ! -f "$PID_FILE" ]; then
  log "No relayers.pid found. Nothing to stop."
  exit 0
fi

log "Stopping relayer processes..."
while IFS= read -r pid; do
  [ -z "$pid" ] && continue
  if kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null && log "  Stopped PID $pid" || log "  Failed to stop PID $pid"
  else
    log "  PID $pid already exited"
  fi
done < "$PID_FILE"

sleep 1

STILL_RUNNING=0
while IFS= read -r pid; do
  [ -z "$pid" ] && continue
  if kill -0 "$pid" 2>/dev/null; then
    log "  Force killing PID $pid"
    kill -9 "$pid" 2>/dev/null || true
    STILL_RUNNING=1
  fi
done < "$PID_FILE"

rm -f "$PID_FILE"
log "All relayer processes stopped."
