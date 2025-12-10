use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_log::{log_event_via_bus, LogEvent, LogLevel, LogSource};
use w2uos_service::TraceId;

use crate::service::MarketDataSubscription;
use crate::types::{ExchangeId, MarketSnapshot, Symbol};

#[derive(Clone)]
pub struct BinanceMarketStream {
    pub bus: Arc<dyn MessageBus>,
    pub ws_url: String,
    pub subscriptions: Vec<MarketDataSubscription>,
    pub history_tx: Option<tokio::sync::mpsc::Sender<MarketSnapshot>>,
}

impl fmt::Debug for BinanceMarketStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BinanceMarketStream")
            .field("ws_url", &self.ws_url)
            .field("subscriptions", &self.subscriptions)
            .finish()
    }
}

impl BinanceMarketStream {
    pub async fn run(&self) -> Result<()> {
        info!(url = %self.ws_url, "connecting Binance spot stream");
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let mut params = Vec::new();
        let mut symbol_map: HashMap<String, Symbol> = HashMap::new();
        for sub in &self.subscriptions {
            let symbol = format!("{}{}", sub.symbol.base, sub.symbol.quote).to_lowercase();
            params.push(format!("{}@ticker", symbol));
            symbol_map.insert(symbol.to_uppercase(), sub.symbol.clone());
        }

        let subscribe_msg = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": params,
            "id": 1,
        });

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .context("send binance subscribe")?;

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(txt) => {
                    if let Err(err) = self.handle_text(&txt, &symbol_map).await {
                        warn!(?err, "failed to process Binance message");
                    }
                }
                Message::Ping(payload) => {
                    write.send(Message::Pong(payload)).await.ok();
                }
                Message::Close(frame) => {
                    warn!(?frame, "Binance stream closed");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_text(&self, txt: &str, symbol_map: &HashMap<String, Symbol>) -> Result<()> {
        if txt.contains("result") {
            return Ok(());
        }

        let ticker: BinanceTicker = serde_json::from_str(txt).context("parse binance ticker")?;
        let symbol_key = ticker.symbol.to_uppercase();
        let Some(symbol) = symbol_map.get(&symbol_key) else {
            return Ok(());
        };

        let trace_id = Some(TraceId::new());
        let snapshot = MarketSnapshot {
            ts: chrono::Utc::now(),
            exchange: ExchangeId::Binance,
            symbol: symbol.clone(),
            last: ticker.last_price,
            bid: ticker.best_bid,
            ask: ticker.best_ask,
            volume_24h: ticker.volume,
            trace_id: trace_id.clone(),
        };

        let subject = format!(
            "market.{}.{}{}",
            snapshot.exchange,
            snapshot.symbol.base.to_uppercase(),
            snapshot.symbol.quote.to_uppercase()
        );

        if let Ok(payload) = serde_json::to_vec(&snapshot) {
            if let Err(err) = self.bus.publish(&subject, BusMessage(payload)).await {
                error!(?err, "failed to publish Binance snapshot");
            }
        }

        if let Some(tx) = self.history_tx.as_ref() {
            if let Err(err) = tx.try_send(snapshot.clone()) {
                warn!(?err, "failed to enqueue binance snapshot for history");
            }
        }

        let evt = LogEvent {
            ts: snapshot.ts,
            level: LogLevel::Info,
            source: LogSource::MarketData,
            message: "binance market tick".to_string(),
            fields: serde_json::json!({"subject": subject}),
            correlation_id: None,
            trace_id,
        };
        let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinanceTicker {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub last_price: f64,
    #[serde(rename = "b")]
    pub best_bid: f64,
    #[serde(rename = "a")]
    pub best_ask: f64,
    #[serde(rename = "v")]
    pub volume: f64,
}
