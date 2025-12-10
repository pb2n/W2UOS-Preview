//! OKX live market data source used in LIVE-3. Connects to the public websocket,
//! subscribes to ticker updates, and emits normalized `MarketSnapshot` events into
//! the internal bus while persisting to the optional history sink.

use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::{sync::mpsc::Sender, time::sleep};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_log::{log_event_via_bus, LogEvent, LogLevel, LogSource};
use w2uos_service::TraceId;

use crate::types::{ExchangeId, MarketSnapshot, Symbol};

pub struct OkxMarketDataSource {
    pub bus: Arc<dyn MessageBus>,
    pub ws_url: String,
    pub instruments: Vec<String>,
    pub history_tx: Option<Sender<MarketSnapshot>>,
}

impl OkxMarketDataSource {
    pub fn new(
        bus: Arc<dyn MessageBus>,
        ws_url: String,
        instruments: Vec<String>,
        history_tx: Option<Sender<MarketSnapshot>>,
    ) -> Self {
        Self {
            bus,
            ws_url,
            instruments,
            history_tx,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.stream_once().await {
                Ok(_) => {
                    backoff = Duration::from_secs(1);
                }
                Err(err) => {
                    warn!(?err, "OKX market stream error; reconnecting");
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    async fn stream_once(&self) -> Result<()> {
        info!(url = %self.ws_url, instruments = ?self.instruments, "connecting OKX public stream");
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let args: Vec<_> = self
            .instruments
            .iter()
            .map(|inst_id| {
                serde_json::json!({
                    "channel": "tickers",
                    "instId": inst_id,
                })
            })
            .collect();

        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args,
        });

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .context("send subscribe")?;
        info!("OKX public stream subscribed");

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(txt) => {
                    if let Err(err) = self.handle_text(&txt).await {
                        warn!(?err, "failed to process OKX message");
                    }
                }
                Message::Ping(payload) => {
                    write.send(Message::Pong(payload)).await.ok();
                }
                Message::Close(frame) => {
                    warn!(?frame, "OKX stream closed by server");
                    return Err(anyhow!("OKX stream closed"));
                }
                _ => {}
            }
        }

        Err(anyhow!("OKX websocket ended"))
    }

    async fn handle_text(&self, txt: &str) -> Result<()> {
        if txt.contains("event") {
            debug!(payload = %txt, "OKX event message");
            return Ok(());
        }

        let envelope: OkxTickerEnvelope =
            serde_json::from_str(txt).context("parse okx ticker envelope")?;

        for ticker in envelope.data {
            let symbol = match instrument_to_symbol(&ticker.inst_id) {
                Some(sym) => sym,
                None => {
                    warn!(inst_id = %ticker.inst_id, "unable to parse OKX instrument id");
                    continue;
                }
            };
            let trace_id = Some(TraceId::new());
            let ts = ticker
                .ts
                .as_deref()
                .and_then(|ts| ts.parse::<i64>().ok())
                .and_then(|millis| chrono::DateTime::from_timestamp_millis(millis))
                .unwrap_or_else(|| chrono::Utc::now());
            let snapshot = MarketSnapshot {
                ts,
                exchange: ExchangeId::Okx,
                symbol: symbol.clone(),
                last: ticker.last.parse::<f64>().unwrap_or_default(),
                bid: ticker.bid_px.parse::<f64>().unwrap_or_default(),
                ask: ticker.ask_px.parse::<f64>().unwrap_or_default(),
                volume_24h: ticker.vol24h.parse::<f64>().unwrap_or_default(),
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
                    error!(?err, "failed to publish OKX snapshot");
                }
            }

            if let Some(tx) = self.history_tx.as_ref() {
                if let Err(err) = tx.try_send(snapshot.clone()) {
                    warn!(?err, "failed to enqueue snapshot for history");
                }
            }

            let evt = LogEvent {
                ts: snapshot.ts,
                level: LogLevel::Info,
                source: LogSource::MarketData,
                message: "okx market tick".to_string(),
                fields: serde_json::json!({"subject": subject}),
                correlation_id: None,
                trace_id,
            };
            let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
        }

        Ok(())
    }
}

/// Convert an OKX instrument identifier (e.g., "BTC-USDT-SWAP") into the internal `Symbol`.
/// Only the base and quote legs are extracted; suffixes like `SWAP` are ignored.
pub fn instrument_to_symbol(inst_id: &str) -> Option<Symbol> {
    let mut parts = inst_id.split('-');
    let base = parts.next()?.to_string();
    let quote = parts.next()?.to_string();
    Some(Symbol { base, quote })
}

#[derive(Debug, Deserialize)]
struct OkxTickerEnvelope {
    #[allow(dead_code)]
    arg: OkxArg,
    data: Vec<OkxTicker>,
}

#[derive(Debug, Deserialize)]
struct OkxArg {
    #[serde(rename = "instId")]
    inst_id: Option<String>,
    #[allow(dead_code)]
    channel: String,
}

#[derive(Debug, Deserialize)]
struct OkxTicker {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(rename = "last")]
    last: String,
    #[serde(rename = "bidPx")]
    bid_px: String,
    #[serde(rename = "askPx")]
    ask_px: String,
    #[serde(rename = "vol24h")]
    vol24h: String,
    #[serde(rename = "ts")]
    ts: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_okx_ticker_message() {
        let payload = r#"{"arg":{"channel":"tickers","instId":"BTC-USDT"},"data":[{"instType":"SPOT","instId":"BTC-USDT","last":"29123.5","lastSz":"0.001","askPx":"29124","askSz":"0.5","bidPx":"29123","bidSz":"0.5","open24h":"30000","high24h":"31000","low24h":"28000","volCcy24h":"123","vol24h":"456","ts":"1700000000000","sodUtc0":"0","sodUtc8":"0"}]}"#;

        let stream = OkxMarketDataSource {
            bus: Arc::new(w2uos_bus::LocalBus::new()),
            ws_url: "".to_string(),
            instruments: vec!["BTC-USDT".to_string()],
            history_tx: None,
        };

        stream.handle_text(payload).await.unwrap();
    }

    #[test]
    fn converts_instrument_to_symbol() {
        let symbol = instrument_to_symbol("BTC-USDT-SWAP").unwrap();
        assert_eq!(symbol.base, "BTC");
        assert_eq!(symbol.quote, "USDT");
    }
}
