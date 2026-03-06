use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct BlockTemplate {
    pub version: u32,
    #[serde(rename = "previousblockhash")]
    pub previous_block_hash: String,
    pub transactions: Vec<TemplateTransaction>,
    pub coinbasevalue: u64,
    pub target: String,
    #[serde(rename = "curtime")]
    pub cur_time: u64,
    pub mintime: u64,
    pub bits: String,
    pub height: u64,
    pub default_witness_commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BlockInfo {
    pub height: u64,
    pub time: u64,
    pub mediantime: u64,
    pub bits: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TemplateTransaction {
    pub data: String,
    pub txid: String,
    pub hash: String,
    pub fee: u64,
    pub weight: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct BlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    pub bestblockhash: String,
    pub initialblockdownload: bool,
    pub verificationprogress: f64,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// JSON-RPC client with HTTP connection pooling.
///
/// Uses `ureq::Agent` to maintain a pool of keep-alive connections,
/// preventing TCP ephemeral port exhaustion from frequent RPC calls.
pub struct RpcClient {
    agent: ureq::Agent,
    url: String,
    auth_header: String,
    request_id: AtomicU64,
}

impl RpcClient {
    pub fn new(url: &str, user: &str, pass: &str) -> Result<Self> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(120))
            .timeout_connect(Duration::from_secs(10))
            .build();

        let credentials = format!("{}:{}", user, pass);
        let auth_header = format!("Basic {}", STANDARD.encode(credentials.as_bytes()));

        Ok(Self {
            agent,
            url: url.to_string(),
            auth_header,
            request_id: AtomicU64::new(0),
        })
    }

    /// Make a raw JSON-RPC call and return the result as a serde_json::Value.
    /// Retries once on transient connection errors (stale keep-alive connections).
    fn call_raw(&self, method: &str, params: &[Value]) -> Result<Value> {
        for attempt in 0..2 {
            match self.try_call(method, params) {
                Ok(value) => return Ok(value),
                Err(e) => {
                    let err_str = format!("{}", e);
                    // Stale keep-alive connection — retry once with a fresh connection
                    if attempt == 0
                        && (err_str.contains("Unexpected EOF")
                            || err_str.contains("Connection reset")
                            || err_str.contains("Broken pipe"))
                    {
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    fn try_call(&self, method: &str, params: &[Value]) -> Result<Value> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "1.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let response_text = match self
            .agent
            .post(&self.url)
            .set("Authorization", &self.auth_header)
            .set("Content-Type", "application/json")
            .send_json(body)
        {
            Ok(resp) => resp.into_string().context("Failed to read response body")?,
            Err(ureq::Error::Status(code, resp)) => {
                // Bitcoin Core returns HTTP 500 for some RPC errors but still
                // includes a valid JSON-RPC response body.
                let body = resp.into_string().unwrap_or_default();
                if body.is_empty() {
                    bail!("{}: HTTP {} with no response body", method, code);
                }
                body
            }
            Err(e) => bail!("{}: {}", method, e),
        };

        if response_text.is_empty() {
            bail!("{}: empty response from server", method);
        }

        let json_resp: JsonRpcResponse =
            serde_json::from_str(&response_text).with_context(|| {
                format!(
                    "{}: failed to parse response: {}",
                    method,
                    &response_text[..response_text.len().min(200)]
                )
            })?;

        if let Some(error) = json_resp.error {
            bail!("{}: {} (code: {})", method, error.message, error.code);
        }

        Ok(json_resp.result.unwrap_or(Value::Null))
    }

    /// Make a JSON-RPC call and deserialize the result into type T.
    fn call<T: serde::de::DeserializeOwned>(&self, method: &str, params: &[Value]) -> Result<T> {
        let raw = self.call_raw(method, params)?;
        serde_json::from_value(raw).context(format!("Failed to deserialize {} result", method))
    }
}

pub fn connect(url: &str, user: &str, pass: &str) -> Result<RpcClient> {
    RpcClient::new(url, user, pass)
}

pub fn get_block_template(client: &RpcClient) -> Result<BlockTemplate> {
    let params = json!({"rules": ["segwit"]});
    client.call("getblocktemplate", &[params])
}

pub fn submit_block(client: &RpcClient, block_hex: &str) -> Result<()> {
    let result = client.call_raw("submitblock", &[json!(block_hex)])?;
    if result.is_null() {
        Ok(())
    } else {
        bail!("Block rejected: {}", result)
    }
}

pub fn get_blockchain_info(client: &RpcClient) -> Result<BlockchainInfo> {
    client.call("getblockchaininfo", &[])
}

pub fn get_block_info(client: &RpcClient, block_hash: &str) -> Result<BlockInfo> {
    client.call("getblock", &[json!(block_hash)])
}
