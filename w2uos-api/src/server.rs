use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use actix::prelude::*;
use actix_web::{
    body::BoxBody,
    dev::{Service, ServiceFactory, ServiceRequest, ServiceResponse},
    web, App, Error, HttpMessage, HttpRequest, HttpResponse, HttpServer, Responder,
};
use actix_web_actors::ws;
use serde::Deserialize;
use tracing::info;
use w2uos_bus::MessageBus;
use w2uos_net::NetProfile;

use crate::auth::{
    authenticate, record_auth_failure, required_role_for_path, ApiUser, ApiUserCredential,
};
use crate::context::ApiContext;
use crate::types::{
    BacktestRequestDto, BacktestStatusDto, ClusterNodeDto, ControlResponseDto, LatencyBucketDto,
    LatencySummaryDto, LiveOrderDto, LiveStatusDto, LiveSwitchRequestDto, LogDto, MarketMetricsDto,
    MarketSummaryResponseDto, NodeStatusDto, PositionDto, RiskStatusDto, SystemMetricsDto,
};
use serde_json::json;
use w2uos_data::{ExchangeId, Symbol};
use w2uos_kernel::{RiskProfile, TradingState};

#[derive(Clone)]
pub struct ApiConfig {
    pub users: Vec<ApiUserCredential>,
    pub net_profile: NetProfile,
    pub bind: String,
    pub bus: Arc<dyn MessageBus>,
}

async fn status_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status: NodeStatusDto = ctx
        .get_node_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(status))
}

async fn positions_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let positions: Vec<PositionDto> = ctx
        .get_positions()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(positions))
}

async fn logs_handler(
    query: web::Query<std::collections::HashMap<String, String>>,
    ctx: web::Data<ApiContext>,
) -> Result<impl Responder, Error> {
    let limit = query
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100);
    let logs: Vec<LogDto> = ctx
        .get_recent_logs(limit)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(logs))
}

async fn health_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status = ctx
        .health_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(status))
}

async fn net_profile_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    Ok(web::Json(ctx.get_net_profile()))
}

async fn latency_summary_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let summary: LatencySummaryDto = ctx
        .get_latency_summary()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(summary))
}

async fn latency_histogram_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let histogram: Vec<LatencyBucketDto> = ctx
        .get_latency_histogram()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(histogram))
}

async fn cluster_nodes_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let nodes: Vec<ClusterNodeDto> = ctx
        .get_cluster_nodes()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(nodes))
}

async fn risk_status_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status: RiskStatusDto = ctx
        .risk_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(status))
}

async fn risk_reset_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    ctx.reset_circuit()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().finish())
}

async fn live_status_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status: LiveStatusDto = ctx
        .live_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(status))
}

async fn live_positions_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let positions: Vec<PositionDto> = ctx
        .live_positions()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(positions))
}

async fn live_orders_handler(
    query: web::Query<std::collections::HashMap<String, String>>,
    ctx: web::Data<ApiContext>,
) -> Result<impl Responder, Error> {
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let orders: Vec<LiveOrderDto> = ctx
        .live_orders(limit)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(orders))
}

async fn system_metrics_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let metrics: SystemMetricsDto = ctx
        .system_metrics()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(metrics))
}

async fn system_status_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status = ctx
        .system_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(status))
}

#[derive(Deserialize)]
struct MarketMetricsQuery {
    symbol: String,
    limit: Option<usize>,
}

async fn market_metrics_handler(
    ctx: web::Data<ApiContext>,
    query: web::Query<MarketMetricsQuery>,
) -> Result<HttpResponse, Error> {
    let limit = query.limit.unwrap_or(50).min(200);
    let metrics: Option<MarketMetricsDto> = ctx
        .market_metrics(&query.symbol, limit)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    if let Some(metrics) = metrics {
        Ok(HttpResponse::Ok().json(metrics))
    } else {
        Ok(HttpResponse::NotFound().json(json!({"error": "symbol_not_found"})))
    }
}

#[derive(Deserialize)]
struct MarketSummaryQuery {
    exchange: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct MarketSnapshotQuery {
    symbol: Option<String>,
}

async fn market_summary_handler(
    ctx: web::Data<ApiContext>,
    query: web::Query<MarketSummaryQuery>,
) -> Result<impl Responder, Error> {
    let limit = query.limit.unwrap_or(50).min(500);
    let response: MarketSummaryResponseDto = ctx
        .market_summary(query.exchange.as_deref(), limit)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(response))
}

