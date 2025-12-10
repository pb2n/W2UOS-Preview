use std::sync::Arc;
use std::time::Duration;

use actix_web::{body::to_bytes, http, http::header::HeaderName, test};
use chrono::Utc;
use serde_json::json;
use w2uos_bus::{BusMessage, LocalBus, MessageBus};
use w2uos_config::hash_api_key;
use w2uos_data::{ExchangeId, MarketSnapshot, Symbol, TradingMode};
use w2uos_kernel::{
    GlobalRiskConfig, Kernel, RiskProfile, RiskSupervisorService, Service, ServiceId, TradingState,
};
use w2uos_log::LogService;
use w2uos_log::{TradeRecord, TradeSide, TradeStatus, TradeSymbol};
use w2uos_net::NetProfile;
use w2uos_service::TraceId;

use crate::auth::{authenticate, ApiUserCredential, Role};
use crate::context::ApiContext;
use crate::server::{build_app, ApiConfig};
use crate::types::{ControlResponseDto, RiskProfileResponseDto, SystemMetricsDto};

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
        "node-test".to_string(),
        "test".to_string(),
    );

    let status = ctx.get_node_status().await.unwrap();
    assert_eq!(status.kernel_state, "Initialized");
    assert_eq!(status.kernel_mode, "Live");
    assert_eq!(status.node_id, "node-test");
    assert_eq!(status.env, "test");
    assert_eq!(status.trading_state, "BOOTING");
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
        "node-test".to_string(),
        "test".to_string(),
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

    tokio::time::sleep(Duration::from_millis(200)).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let req = test::TestRequest::get()
        .uri("/status")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(decoded
        .get("trading_state")
        .and_then(|s| s.as_str())
        .is_some());
    assert_eq!(decoded["node_id"], "node-test");

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
        "node-test".to_string(),
        "test".to_string(),
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
        mode: "sim".to_string(),
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
async fn system_metrics_endpoint_returns_payload() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let kernel = Arc::new(Kernel::default().with_message_bus(Arc::clone(&bus)));

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
        "node-test".to_string(),
        "test".to_string(),
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
        .uri("/metrics/system")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: SystemMetricsDto = serde_json::from_slice(&body).unwrap();

    assert!(!decoded.node.trading_state.is_empty());
    assert!(decoded.runtime.mem_total_mb >= 0.0);
    assert!(!decoded.risk.risk_profile.is_empty());
}

