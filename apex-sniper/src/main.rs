mod local_state;
mod ws_tuned;

use alloy::{
    network::TransactionBuilder, // <-- FIX 2: Added the missing trait
    primitives::{address, Address, Bytes, U256},
    providers::{Provider, ProviderBuilder, RootProvider, WsConnect},
    transports::http::{Client, Http},
    rpc::types::eth::TransactionRequest,
    sol,
    sol_types::SolCall,
};
use futures_util::StreamExt;
use std::sync::Arc;
use eyre::Result;

sol! {
    interface ITriArb {
        function executeTriArb(
            address tokenIn,
            address tokenOut,
            uint256 amountIn,
            uint256 minProfitOut, 
            address poolA,
            address poolB
        ) external;
    }
}

const IS_SIMULATION: bool = false; 
const MIN_PROFIT_WEI: u128 = 5_000_000_000_000_000; // 0.005 ETH

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    
    // --- FIX 1: Parse addresses at runtime to prevent macro panics from invisible characters ---
    let mev_contract: Address = "0x654d4AcE007F76986B59baE921457c86db27184A".parse().expect("Invalid Contract Address");
    let my_wallet: Address = "0x14954a074cE69096937E7a30B956550787674796".parse().expect("Invalid Wallet Address");

    let alchemy_ws = std::env::var("ALCHEMY_WS_URL").expect("Missing ALCHEMY_WS_URL");
    let amm_state = Arc::new(local_state::LocalAmmState::new());

    let rpc_endpoints = vec![
        std::env::var("ALCHEMY_HTTP_URL").unwrap_or_default(),
        "https://mainnet.base.org".to_string(),
        "https://base.publicnode.com".to_string(),
        "https://base.meowrpc.com".to_string(),
    ];

    let mut http_provider = None;
    for url in rpc_endpoints {
        if url.is_empty() { continue; }
        if let Ok(url_parsed) = url.parse() {
            // --- FIX 3: Removed `if let Ok` because on_http returns the builder directly ---
            let builder = ProviderBuilder::new().on_http(url_parsed);
            println!("🔗 Connected to HTTP RPC: {}", url);
            http_provider = Some(Arc::new(builder));
            break;
        }
    }
    let http_provider = http_provider.expect("FATAL: All HTTP RPC endpoints failed.");

    let pools = vec![
        address!("d0b53D9277642d899DF5C87A3966A349A798F224"), // UniV3 0.05%
        address!("4c36388be6f416a29c8d8eee81c771ce6be14b18"), // Pancake V3
        address!("b2cc224c1c9feE385f8ad6a55b4d94E92359DC59"), // Aero Slipstream
        address!("2236E0D251786596A95B6A88E125D4F9496f88A4"), // Sushi V3
        address!("319E0d977461E8D28842240905D9183416F669B0"), // Velocimeter
        address!("57713f7716E0B0f65ec116912F834E49805480d2"), // Extra V3
        address!("88960139151F5F493208E776F6D4e58A8911C289"), // BaseSwap
        address!("673010372863B8F8073570F284799014603B5299"), // AlienBase
        address!("9E973a4b706c99c687f65f7c355f32E3036F6E54"), // SwapBased
        address!("8ad599c3A0ff1De082011EFDDc58f1908eb6e6D5"), // UniV3 0.3%
        address!("4e68Ccd3E89f51C3074ca5072bbAC773960dFa36"), // UniV3 1%
        address!("17E298066f1e5df13364fF16bcC39dFCA5e73004"), // Aero Slipstream
        address!("1c296541f9EAc3255D50041aBA92A1e2A7c1e54F"), // UniV3
        address!("B0f62d100778747BE3D4EEA0a4a625Fdd94df517"), // Pancake V3
        address!("22244f777E5f4035Ebbd8fC820f4B0aBB26ceA1E"), // Aero Slipstream 2
        address!("0B25c51637c43de87BCA202B196C9c02DE85B171"), // Aero Slipstream
        address!("40C3335Fdb7643b1790dB8799a4c8fcf4bf6f68b"), // UniV3
        address!("36f01837F80a060eCcDbccf4153C148F0D01E171"), // Pancake V3
        address!("B2534f31E7F19343351CfaF92632b7188bDE4e19"), // Aero Slipstream
        address!("2a12B4dD1A105B8C0D4b50AA26284699DEfc1A75"), // UniV3
        address!("c9034c3E7F5802316547CEfb1EEcEda120D23F8a"), // Aero Slipstream
        address!("c19669A405067927865B40Ea045a2baabbbe572e"), // UniV3
        address!("52A7b2CA4fFCAE3d23315B22dBfA37c6abD3B4e3"), // Aero Slipstream
        address!("e62C619c62Cba74558235Ff09581971485cEb7d3"), // UniV3
        address!("11bc894569f202a0a2569806f71d53205b382903"), // Aero Slipstream
        address!("bB1De0b1F680B1A7E141B4e98f065365eCFCdD5B"), // UniV3
        address!("0c4d7A8F53fEd508aBDcbe1C00f5C372F245DF55"), // Pancake V3
        address!("28A944a6016aE886C8fE28A3bFf1F5B4B18302f2"), // Aero Slipstream
        address!("1E5Ca0BF4786B6fD1d4B5d9F81e64146f7B3f8e6"), // UniV3
        address!("7739506D67ca71E368aB15a77f1e73715c0a3739"), // UniV3
        address!("257D4A47c87cAE8f22030559fF37D3b5b5C320db"), // Aero Slipstream
        address!("0AD08370c76Ff426F534bb2AFFD9b5555338ee68"), // Aero
        address!("9cabe00d0325ff1e8bae816ae18632c1c987582b"), // UniV3
        address!("f5601f95708256a118ef5971820327f362442d2d"), // Aero
        address!("5b6cc3e78525fdc2d97c9b011f12ab886d57cc26"), // UniV3
        address!("6ad654ac2872b92ff88298a1ea67e1ace92fe6fe"), // Aero
        address!("92f9ad9d4290189787246a25586aa17c98fa19a2"), // UniV3
        address!("60c8c29ff62edf41d419ed8c413398426830ce4c"), // Aero
        address!("aef57fe961cc9d014114204d44e6018c4b83c256"), // UniV3
        address!("510b2d8e30e8c79247c51e04d5be8bf7262f9938"), // Aero
        address!("eC8E5342B19977B4eF8892e02D8DAEcfa1315831"), // UniV3
    ];

    amm_state.seed_with_retry(&http_provider, pools.clone()).await?;

    let ws_provider = ProviderBuilder::new().on_ws(WsConnect::new(&alchemy_ws)).await?;
    let subscription = Provider::subscribe_blocks(&ws_provider).await?;
    let mut block_stream = subscription.into_stream();

    let ws_logs = ws_tuned::connect_tuned_ws(&alchemy_ws).await?;
    let mut logs_stream = ws_tuned::subscribe_tuned_swap_logs(ws_logs, pools.clone()).await?;

    println!("🚀 APEX SNIPER LIVE | SIMULATION: {} | POOLS: {}", IS_SIMULATION, pools.len());
    println!("🛡️ CONTRACT LOADED: {}", mev_contract);

    let state_handle = Arc::clone(&amm_state);
    tokio::spawn(async move {
        while let Some(log) = logs_stream.next().await {
            state_handle.handle_swap_log(&log);
        }
    });

    while let Some(block) = block_stream.next().await {
        let block_number = block.header.number.unwrap_or_default();
        
        if block_number % 10 == 0 {
            println!("🔄 [SYNC] Block {} - Forcing hard state resync...", block_number);
            let _ = amm_state.seed_with_retry(&http_provider, pools.clone()).await;
        }

        let state = Arc::clone(&amm_state);
        let pool_list = pools.clone();
        let provider_clone = Arc::clone(&http_provider); 
        let mev_clone = mev_contract;
        let wallet_clone = my_wallet;
        
        tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            let _ = evaluate_simulation(block_number, state, pool_list, provider_clone, mev_clone, wallet_clone).await;
            println!("⚡ Block {} evaluated in {:.2}ms", block_number, t0.elapsed().as_secs_f64() * 1000.0);
        });
    }
    Ok(())
}

