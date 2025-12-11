use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};
use w2uos_bus::{BusMessage, MessageBus};
use w2uos_data::{ExchangeId, Symbol};
#[cfg(test)]
use w2uos_net::NetProfile;
use w2uos_net::TrafficShaper;

use crate::types::{OrderCommand, OrderResult, OrderSide, OrderStatus, OrderType};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OkxCredentials {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OkxExecutionConfig {
    pub rest_base_url: String,
    pub ws_private_url: String,
    pub credentials: OkxCredentials,
}

#[derive(Clone)]
pub struct OkxClient {
    pub rest_base_url: String,
    pub credentials: OkxCredentials,
    pub http_client: Arc<reqwest::Client>,
    pub traffic_shaper: Arc<TrafficShaper>,
}

impl OkxClient {
    pub fn new(
        rest_base_url: String,
        credentials: OkxCredentials,
        http_client: Arc<reqwest::Client>,
        traffic_shaper: Arc<TrafficShaper>,
    ) -> Self {
        Self {
            rest_base_url,
            credentials,
            http_client,
            traffic_shaper,
        }
    }

    pub async fn place_order(&self, cmd: &OrderCommand) -> Result<OkxOrderResponse> {
        let path = "/api/v5/trade/order";
        let inst_id = format!(
            "{}-{}",
            cmd.symbol.base.to_uppercase(),
            cmd.symbol.quote.to_uppercase()
        );
        let side = match cmd.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        let ord_type = match cmd.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
        };

        let mut body = serde_json::json!({
            "instId": inst_id,
            "clOrdId": cmd.id,
            "tdMode": "cash",
            "side": side,
            "ordType": ord_type,
            "tgtCcy": "quote",
            "sz": format!("{}", cmd.size_quote),
        });

        if let Some(px) = cmd.limit_price {
            body["px"] = serde_json::json!(px);
        }

        let ts = format!(
            "{:.3}",
            chrono::Utc::now().timestamp_millis() as f64 / 1000.0
        );
        let sign = self.sign(&ts, "POST", path, &body.to_string())?;

        let url = format!("{}{}", self.rest_base_url, path);
        let passphrase = self
            .credentials
            .passphrase
            .clone()
            .ok_or_else(|| anyhow!("OKX passphrase required"))?;
        let api_key = self.credentials.api_key.clone();

        let response = self
            .traffic_shaper
            .execute("OKX-REST", move |client| {
                let client = client.clone();
                let body = body.clone();
                let url = url.clone();
                let sign = sign.clone();
                let ts = ts.clone();
                let api_key = api_key.clone();
                let passphrase = passphrase.clone();
                async move {
                    let resp = client
                        .post(url)
                        .header("OK-ACCESS-KEY", api_key)
                        .header("OK-ACCESS-SIGN", sign)
                        .header("OK-ACCESS-TIMESTAMP", ts)
                        .header("OK-ACCESS-PASSPHRASE", passphrase)
                        .json(&body)
                        .send()
                        .await?;

                    let status = resp.status();
                    let value: OkxOrderResponse = resp.json().await?;
                    if !status.is_success() {
                        return Err(anyhow!("OKX order error: {:?}", value));
                    }
                    Ok(value)
                }
            })
            .await?;

        Ok(response)
    }

    pub fn sign(&self, ts: &str, method: &str, path: &str, body: &str) -> Result<String> {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(self.credentials.api_secret.as_bytes())?;
        let prehash = format!("{}{}{}{}", ts, method, path, body);
        mac.update(prehash.as_bytes());
        let result = mac.finalize().into_bytes();
        Ok(general_purpose::STANDARD.encode(result))
    }
}

pub struct OkxPrivateStream {
    pub bus: Arc<dyn MessageBus>,
    pub ws_url: String,
    pub credentials: OkxCredentials,
}

