use alloy::{
    primitives::U256,
    providers::Provider,
};

/// All EIP-1559 fields needed for a TransactionRequest.
#[derive(Debug, Clone)]
pub struct GasParams {
    /// max_fee_per_gas = base_fee * multiplier + priority_fee
    pub max_fee_per_gas:      u128,
    /// tip paid to the validator — keep low on Base (L2)
    pub max_priority_fee_gas: u128,
    /// conservative gas limit for a 3-hop V3 arb
    pub gas_limit:            u64,
}

/// Fetch the *next* block base fee and scale it.
///
/// `urgency` controls the base-fee multiplier:
///   1.10 = normal   (next block inclusion likely)
///   1.25 = elevated (high mempool congestion)
///   1.50 = critical (must land next block or opportunity is gone)
pub async fn compute_gas_params<P: Provider>(
    provider: &P,
    urgency:  f64,        // e.g. 1.25
    tip_gwei: f64,        // e.g. 0.005 on Base
) -> eyre::Result<GasParams> {
    let block = provider
        .get_block_by_number(alloy::eips::BlockNumberOrTag::Latest, false.into())
        .await?
        .ok_or_else(|| eyre::eyre!("latest block missing"))?;

    // base_fee is the *current* block's fee; next block will be ±12.5% of this.
    // Scaling by `urgency` buys headroom against that variance.
    let base_fee = block
        .header
        .base_fee_per_gas
        .ok_or_else(|| eyre::eyre!("pre-EIP-1559 block — wrong network?"))?;

    let priority_fee = (tip_gwei * 1e9) as u128;  // gwei → wei

    // Scale base_fee using fixed-point arithmetic to avoid f64 precision loss
    // on large values. Multiply then divide to stay in u128.
    let urgency_bp = (urgency * 10_000.0) as u128;  // e.g. 1.25 → 12_500
    let scaled_base = (base_fee as u128)
        .checked_mul(urgency_bp)
        .ok_or_else(|| eyre::eyre!("base_fee overflow"))?
        / 10_000;

    Ok(GasParams {
        max_fee_per_gas:      scaled_base + priority_fee,
        max_priority_fee_gas: priority_fee,
        // Three V3 swaps ≈ 150k–180k gas each; 600k gives ~10% headroom.
        // Profile your contract and tighten this — unused gas is refunded but
        // over-estimating makes your bundle look expensive to validators.
        gas_limit: 600_000,
    })
}