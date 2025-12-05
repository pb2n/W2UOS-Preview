use std::io::Write;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use sqlx::any::{install_default_drivers, AnyPoolOptions};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

use crate::types::{LatencyRecord, LogEvent, TradeRecord};

#[async_trait::async_trait]
pub trait LogSink: Send + Sync {
    async fn write_event(&self, event: LogEvent) -> Result<()>;
    async fn write_trade(&self, trade: TradeRecord) -> Result<()>;
    async fn write_latency(&self, latency: LatencyRecord) -> Result<()>;
}

pub struct FileLogSink {
    writer: Arc<Mutex<RollingFileAppender>>,
}

impl FileLogSink {
    pub fn new(log_dir: &std::path::Path, prefix: &str) -> Result<Self> {
        std::fs::create_dir_all(log_dir)?;
        let appender = RollingFileAppender::new(Rotation::DAILY, log_dir, prefix);
        Ok(Self {
            writer: Arc::new(Mutex::new(appender)),
        })
    }

    async fn write_json_line<T: serde::Serialize>(&self, value: &T) -> Result<()> {
        let payload = serde_json::to_vec(value)?;
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut guard = writer
                .lock()
                .map_err(|_| anyhow::anyhow!("failed to lock log writer"))?;
            guard.write_all(&payload)?;
            guard.write_all(b"\n")?;
            guard.flush()?;
            Ok(())
        })
        .await??;

        Ok(())
    }
}

#[async_trait::async_trait]
impl LogSink for FileLogSink {
    async fn write_event(&self, event: LogEvent) -> Result<()> {
        self.write_json_line(&event).await
    }

    async fn write_trade(&self, trade: TradeRecord) -> Result<()> {
        self.write_json_line(&trade).await
    }

    async fn write_latency(&self, latency: LatencyRecord) -> Result<()> {
        self.write_json_line(&latency).await
    }
}

pub struct SqlLogSink {
    pool: sqlx::AnyPool,
}

impl SqlLogSink {
    pub async fn connect(database_url: &str) -> Result<Self> {
        prepare_sqlite_path(database_url)?;
        install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        let sink = Self { pool };
        sink.init_schema().await?;
        Ok(sink)
    }

    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS log_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                level TEXT NOT NULL,
                source TEXT NOT NULL,
                message TEXT NOT NULL,
                fields TEXT NOT NULL,
                correlation_id TEXT,
                trace_id TEXT
            )"#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS trades (
                id TEXT PRIMARY KEY,
                ts TEXT NOT NULL,
                exchange TEXT NOT NULL,
                symbol_base TEXT NOT NULL,
                symbol_quote TEXT NOT NULL,
                side TEXT NOT NULL,
                size_quote REAL NOT NULL,
                price REAL NOT NULL,
                status TEXT NOT NULL,
                strategy_id TEXT,
                correlation_id TEXT
            )"#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS latency (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id TEXT NOT NULL,
                ts_start_tick TEXT NOT NULL,
                ts_strategy_decision TEXT NOT NULL,
                ts_order_result TEXT NOT NULL,
                tick_to_strategy_ms INTEGER NOT NULL,
                strategy_to_exec_ms INTEGER NOT NULL,
                tick_to_result_ms INTEGER NOT NULL
            )"#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

fn prepare_sqlite_path(database_url: &str) -> Result<()> {
    if database_url.starts_with("sqlite://") && !database_url.contains("memory") {
        let path_part = database_url.trim_start_matches("sqlite://");
        let fs_path = if path_part.starts_with('/') {
            std::path::PathBuf::from(path_part)
        } else {
            std::path::PathBuf::from(format!("./{}", path_part))
        };

        if let Some(parent) = fs_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&fs_path)?;
    }

    Ok(())
}

#[async_trait::async_trait]
impl LogSink for SqlLogSink {
    async fn write_event(&self, event: LogEvent) -> Result<()> {
        sqlx::query(
            "INSERT INTO log_events (ts, level, source, message, fields, correlation_id, trace_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(event.ts.to_rfc3339())
        .bind(format!("{:?}", event.level))
        .bind(match &event.source {
            crate::types::LogSource::Other(name) => name.clone(),
            _ => format!("{:?}", event.source),
        })
        .bind(event.message.clone())
        .bind(event.fields.to_string())
        .bind(event.correlation_id.clone())
        .bind(event.trace_id.as_ref().map(|t| t.0.clone()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn write_trade(&self, trade: TradeRecord) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO trades (id, ts, exchange, symbol_base, symbol_quote, side, size_quote, price, status, strategy_id, correlation_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(trade.id.clone())
        .bind(trade.ts.to_rfc3339())
        .bind(trade.exchange.clone())
        .bind(trade.symbol.base.clone())
        .bind(trade.symbol.quote.clone())
        .bind(format!("{:?}", trade.side))
        .bind(trade.size_quote)
        .bind(trade.price)
        .bind(String::from(&trade.status))
        .bind(trade.strategy_id.clone())
        .bind(trade.correlation_id.clone())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn write_latency(&self, latency: LatencyRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO latency (trace_id, ts_start_tick, ts_strategy_decision, ts_order_result, tick_to_strategy_ms, strategy_to_exec_ms, tick_to_result_ms) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(latency.trace_id.0)
        .bind(latency.ts_start_tick.to_rfc3339())
        .bind(latency.ts_strategy_decision.to_rfc3339())
        .bind(latency.ts_order_result.to_rfc3339())
        .bind(latency.tick_to_strategy_ms)
        .bind(latency.strategy_to_exec_ms)
        .bind(latency.tick_to_result_ms)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

pub struct CompositeLogSink {
    sinks: Vec<Arc<dyn LogSink>>,
}

impl CompositeLogSink {
    pub fn new(sinks: Vec<Arc<dyn LogSink>>) -> Self {
        Self { sinks }
    }
}

#[async_trait::async_trait]
impl LogSink for CompositeLogSink {
    async fn write_event(&self, event: LogEvent) -> Result<()> {
        for sink in &self.sinks {
            sink.write_event(event.clone()).await?;
        }
        Ok(())
    }

    async fn write_trade(&self, trade: TradeRecord) -> Result<()> {
        for sink in &self.sinks {
            sink.write_trade(trade.clone()).await?;
        }
        Ok(())
    }

    async fn write_latency(&self, latency: LatencyRecord) -> Result<()> {
        for sink in &self.sinks {
            sink.write_latency(latency.clone()).await?;
        }
        Ok(())
    }
}
