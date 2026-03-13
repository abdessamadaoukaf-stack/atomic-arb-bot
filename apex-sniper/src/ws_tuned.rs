use alloy::primitives::{Address, keccak256};
use alloy::rpc::types::{Block, Filter, Log};
use futures_util::{stream::BoxStream, SinkExt, StreamExt};
use tokio_tungstenite::{tungstenite::Message, connect_async_with_config, WebSocketStream};
use tokio::net::TcpStream;
use tungstenite::protocol::WebSocketConfig;
use serde_json::{json, Value};

pub type TunedWsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

pub async fn connect_tuned_ws(alchemy_url: &str) -> eyre::Result<TunedWsStream> {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(64 * 1024 * 1024);
    config.max_frame_size = Some(16 * 1024 * 1024);

    let (ws_stream, _) = connect_async_with_config(
        alchemy_url,
        Some(config),
        true, 
    ).await?;
    Ok(ws_stream)
}

pub async fn subscribe_tuned_blocks(mut ws: TunedWsStream) -> eyre::Result<BoxStream<'static, Block>> {
    let sub_req = json!({"jsonrpc": "2.0", "id": 1, "method": "eth_subscribe", "params": ["newHeads"]});
    ws.send(Message::Text(sub_req.to_string().into())).await?;

    let stream = ws.filter_map(|msg| async move {
        match msg {
            Ok(Message::Text(txt)) => {
                if let Ok(val) = serde_json::from_str::<Value>(&txt.to_string()) {
                    if let Some(result) = val
                        .get("params")
                        .and_then(|p: &Value| p.get("result"))
                        .and_then(|r: &Value| serde_json::from_value::<Block>(r.clone()).ok()) 
                    {
                        return Some(result);
                    }
                }
                None
            }
            _ => None,
        }
    }).boxed(); 
    Ok(stream)
}

pub async fn subscribe_tuned_swap_logs(mut ws: TunedWsStream, pool_addresses: Vec<Address>) -> eyre::Result<BoxStream<'static, Log>> {
    let filter = Filter::new()
        .address(pool_addresses)
        .event_signature(keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)")); 

    let sub_req = json!({"jsonrpc": "2.0", "id": 2, "method": "eth_subscribe", "params": ["logs", filter]});
    ws.send(Message::Text(sub_req.to_string().into())).await?;

    let stream = ws.filter_map(|msg| async move {
        match msg {
            Ok(Message::Text(txt)) => {
                if let Ok(val) = serde_json::from_str::<Value>(&txt.to_string()) {
                    if let Some(result) = val
                        .get("params")
                        .and_then(|p: &Value| p.get("result"))
                        .and_then(|r: &Value| serde_json::from_value::<Log>(r.clone()).ok()) 
                    {
                        return Some(result);
                    }
                }
                None
            }
            _ => None,
        }
    }).boxed();
    Ok(stream)
}