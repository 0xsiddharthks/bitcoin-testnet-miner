use bitcoin::block::Header;
use bitcoin::{BlockHash, Target};
use log::info;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub struct MiningResult {
    pub nonce: u32,
    pub hash: BlockHash,
}

/// Mine a block by searching for a valid nonce in parallel across threads.
///
/// Returns Some(MiningResult) if a valid nonce is found, None if the entire
/// nonce space is exhausted or mining was stopped via stop_flag.
pub fn mine_block(
    header: Header,
    num_threads: usize,
    stop_flag: &AtomicBool,
    hash_count: &AtomicU64,
) -> Option<MiningResult> {
    let target = Target::from(header.bits);
    let result: Mutex<Option<MiningResult>> = Mutex::new(None);
    let found = AtomicBool::new(false);

    let chunk_size = (u32::MAX as u64 + 1) / num_threads as u64;

    (0..num_threads).into_par_iter().for_each(|thread_id| {
        let start = (thread_id as u64 * chunk_size) as u32;
        let end = if thread_id == num_threads - 1 {
            u32::MAX
        } else {
            ((thread_id as u64 + 1) * chunk_size - 1) as u32
        };

        let mut h = header;
        let mut local_count: u64 = 0;

        for nonce in start..=end {
            if found.load(Ordering::Relaxed) || stop_flag.load(Ordering::Relaxed) {
                break;
            }

            h.nonce = nonce;

            if let Ok(hash) = h.validate_pow(target) {
                found.store(true, Ordering::Relaxed);
                *result.lock().unwrap() = Some(MiningResult { nonce, hash });
                break;
            }

            local_count += 1;
            if local_count % 100_000 == 0 {
                hash_count.fetch_add(100_000, Ordering::Relaxed);
            }
        }

        // Add remaining count
        hash_count.fetch_add(local_count % 100_000, Ordering::Relaxed);
    });

    result.into_inner().unwrap()
}

/// Check if the given bits represent minimum difficulty (20-minute rule).
pub fn is_min_difficulty(bits_hex: &str) -> bool {
    bits_hex == "1d00ffff"
}

/// Print mining statistics.
pub fn print_stats(hash_count: &AtomicU64, start_time: Instant, blocks_found: u32) {
    let elapsed = start_time.elapsed().as_secs_f64();
    let hashes = hash_count.load(Ordering::Relaxed);
    if elapsed > 0.0 {
        let hashrate = hashes as f64 / elapsed;
        let (rate, unit) = if hashrate > 1_000_000.0 {
            (hashrate / 1_000_000.0, "MH/s")
        } else if hashrate > 1_000.0 {
            (hashrate / 1_000.0, "KH/s")
        } else {
            (hashrate, "H/s")
        };
        info!(
            "Hashrate: {:.2} {} | Total hashes: {} | Blocks found: {} | Uptime: {:.0}s",
            rate, unit, hashes, blocks_found, elapsed
        );
    }
}