async fn market_snapshot_handler(
    ctx: web::Data<ApiContext>,
    query: web::Query<MarketSnapshotQuery>,
) -> Result<HttpResponse, Error> {
    let snapshot = ctx
        .market_snapshot(query.symbol.as_deref())
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;

    match snapshot {
        Some(snap) => Ok(HttpResponse::Ok().json(snap)),
        None => Ok(HttpResponse::NoContent().finish()),
    }
}

#[derive(Deserialize)]
struct ControlReasonRequest {
    reason: Option<String>,
}

#[derive(Deserialize)]
struct FreezeRequest {
    reason: Option<String>,
    cancel_open_orders: Option<bool>,
}

#[derive(Deserialize)]
struct UnfreezeRequest {
    reason: Option<String>,
    target_state: String,
}

#[derive(Deserialize)]
struct FlattenRequest {
    reason: Option<String>,
    mode: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct RiskProfileRequest {
    profile: String,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct ModeSwitchRequest {
    target_mode: String,
    double_confirm_token: Option<String>,
    reason: Option<String>,
}

fn actor_from_request(req: &HttpRequest) -> String {
    req.extensions()
        .get::<ApiUser>()
        .map(|user| format!("api@{}", user.id))
        .unwrap_or_else(|| "api@unknown".to_string())
}

fn parse_target_state(target: &str) -> Result<TradingState, Error> {
    match target {
        "SIMULATING" => Ok(TradingState::Simulating),
        "LIVE_DRY_RUN" => Ok(TradingState::LiveDryRun),
        _ => Err(actix_web::error::ErrorBadRequest(
            "invalid target state for unfreeze",
        )),
    }
}

fn parse_mode_target(target: &str) -> Result<TradingState, Error> {
    match target {
        "SIMULATING" => Ok(TradingState::Simulating),
        "LIVE_DRY_RUN" => Ok(TradingState::LiveDryRun),
        "LIVE_ARMED" => Ok(TradingState::LiveArmed),
        _ => Err(actix_web::error::ErrorBadRequest("invalid target mode")),
    }
}

fn parse_risk_profile(profile: &str) -> Result<RiskProfile, Error> {
    match profile.to_ascii_uppercase().as_str() {
        "ULTRA_CONSERVATIVE" => Ok(RiskProfile::UltraConservative),
        "CONSERVATIVE" => Ok(RiskProfile::Conservative),
        "DEFAULT" => Ok(RiskProfile::Default),
        "AGGRESSIVE" => Ok(RiskProfile::Aggressive),
        _ => Err(actix_web::error::ErrorBadRequest("invalid risk profile")),
    }
}

fn parse_symbol(sym: &str) -> Symbol {
    if let Some((base, quote)) = sym.split_once('/') {
        Symbol {
            base: base.to_string(),
            quote: quote.to_string(),
        }
    } else if sym.len() > 3 {
        let (base, quote) = sym.split_at(3);
        Symbol {
            base: base.to_string(),
            quote: quote.to_string(),
        }
    } else {
        Symbol {
            base: sym.to_string(),
            quote: "USDT".to_string(),
        }
    }
}

async fn pause_handler(
    payload: web::Json<ControlReasonRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let resp: ControlResponseDto = ctx
        .control_pause(payload.reason.clone(), &actor)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(resp))
}

async fn resume_handler(
    payload: web::Json<ControlReasonRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let resp = ctx
        .control_resume(payload.reason.clone(), &actor)
        .await
        .map_err(actix_web::error::ErrorBadRequest)?;
    Ok(web::Json(resp))
}

async fn freeze_handler(
    payload: web::Json<FreezeRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let resp = ctx
        .control_freeze(
            payload.reason.clone(),
            payload.cancel_open_orders.unwrap_or(false),
            &actor,
        )
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(resp))
}

async fn unfreeze_handler(
    payload: web::Json<UnfreezeRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let target = parse_target_state(&payload.target_state)?;
    let actor = actor_from_request(&req);
    let resp = ctx
        .control_unfreeze(payload.reason.clone(), target, &actor)
        .await
        .map_err(actix_web::error::ErrorBadRequest)?;
    Ok(web::Json(resp))
}

async fn flatten_handler(
    payload: web::Json<FlattenRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let resp = ctx
        .control_flatten(
            payload.reason.clone(),
            payload.mode.clone(),
            payload.timeout_ms,
            &actor,
        )
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(resp))
}

async fn live_switch_handler(
    payload: web::Json<LiveSwitchRequestDto>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let resp = ctx
        .control_live_switch(payload.armed, payload.reason.clone(), &actor)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(resp))
}

async fn control_state_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let resp = ctx
        .control_state()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(resp))
}

