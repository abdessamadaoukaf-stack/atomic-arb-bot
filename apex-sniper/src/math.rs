use alloy::primitives::U256;

/// Q96 fixed-point denominator used by Uniswap V3
const Q96: u128 = 1u128 << 96;

/// Decode the human-readable price (token1 per token0) from a V3 slot0 value.
///
/// Formula: price = (sqrtPriceX96 / 2^96)^2
/// Returns price scaled by 1e18 (i.e. multiply result by 1 to get 18-dec units).
pub fn price_from_sqrt(sqrt_price_x96: U256) -> f64 {
    // Cast to u128 — sqrtPriceX96 fits in ~160 bits for realistic prices,
    // but for ultra-low-price pairs use full U256 division below.
    let sqrt = sqrt_price_x96.as_limbs()[0] as f64  // lo 64 bits
        + sqrt_price_x96.as_limbs()[1] as f64 * (u64::MAX as f64 + 1.0); // hi 64 bits

    let q = Q96 as f64;
    (sqrt / q).powi(2)
}

/// Estimate the maximum input amount that can be absorbed by a V3 pool
/// given its current liquidity (used for sizing the arb bundle).
///
/// Δsqrt = Δx * sqrtP / (L + Δx * sqrtP)  — simplified for small trades
/// Here we return the liquidity-based cap as a U256 wei amount.
pub fn max_amount_in(liquidity: u128, sqrt_price_x96: U256, token0_decimals: u8) -> U256 {
    // Conservative: cap at 1% of the virtual reserve depth to stay within
    // the current tick and avoid multi-tick math.
    let q96 = U256::from(Q96);
    let liq  = U256::from(liquidity);

    // virtual reserve0 = L / sqrtP  (in raw units)
    let virtual_reserve0 = liq
        .checked_mul(q96)
        .unwrap_or(U256::MAX)
        .checked_div(sqrt_price_x96)
        .unwrap_or(U256::ZERO);

    // Use 1% of virtual reserve as max input
    virtual_reserve0 / U256::from(100u64)
}

/// Compute the cross-pool arb profit estimate.
/// Returns (amount_in, expected_profit_wei) or None if not profitable.
pub fn estimate_arb(
    sqrt_a: U256, liq_a: u128,   // pool A: we buy token1 here (lower price)
    sqrt_b: U256, _liq_b: u128,  // pool B: we sell token1 here (higher price)
    sqrt_c: U256, _liq_c: u128,  // pool C: rebalance leg
    amount_in: U256,
    gas_cost_wei: U256,           // max_fee * gas_limit, used as profitability floor
) -> Option<(U256, U256)> {
    let price_a = price_from_sqrt(sqrt_a);
    let price_b = price_from_sqrt(sqrt_b);
    let price_c = price_from_sqrt(sqrt_c);

    // Simplified 3-hop check: buy on A, sell on B, rebalance on C
    // A real implementation uses exact constant-product output math per hop
    let spread = price_b - price_a;
    if spread <= 0.0 {
        return None;
    }

    let gross_profit_f64 = spread * amount_in.to::<u128>() as f64 / price_a;
    let gross_profit = U256::from(gross_profit_f64 as u128);

    if gross_profit <= gas_cost_wei {
        tracing::debug!(
            gross = %gross_profit,
            gas   = %gas_cost_wei,
            "below gas threshold — skipping"
        );
        return None;
    }

    let _ = price_c; // rebalance leg: use in full impl
    Some((amount_in, gross_profit - gas_cost_wei))
}