use std::{path::PathBuf, str::FromStr, sync::Arc, time::Duration};

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::{signal, time::sleep};
use tracing::{info, warn};
use w2uos_api::{
    auth::{ApiUserCredential, Role},
    context::ApiContext,
    server::{run_server, ApiConfig},
};
use w2uos_backtest::{BacktestConfig, BacktestCoordinator, BacktestKernelService};
use w2uos_bus::{BusMessage, LocalBus, MessageBus};
use w2uos_config::{ExchangeConfig, ExchangeKind, NodeConfig};
use w2uos_data::{
    DataMode, ExchangeId, HistoricalStore, MarketDataKernelService, MarketDataService,
    MarketSnapshot, Symbol,
};
use w2uos_exec::{
    ExecutionConfig as ExecCfg, ExecutionKernelService, ExecutionMode, ExecutionService,
    OkxCredentials, OkxExecutionConfig, OrderCommand, OrderResult, OrderSide, OrderType,
};
use w2uos_kernel::{
    ClusterService, ConfigManagerService, Kernel, KernelMode, NodeId, NodeRole,
    RiskSupervisorService, StrategyBackend, StrategyHostConfig, StrategyHostService,
    StrategySubscription,
};
use w2uos_log::{CompositeLogSink, FileLogSink, LogKernelService, LogService, LogSink, SqlLogSink};
use w2uos_service::TraceId;
use wru_strategy::{BotConfig, PairId, StrategyRuntime};

