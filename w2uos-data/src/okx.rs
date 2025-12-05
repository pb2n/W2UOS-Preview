use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_log::{log_event_via_bus, LogEvent, LogLevel, LogSource};
use w2uos_service::TraceId;

use crate::service::MarketDataSubscription;
use crate::types::{ExchangeId, MarketSnapshot, Symbol};

pub struct OkxMarketStream {
    pub bus: Arc<dyn MessageBus>,
    pub ws_url: String,
    pub subscriptions: Vec<MarketDataSubscription>,
    pub history_tx: Option<tokio::sync::mpsc::Sender<MarketSnapshot>>,
}

impl OkxMarketStream {
    pub async fn run(&self) -> Result<()> {
        info!(url = %self.ws_url, "connecting OKX public stream");
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let args: Vec<_> = self
            .subscriptions
            .iter()
            .map(|sub| {
                serde_json::json!({
                    "channel": "tickers",
                    "instId": format!("{}-{}", sub.symbol.base.to_uppercase(), sub.symbol.quote.to_uppercase())
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
                    warn!(?frame, "OKX stream closed");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_text(&self, txt: &str) -> Result<()> {
        if txt.contains("event") {
            debug!(payload = %txt, "OKX event message");
            return Ok(());
        }

        let envelope: OkxTickerEnvelope =
            serde_json::from_str(txt).context("parse okx ticker envelope")?;

        for ticker in envelope.data {
            let symbol = Self::parse_symbol(&ticker.inst_id)?;
            let trace_id = Some(TraceId::new());
            let snapshot = MarketSnapshot {
                ts: chrono::Utc::now(),
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

    fn parse_symbol(inst_id: &str) -> Result<Symbol> {
        let mut parts = inst_id.split('-');
        let base = parts
            .next()
            .ok_or_else(|| anyhow!("missing base in instId"))?
            .to_string();
        let quote = parts
            .next()
            .ok_or_else(|| anyhow!("missing quote in instId"))?
            .to_string();
        Ok(Symbol { base, quote })
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_okx_ticker_message() {
        let payload = r#"{"arg":{"channel":"tickers","instId":"BTC-USDT"},"data":[{"instType":"SPOT","instId":"BTC-USDT","last":"29123.5","lastSz":"0.001","askPx":"29124","askSz":"0.5","bidPx":"29123","bidSz":"0.5","open24h":"30000","high24h":"31000","low24h":"28000","volCcy24h":"123","vol24h":"456","ts":"1700000000000","sodUtc0":"0","sodUtc8":"0"}]}"#;

        let stream = OkxMarketStream {
            bus: Arc::new(w2uos_bus::LocalBus::new()),
            ws_url: "".to_string(),
            subscriptions: vec![],
            history_tx: None,
        };

        stream.handle_text(payload).await.unwrap();
    }
}
