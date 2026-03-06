#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

ADDRESS="${1:-tb1pdsmwnncm8kq0f57rkt3z0ltjqn3tuksz0nvnzhut3660mxxgtfaqv86lsy}"
THREADS="${2:-16}"

# Ensure bitcoind is running
if ! bitcoin-cli -testnet4 getblockchaininfo &>/dev/null; then
    echo "bitcoind is not running. Starting..."
    "$SCRIPT_DIR/start-node.sh"
fi

# Check sync status
SYNCED=$(bitcoin-cli -testnet4 getblockchaininfo | python3 -c "
import sys, json
d = json.load(sys.stdin)
print('true' if not d['initialblockdownload'] else 'false')
")

if [ "$SYNCED" != "true" ]; then
    echo "WARNING: Node is not fully synced. Miner will wait for sync to complete."
fi

echo "Starting testnet4 miner with $THREADS threads..."
echo "Mining address: $ADDRESS"
echo "Logs: $PROJECT_DIR/miner.log"
echo ""

cd "$PROJECT_DIR"
RUST_LOG=info cargo run --release -- \
    --address "$ADDRESS" \
    --threads "$THREADS"
