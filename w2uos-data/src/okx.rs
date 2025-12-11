//! OKX live market data source used in LIVE-3. Connects to the public websocket,
//! subscribes to ticker updates, and emits normalized `MarketSnapshot` events into
//! the internal bus while persisting to the optional history sink.

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

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

#[derive(Clone)]
pub struct OkxMarketDataSource {
    pub bus: Arc<dyn MessageBus>,
    pub ws_url: String,
    pub instruments: Vec<String>,
    pub history_tx: Option<Sender<MarketSnapshot>>,
}

impl fmt::Debug for OkxMarketDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OkxMarketDataSource")
            .field("ws_url", &self.ws_url)
            .field("instruments", &self.instruments)
            .finish()
    }
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

        let mut args: Vec<_> = Vec::new();
        for inst_id in &self.instruments {
            args.push(serde_json::json!({
                "channel": "tickers",
                "instId": inst_id,
            }));
            args.push(serde_json::json!({
                "channel": "books5",
                "instId": inst_id,
            }));
            args.push(serde_json::json!({
                "channel": "trades",
                "instId": inst_id,
            }));
        }

        let subscribe_msg = serde_json::json!({
            "op": "subscribe",
            "args": args,
        });

        write
            .send(Message::Text(subscribe_msg.to_string()))
            .await
            .context("send subscribe")?;
        info!("OKX public stream subscribed");

        let mut state: HashMap<String, MarketSnapshot> = HashMap::new();
        let mut initial_logs = 0usize;

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(txt) => {
                    if let Err(err) = self.handle_text(&txt, &mut state, &mut initial_logs).await {
                        warn!(?err, "failed to process OKX message");
                    }
                }
                Message::Binary(bin) => {
                    if let Ok(txt) = String::from_utf8(bin.clone()) {
                        if let Err(err) =
                            self.handle_text(&txt, &mut state, &mut initial_logs).await
                        {
                            warn!(?err, "failed to process OKX binary message");
                        }
                    } else {
                        warn!(?bin, "unrecognized binary frame from OKX");
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

    async fn handle_text(
        &self,
        txt: &str,
        state: &mut HashMap<String, MarketSnapshot>,
        initial_logs: &mut usize,
    ) -> Result<()> {
        if txt.contains("event") {
            debug!(payload = %txt, "OKX event message");
            return Ok(());
        }

        let envelope: serde_json::Value =
            serde_json::from_str(txt).context("parse okx envelope")?;
        let channel = envelope
            .get("arg")
            .and_then(|a| a.get("channel"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        match channel {
            "tickers" => {
                let parsed: OkxEnvelope<OkxTicker> = serde_json::from_value(envelope)?;
                for ticker in parsed.data {
                    if let Some(snapshot) = self.update_snapshot_from_ticker(&ticker, state)? {
                        self.maybe_log_initial(&snapshot, initial_logs);
                        self.emit_snapshot(snapshot).await;
                    }
                }
            }
            "books5" => {
                let parsed: OkxEnvelope<OkxBook> = serde_json::from_value(envelope)?;
                for book in parsed.data {
                    if let Some(snapshot) = self.update_snapshot_from_book(&book, state)? {
                        self.maybe_log_initial(&snapshot, initial_logs);
                        self.emit_snapshot(snapshot).await;
                    }
                }
            }
            "trades" => {
                let parsed: OkxEnvelope<OkxTrade> = serde_json::from_value(envelope)?;
                for trade in parsed.data {
                    if let Some(snapshot) = self.update_snapshot_from_trade(&trade, state)? {
                        self.maybe_log_initial(&snapshot, initial_logs);
                        self.emit_snapshot(snapshot).await;
                    }
                }
            }
            _ => debug!(channel = %channel, "unknown okx channel"),
        }

        Ok(())
    }

    fn update_snapshot_from_ticker(
        &self,
        ticker: &OkxTicker,
        state: &mut HashMap<String, MarketSnapshot>,
    ) -> Result<Option<MarketSnapshot>> {
        let symbol = match instrument_to_symbol(&ticker.inst_id) {
            Some(sym) => sym,
            None => {
                warn!(inst_id = %ticker.inst_id, "unable to parse OKX instrument id");
                return Ok(None);
            }
        };
        let ts = ticker
            .ts
            .as_deref()
            .and_then(|ts| ts.parse::<i64>().ok())
            .and_then(|millis| chrono::DateTime::from_timestamp_millis(millis))
            .unwrap_or_else(|| chrono::Utc::now());
        let key = ticker.inst_id.clone();
        let snap = state.entry(key.clone()).or_insert_with(|| MarketSnapshot {
            ts,
            exchange: ExchangeId::Okx,
            symbol: symbol.clone(),
            last: 0.0,
            bid: 0.0,
            ask: 0.0,
            volume_24h: 0.0,
            trace_id: None,
        });

        snap.ts = ts;
        snap.symbol = symbol;
        snap.last = ticker.last.parse::<f64>().unwrap_or(snap.last);
        snap.bid = ticker.bid_px.parse::<f64>().unwrap_or(snap.bid);
        snap.ask = ticker.ask_px.parse::<f64>().unwrap_or(snap.ask);
        snap.volume_24h = ticker.vol24h.parse::<f64>().unwrap_or(snap.volume_24h);
        Ok(Some(snap.clone()))
    }

    fn update_snapshot_from_book(
        &self,
        book: &OkxBook,
        state: &mut HashMap<String, MarketSnapshot>,
    ) -> Result<Option<MarketSnapshot>> {
        let symbol = match instrument_to_symbol(&book.inst_id) {
            Some(sym) => sym,
            None => {
                warn!(inst_id = %book.inst_id, "unable to parse OKX instrument id");
                return Ok(None);
            }
        };
        let key = book.inst_id.clone();
        let snap = state.entry(key.clone()).or_insert_with(|| MarketSnapshot {
            ts: chrono::Utc::now(),
            exchange: ExchangeId::Okx,
            symbol: symbol.clone(),
            last: 0.0,
            bid: 0.0,
            ask: 0.0,
            volume_24h: 0.0,
            trace_id: None,
        });

        snap.symbol = symbol;
        snap.ts = chrono::Utc::now();
        if let Some(bid) = book.bids.first() {
            snap.bid = bid
                .get(0)
                .and_then(|p| p.parse::<f64>().ok())
                .unwrap_or(snap.bid);
        }
        if let Some(ask) = book.asks.first() {
            snap.ask = ask
                .get(0)
                .and_then(|p| p.parse::<f64>().ok())
                .unwrap_or(snap.ask);
        }

        Ok(Some(snap.clone()))
    }

    fn update_snapshot_from_trade(
        &self,
        trade: &OkxTrade,
        state: &mut HashMap<String, MarketSnapshot>,
    ) -> Result<Option<MarketSnapshot>> {
        let symbol = match instrument_to_symbol(&trade.inst_id) {
            Some(sym) => sym,
            None => {
                warn!(inst_id = %trade.inst_id, "unable to parse OKX instrument id");
                return Ok(None);
            }
        };
        let key = trade.inst_id.clone();
        let snap = state.entry(key.clone()).or_insert_with(|| MarketSnapshot {
            ts: chrono::Utc::now(),
            exchange: ExchangeId::Okx,
            symbol: symbol.clone(),
            last: 0.0,
            bid: 0.0,
            ask: 0.0,
            volume_24h: 0.0,
            trace_id: None,
        });

        snap.symbol = symbol;
        snap.ts = chrono::Utc::now();
        snap.last = trade.last_px.parse::<f64>().unwrap_or(snap.last);
        Ok(Some(snap.clone()))
    }

    async fn emit_snapshot(&self, snapshot: MarketSnapshot) {
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
            trace_id: snapshot.trace_id.clone().or_else(|| Some(TraceId::new())),
        };
        let _ = log_event_via_bus(self.bus.as_ref(), &evt).await;
    }

    fn maybe_log_initial(&self, snapshot: &MarketSnapshot, counter: &mut usize) {
        if *counter < 5 {
            info!(
                exchange = ?snapshot.exchange,
                symbol = %format!("{}/{}", snapshot.symbol.base, snapshot.symbol.quote),
                last = snapshot.last,
                bid = snapshot.bid,
                ask = snapshot.ask,
                "received live OKX snapshot"
            );
            *counter += 1;
        }
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
struct OkxEnvelope<T> {
    arg: OkxArg,
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct OkxArg {
    #[serde(rename = "instId")]
    inst_id: Option<String>,
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

#[derive(Debug, Deserialize)]
struct OkxBook {
    #[serde(rename = "instId")]
    inst_id: String,
    bids: Vec<Vec<String>>, // [price, size, ...]
    asks: Vec<Vec<String>>, // [price, size, ...]
}

#[derive(Debug, Deserialize)]
struct OkxTrade {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(rename = "px")]
    last_px: String,
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

        let mut state = std::collections::HashMap::new();
        stream.handle_text(payload, &mut state).await.unwrap();
    }

    #[test]
    fn converts_instrument_to_symbol() {
        let symbol = instrument_to_symbol("BTC-USDT-SWAP").unwrap();
        assert_eq!(symbol.base, "BTC");
        assert_eq!(symbol.quote, "USDT");
    }
}
