use alloy::{
    network::{EthereumWallet, TransactionBuilder},
    primitives::{address, Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
    signers::local::PrivateKeySigner,
    sol,
    sol_types::SolCall,
};
use eyre::Result;
use std::sync::Arc;

use crate::gas::{compute_gas_params, GasParams};
use crate::math::{estimate_arb, max_amount_in, price_from_sqrt};

// ── ABI binding ──────────────────────────────────────────────────────────────

sol! {
    interface MevArbitrage {
        function executeTriArb(
            address poolA,
            address poolB,
            address poolC,
            uint256 amountIn,
            bool    zeroForOneA,
            bool    zeroForOneB,
            bool    zeroForOneC,
            uint256 minProfit          // slippage guard inside the contract
        ) external;
    }
}

// ── Pool snapshot passed in from your stream consumer ────────────────────────

#[derive(Debug, Clone)]
pub struct PoolSnapshot {
    pub address:        Address,
    pub sqrt_price_x96: U256,
    pub liquidity:      u128,
    pub zero_for_one:   bool,   // direction for this leg
}

// ── Executor — wraps the provider and signer ─────────────────────────────────

#[derive(Clone)]
pub struct BundleExecutor {
    /// Public RPC — used only for reads (gas, nonce)
    pub read_provider: Arc<dyn Provider>,
    /// Signer — kept separate so signing is always local, never sent over the wire
    pub signer:        Arc<PrivateKeySigner>,
    pub wallet:        Arc<EthereumWallet>,
    pub mev_contract:  Address,
    pub chain_id:      u64,
}

impl BundleExecutor {
    pub async fn new(public_rpc: &str, private_key_hex: &str) -> Result<Self> {
        let signer: PrivateKeySigner = private_key_hex.parse()?;
        let wallet = EthereumWallet::from(signer.clone());

        let read_provider = ProviderBuilder::new()
            .on_builtin(public_rpc)
            .await?;

        Ok(Self {
            read_provider: Arc::new(read_provider),
            signer:        Arc::new(signer),
            wallet:        Arc::new(wallet),
            mev_contract:  address!("9999999999999999999999999999999999999999"),
            chain_id:      8453, // Base mainnet
        })
    }
}

// ── construct_bundle — the main entry point ───────────────────────────────────

/// Builds a signed TransactionRequest for a 3-pool arb and broadcasts it to a
/// private relay.
///
/// Returns the tx hash if broadcast succeeded, or an error with context.
pub async fn construct_bundle(
    executor:  &BundleExecutor,
    pool_a:    PoolSnapshot,
    pool_b:    PoolSnapshot,
    pool_c:    PoolSnapshot,
    urgency:   f64,    // gas multiplier: 1.10 / 1.25 / 1.50
    tip_gwei:  f64,    // priority fee in gwei (Base: ~0.001–0.01)
) -> Result<alloy::primitives::TxHash> {
    // ── 1. Gas params (reads latest block from public RPC) ────────────────
    let gas = compute_gas_params(&*executor.read_provider, urgency, tip_gwei).await?;

    let gas_cost = U256::from(gas.max_fee_per_gas)
        .saturating_mul(U256::from(gas.gas_limit));

    // ── 2. Size the trade from pool A's liquidity ─────────────────────────
    let amount_in = max_amount_in(pool_a.liquidity, pool_a.sqrt_price_x96, 18);

    // ── 3. Profitability check ─────────────────────────────────────────────
    let (sized_amount_in, min_profit) = estimate_arb(
        pool_a.sqrt_price_x96, pool_a.liquidity,
        pool_b.sqrt_price_x96, pool_b.liquidity,
        pool_c.sqrt_price_x96, pool_c.liquidity,
        amount_in,
        gas_cost,
    )
    .ok_or_else(|| eyre::eyre!("no profitable arb at current prices"))?;

    tracing::info!(
        amount_in = %sized_amount_in,
        min_profit = %min_profit,
        max_fee    = gas.max_fee_per_gas,
        gas_limit  = gas.gas_limit,
        "bundle profitable — constructing tx"
    );

    // ── 4. Encode calldata ────────────────────────────────────────────────
    let calldata = encode_tri_arb(
        pool_a.address,   pool_b.address,   pool_c.address,
        sized_amount_in,
        pool_a.zero_for_one, pool_b.zero_for_one, pool_c.zero_for_one,
        min_profit,
    );

    // ── 5. Fetch nonce (public RPC) ───────────────────────────────────────
    let sender = executor.signer.address();
    let nonce  = executor.read_provider
        .get_transaction_count(sender)
        .await?;

    // ── 6. Build TransactionRequest ───────────────────────────────────────
    let tx = TransactionRequest::default()
        .from(sender)
        .to(executor.mev_contract)
        .value(U256::ZERO)
        .with_input(calldata)
        .nonce(nonce)
        .chain_id(executor.chain_id)
        .gas_limit(gas.gas_limit)
        .max_fee_per_gas(gas.max_fee_per_gas)
        .max_priority_fee_per_gas(gas.max_priority_fee_gas);

    // ── 7. Sign locally ───────────────────────────────────────────────────
    // build_transaction fills in type + chain_id, sign() runs ECDSA locally.
    // No private key material ever leaves this process.
    let signed_tx = tx
        .build(&*executor.wallet)
        .await?;

    // encoded_2718() = RLP-encoded EIP-2718 envelope — the raw bytes the relay expects
    let raw_bytes = signed_tx.encoded_2718();

    // ── 8. Broadcast to private relay ────────────────────────────────────
    let tx_hash = broadcast_private(raw_bytes).await?;
    Ok(tx_hash)
}

// ── Private relay broadcast ───────────────────────────────────────────────────

/// Sends the signed raw transaction to a private relay via `eth_sendRawTransaction`.
///
/// Private relays that work with Base:
///   • Flashbots Protect  — https://rpc.flashbots.net/fast  (also supports Base)
///   • MEV Blocker        — https://rpc.mevblocker.io
///   • Base Sequencer RPC — op-mainnet private endpoint (contact OP Labs)
///   • BloXroute          — https://mdn.bloxroute.com/...  (paid tier required)
async fn broadcast_private(
    raw_tx: Vec<u8>,
) -> Result<alloy::primitives::TxHash> {
    // Set via environment variable — never hard-code relay credentials in source.
    let relay_url = std::env::var("PRIVATE_RELAY_URL")
        .unwrap_or_else(|_| "https://rpc.flashbots.net/fast".into());

    // Standard JSON-RPC envelope
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id":      1,
        "method":  "eth_sendRawTransaction",
        "params":  [ format!("0x{}", hex::encode(&raw_tx)) ]
    });

    let client   = reqwest::Client::new();
    let response = client
        .post(&relay_url)
        .header("Content-Type", "application/json")
        // Flashbots Protect accepts an optional refund address header
        // .header("X-Flashbots-Identity", "0xYOUR_ADDRESS")
        .json(&body)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    if let Some(err) = response.get("error") {
        eyre::bail!("relay rejected tx: {err}");
    }

    let hash_str = response["result"]
        .as_str()
        .ok_or_else(|| eyre::eyre!("relay returned no tx hash: {response}"))?;

    Ok(hash_str.parse()?)
}

// ── ABI encoding helper ───────────────────────────────────────────────────────

fn encode_tri_arb(
    pool_a: Address, pool_b: Address, pool_c: Address,
    amount_in:     U256,
    zfo_a: bool,   zfo_b: bool,   zfo_c: bool,
    min_profit:    U256,
) -> Bytes {
    Bytes::from(
        MevArbitrage::executeTriArbCall {
            poolA:       pool_a,
            poolB:       pool_b,
            poolC:       pool_c,
            amountIn:    amount_in,
            zeroForOneA: zfo_a,
            zeroForOneB: zfo_b,
            zeroForOneC: zfo_c,
            minProfit:   min_profit,
        }
        .abi_encode(),
    )
}