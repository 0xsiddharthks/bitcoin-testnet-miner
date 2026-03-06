use anyhow::{Context, Result};
use bitcoin::block::{Header, Version};
use bitcoin::consensus::{deserialize, encode::serialize};
use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, Block, BlockHash, CompactTarget, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxMerkleNode, TxOut, Witness,
};

use crate::rpc::BlockTemplate;

/// Encode block height for BIP34 coinbase script_sig.
fn encode_bip34_height(height: u64) -> Vec<u8> {
    // BIP34: encode height as CScriptNum in script_sig
    if height == 0 {
        return vec![1, 0]; // push 1 byte: 0x00
    }

    let mut height_bytes = Vec::new();
    let mut h = height;
    while h > 0 {
        height_bytes.push((h & 0xff) as u8);
        h >>= 8;
    }
    // If the high bit is set, add 0x00 to keep it positive
    if height_bytes.last().unwrap() & 0x80 != 0 {
        height_bytes.push(0x00);
    }

    let mut script = vec![height_bytes.len() as u8];
    script.extend(&height_bytes);
    script
}

/// Build the coinbase transaction for a block.
pub fn build_coinbase_tx(
    height: u64,
    coinbase_value: u64,
    miner_script_pubkey: &ScriptBuf,
    witness_commitment: Option<&str>,
    extra_nonce: u32,
) -> Result<Transaction> {
    // Build script_sig: BIP34 height + extra nonce + tag
    let mut script_sig_bytes = encode_bip34_height(height);
    script_sig_bytes.extend(&extra_nonce.to_le_bytes());
    // Add a miner tag
    script_sig_bytes.extend(b"/testnet4-miner/");
    let script_sig = ScriptBuf::from_bytes(script_sig_bytes);

    // Coinbase input
    let mut coinbase_input = TxIn {
        previous_output: OutPoint {
            txid: bitcoin::Txid::from_byte_array([0u8; 32]),
            vout: 0xFFFFFFFF,
        },
        script_sig,
        sequence: Sequence::MAX,
        witness: Witness::new(),
    };

    // Outputs
    let mut outputs = vec![
        // Output 0: Miner reward
        TxOut {
            value: Amount::from_sat(coinbase_value),
            script_pubkey: miner_script_pubkey.clone(),
        },
    ];

    // Output 1: Witness commitment (if segwit transactions present)
    if let Some(commitment_hex) = witness_commitment {
        let commitment_bytes =
            hex::decode(commitment_hex).context("Invalid witness commitment hex")?;
        outputs.push(TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_bytes(commitment_bytes),
        });

        // Add witness reserved value (32 zero bytes) to coinbase input
        coinbase_input.witness.push([0u8; 32]);
    }

    Ok(Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::blockdata::locktime::absolute::LockTime::ZERO,
        input: vec![coinbase_input],
        output: outputs,
    })
}

/// Parse compact target bits from hex string to CompactTarget.
pub fn parse_bits(bits_hex: &str) -> Result<CompactTarget> {
    let bits = u32::from_str_radix(bits_hex, 16).context("Invalid bits hex")?;
    Ok(CompactTarget::from_consensus(bits))
}

/// Parse a block hash from hex string.
pub fn parse_block_hash(hash_hex: &str) -> Result<BlockHash> {
    hash_hex
        .parse::<BlockHash>()
        .map_err(|e| anyhow::anyhow!("Invalid block hash: {}", e))
}

/// Build a complete block from template and coinbase transaction.
///
/// `override_time` and `override_bits` allow the caller to set a custom
/// timestamp and difficulty (for exploiting the 20-minute rule on testnet4).
pub fn build_block(
    template: &BlockTemplate,
    coinbase_tx: Transaction,
    override_time: Option<u32>,
    override_bits: Option<CompactTarget>,
    include_template_txs: bool,
) -> Result<(Block, CompactTarget)> {
    let bits = match override_bits {
        Some(b) => b,
        None => parse_bits(&template.bits)?,
    };
    let block_time = override_time.unwrap_or(template.cur_time as u32);
    let prev_hash = parse_block_hash(&template.previous_block_hash)?;

    // Collect transactions: coinbase first, then optionally template txs
    let mut txdata = vec![coinbase_tx];
    if include_template_txs {
        for ttx in &template.transactions {
            let tx_bytes = hex::decode(&ttx.data).context("Invalid template tx hex")?;
            let tx: Transaction = deserialize(&tx_bytes).context("Failed to deserialize template tx")?;
            txdata.push(tx);
        }
    }

    // Build block with placeholder merkle root
    let mut block = Block {
        header: Header {
            version: Version::from_consensus(template.version as i32),
            prev_blockhash: prev_hash,
            merkle_root: TxMerkleNode::from_byte_array([0u8; 32]),
            time: block_time,
            bits,
            nonce: 0,
        },
        txdata,
    };

    // Compute correct merkle root from transactions
    let merkle_root = block
        .compute_merkle_root()
        .expect("Block has at least one transaction");
    block.header.merkle_root = merkle_root;

    Ok((block, bits))
}

/// The minimum difficulty CompactTarget for testnet4 (nBits = 0x1d00ffff).
pub const MIN_DIFFICULTY_BITS: u32 = 0x1d00ffff;

/// Serialize a block to hex string for RPC submission.
pub fn serialize_block(block: &Block) -> String {
    hex::encode(serialize(block))
}
