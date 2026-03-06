mod block_builder;
mod miner;
mod rpc;

use anyhow::Result;
use bitcoin::CompactTarget;
use clap::Parser;
use log::{error, info, warn};
use std::fs::OpenOptions;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The 20-minute rule threshold in seconds.
const TWENTY_MIN_SECS: u64 = 1200;
/// Max allowed future time offset (2 hours in seconds).
const MAX_FUTURE_BLOCK_TIME: u64 = 7200;
/// Bitcoin difficulty adjustment interval (blocks).
const DIFFICULTY_ADJUSTMENT_INTERVAL: u64 = 2016;

#[derive(Parser)]
#[command(name = "testnet4-miner")]
#[command(about = "Bitcoin Testnet4 CPU Miner")]
struct Args {
    /// Mining address (tb1p... or tb1q...)
    #[arg(long)]
    address: String,

    /// RPC URL
    #[arg(long, default_value = "http://127.0.0.1:48332")]
    rpc_url: String,

    /// RPC username
    #[arg(long, default_value = "testnet4miner")]
    rpc_user: String,

    /// RPC password
    #[arg(long, default_value = "localMiningPassword2024")]
    rpc_pass: String,

    /// Number of mining threads (default: all CPUs)
    #[arg(long)]
    threads: Option<usize>,

    /// Mine at any difficulty (don't wait for 20-minute minimum difficulty rule)
    #[arg(long)]
    any_difficulty: bool,

    /// Include mempool transactions in blocks (default: mine empty blocks for speed)
    #[arg(long)]
    include_txs: bool,

    /// Log file path (logs written to both stderr and this file)
    #[arg(long, default_value = "miner.log")]
    log_file: String,
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Block subsidy in satoshis for a given height.
fn block_subsidy(height: u64) -> u64 {
    let halvings = height / 210_000;
    if halvings >= 64 {
        return 0;
    }
    50_0000_0000u64 >> halvings
}

fn setup_logging(log_path: &str) {
    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .expect("Failed to open log file"),
    ));

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(move |buf, record| {
            use std::io::Write;
            let line = format!(
                "[{} {:5} {}] {}",
                buf.timestamp_millis(),
                record.level(),
                record.module_path().unwrap_or(""),
                record.args()
            );
            writeln!(buf, "{}", line)?;
            if let Ok(mut f) = log_file.lock() {
                let _ = writeln!(f, "{}", line);
            }
            Ok(())
        })
        .init();
}