async fn audit_log_handler(
    query: web::Query<std::collections::HashMap<String, String>>,
    ctx: web::Data<ApiContext>,
) -> Result<impl Responder, Error> {
    let limit = query
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100);
    let entries = ctx
        .control_audit_log(limit)
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(entries))
}

async fn risk_profile_handler(
    payload: web::Json<RiskProfileRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let profile = parse_risk_profile(&payload.profile)?;
    let resp = ctx
        .control_risk_profile(profile, payload.reason.clone(), &actor)
        .await
        .map_err(actix_web::error::ErrorBadRequest)?;
    Ok(web::Json(resp))
}

async fn mode_switch_handler(
    payload: web::Json<ModeSwitchRequest>,
    ctx: web::Data<ApiContext>,
    req: HttpRequest,
) -> Result<impl Responder, Error> {
    let actor = actor_from_request(&req);
    let target = parse_mode_target(&payload.target_mode)?;
    let resp = ctx
        .control_mode_switch(
            target,
            payload.double_confirm_token.clone(),
            payload.reason.clone(),
            &actor,
        )
        .await
        .map_err(actix_web::error::ErrorBadRequest)?;
    Ok(web::Json(resp))
}

async fn backtest_start_handler(
    payload: web::Json<BacktestRequestDto>,
    ctx: web::Data<ApiContext>,
) -> Result<impl Responder, Error> {
    let symbols: Vec<Symbol> = payload.symbols.iter().map(|s| parse_symbol(s)).collect();
    ctx.start_backtest(
        ExchangeId::from(payload.exchange.as_str()),
        symbols,
        payload.start,
        payload.end,
        payload.speed_factor,
    )
    .await
    .map_err(actix_web::error::ErrorInternalServerError)?;

    Ok(HttpResponse::Accepted().finish())
}

async fn backtest_status_handler(ctx: web::Data<ApiContext>) -> Result<impl Responder, Error> {
    let status = ctx
        .backtest_status()
        .await
        .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(web::Json(BacktestStatusDto::from(status)))
}

pub fn build_app(
    ctx: ApiContext,
    config: ApiConfig,
) -> App<
    impl ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<BoxBody>,
        Error = Error,
        InitError = (),
    >,
> {
    let user_config = config.users.clone();
    let _net_profile = config.net_profile.clone();
    let bus = Arc::clone(&config.bus);
    App::new()
        .wrap_fn(move |req, srv| {
            let users = user_config.clone();
            let bus = Arc::clone(&bus);
            let path = req.path().to_string();
            if path == "/health" {
                let fut = srv.call(req);
                return Box::pin(async move { fut.await })
                    as Pin<Box<dyn Future<Output = Result<ServiceResponse<BoxBody>, Error>>>>;
            }
            let maybe_key = req
                .headers()
                .get("X-API-KEY")
                .and_then(|val| val.to_str().ok());
            let required = required_role_for_path(&req);

            let Some(api_key) = maybe_key else {
                record_auth_failure(Arc::clone(&bus), "missing_api_key", &path);
                let response = req.into_response(
                    HttpResponse::Unauthorized().json(json!({"error": "unauthorized"})),
                );
                return Box::pin(async { Ok(response) })
                    as Pin<Box<dyn Future<Output = Result<ServiceResponse<BoxBody>, Error>>>>;
            };

            let Some(user) = authenticate(api_key, &users) else {
                record_auth_failure(Arc::clone(&bus), "invalid_api_key", &path);
                let response = req.into_response(
                    HttpResponse::Unauthorized().json(json!({"error": "unauthorized"})),
                );
                return Box::pin(async { Ok(response) })
                    as Pin<Box<dyn Future<Output = Result<ServiceResponse<BoxBody>, Error>>>>;
            };

            if !user.role.allows(&required) {
                record_auth_failure(Arc::clone(&bus), "forbidden", &path);
                let response = req.into_response(HttpResponse::Forbidden().finish());
                return Box::pin(async { Ok(response) })
                    as Pin<Box<dyn Future<Output = Result<ServiceResponse<BoxBody>, Error>>>>;
            }

            req.extensions_mut().insert(user);
            let fut = srv.call(req);
            Box::pin(async move { fut.await })
                as Pin<Box<dyn Future<Output = Result<ServiceResponse<BoxBody>, Error>>>>
        })
        .app_data(web::Data::new(ctx.clone()))
        .route("/status", web::get().to(status_handler))
        .route("/system/status", web::get().to(system_status_handler))
        .route("/live/status", web::get().to(live_status_handler))
        .route("/health", web::get().to(health_handler))
        .route("/positions", web::get().to(positions_handler))
        .route("/live/positions", web::get().to(live_positions_handler))
        .route("/logs", web::get().to(logs_handler))
        .route("/live/orders", web::get().to(live_orders_handler))
        .route("/net/profile", web::get().to(net_profile_handler))
        .route("/cluster/nodes", web::get().to(cluster_nodes_handler))
        .route("/risk/status", web::get().to(risk_status_handler))
        .route("/risk/reset_circuit", web::post().to(risk_reset_handler))
        .route("/control/pause", web::post().to(pause_handler))
        .route("/control/resume", web::post().to(resume_handler))
        .route("/control/freeze", web::post().to(freeze_handler))
        .route("/control/unfreeze", web::post().to(unfreeze_handler))
        .route("/control/flatten", web::post().to(flatten_handler))
        .route("/control/live_switch", web::post().to(live_switch_handler))
        .route("/control/state", web::get().to(control_state_handler))
        .route(
            "/control/risk-profile",
            web::post().to(risk_profile_handler),
        )
        .route("/control/mode", web::post().to(mode_switch_handler))
        .route("/control/audit-log", web::get().to(audit_log_handler))
        .route(
            "/metrics/latency/summary",
            web::get().to(latency_summary_handler),
        )
        .route(
            "/metrics/latency/histogram",
            web::get().to(latency_histogram_handler),
        )
        .route("/metrics/market", web::get().to(market_metrics_handler))
        .route(
            "/metrics/market/summary",
            web::get().to(market_summary_handler),
        )
        .route("/market/snapshot", web::get().to(market_snapshot_handler))
        .route("/metrics/system", web::get().to(system_metrics_handler))
        .route("/backtest/start", web::post().to(backtest_start_handler))
        .route("/backtest/status", web::get().to(backtest_status_handler))
        .route("/ws/stream", web::get().to(ws_handler))
}

