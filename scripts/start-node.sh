#!/bin/bash
set -e

# Check if bitcoind is already running on testnet4
if bitcoin-cli -testnet4 getblockchaininfo &>/dev/null; then
    echo "bitcoind is already running on testnet4"
    bitcoin-cli -testnet4 getblockchaininfo | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(f'  Chain: {d[\"chain\"]}')
print(f'  Blocks: {d[\"blocks\"]}/{d[\"headers\"]}')
print(f'  Progress: {d[\"verificationprogress\"]*100:.1f}%')
print(f'  IBD: {d[\"initialblockdownload\"]}')
"
    exit 0
fi

# Increase file descriptor limit (bitcoind needs this)
ulimit -n 10240

echo "Starting bitcoind on testnet4..."
bitcoind -daemon

# Wait for RPC to become available
echo "Waiting for RPC..."
for i in $(seq 1 30); do
    if bitcoin-cli -testnet4 getblockchaininfo &>/dev/null; then
        echo "bitcoind is ready!"
        bitcoin-cli -testnet4 getblockchaininfo | python3 -c "
import sys, json
d = json.load(sys.stdin)
print(f'  Chain: {d[\"chain\"]}')
print(f'  Blocks: {d[\"blocks\"]}/{d[\"headers\"]}')
"
        exit 0
    fi
    sleep 2
done

echo "ERROR: bitcoind did not start within 60 seconds"
exit 1