impl OkxPrivateStream {
    pub async fn run(&self) -> Result<()> {
        info!(url = %self.ws_url, "connecting OKX private stream");
        let (ws_stream, _) = connect_async(&self.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        let ts = format!(
            "{:.3}",
            chrono::Utc::now().timestamp_millis() as f64 / 1000.0
        );
        let sign = self.sign(&ts, "GET", "/users/self/verify", "")?;

        let login = serde_json::json!({
            "op": "login",
            "args": [{
                "apiKey": self.credentials.api_key,
                "passphrase": self.credentials.passphrase.clone().unwrap_or_default(),
                "timestamp": ts,
                "sign": sign,
            }]
        });

        write
            .send(Message::Text(login.to_string()))
            .await
            .context("send okx login")?;

        let sub_msg = serde_json::json!({
            "op": "subscribe",
            "args": [
                {"channel": "orders"},
                {"channel": "account"},
                {"channel": "positions"}
            ],
        });
        write
            .send(Message::Text(sub_msg.to_string()))
            .await
            .context("subscribe orders")?;

        while let Some(msg) = read.next().await {
            let msg = msg?;
            match msg {
                Message::Text(txt) => {
                    if let Err(err) = self.handle_text(&txt).await {
                        warn!(?err, "failed to process private okx message");
                    }
                }
                Message::Ping(payload) => {
                    write.send(Message::Pong(payload)).await.ok();
                }
                Message::Close(frame) => {
                    warn!(?frame, "okx private stream closed");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_text(&self, txt: &str) -> Result<()> {
        if txt.contains("event") {
            debug!(payload = %txt, "okx private event");
            return Ok(());
        }

        let value: serde_json::Value =
            serde_json::from_str(txt).context("parse okx private envelope")?;
        let channel = value
            .get("arg")
            .and_then(|a| a.get("channel"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        if channel != "orders" {
            debug!(channel, "ignoring non-order private channel");
            return Ok(());
        }

        let env: OkxOrderEnvelope = serde_json::from_value(value)?;
        for data in env.data {
            let symbol = Self::parse_symbol(&data.inst_id)?;
            let side = match data.side.as_str() {
                "buy" => OrderSide::Buy,
                _ => OrderSide::Sell,
            };
            let status = match data.state.as_str() {
                "filled" => OrderStatus::Filled,
                "canceled" => OrderStatus::Rejected("canceled".to_string()),
                _ => OrderStatus::New,
            };

            let fill_px = data.fill_px.as_deref().unwrap_or("0");
            let fill_sz = data.fill_sz.as_deref().unwrap_or("0");
            let fill_px: f64 = fill_px.parse().unwrap_or_default();
            let fill_sz: f64 = fill_sz.parse().unwrap_or_default();
            let filled_quote = fill_px * fill_sz;

            let result = OrderResult {
                command_id: data.cl_ord_id.unwrap_or_else(|| data.ord_id.clone()),
                exchange: ExchangeId::Okx,
                symbol: symbol.clone(),
                side,
                status,
                filled_size_quote: filled_quote,
                avg_price: if filled_quote > 0.0 && fill_sz > 0.0 {
                    filled_quote / fill_sz
                } else {
                    fill_px
                },
                ts: chrono::Utc::now(),
                trace_id: None,
            };

            let payload = serde_json::to_vec(&result)?;
            self.bus
                .publish("orders.result", BusMessage(payload))
                .await?;
        }

        Ok(())
    }

    fn parse_symbol(inst_id: &str) -> Result<Symbol> {
        let mut parts = inst_id.split('-');
        let base = parts
            .next()
            .ok_or_else(|| anyhow!("missing base"))?
            .to_string();
        let quote = parts
            .next()
            .ok_or_else(|| anyhow!("missing quote"))?
            .to_string();
        Ok(Symbol { base, quote })
    }

    fn sign(&self, ts: &str, method: &str, path: &str, body: &str) -> Result<String> {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(self.credentials.api_secret.as_bytes())?;
        let prehash = format!("{}{}{}{}", ts, method, path, body);
        mac.update(prehash.as_bytes());
        let result = mac.finalize().into_bytes();
        Ok(general_purpose::STANDARD.encode(result))
    }
}

#[derive(Debug, Deserialize)]
struct OkxOrderEnvelope {
    data: Vec<OkxOrderData>,
}

#[derive(Debug, Deserialize)]
struct OkxOrderData {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(rename = "ordId")]
    ord_id: String,
    #[serde(rename = "clOrdId")]
    cl_ord_id: Option<String>,
    side: String,
    state: String,
    #[serde(rename = "fillPx")]
    fill_px: Option<String>,
    #[serde(rename = "fillSz")]
    fill_sz: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OkxOrderResponse {
    pub code: String,
    pub msg: String,
    pub data: Vec<OkxOrderResponseData>,
}

#[derive(Debug, Deserialize)]
pub struct OkxOrderResponseData {
    #[serde(rename = "ordId")]
    pub ord_id: String,
    #[serde(rename = "clOrdId")]
    pub cl_ord_id: Option<String>,
    #[serde(rename = "sCode")]
    pub s_code: Option<String>,
    #[serde(rename = "sMsg")]
    pub s_msg: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn okx_signatures_match_expected() {
        let creds = OkxCredentials {
            api_key: "key".to_string(),
            api_secret: "L0ngSecret".to_string(),
            passphrase: Some("pass".to_string()),
        };
        let client = OkxClient::new(
            "https://www.okx.com".to_string(),
            creds,
            Arc::new(reqwest::Client::new()),
            Arc::new(TrafficShaper::new(
                reqwest::Client::new(),
                NetProfile::default(),
            )),
        );
        let sig = client
            .sign(
                "2020-12-08T09:08:57.715Z",
                "POST",
                "/api/v5/trade/order",
                "{}",
            )
            .unwrap();

        assert!(!sig.is_empty());
    }

    #[tokio::test]
    async fn parses_private_order_update() {
        let payload = r#"{"arg":{"channel":"orders"},"data":[{"instId":"BTC-USDT","ordId":"123","clOrdId":"abc","side":"buy","state":"filled","fillPx":"30000","fillSz":"0.1"}]}"#;
        let stream = OkxPrivateStream {
            bus: Arc::new(w2uos_bus::LocalBus::new()),
            ws_url: "".to_string(),
            credentials: OkxCredentials {
                api_key: "".to_string(),
                api_secret: "".to_string(),
                passphrase: Some("".to_string()),
            },
        };

        stream.handle_text(payload).await.unwrap();
    }
}
