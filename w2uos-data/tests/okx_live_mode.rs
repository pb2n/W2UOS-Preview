use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::time::timeout;
use w2uos_bus::MessageBus;
use w2uos_data::{okx::OkxMarketDataSource, ExchangeId, MarketSnapshot};

#[tokio::test]
async fn okx_live_mode_publishes_snapshot_to_bus() {
    let bus: Arc<dyn MessageBus> = Arc::new(w2uos_bus::LocalBus::new());
    let mut sub = bus
        .subscribe("market.OKX.BTCUSDT")
        .await
        .expect("subscribe");

    let source = OkxMarketDataSource::new(
        Arc::clone(&bus),
        "wss://example.okx.endpoint".to_string(),
        vec!["BTC-USDT".to_string()],
        None,
    );

    let mut state = HashMap::new();
    let mut initial_logs = 0usize;
    let payload = r#"{"arg":{"channel":"tickers","instId":"BTC-USDT"},"data":[{"instType":"SPOT","instId":"BTC-USDT","last":"29123.5","lastSz":"0.001","askPx":"29124","askSz":"0.5","bidPx":"29123","bidSz":"0.5","open24h":"30000","high24h":"31000","low24h":"28000","volCcy24h":"123","vol24h":"456","ts":"1700000000000","sodUtc0":"0","sodUtc8":"0"}]}"#;

    source
        .handle_text(payload, &mut state, &mut initial_logs)
        .await
        .expect("handle text");

    let msg = timeout(Duration::from_secs(1), sub.receiver.recv())
        .await
        .expect("message available")
        .expect("snapshot payload");

    let snapshot: MarketSnapshot = serde_json::from_slice(&msg.0).expect("decode snapshot");
    assert_eq!(snapshot.exchange, ExchangeId::Okx);
    assert_eq!(snapshot.symbol.base, "BTC");
    assert_eq!(snapshot.symbol.quote, "USDT");
    assert!((snapshot.last - 29_123.5).abs() < f64::EPSILON);
    assert!((snapshot.bid - 29_123.0).abs() < f64::EPSILON);
    assert!((snapshot.ask - 29_124.0).abs() < f64::EPSILON);
    assert!((snapshot.volume_24h - 456.0).abs() < f64::EPSILON);
}