#[tokio::main]
async fn main() -> Result<()> {
    Kernel::init_tracing();
    let env_name = std::env::var("NODE_ENV").ok();
    let config_path = std::env::var("W2UOS_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/w2uos/config.toml"));
    let resolved_path = if config_path.exists() {
        config_path.clone()
    } else {
        let fallback = PathBuf::from("config/config.toml");
        if fallback.exists() {
            fallback
        } else {
            config_path.clone()
        }
    };
    let node_config = match NodeConfig::load_with_env(&resolved_path, env_name.clone()) {
        Ok(cfg) => cfg,
        Err(err) => {
            warn!(
                ?err,
                ?resolved_path,
                "failed to load config file, using defaults"
            );
            NodeConfig::default()
        }
    };
    let _ = Kernel::reload_tracing_filter(&node_config.logging.level);
    let node_role = match node_config.node_role {
        w2uos_config::NodeRole::Data => NodeRole::Data,
        w2uos_config::NodeRole::Exec => NodeRole::Exec,
        w2uos_config::NodeRole::Strategy => NodeRole::Strategy,
        w2uos_config::NodeRole::AllInOne => NodeRole::AllInOne,
    };
    let kernel_mode = match node_config.kernel_mode {
        w2uos_config::KernelMode::Backtest => KernelMode::Backtest,
        _ => KernelMode::Live,
    };
    let node_id = NodeId(node_config.node_id.clone());
    let risk_config = w2uos_kernel::GlobalRiskConfig {
        max_daily_loss_pct: node_config.global_risk.max_daily_loss_pct,
        max_daily_loss_abs: node_config.global_risk.max_daily_loss_abs,
        max_total_notional: node_config.global_risk.max_total_notional,
        max_open_positions: node_config.global_risk.max_open_positions,
        max_error_rate_per_minute: node_config.global_risk.max_error_rate_per_minute,
    };
    info!(
        mode = %node_config.net_profile.mode,
        proxy_configured = node_config.net_profile.proxy_url.is_some(),
        "network profile loaded"
    );

    let bus: Arc<dyn MessageBus> = Arc::new(LocalBus::new());

    let mut kernel = Kernel::default()
        .with_message_bus(Arc::clone(&bus))
        .with_mode(kernel_mode);

    let config_manager = Arc::new(ConfigManagerService::new(
        "config".to_string(),
        resolved_path.clone(),
        env_name.clone(),
        Arc::clone(&bus),
        node_config.clone(),
    ));
    kernel.register_service(config_manager.clone() as Arc<dyn w2uos_kernel::Service>);

    let cluster_service = Arc::new(ClusterService::new(
        "cluster".to_string(),
        node_id.clone(),
        node_role,
        Arc::clone(&bus),
    ));
    kernel.register_service(cluster_service.clone() as Arc<dyn w2uos_kernel::Service>);

    let log_dir = node_config
        .logging
        .log_path
        .clone()
        .unwrap_or_else(|| "logs".to_string());
    let file_sink = Arc::new(FileLogSink::new(
        std::path::Path::new(&log_dir),
        "w2uos.log",
    )?);
    let sql_sink = Arc::new(SqlLogSink::connect("sqlite://./w2uos_logs.db").await?);
    let sinks: Vec<Arc<dyn LogSink>> = vec![file_sink, sql_sink];
    let composite_sink: Arc<dyn LogSink> = Arc::new(CompositeLogSink::new(sinks));
    let log_service = Arc::new(LogService::new(
        Arc::clone(&composite_sink),
        Arc::clone(&bus),
    ));
    let log_kernel_service = Arc::new(LogKernelService::new(
        "log".to_string(),
        Arc::clone(&log_service),
    ));

    kernel.register_service(log_kernel_service);

    let history_config = node_config
        .history
        .clone()
        .or_else(|| node_config.market_data.history.clone());

    let mut config = node_config.market_data.clone();
    config.net_profile = node_config.net_profile.clone();
    config.history = history_config.clone();
    let okx_exchange: Option<ExchangeConfig> = node_config
        .exchanges
        .iter()
        .find(|ex| ex.kind == ExchangeKind::Okx && ex.credentials.is_some())
        .cloned()
        .or_else(|| {
            node_config
                .exchanges
                .iter()
                .find(|ex| ex.kind == ExchangeKind::Okx)
                .cloned()
        });

    if matches!(config.mode, DataMode::LiveOkx)
        || (matches!(config.mode, DataMode::Simulated)
            && node_config.execution.live_trading_enabled)
    {
        if let Some(okx_cfg) = &okx_exchange {
            for sub in &mut config.subscriptions {
                if matches!(sub.exchange, ExchangeId::Okx) {
                    sub.ws_url = okx_cfg.ws_public_url.clone();
                }
            }
            config.mode = DataMode::LiveOkx;
        }
    }

    let market_subjects: Vec<String> = config
        .subscriptions
        .iter()
        .map(|sub| {
            format!(
                "market.{}.{}{}",
                sub.exchange,
                sub.symbol.base.to_uppercase(),
                sub.symbol.quote.to_uppercase()
            )
        })
        .collect();

    let history_store = if let Some(conn) = history_config
        .as_ref()
        .map(|cfg| cfg.connection_string.clone())
    {
        Some(HistoricalStore::connect(&conn).await?)
    } else {
        None
    };

    let backtest_coordinator = history_store
        .as_ref()
        .map(|store| Arc::new(BacktestCoordinator::new(store.clone(), Arc::clone(&bus))));

    if kernel_mode == KernelMode::Live {
        let market_service =
            Arc::new(MarketDataService::new(Arc::clone(&bus), config.clone()).await?);
        let market_kernel_service = Arc::new(MarketDataKernelService::new(
            "market-data".to_string(),
            Arc::clone(&market_service),
        ));

        kernel.register_service(market_kernel_service);
    } else if let Some(store) = history_store.clone() {
        let bt_config = BacktestConfig {
            exchange: ExchangeId::Okx,
            symbols: config
                .subscriptions
                .iter()
                .map(|s| s.symbol.clone())
                .collect(),
            start: Utc::now() - ChronoDuration::minutes(5),
            end: Utc::now(),
            speed_factor: 50.0,
        };

        let status_handle = backtest_coordinator
            .as_ref()
            .map(|c| c.status_handle())
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(Default::default())));
        let engine = w2uos_backtest::BacktestEngine::with_status(
            store,
            Arc::clone(&bus),
            bt_config,
            status_handle,
        );
        let backtest_kernel = Arc::new(BacktestKernelService::new(
            "backtest".to_string(),
            Arc::new(engine),
        ));
        kernel.register_service(backtest_kernel);
    }

    let mut exec_config: ExecCfg = node_config.execution.clone();
    if exec_config.live_trading_enabled {
        if let Some(okx_cfg) = okx_exchange.clone() {
            if let (Some(creds), Some(ws_private)) =
                (okx_cfg.credentials.clone(), okx_cfg.ws_private_url.clone())
            {
                exec_config.okx = Some(OkxExecutionConfig {
                    rest_base_url: okx_cfg.rest_base_url,
                    ws_private_url: ws_private,
                    credentials: OkxCredentials {
                        api_key: creds.api_key,
                        api_secret: creds.api_secret,
                        passphrase: creds.passphrase,
                    },
                });
                exec_config.mode = ExecutionMode::LiveOkx;
            } else {
                warn!("live trading requested but OKX credentials or private URL missing; using simulated mode");
                exec_config.live_trading_enabled = false;
                exec_config.mode = ExecutionMode::Simulated;
            }
        } else {
            warn!("no OKX exchange config found; using simulated execution");
            exec_config.mode = ExecutionMode::Simulated;
        }
    } else {
        exec_config.mode = ExecutionMode::Simulated;
    }

    let exec_service = Arc::new(ExecutionService::new(Arc::clone(&bus), exec_config)?);
    let exec_kernel_service = Arc::new(ExecutionKernelService::new(
        "execution".to_string(),
        Arc::clone(&exec_service),
    ));

    kernel.register_service(exec_kernel_service);

    let risk_service = Arc::new(RiskSupervisorService::new(
        "risk".to_string(),
        risk_config,
        Arc::clone(&bus),
    ));
    kernel.register_service(risk_service.clone() as Arc<dyn w2uos_kernel::Service>);

    let bot_config = BotConfig {
        name: "default-bot".to_string(),
        pairs: config
            .subscriptions
            .iter()
            .map(|sub| PairId {
                exchange: sub.exchange.clone(),
                symbol: sub.symbol.clone(),
            })
            .collect(),
    };

    let strategy_runtime = StrategyRuntime::new(&bot_config)?;
    let strategy_backend = StrategyBackend::from_runtime(strategy_runtime);
    let strategy_config = StrategyHostConfig {
        subscriptions: bot_config
            .pairs
            .iter()
            .map(StrategySubscription::from)
            .collect(),
        bot_config: bot_config.clone(),
    };

    let strategy_service = Arc::new(StrategyHostService::new(
        "strategy".to_string(),
        Arc::clone(&bus),
        strategy_backend,
        strategy_config,
    ));

    kernel.register_service(strategy_service);

    let kernel = Arc::new(kernel);

    let api_context = ApiContext::new(
        Arc::clone(&kernel),
        Arc::clone(&bus),
        Some(Arc::clone(&log_service)),
        Some(Arc::clone(&exec_service)),
        Some(Arc::clone(&cluster_service)),
        backtest_coordinator.clone(),
        market_subjects,
        node_config.net_profile.clone(),
        Some(Arc::clone(&risk_service)),
    );

    let api_users: Vec<ApiUserCredential> = node_config
        .api
        .users
        .iter()
        .filter_map(|user| {
            let role = Role::from_str(&user.role).unwrap_or(Role::ReadOnly);
            Some(ApiUserCredential {
                id: user.id.clone(),
                role,
                api_key_hash: user.api_key_hash.clone(),
            })
        })
        .collect();

    let api_net_profile = node_config.net_profile.clone();
    let api_bind = node_config.api.bind.clone();
    let api_bus = Arc::clone(&bus);
    let api_handle = tokio::task::spawn_blocking(move || {
        actix_rt::System::new().block_on(run_server(
            api_context,
            ApiConfig {
                users: api_users,
                net_profile: api_net_profile,
                bind: api_bind,
                bus: api_bus,
            },
        ))
    });

    let debug_bus = Arc::clone(&bus);
    tokio::spawn(async move {
        match debug_bus.subscribe("market.OKX.BTCUSDT").await {
            Ok(mut sub) => {
                while let Some(msg) = sub.receiver.recv().await {
                    match serde_json::from_slice::<MarketSnapshot>(&msg.0) {
                        Ok(snapshot) => info!(?snapshot, "received snapshot"),
                        Err(err) => warn!(?err, "failed to parse snapshot"),
                    }
                }
            }
            Err(err) => warn!(?err, "failed to subscribe to debug feed"),
        }
    });

    kernel.start_all().await?;

    let bus_for_orders = Arc::clone(&bus);
    tokio::spawn(async move {
        match bus_for_orders.subscribe("orders.result").await {
            Ok(mut sub) => {
                sleep(Duration::from_millis(200)).await;
                let command = OrderCommand {
                    id: "test-order-1".to_string(),
                    exchange: ExchangeId::Okx,
                    symbol: Symbol {
                        base: "BTC".to_string(),
                        quote: "USDT".to_string(),
                    },
                    side: OrderSide::Buy,
                    order_type: OrderType::Market,
                    size_quote: 1000.0,
                    limit_price: None,
                    trace_id: Some(TraceId::new()),
                };

                if let Ok(payload) = serde_json::to_vec(&command) {
                    if let Err(err) = bus_for_orders
                        .publish("orders.command", BusMessage(payload))
                        .await
                    {
                        warn!(?err, "failed to publish test order command");
                    }
                }

                if let Some(msg) = sub.receiver.recv().await {
                    match serde_json::from_slice::<OrderResult>(&msg.0) {
                        Ok(result) => info!(?result, "received order result"),
                        Err(err) => warn!(?err, "failed to parse order result"),
                    }
                }
            }
            Err(err) => warn!(?err, "failed to subscribe to order results"),
        }
    });

    info!("W2UOS node online");

    signal::ctrl_c().await?;
    api_handle.abort();
    Ok(())
}
