use alloy::{
    network::TransactionBuilder,
    primitives::{Address, Bytes, U256},
    providers::{Provider, RootProvider},
    transports::http::{Client, Http},
    rpc::types::{Log, TransactionRequest},
    sol,
    sol_types::SolCall,
};
use dashmap::DashMap;
use eyre::Result;
use std::sync::Arc;
use std::hash::BuildHasherDefault;
use ahash::AHasher;

sol! {
    interface IUniswapV3Pool {
        event Swap(address indexed sender, address indexed recipient, int256 amount0, int256 amount1, uint160 sqrtPriceX96, uint128 liquidity, int24 tick);
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        function liquidity() external view returns (uint128);
        function token0() external view returns (address);
        function token1() external view returns (address);
    }
}

sol! {
    interface IAerodromePool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, bool unlocked);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PoolType { UniswapV3, Aerodrome }

#[derive(Debug, Clone)]
pub struct PoolState {
    pub price: U256,
    pub liquidity: u128,
    pub pool_type: PoolType,
    pub token0: Address,
    pub token1: Address,
}

pub type FastMap = DashMap<Address, PoolState, BuildHasherDefault<AHasher>>;

#[derive(Clone)]
pub struct LocalAmmState {
    pub reserves: Arc<FastMap>,
}

impl LocalAmmState {
    pub fn new() -> Self {
        Self { reserves: Arc::new(DashMap::with_hasher(BuildHasherDefault::<AHasher>::default())) }
    }

    pub async fn seed_with_retry(
        &self,
        provider: &RootProvider<Http<Client>>, 
        pool_addresses: Vec<Address>,
    ) -> Result<()> {
        println!("🌱 Seeding {} pools and fetching tokens...", pool_addresses.len());
        for addr in pool_addresses {
            let slot0_cd = Bytes::from(IUniswapV3Pool::slot0Call {}.abi_encode());
            let liq_cd   = Bytes::from(IUniswapV3Pool::liquidityCall {}.abi_encode());
            let t0_cd    = Bytes::from(IUniswapV3Pool::token0Call {}.abi_encode());
            let t1_cd    = Bytes::from(IUniswapV3Pool::token1Call {}.abi_encode());

            let slot0_req = TransactionRequest::default().to(addr).with_input(slot0_cd);
            let liq_req   = TransactionRequest::default().to(addr).with_input(liq_cd);
            let t0_req    = TransactionRequest::default().to(addr).with_input(t0_cd);
            let t1_req    = TransactionRequest::default().to(addr).with_input(t1_cd);

            let s0_res  = provider.call(&slot0_req).await;
            let liq_res = provider.call(&liq_req).await;
            let t0_res  = provider.call(&t0_req).await;
            let t1_res  = provider.call(&t1_req).await;

            if let (Ok(s0_raw), Ok(liq_raw), Ok(t0_raw), Ok(t1_raw)) = (s0_res, liq_res, t0_res, t1_res) {
                let mut price = U256::ZERO;
                let mut p_type = PoolType::UniswapV3;

                if let Ok(d) = IUniswapV3Pool::slot0Call::abi_decode_returns(s0_raw.as_ref(), true) {
                    price = U256::from(d.sqrtPriceX96);
                } else if let Ok(d) = IAerodromePool::slot0Call::abi_decode_returns(s0_raw.as_ref(), true) {
                    price = U256::from(d.sqrtPriceX96);
                    p_type = PoolType::Aerodrome;
                }

                if let (Ok(l_dec), Ok(t0_dec), Ok(t1_dec)) = (
                    IUniswapV3Pool::liquidityCall::abi_decode_returns(liq_raw.as_ref(), true),
                    IUniswapV3Pool::token0Call::abi_decode_returns(t0_raw.as_ref(), true),
                    IUniswapV3Pool::token1Call::abi_decode_returns(t1_raw.as_ref(), true)
                ) {
                    self.reserves.insert(addr, PoolState { 
                        price, 
                        liquidity: l_dec._0, 
                        pool_type: p_type,
                        token0: t0_dec._0,
                        token1: t1_dec._0
                    });
                }
            }
        }
        println!("✅ Seeding Complete.");
        Ok(())
    }

    pub fn handle_swap_log(&self, log: &Log) {
        if let Ok(decoded) = log.log_decode::<IUniswapV3Pool::Swap>() {
            if let Some(mut entry) = self.reserves.get_mut(&log.address()) {
                entry.price = U256::from(decoded.inner.data.sqrtPriceX96);
                entry.liquidity = decoded.inner.data.liquidity;
            }
        }
    }

    // Now returns the tokens alongside price and liquidity
    pub fn get_reserves(&self, address: &Address) -> Option<(U256, u128, Address, Address)> {
        self.reserves.get(address).map(|r| (r.price, r.liquidity, r.token0, r.token1))
    }
}