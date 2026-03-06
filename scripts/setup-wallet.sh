#!/bin/bash
set -e

WALLET_NAME="mining"

# Check if bitcoind is running
if ! bitcoin-cli -testnet4 getblockchaininfo &>/dev/null; then
    echo "ERROR: bitcoind is not running. Run ./scripts/start-node.sh first."
    exit 1
fi

# Check if wallet already exists and is loaded
if bitcoin-cli -testnet4 -rpcwallet="$WALLET_NAME" getwalletinfo &>/dev/null; then
    echo "Wallet '$WALLET_NAME' already exists and is loaded."
else
    # Try to load existing wallet
    if bitcoin-cli -testnet4 loadwallet "$WALLET_NAME" &>/dev/null; then
        echo "Loaded existing wallet '$WALLET_NAME'."
    else
        # Create new wallet
        echo "Creating wallet '$WALLET_NAME'..."
        bitcoin-cli -testnet4 createwallet "$WALLET_NAME" false false "" false true
        echo "Wallet created."
    fi
fi

# Generate a new mining address
ADDRESS=$(bitcoin-cli -testnet4 -rpcwallet="$WALLET_NAME" getnewaddress "mining-reward" "bech32m")
echo ""
echo "=== Mining Address ==="
echo "$ADDRESS"
echo ""
echo "Start mining with:"
echo "  cargo run --release -- --address $ADDRESS"
echo ""

# Show current balance
BALANCE=$(bitcoin-cli -testnet4 -rpcwallet="$WALLET_NAME" getbalance)
echo "Current balance: $BALANCE BTC"