struct WsSession {
    bus: Arc<dyn MessageBus>,
    subjects: Vec<String>,
}

impl WsSession {
    fn new(bus: Arc<dyn MessageBus>, subjects: Vec<String>) -> Self {
        Self { bus, subjects }
    }
}

impl actix::Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let addr = ctx.address();
        for subject in self.subjects.clone() {
            let bus = Arc::clone(&self.bus);
            let addr = addr.clone();
            actix_rt::spawn(async move {
                match bus.subscribe(&subject).await {
                    Ok(mut sub) => {
                        while let Some(msg) = sub.receiver.recv().await {
                            let payload = String::from_utf8_lossy(&msg.0).to_string();
                            addr.do_send(BusEvent { payload });
                        }
                    }
                    Err(err) => {
                        addr.do_send(BusEvent {
                            payload: format!(
                                "{{\"error\": \"failed to subscribe {}: {}\"}}",
                                subject, err
                            ),
                        });
                    }
                }
            });
        }
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, item: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match item {
            Ok(ws::Message::Ping(msg)) => ctx.pong(&msg),
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            _ => {}
        }
    }
}

#[derive(actix::Message)]
#[rtype(result = "()")]
pub struct BusEvent {
    pub payload: String,
}

impl actix::Handler<BusEvent> for WsSession {
    type Result = ();

    fn handle(&mut self, msg: BusEvent, ctx: &mut Self::Context) {
        ctx.text(msg.payload);
    }
}

async fn ws_handler(
    req: HttpRequest,
    stream: web::Payload,
    ctx: web::Data<ApiContext>,
) -> Result<HttpResponse, Error> {
    let mut subjects = ctx.market_subjects.clone();
    subjects.push("orders.result".to_string());
    subjects.push("log.event".to_string());

    let session = WsSession::new(Arc::clone(&ctx.bus), subjects);
    ws::start(session, &req, stream)
}

pub async fn run_server(ctx: ApiContext, config: ApiConfig) -> std::io::Result<()> {
    let ctx_data = ctx.clone();
    let users = config.users.clone();
    let net_profile = config.net_profile.clone();
    let bind_addr = config.bind.clone();
    let bind_template = bind_addr.clone();
    let bus = Arc::clone(&config.bus);
    info!(bind = %bind_addr, "starting api server");
    HttpServer::new(move || {
        let bind_clone = bind_template.clone();
        build_app(
            ctx_data.clone(),
            ApiConfig {
                users: users.clone(),
                net_profile: net_profile.clone(),
                bind: bind_clone,
                bus: Arc::clone(&bus),
            },
        )
    })
    .bind(&bind_addr)?
    .run()
    .await
}