async fn evaluate_simulation(
    block_number: u64, 
    state: Arc<local_state::LocalAmmState>, 
    pools: Vec<Address>,
    provider: Arc<RootProvider<Http<Client>>>,
    mev_contract: Address,
    my_wallet: Address
) -> Result<()> {
    for i in 0..pools.len() {
        for j in i+1..pools.len() {
            let (p_a, l_a, t0_a, t1_a) = match state.get_reserves(&pools[i]) { Some(res) => res, None => continue };
            let (p_b, l_b, t0_b, t1_b) = match state.get_reserves(&pools[j]) { Some(res) => res, None => continue };

            if t0_a != t0_b || t1_a != t1_b { continue; }

            let (pc, pd, lc, ld) = if p_a > p_b { (p_b, p_a, l_b, l_a) } else { (p_a, p_b, l_a, l_b) };
            
            let mut best_p = U256::ZERO;
            let mut low = U256::from(1_000_000u128);
            let mut high = U256::from(5_000_000_000u128); 
            let mut optimal_trade_size = U256::ZERO;

            for _ in 0..12 {
                let mid = (low + high) / U256::from(2);
                let profit = calculate_v3_arb_profit(mid, pc, pd, lc, ld);
                if profit > best_p { 
                    best_p = profit; 
                    optimal_trade_size = mid;
                    low = mid + U256::from(1); 
                } else { 
                    high = mid - U256::from(1); 
                }
            }

            if best_p > U256::from(MIN_PROFIT_WEI) {
                let profit_eth = best_p.to_string().parse::<f64>().unwrap_or(0.0) / 1e18;
                
                let call = ITriArb::executeTriArbCall {
                    tokenIn: t0_a,
                    tokenOut: t1_a,
                    amountIn: optimal_trade_size,
                    minProfitOut: best_p,
                    poolA: pools[i],
                    poolB: pools[j],
                };
                let calldata: Bytes = call.abi_encode().into();

                let tx_request = TransactionRequest::default()
                    .to(mev_contract)
                    .from(my_wallet)
                    .with_input(calldata.clone());

                match provider.estimate_gas(&tx_request).await {
                    Ok(estimated_gas) => {
                        let gas_limit: u64 = estimated_gas as u64; 

                        let bribe = best_p * U256::from(3) / U256::from(10); 
                        let bribe_u128: u128 = bribe.try_into().unwrap_or(u128::MAX);
                        let mut max_priority_fee_per_gas: u128 = bribe_u128 / (gas_limit as u128);
                        
                        if max_priority_fee_per_gas > 100_000_000_000 {
                            max_priority_fee_per_gas = 100_000_000_000;
                        }

                        let msg = format!(
                            "🟢 [SHIELD PASSED] PROFIT: {:.4} ETH | Block: {} \n🛣️ Route: {} -> {} \n⛽ Gas Limit: {} \n🛡️ Bribe: {} Gwei", 
                            profit_eth, block_number, pools[i], pools[j], gas_limit, (max_priority_fee_per_gas / 1_000_000_000)
                        );
                        println!("{}", msg);
                        send_telegram_alert(msg);
                    },
                    Err(_) => {
                        println!("🔴 [SHIELD] Revert prevented on block {}. Math failed on-chain. Saved $0.02.", block_number);
                    }
                }
            }
        }
    }
    Ok(())
}

fn calculate_v3_arb_profit(input: U256, pc: U256, pd: U256, lc: u128, ld: u128) -> U256 {
    if lc == 0 || ld == 0 { return U256::ZERO; }
    let (lc_u, ld_u) = (U256::from(lc), U256::from(ld));
    let next_pc = pc + (input << 96) / lc_u;
    let weth = ((lc_u << 96) / pc) - ((lc_u << 96) / next_pc);
    let p_dear_shift = (weth * pd) / (ld_u << 96);
    if pd <= p_dear_shift { return U256::ZERO; }
    let out = (ld_u * (pd - (pd - p_dear_shift))) >> 96;
    let gas_fee = U256::from(150_000_000_000_000u128); 
    if out > (input + gas_fee) { out - input - gas_fee } else { U256::ZERO }
}

fn send_telegram_alert(message: String) {
    tokio::spawn(async move {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
        if token.is_empty() || chat_id.is_empty() { return; }
        let _ = reqwest::Client::new().post(format!("https://api.telegram.org/bot{}/sendMessage", token))
            .form(&[("chat_id", chat_id), ("text", message)]).send().await;
    });
}