#!/bin/bash
set -euo pipefail

PIDS=()

cleanup() {
    echo "Shutting down..."
    for pid in "${PIDS[@]}"; do
        kill -TERM "$pid" 2>/dev/null || true
    done
    for pid in "${PIDS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    exit 0
}

trap cleanup SIGTERM SIGINT

ROLE="${ROLE:-both}"

case "$ROLE" in
    listener)
        /usr/local/bin/listener &
        PIDS+=($!)
        ;;
    submitter)
        /usr/local/bin/submitter &
        PIDS+=($!)
        ;;
    both)
        /usr/local/bin/listener &
        PIDS+=($!)
        /usr/local/bin/submitter &
        PIDS+=($!)
        ;;
    *)
        echo "Unknown role: $ROLE"
        exit 1
        ;;
esac

wait -n "${PIDS[@]}"