fn main() -> Result<()> {
    let args = Args::parse();
    setup_logging(&args.log_file);

    let threads = args.threads.unwrap_or_else(num_cpus);
    let empty_blocks = !args.include_txs;

    info!("=== Testnet4 CPU Miner ===");
    info!("Mining address: {}", args.address);
    info!("RPC: {}", args.rpc_url);
    info!("Threads: {}", threads);
    info!("Empty blocks: {} (faster submission)", empty_blocks);
    info!("Log file: {}", args.log_file);
    info!(
        "Difficulty mode: {}",
        if args.any_difficulty {
            "mine at any difficulty"
        } else {
            "exploit 20-minute rule for minimum difficulty"
        }
    );

    // Parse mining address — assume_checked since testnet4 uses same prefix as testnet3
    let address: bitcoin::Address<bitcoin::address::NetworkUnchecked> = args
        .address
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid address: {}", e))?;
    let address = address.assume_checked();
    let miner_script_pubkey = address.script_pubkey();

    info!("Script pubkey: {}", miner_script_pubkey);

    // Connect to bitcoind (uses HTTP connection pooling — no TCP port exhaustion)
    let client = rpc::connect(&args.rpc_url, &args.rpc_user, &args.rpc_pass)?;

    // Wait for node to sync
    wait_for_sync(&client)?;

    // Mining state
    let hash_count = AtomicU64::new(0);
    let stop_flag = AtomicBool::new(false);
    let start_time = Instant::now();
    let mut blocks_found: u32 = 0;

    info!("Starting mining loop...");

    loop {
        // Get block template
        let template = match rpc::get_block_template(&client) {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to get block template: {}. Retrying in 5s...", e);
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        info!(
            "Template: height={}, bits={}, txs={}, reward={} sats",
            template.height,
            template.bits,
            template.transactions.len(),
            template.coinbasevalue
        );

        // Compute the 20-minute rule timestamp and difficulty
        let (block_time, block_bits, secs_until_submittable) = if args.any_difficulty {
            (None, None, 0u64)
        } else {
            match compute_twenty_min_rule_extended(&client, &template) {
                Ok(Some(rule)) => {
                    // Always pre-mine immediately — no waiting threshold.
                    // This gives us a head start over miners who wait.
                    if rule.secs_until_valid > 0 {
                        info!(
                            "PRE-MINING: window opens in ~{}s ({:.1} min). Mining NOW.",
                            rule.secs_until_valid, rule.secs_until_valid as f64 / 60.0
                        );
                    } else {
                        info!("20-minute rule active! Mining at minimum difficulty.");
                    }
                    (Some(rule.block_time), Some(rule.block_bits), rule.secs_until_valid)
                }
                Ok(None) => {
                    // Can't use 20-minute rule (e.g., difficulty adjustment boundary)
                    // Poll every 30s waiting for another miner to mine this block
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                }
                Err(e) => {
                    warn!("Error computing 20-min rule: {}. Using template values.", e);
                    (None, None, 0)
                }
            }
        };

        // Determine coinbase value and whether to include template transactions
        let (cb_value, witness_commitment) = if empty_blocks {
            (block_subsidy(template.height), None)
        } else {
            (
                template.coinbasevalue,
                template.default_witness_commitment.as_deref(),
            )
        };

        // Mine with extra_nonce cycling — try multiple coinbase variations
        let mine_start = Instant::now();
        let mut block_accepted = false;

        'extra_nonce: for extra_nonce in 0u32..256 {
            // Check if a new block arrived (template stale)
            if extra_nonce > 0 {
                if let Ok(info) = rpc::get_blockchain_info(&client) {
                    if info.bestblockhash != template.previous_block_hash {
                        info!("New block arrived, template stale. Getting fresh template...");
                        break 'extra_nonce;
                    }
                }
            }

            // Build coinbase transaction with this extra_nonce
            let coinbase_tx = block_builder::build_coinbase_tx(
                template.height,
                cb_value,
                &miner_script_pubkey,
                witness_commitment,
                extra_nonce,
            )?;

            // Build block
            let (mut block, _bits) = block_builder::build_block(
                &template,
                coinbase_tx,
                block_time,
                block_bits,
                !empty_blocks,
            )?;

            if extra_nonce == 0 {
                info!(
                    "Mining block {} (bits: {:08x}, time: {}, txs: {})",
                    template.height,
                    block.header.bits.to_consensus(),
                    block.header.time,
                    block.txdata.len()
                );
            } else {
                info!("Trying extra_nonce={}, new merkle root", extra_nonce);
            }

            // Mine
            match miner::mine_block(block.header, threads, &stop_flag, &hash_count) {
                Some(result) => {
                    block.header.nonce = result.nonce;
                    let elapsed = mine_start.elapsed();

                    info!(
                        "*** BLOCK FOUND! *** Height: {}, Hash: {}, Nonce: {}, Extra: {}, Time: {:.1}s",
                        template.height, result.hash, result.nonce, extra_nonce, elapsed.as_secs_f64()
                    );

                    // Serialize block for submission
                    let block_hex = block_builder::serialize_block(&block);

                    // Wait for the submission window if we pre-mined,
                    // checking for staleness (reorgs) every 15 seconds
                    if secs_until_submittable > 0 {
                        let elapsed_secs = mine_start.elapsed().as_secs();
                        if elapsed_secs < secs_until_submittable {
                            let remaining = secs_until_submittable - elapsed_secs;
                            if remaining > 2 {
                                info!(
                                    "Pre-mined! Waiting {}s for submission window (checking staleness every 5s)...",
                                    remaining - 1
                                );
                                let wait_end =
                                    Instant::now() + Duration::from_secs(remaining - 1);
                                let mut stale = false;
                                while Instant::now() < wait_end {
                                    // Check if tip changed (reorg during wait)
                                    if let Ok(info) = rpc::get_blockchain_info(&client) {
                                        if info.bestblockhash != template.previous_block_hash {
                                            warn!(
                                                "Chain tip changed during wait — template stale, re-mining..."
                                            );
                                            stale = true;
                                            break;
                                        }
                                    }
                                    let left = wait_end
                                        .checked_duration_since(Instant::now())
                                        .unwrap_or(Duration::ZERO);
                                    if left.is_zero() {
                                        break;
                                    }
                                    std::thread::sleep(left.min(Duration::from_secs(5)));
                                }
                                if stale {
                                    break 'extra_nonce;
                                }
                            }
                            info!("Polling for submission window...");
                        }
                    }

                    // Submit with aggressive 100ms polling for time-too-new
                    let mut submitted = false;
                    for attempt in 0..1500 {
                        match rpc::submit_block(&client, &block_hex) {
                            Ok(()) => {
                                blocks_found += 1;
                                info!(
                                    "Block {} accepted! Total blocks mined: {}",
                                    template.height, blocks_found
                                );
                                // Verify our block is the chain tip
                                if let Ok(tip) = rpc::get_blockchain_info(&client) {
                                    if tip.bestblockhash == result.hash.to_string() {
                                        info!(
                                            "Confirmed: our block is the chain tip. Mining next block immediately..."
                                        );
                                    } else {
                                        warn!(
                                            "Chain tip is not our block — possible race loss. Continuing..."
                                        );
                                    }
                                }
                                block_accepted = true;
                                submitted = true;
                                break 'extra_nonce;
                            }
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if err_str.contains("time-too-new") && attempt < 1499 {
                                    if attempt == 0 {
                                        info!(
                                            "Block timestamp not yet valid. Polling every 10ms..."
                                        );
                                    }
                                    std::thread::sleep(Duration::from_millis(10));
                                } else if err_str.contains("inconclusive")
                                    || err_str.contains("duplicate")
                                {
                                    warn!("Block already submitted or stale. Moving on.");
                                    submitted = true;
                                    break 'extra_nonce;
                                } else if err_str.contains("bad-diffbits") {
                                    error!(
                                        "Block rejected: difficulty mismatch. Template may be stale."
                                    );
                                    submitted = true;
                                    break 'extra_nonce;
                                } else {
                                    error!("Block submission failed: {}", e);
                                    break;
                                }
                            }
                        }
                    }
                    if !submitted {
                        error!("Failed to submit block after all retries.");
                    }
                    break 'extra_nonce;
                }
                None => {
                    // Nonce space exhausted for this extra_nonce, try next
                    continue;
                }
            }
        }

        if !block_accepted {
            let elapsed = mine_start.elapsed();
            info!("Mining attempt completed in {:.1}s", elapsed.as_secs_f64());
        }

        miner::print_stats(&hash_count, start_time, blocks_found);
    }
}