#[actix_rt::test]
async fn market_metrics_and_summary_return_data() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let kernel = Kernel::default().with_message_bus(Arc::clone(&bus));
    let kernel = Arc::new(kernel);

    let ctx = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        None,
        None,
        None,
        None,
        vec!["market.ticker".to_string()],
        NetProfile::default(),
        None,
        TradingMode::Simulated,
        vec![],
        Some(("OKX".to_string(), "live-paper".to_string())),
        vec!["BTC-USDT".to_string()],
        "node-test".to_string(),
        "test".to_string(),
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
        bid: 29_995.0,
        ask: 30_005.0,
        volume_24h: 1234.5,
        trace_id: Some(TraceId::new()),
    };

    for _ in 0..2 {
        bus.publish(
            "market.ticker",
            BusMessage(serde_json::to_vec(&snapshot).unwrap()),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    let req = test::TestRequest::get()
        .uri("/metrics/market?symbol=BTC/USDT&limit=5")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();

    let resp = test::call_service(&app, req).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(decoded.get("symbol").unwrap(), "BTC/USDT");
    assert!(decoded.get("spread_bp").unwrap().as_f64().unwrap() > 0.0);

    let req_summary = test::TestRequest::get()
        .uri("/metrics/market/summary?limit=10")
        .insert_header((HeaderName::from_static("x-api-key"), "test-key"))
        .to_request();

    let resp = test::call_service(&app, req_summary).await;
    assert!(resp.status().is_success());
    let body = to_bytes(resp.into_body()).await.unwrap();
    let decoded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(decoded.get("symbols").unwrap().is_array());
}

#[actix_rt::test]
async fn control_endpoints_drive_state_and_audit_log() {
    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());
    let kernel = Kernel::default().with_message_bus(Arc::clone(&bus));
    kernel.set_trading_state(TradingState::LiveDryRun);
    let kernel = Arc::new(kernel);

    let log_service = Arc::new(LogService::new(
        Arc::new(
            w2uos_log::sink::FileLogSink::new(
                std::path::Path::new("./test-control-logs"),
                "api.log",
            )
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
        TradingMode::LiveOkx,
        vec![],
        None,
        vec![],
        "node-test".to_string(),
        "test".to_string(),
    );

    let app_users = vec![ApiUserCredential {
        id: "trader".to_string(),
        role: Role::Trader,
        api_key_hash: hash_api_key("trader-key"),
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

    let pause_req = test::TestRequest::post()
        .uri("/control/pause")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"reason": "manual_pause_from_test"}))
        .to_request();
    let pause_resp = test::call_service(&app, pause_req).await;
    assert!(pause_resp.status().is_success());
    let pause_body = to_bytes(pause_resp.into_body()).await.unwrap();
    let pause_decoded: ControlResponseDto = serde_json::from_slice(&pause_body).unwrap();
    assert_eq!(pause_decoded.state, "PAUSED");
    assert_eq!(kernel.trading_state().await, TradingState::Paused);

    let resume_req = test::TestRequest::post()
        .uri("/control/resume")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"reason": "resume_after_test"}))
        .to_request();
    let resume_resp = test::call_service(&app, resume_req).await;
    assert!(resume_resp.status().is_success());
    let resume_body = to_bytes(resume_resp.into_body()).await.unwrap();
    let resume_decoded: ControlResponseDto = serde_json::from_slice(&resume_body).unwrap();
    assert_eq!(resume_decoded.state, "LIVE_DRY_RUN");
    assert_eq!(kernel.trading_state().await, TradingState::LiveDryRun);

    let freeze_req = test::TestRequest::post()
        .uri("/control/freeze")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"reason": "emergency_freeze", "cancel_open_orders": true}))
        .to_request();
    let freeze_resp = test::call_service(&app, freeze_req).await;
    assert!(freeze_resp.status().is_success());
    assert_eq!(kernel.trading_state().await, TradingState::Frozen);

    let unfreeze_req = test::TestRequest::post()
        .uri("/control/unfreeze")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"reason": "manual_unfreeze", "target_state": "SIMULATING"}))
        .to_request();
    let unfreeze_resp = test::call_service(&app, unfreeze_req).await;
    assert!(unfreeze_resp.status().is_success());
    assert_eq!(kernel.trading_state().await, TradingState::Simulating);

    let flatten_req = test::TestRequest::post()
        .uri("/control/flatten")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"reason": "flatten_all_positions", "mode": "market", "timeout_ms": 1000}))
        .to_request();
    let flatten_resp = test::call_service(&app, flatten_req).await;
    assert!(flatten_resp.status().is_success());

    let risk_req = test::TestRequest::post()
        .uri("/control/risk-profile")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({"profile": "aggressive", "reason": "manual_switch_from_test"}))
        .to_request();
    let risk_resp = test::call_service(&app, risk_req).await;
    assert!(risk_resp.status().is_success());
    let risk_body = to_bytes(risk_resp.into_body()).await.unwrap();
    let risk_decoded: RiskProfileResponseDto = serde_json::from_slice(&risk_body).unwrap();
    assert_eq!(risk_decoded.risk_profile, "AGGRESSIVE");
    assert_eq!(kernel.risk_profile().await.0, RiskProfile::Aggressive);

    let mode_req = test::TestRequest::post()
        .uri("/control/mode")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .set_json(&json!({
            "target_mode": "LIVE_ARMED",
            "double_confirm_token": "ok",
            "reason": "go_live",
        }))
        .to_request();
    let mode_resp = test::call_service(&app, mode_req).await;
    assert!(mode_resp.status().is_success());
    let mode_body = to_bytes(mode_resp.into_body()).await.unwrap();
    let mode_decoded: ControlResponseDto = serde_json::from_slice(&mode_body).unwrap();
    assert_eq!(mode_decoded.state, "LIVE_ARMED");
    assert_eq!(kernel.trading_state().await, TradingState::LiveArmed);

    let audit_req = test::TestRequest::get()
        .uri("/control/audit-log?limit=10")
        .insert_header((HeaderName::from_static("x-api-key"), "trader-key"))
        .to_request();
    let audit_resp = test::call_service(&app, audit_req).await;
    assert!(audit_resp.status().is_success());
    let audit_body = to_bytes(audit_resp.into_body()).await.unwrap();
    let audit_decoded: serde_json::Value = serde_json::from_slice(&audit_body).unwrap();
    assert!(audit_decoded.is_array());
    assert!(!audit_decoded.as_array().unwrap().is_empty());
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
        "node-test".to_string(),
        "test".to_string(),
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
