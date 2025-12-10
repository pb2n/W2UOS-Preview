use std::sync::Arc;

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use w2uos_data::{ExchangeId, Symbol};
use w2uos_net::TrafficShaper;

use crate::types::{OrderCommand, OrderResult, OrderSide, OrderStatus, OrderType};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinanceCredentials {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BinanceExecutionConfig {
    pub rest_base_url: String,
    pub credentials: BinanceCredentials,
}

#[derive(Clone)]
pub struct BinanceClient {
    pub rest_base_url: String,
    pub credentials: BinanceCredentials,
    pub http_client: Arc<reqwest::Client>,
    pub traffic_shaper: Arc<TrafficShaper>,
}

impl BinanceClient {
    pub fn new(
        rest_base_url: String,
        credentials: BinanceCredentials,
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

    pub async fn place_order(&self, cmd: &OrderCommand) -> Result<OrderResult> {
        let path = "/api/v3/order";
        let symbol = format!(
            "{}{}",
            cmd.symbol.base.to_uppercase(),
            cmd.symbol.quote.to_uppercase()
        );
        let side = match cmd.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };
        let order_type = match cmd.order_type {
            OrderType::Market => "MARKET",
            OrderType::Limit => "LIMIT",
        };

        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut params = vec![
            format!("symbol={}", symbol),
            format!("side={}", side),
            format!("type={}", order_type),
            format!("timestamp={}", timestamp),
            format!("newClientOrderId={}", cmd.id),
        ];

        match cmd.order_type {
            OrderType::Market => {
                params.push(format!("quoteOrderQty={}", cmd.size_quote));
            }
            OrderType::Limit => {
                let price = cmd
                    .limit_price
                    .ok_or_else(|| anyhow!("limit price required for limit orders"))?;
                let qty = if price > 0.0 {
                    cmd.size_quote / price
                } else {
                    0.0
                };
                params.push(format!("timeInForce=GTC"));
                params.push(format!("quantity={}", qty));
                params.push(format!("price={}", price));
            }
        }

        let query = params.join("&");
        let signature = self.sign(&query)?;
        let url = format!(
            "{}{}?{}&signature={}",
            self.rest_base_url, path, query, signature
        );
        let api_key = self.credentials.api_key.clone();

        let response = self
            .traffic_shaper
            .execute("BINANCE-REST", move |client| {
                let client = client.clone();
                let url = url.clone();
                let api_key = api_key.clone();
                async move {
                    let resp = client
                        .post(url)
                        .header("X-MBX-APIKEY", api_key)
                        .send()
                        .await?;

                    let status = resp.status();
                    let value: BinanceOrderResponse = resp.json().await?;
                    if !status.is_success() {
                        return Err(anyhow!("Binance order error: {:?}", value));
                    }
                    Ok(value)
                }
            })
            .await?;

        Ok(OrderResult {
            command_id: cmd.id.clone(),
            exchange: ExchangeId::Binance,
            symbol: Symbol {
                base: cmd.symbol.base.clone(),
                quote: cmd.symbol.quote.clone(),
            },
            side: cmd.side,
            status: OrderStatus::New,
            filled_size_quote: 0.0,
            avg_price: 0.0,
            ts: chrono::Utc::now(),
            trace_id: cmd.trace_id.clone(),
        })
    }

    fn sign(&self, query: &str) -> Result<String> {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(self.credentials.api_secret.as_bytes())?;
        mac.update(query.as_bytes());
        let result = mac.finalize().into_bytes();
        Ok(hex::encode(result))
    }
}

#[derive(Debug, Deserialize)]
struct BinanceOrderResponse {
    #[allow(dead_code)]
    symbol: Option<String>,
    #[allow(dead_code)]
    order_id: Option<u64>,
}