struct TwentyMinRule {
    block_time: u32,
    block_bits: CompactTarget,
    /// Seconds until the block can actually be submitted (0 = now)
    secs_until_valid: u64,
}

/// Compute the 20-minute rule parameters.
fn compute_twenty_min_rule_extended(
    client: &rpc::RpcClient,
    template: &rpc::BlockTemplate,
) -> Result<Option<TwentyMinRule>> {
    if template.height % DIFFICULTY_ADJUSTMENT_INTERVAL == 0 {
        if template.bits == "1d00ffff" {
            info!(
                "Block {} is a difficulty adjustment block, but difficulty is already minimum. Mining normally.",
                template.height
            );
            return Ok(Some(TwentyMinRule {
                block_time: template.cur_time as u32,
                block_bits: CompactTarget::from_consensus(block_builder::MIN_DIFFICULTY_BITS),
                secs_until_valid: 0,
            }));
        }
        warn!(
            "Block {} is a difficulty adjustment boundary (height % {} == 0). \
             20-minute rule does NOT apply. Template difficulty: {}. \
             Cannot CPU mine — waiting for ASIC miner to mine this block...",
            template.height, DIFFICULTY_ADJUSTMENT_INTERVAL, template.bits
        );
        return Ok(None);
    }

    let prev_block = rpc::get_block_info(client, &template.previous_block_hash)?;

    let target_time = prev_block.time + TWENTY_MIN_SECS + 1;
    let min_time = template.mintime;
    let block_time = target_time.max(min_time);

    let system_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let need_sys_time = block_time.saturating_sub(MAX_FUTURE_BLOCK_TIME);
    let secs_until_valid = need_sys_time.saturating_sub(system_time);

    let min_bits = CompactTarget::from_consensus(block_builder::MIN_DIFFICULTY_BITS);

    Ok(Some(TwentyMinRule {
        block_time: block_time as u32,
        block_bits: min_bits,
        secs_until_valid,
    }))
}

fn wait_for_sync(client: &rpc::RpcClient) -> Result<()> {
    loop {
        match rpc::get_blockchain_info(client) {
            Ok(info) => {
                if info.chain != "testnet4" {
                    anyhow::bail!(
                        "Node is NOT on testnet4! Chain: {}. Check bitcoin.conf.",
                        info.chain
                    );
                }
                if !info.initialblockdownload {
                    info!(
                        "Node synced. Chain: {}, Blocks: {}",
                        info.chain, info.blocks
                    );
                    return Ok(());
                }
                info!(
                    "Node syncing... {:.1}% (blocks: {}/{})",
                    info.verificationprogress * 100.0,
                    info.blocks,
                    info.headers
                );
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(e) => {
                warn!("Cannot reach bitcoind: {}. Retrying in 5s...", e);
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}
