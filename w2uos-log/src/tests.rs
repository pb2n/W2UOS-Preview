use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::sleep;
use w2uos_bus::{BusMessage, LocalBus, MessageBus};

use crate::service::LogService;
use crate::sink::LogSink;
use crate::types::{LogEvent, LogLevel, LogSource, TradeRecord};

struct TestSink {
    events: Mutex<Vec<LogEvent>>,
    trades: Mutex<Vec<TradeRecord>>,
    latencies: Mutex<Vec<crate::types::LatencyRecord>>,
}

impl TestSink {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            trades: Mutex::new(Vec::new()),
            latencies: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl LogSink for TestSink {
    async fn write_event(&self, event: LogEvent) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }

    async fn write_trade(&self, trade: TradeRecord) -> anyhow::Result<()> {
        self.trades.lock().unwrap().push(trade);
        Ok(())
    }

    async fn write_latency(&self, latency: crate::types::LatencyRecord) -> anyhow::Result<()> {
        self.latencies.lock().unwrap().push(latency);
        Ok(())
    }
}

#[tokio::test]
async fn file_log_sink_writes_json_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sink = crate::sink::FileLogSink::new(dir.path(), "test.log").expect("file sink");

    let event1 = LogEvent {
        ts: chrono::Utc::now(),
        level: LogLevel::Info,
        source: LogSource::Kernel,
        message: "event one".to_string(),
        fields: serde_json::json!({"k": 1}),
        correlation_id: None,
        trace_id: None,
    };

    let event2 = LogEvent {
        ts: chrono::Utc::now(),
        level: LogLevel::Warn,
        source: LogSource::Execution,
        message: "event two".to_string(),
        fields: serde_json::json!({"k": 2}),
        correlation_id: Some("corr-2".to_string()),
        trace_id: None,
    };

    sink.write_event(event1.clone()).await.expect("write1");
    sink.write_event(event2.clone()).await.expect("write2");

    let log_file = dir
        .path()
        .read_dir()
        .expect("read dir")
        .next()
        .expect("file")
        .expect("entry")
        .path();

    let contents = std::fs::read_to_string(log_file).expect("read log file");
    let mut lines = contents.lines();

    let parsed1: LogEvent = serde_json::from_str(lines.next().expect("line1")).expect("json1");
    let parsed2: LogEvent = serde_json::from_str(lines.next().expect("line2")).expect("json2");

    assert_eq!(parsed1.message, event1.message);
    assert_eq!(parsed2.message, event2.message);
}

#[tokio::test]
async fn log_service_processes_bus_events() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let sink: Arc<TestSink> = Arc::new(TestSink::new());
    let service = Arc::new(LogService::new(sink.clone(), Arc::clone(&bus)));

    let svc_handle = {
        let svc = Arc::clone(&service);
        tokio::spawn(async move {
            svc.run().await.expect("service run");
        })
    };

    sleep(Duration::from_millis(50)).await;

    let event = LogEvent {
        ts: chrono::Utc::now(),
        level: LogLevel::Debug,
        source: LogSource::MarketData,
        message: "bus event".to_string(),
        fields: serde_json::json!({"foo": "bar"}),
        correlation_id: Some("corr".to_string()),
        trace_id: None,
    };

    let payload = serde_json::to_vec(&event).expect("serialize event");
    bus.publish("log.event", BusMessage(payload))
        .await
        .expect("publish");

    sleep(Duration::from_millis(200)).await;
    service.shutdown();
    svc_handle.await.expect("join");

    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, event.message);
}

#[tokio::test]
async fn latency_records_emit_after_stages() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let sink: Arc<TestSink> = Arc::new(TestSink::new());
    let service = Arc::new(LogService::new(sink.clone(), Arc::clone(&bus)));

    let svc_handle = {
        let svc = Arc::clone(&service);
        tokio::spawn(async move { svc.run().await.expect("service run") })
    };

    sleep(Duration::from_millis(10)).await;
    let trace_id = w2uos_service::TraceId::new();

    for stage in ["tick", "strategy_decision", "order_result"] {
        let event = LogEvent {
            ts: chrono::Utc::now(),
            level: LogLevel::Info,
            source: LogSource::Kernel,
            message: format!("stage {stage}"),
            fields: serde_json::json!({"stage": stage}),
            correlation_id: None,
            trace_id: Some(trace_id.clone()),
        };
        let payload = serde_json::to_vec(&event).expect("serialize");
        bus.publish("log.event", BusMessage(payload))
            .await
            .expect("publish");
    }

    sleep(Duration::from_millis(100)).await;
    service.shutdown();
    svc_handle.await.expect("join");

    let latencies = sink.latencies.lock().unwrap();
    assert_eq!(latencies.len(), 1);
    assert_eq!(latencies[0].trace_id.0, trace_id.0);
}
