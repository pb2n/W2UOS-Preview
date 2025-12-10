use std::sync::Arc;
use std::time::Duration;

use actix_web::{body::to_bytes, http, http::header::HeaderName, test};
use chrono::Utc;
use w2uos_bus::{BusMessage, LocalBus, MessageBus};
use w2uos_config::hash_api_key;
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol, TradingMode};
use w2uos_kernel::{GlobalRiskConfig, Kernel, RiskSupervisorService, Service, ServiceId};
use w2uos_log::LogService;
use w2uos_log::{TradeRecord, TradeSide, TradeStatus, TradeSymbol};
use w2uos_net::NetProfile;
use w2uos_service::TraceId;

use crate::auth::{authenticate, ApiUserCredential, Role};
use crate::context::ApiContext;
use crate::server::{build_app, ApiConfig};

struct TestService(ServiceId);

#[async_trait::async_trait]
impl Service for TestService {
    fn id(&self) -> &ServiceId {
        &self.0
    }

    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn node_status_reports_registered_service() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let mut kernel = Kernel::default().with_message_bus(Arc::clone(&bus));
    kernel.register_service(Arc::new(TestService("svc-1".to_string())));
    let kernel = Arc::new(kernel);
    let risk_service = Arc::new(RiskSupervisorService::new(
        "risk".to_string(),
        GlobalRiskConfig::default(),
        Arc::clone(&bus),
    ));
    let ctx = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        None,
        None,
        None,
        None,
        vec![],
        NetProfile::default(),
        Some(Arc::clone(&risk_service)),
        TradingMode::Simulated,
        vec![],
        None,
        vec![],
    );

    let status = ctx.get_node_status().await.unwrap();
    assert_eq!(status.kernel_state, "Initialized");
    assert_eq!(status.kernel_mode, "Live");
    assert_eq!(status.services.len(), 1);
    assert_eq!(status.services[0].id, "svc-1");
}

#[tokio::test]
async fn authenticate_matches_hashed_keys() {
    let creds = vec![ApiUserCredential {
        id: "admin".to_string(),
        role: Role::Admin,
        api_key_hash: hash_api_key("super-secret"),
    }];

    let user = authenticate("super-secret", &creds).expect("should authenticate");
    assert_eq!(user.id, "admin");
    assert_eq!(user.role, Role::Admin);
    assert!(authenticate("wrong", &creds).is_none());
}

#[actix_rt::test]
async fn status_and_logs_endpoints_work() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let mut kernel = Kernel::default().with_message_bus(Arc::clone(&bus));
    kernel.register_service(Arc::new(TestService("svc-1".to_string())));
    let kernel = Arc::new(kernel);

    let log_service = Arc::new(LogService::new(
        Arc::new(
            w2uos_log::sink::FileLogSink::new(std::path::Path::new("./test-logs"), "api.log")
                .unwrap(),
        ),
        Arc::clone(&bus),
    ));

    let ctx = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        Some(log_service),
        None,
        None,
        None,
        vec![],
        NetProfile::default(),
        None,
        TradingMode::Simulated,
        vec![],
        None,
        vec![],
    );
    let app_users = vec![ApiUserCredential {
        id: "tester".to_string(),
        role: Role::ReadOnly,
        api_key_hash: hash_api_key("test-key"),
    }];
    let app = test::init_service(build_app(
        ctx,
        ApiConfig {
            users: app_users,
            net_profile: NetProfile::default(),
            bind: "127.0.0.1:0".to_string(),
            bus: Arc::clone(&bus),
        },
    ))
    .await;

    let req = test::TestRequest::get()
        .uri("/status")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/logs?limit=10")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(decoded.is_array());

    let req = test::TestRequest::get()
        .uri("/cluster/nodes")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/metrics/latency/summary")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/metrics/latency/histogram")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/net/profile")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
}

#[actix_rt::test]
async fn live_status_and_orders_endpoints_work() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let kernel = Arc::new(Kernel::default().with_message_bus(Arc::clone(&bus)));
    let market_subject = "market.OKX.BTCUSDT".to_string();

    let ctx = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        None,
        None,
        None,
        None,
        vec![market_subject.clone()],
        NetProfile::default(),
        None,
        TradingMode::LiveOkx,
        vec!["okx".to_string()],
        Some(("okx".to_string(), "LivePaper".to_string())),
        vec!["BTC-USDT".to_string()],
    );
    let app_users = vec![ApiUserCredential {
        id: "tester".to_string(),
        role: Role::ReadOnly,
        api_key_hash: hash_api_key("test-key"),
    }];
    let app = test::init_service(build_app(
        ctx,
        ApiConfig {
            users: app_users,
            net_profile: NetProfile::default(),
            bind: "127.0.0.1:0".to_string(),
            bus: Arc::clone(&bus),
        },
    ))
    .await;

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
        volume_24h: 1_000_000.0,
        trace_id: Some(TraceId::new()),
    };
    bus.publish(
        &market_subject,
        BusMessage(serde_json::to_vec(&snapshot).unwrap()),
    )
    .await
    .unwrap();

    let trade = TradeRecord {
        id: "order-1".to_string(),
        ts: snapshot.ts,
        exchange: "okx".to_string(),
        symbol: TradeSymbol {
            base: "BTC".to_string(),
            quote: "USDT".to_string(),
        },
        side: TradeSide::Buy,
        size_quote: 100.0,
        price: snapshot.ask,
        status: TradeStatus::Filled,
        strategy_id: None,
        correlation_id: None,
    };
    bus.publish("log.trade", BusMessage(serde_json::to_vec(&trade).unwrap()))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = test::TestRequest::get()
        .uri("/live/status")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());

    let req = test::TestRequest::get()
        .uri("/live/orders?limit=10")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(decoded.is_array());
    assert_eq!(decoded.as_array().unwrap().len(), 1);

    let req = test::TestRequest::get()
        .uri("/live/positions")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
}

#[actix_rt::test]
async fn rejects_missing_or_low_privilege_keys() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let mut kernel = Kernel::default().with_message_bus(Arc::clone(&bus));
    kernel.register_service(Arc::new(TestService("svc-1".to_string())));
    let kernel = Arc::new(kernel);
    let ctx = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        None,
        None,
        None,
        None,
        vec![],
        NetProfile::default(),
        None,
        TradingMode::Simulated,
        vec![],
        None,
        vec![],
    );

    let app = test::init_service(build_app(
        ctx,
        ApiConfig {
            users: vec![
                ApiUserCredential {
                    id: "ro".to_string(),
                    role: Role::ReadOnly,
                    api_key_hash: hash_api_key("readonly"),
                },
                ApiUserCredential {
                    id: "trader".to_string(),
                    role: Role::Trader,
                    api_key_hash: hash_api_key("trader"),
                },
            ],
            net_profile: NetProfile::default(),
            bind: "127.0.0.1:0".to_string(),
            bus: Arc::clone(&bus),
        },
    ))
    .await;

    let req = test::TestRequest::get().uri("/status").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), http::StatusCode::UNAUTHORIZED);

    let req = test::TestRequest::post()
        .uri("/risk/reset_circuit")
        .insert_header((
            http::header::HeaderName::from_static("x-api-key"),
            "readonly",
        ))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), http::StatusCode::FORBIDDEN);

    let req = test::TestRequest::post()
        .uri("/risk/reset_circuit")
        .insert_header((http::header::HeaderName::from_static("x-api-key"), "trader"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_ne!(resp.status(), http::StatusCode::UNAUTHORIZED);
    assert_ne!(resp.status(), http::StatusCode::FORBIDDEN);
}
