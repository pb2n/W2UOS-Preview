use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{any::AnyPoolOptions, Any, AnyPool, Row};

use crate::types::{ExchangeId, MarketSnapshot, Symbol};

#[derive(Clone)]
pub struct HistoricalStore {
    pool: AnyPool,
}

impl HistoricalStore {
    pub async fn connect(connection: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(connection)
            .await?;
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<()> {
        sqlx::query::<Any>(
            "CREATE TABLE IF NOT EXISTS market_history (
                ts TEXT NOT NULL,
                exchange TEXT NOT NULL,
                symbol_base TEXT NOT NULL,
                symbol_quote TEXT NOT NULL,
                last REAL NOT NULL,
                bid REAL NOT NULL,
                ask REAL NOT NULL,
                volume_24h REAL NOT NULL,
                metadata TEXT
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_snapshot(&self, snapshot: &MarketSnapshot) -> Result<()> {
        sqlx::query::<Any>(
            "INSERT INTO market_history (ts, exchange, symbol_base, symbol_quote, last, bid, ask, volume_24h, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(snapshot.ts.to_rfc3339())
        .bind(snapshot.exchange.to_string())
        .bind(snapshot.symbol.base.clone())
        .bind(snapshot.symbol.quote.clone())
        .bind(snapshot.last)
        .bind(snapshot.bid)
        .bind(snapshot.ask)
        .bind(snapshot.volume_24h)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_snapshots(
        &self,
        exchange: ExchangeId,
        symbol: Symbol,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<MarketSnapshot>> {
        let rows = sqlx::query::<Any>(
            "SELECT ts, exchange, symbol_base, symbol_quote, last, bid, ask, volume_24h FROM market_history
            WHERE exchange = ? AND symbol_base = ? AND symbol_quote = ? AND ts >= ? AND ts <= ?
            ORDER BY ts ASC",
        )
        .bind(exchange.to_string())
        .bind(symbol.base.clone())
        .bind(symbol.quote.clone())
        .bind(start.to_rfc3339())
        .bind(end.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        let mut snapshots = Vec::with_capacity(rows.len());
        for row in rows {
            let ts: String = row.get("ts");
            let ts = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
            let exchange_str: String = row.get("exchange");
            let exchange = ExchangeId::from(exchange_str.as_str());
            let base: String = row.get("symbol_base");
            let quote: String = row.get("symbol_quote");
            let last: f64 = row.get("last");
            let bid: f64 = row.get("bid");
            let ask: f64 = row.get("ask");
            let volume_24h: f64 = row.get("volume_24h");

            snapshots.push(MarketSnapshot {
                ts,
                exchange,
                symbol: Symbol { base, quote },
                last,
                bid,
                ask,
                volume_24h,
                trace_id: None,
            });
        }

        Ok(snapshots)
    }
}

impl sqlx::FromRow<'_, sqlx::any::AnyRow> for MarketSnapshot {
    fn from_row(row: &sqlx::any::AnyRow) -> Result<Self, sqlx::Error> {
        let ts: String = row.try_get("ts")?;
        let ts = DateTime::parse_from_rfc3339(&ts)
            .map_err(|err| sqlx::Error::ColumnDecode {
                index: "ts".into(),
                source: Box::new(err),
            })?
            .with_timezone(&Utc);
        let exchange: String = row.try_get("exchange")?;
        let exchange = ExchangeId::from(exchange.as_str());
        let symbol_base: String = row.try_get("symbol_base")?;
        let symbol_quote: String = row.try_get("symbol_quote")?;
        let last: f64 = row.try_get("last")?;
        let bid: f64 = row.try_get("bid")?;
        let ask: f64 = row.try_get("ask")?;
        let volume_24h: f64 = row.try_get("volume_24h")?;

        Ok(MarketSnapshot {
            ts,
            exchange,
            symbol: Symbol {
                base: symbol_base,
                quote: symbol_quote,
            },
            last,
            bid,
            ask,
            volume_24h,
            trace_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;

    #[tokio::test]
    async fn inserts_and_loads_snapshots() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let conn = format!("sqlite://{}", tmp.path().to_string_lossy());
        let store = HistoricalStore::connect(&conn).await.unwrap();
        let snapshot = MarketSnapshot {
            ts: Utc::now(),
            exchange: ExchangeId::Okx,
            symbol: Symbol {
                base: "BTC".to_string(),
                quote: "USDT".to_string(),
            },
            last: 30_000.0,
            bid: 29_999.0,
            ask: 30_001.0,
            volume_24h: 1234.5,
            trace_id: None,
        };

        store.insert_snapshot(&snapshot).await.unwrap();

        let loaded = store
            .load_snapshots(
                ExchangeId::Okx,
                Symbol {
                    base: "BTC".into(),
                    quote: "USDT".into(),
                },
                snapshot.ts - chrono::Duration::seconds(1),
                snapshot.ts + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].symbol.base, "BTC");
        assert_eq!(loaded[0].symbol.quote, "USDT");
    }
}
