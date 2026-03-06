# Bitcoin Testnet4 CPU Miner

A Rust-based CPU miner for Bitcoin's testnet4 network, designed to exploit testnet4's **20-minute rule** — when no block has been mined for 20 minutes, difficulty drops to the minimum (1), making CPU mining viable.

Built for Apple Silicon (M4 Max), achieves ~26 MH/s across 16 threads.

## How It Works

Testnet4 has a special difficulty rule: if no block is found within 20 minutes of the previous block's timestamp, the next block can be mined at minimum difficulty (`nBits = 0x1d00ffff`). This miner exploits that rule with a pre-mining strategy:

1. **Get a block template** from Bitcoin Core via `getblocktemplate` RPC
2. **Compute the future timestamp** when the 20-minute window opens (`prev_block.time + 1201`)
3. **Start mining immediately** with that future timestamp and minimum difficulty — the block hash doesn't depend on when we submit, only on what's in the header
4. **Wait for the submission window** to open, then submit the pre-mined block as fast as possible
5. **Verify and extend** — confirm our block is the chain tip, then immediately mine the next block to make our chain harder to orphan

## Prerequisites

- **Rust** 1.75+ (`rustup` recommended)
- **Bitcoin Core** v28+ with testnet4 support (v30+ recommended)
- macOS, Linux, or Windows (optimized for macOS/Apple Silicon)

## Quick Start

### 1. Configure Bitcoin Core

Add to `~/Library/Application Support/Bitcoin/bitcoin.conf` (macOS) or `~/.bitcoin/bitcoin.conf` (Linux):

```ini
testnet4=1

[testnet4]
server=1
rpcuser=testnet4miner
rpcpassword=localMiningPassword2024
rpcallowip=127.0.0.1
rpcbind=127.0.0.1
port=48333
rpcport=48332
txindex=1
dbcache=4096
```

### 2. Start the Node

```bash
# May need increased file descriptor limit
ulimit -n 10240 && bitcoind -daemon

# Or use the helper script
./scripts/start-node.sh
```

Wait for initial block download to complete (can take several hours):

```bash
bitcoin-cli -testnet4 getblockchaininfo
```

### 3. Create a Wallet

```bash
./scripts/setup-wallet.sh
# Or manually:
bitcoin-cli -testnet4 createwallet "mining" false false "" false true
bitcoin-cli -testnet4 -rpcwallet=mining getnewaddress "mining-reward" "bech32m"
```

### 4. Start Mining

```bash
# Using the helper script (recommended)
./scripts/start-miner.sh

# Or directly with cargo
cargo run --release -- --address <YOUR_TB1P_ADDRESS>

# With custom options
cargo run --release -- \
    --address tb1p... \
    --threads 16 \
    --rpc-url http://127.0.0.1:48332 \
    --log-file miner.log
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--address` | *required* | Mining reward address (tb1p... or tb1q...) |
| `--rpc-url` | `http://127.0.0.1:48332` | Bitcoin Core RPC endpoint |
| `--rpc-user` | `testnet4miner` | RPC username |
| `--rpc-pass` | `localMiningPassword2024` | RPC password |
| `--threads` | all CPUs | Number of mining threads |
| `--any-difficulty` | false | Mine at any difficulty (don't wait for 20-min rule) |
| `--include-txs` | false | Include mempool transactions (slower) |
| `--log-file` | `miner.log` | Log file path (also logs to stderr) |

## Architecture

```
src/
  main.rs           # CLI, mining loop, 20-minute rule logic, submission
  rpc.rs            # Bitcoin Core JSON-RPC client (HTTP connection pooling)
  block_builder.rs  # Coinbase TX construction, merkle root, block assembly
  miner.rs          # Parallel SHA256d mining engine (rayon)
scripts/
  start-node.sh     # Start bitcoind with status check
  setup-wallet.sh   # Create wallet and generate address
  start-miner.sh    # Build and run the miner
```

### Key Components

- **RPC Client** (`rpc.rs`): Custom JSON-RPC client using `ureq::Agent` for HTTP keep-alive connection pooling. Prevents TCP ephemeral port exhaustion from frequent RPC calls. Retries on stale connections.

- **Block Builder** (`block_builder.rs`): Constructs BIP34-compliant coinbase transactions with witness commitment support. Handles extra-nonce cycling (up to 256 variations) for new merkle roots when the nonce space is exhausted.

- **Mining Engine** (`miner.rs`): Parallel nonce search using `rayon`. Splits the 2^32 nonce space evenly across threads. Uses `bitcoin` crate's `validate_pow` for double-SHA256 + target comparison.

- **Main Loop** (`main.rs`): Orchestrates template fetching, 20-minute rule computation, pre-mining, submission with aggressive retry polling, and chain-tip verification.

## Mining Strategy

### Pre-Mining Optimization

The miner doesn't wait for the 20-minute window — it starts mining immediately with the future timestamp. Since the block hash only depends on the header contents (not submission time), a valid nonce found early is still valid later.

### Race-Winning

After successfully submitting a block:
1. Verify our block is the chain tip via `getbestblockhash`
2. Immediately mine the next block to extend our chain
3. A 2-block lead makes our chain much harder to orphan

### Difficulty Adjustment Boundaries

At blocks where `height % 2016 == 0`, the 20-minute rule does **not** apply. Bitcoin Core uses normal difficulty adjustment at these boundaries. The miner detects this and waits for ASIC miners to handle these blocks.

## Checking Your Balance

```bash
# Load wallet (needed after bitcoind restart)
bitcoin-cli -testnet4 loadwallet "mining"

# Check all balance types
bitcoin-cli -testnet4 -rpcwallet=mining getbalances

# Note: coinbase rewards need 100 confirmations to mature
# "immature" balance becomes "trusted" after 100 blocks
```

## Monitoring

```bash
# Follow miner logs
tail -f miner.log

# Check node status
bitcoin-cli -testnet4 getblockchaininfo

# Verify a mined block
bitcoin-cli -testnet4 getblock <block_hash>
```

## Known Limitations

- **Difficulty adjustment blocks** (`height % 2016 == 0`) cannot be CPU mined — must wait for ASIC miners
- **Timestamp stacking**: Consecutive min-difficulty blocks push the chain timestamp ~20 min into the future; after ~6 blocks, the 2-hour future limit forces a real-time wait
- **Competition**: Other testnet4 miners may find blocks faster, causing template staleness
- **Coinbase maturity**: Mined coins require 100 confirmations before they're spendable

## License

MIT
