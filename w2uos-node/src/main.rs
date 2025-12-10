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
use w2uos_config::{ExchangeConfig, ExchangeMode, NodeConfig};
use w2uos_data::{
    okx::instrument_to_symbol, ExchangeDataConfig, ExchangeDataMode, ExchangeId, HistoricalStore,
    MarketDataKernelService, MarketDataService, MarketDataSubscription, MarketSnapshot, Symbol,
    TradingMode,
};
use w2uos_exec::{
    BinanceCredentials, BinanceExecutionConfig, ExecutionConfig as ExecCfg, ExecutionKernelService,
    ExecutionMode, ExecutionService, OkxCredentials, OkxExecutionConfig, OrderCommand, OrderResult,
    OrderSide, OrderType, PaperExecutionConfig,
};
use w2uos_kernel::{
    ClusterService, ConfigManagerService, Kernel, KernelMode, NodeId, NodeRole,
    RiskSupervisorService, StrategyBackend, StrategyHostConfig, StrategyHostService,
    StrategySubscription, TradingState,
};
use w2uos_log::{CompositeLogSink, FileLogSink, LogKernelService, LogService, LogSink, SqlLogSink};
use w2uos_service::TraceId;
use wru_strategy::{BotConfig, PairId, StrategyRuntime};

#[tokio::main]
async fn main() -> Result<()> {
    Kernel::init_tracing();
    let env_name = std::env::var("NODE_ENV").ok();
    let env_label = env_name.clone().unwrap_or_else(|| "dev".to_string());
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
    let exchange_cfg: Option<ExchangeConfig> = node_config.exchange.clone();
    let exchange_data_cfg: Option<ExchangeDataConfig> =
        exchange_cfg.as_ref().map(|ex| ExchangeDataConfig {
            name: ex.name.clone(),
            mode: match ex.mode {
                ExchangeMode::Simulation => ExchangeDataMode::Simulation,
                ExchangeMode::LivePaper => ExchangeDataMode::LivePaper,
                ExchangeMode::LiveReal => ExchangeDataMode::LiveReal,
            },
            ws_public_url: ex.ws_public_url.clone(),
        });

    if config.subscriptions.is_empty() && !config.symbols.is_empty() {
        for inst_id in &config.symbols {
            if let Some(symbol) = instrument_to_symbol(inst_id) {
                let ws_url = exchange_data_cfg
                    .as_ref()
                    .map(|ex| ex.ws_public_url.clone())
                    .unwrap_or_default();

                config.subscriptions.push(MarketDataSubscription {
                    exchange: ExchangeId::Okx,
                    symbol,
                    ws_url,
                });
            } else {
                warn!(
                    inst_id = inst_id,
                    "unable to parse instrument id from config; skipping"
                );
            }
        }
    }

    let mut trading_mode = node_config.execution.mode.clone();
    if trading_mode != node_config.market_data.mode {
        warn!("execution and market data modes differed; using execution mode");
    }

    if !node_config.execution.live_trading_enabled
        && !matches!(trading_mode, TradingMode::Simulated)
    {
        warn!("live trading disabled; falling back to simulated mode");
        trading_mode = TradingMode::Simulated;
    }

    match trading_mode {
        TradingMode::LiveOkx => {
            if let Some(okx_cfg) = exchange_data_cfg
                .as_ref()
                .filter(|ex| ex.name.eq_ignore_ascii_case("okx"))
            {
                for sub in &mut config.subscriptions {
                    if matches!(sub.exchange, ExchangeId::Okx) {
                        sub.ws_url = okx_cfg.ws_public_url.clone();
                    }
                }
            } else {
                warn!("OKX mode requested but exchange config missing; using simulated");
                trading_mode = TradingMode::Simulated;
            }
        }
        TradingMode::LiveBinance => {
            if let Some(binance_cfg) = exchange_cfg
                .as_ref()
                .filter(|ex| ex.name.eq_ignore_ascii_case("binance"))
            {
                for sub in &mut config.subscriptions {
                    if matches!(sub.exchange, ExchangeId::Binance) {
                        sub.ws_url = binance_cfg.ws_public_url.clone();
                    }
                }
            } else {
                warn!("Binance mode requested but exchange config missing; using simulated");
                trading_mode = TradingMode::Simulated;
            }
        }
        TradingMode::Simulated => {}
    }
    config.mode = trading_mode.clone();

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
    let subscribed_symbols: Vec<String> = if !config.symbols.is_empty() {
        config.symbols.clone()
    } else {
        config
            .subscriptions
            .iter()
            .map(|sub| {
                format!(
                    "{}-{}",
                    sub.symbol.base.to_uppercase(),
                    sub.symbol.quote.to_uppercase()
                )
            })
            .collect()
    };

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
        let market_service = Arc::new(
            MarketDataService::new(Arc::clone(&bus), config.clone(), exchange_data_cfg.clone())
                .await?,
        );
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
    exec_config.mode = trading_mode.clone();
    exec_config.execution_mode = match exchange_cfg.as_ref().map(|ex| ex.mode) {
        Some(ExchangeMode::LivePaper) => ExecutionMode::Paper,
        Some(ExchangeMode::LiveReal) => ExecutionMode::Live,
        _ => ExecutionMode::Sim,
    };
    exec_config.paper_trading = matches!(
        exchange_cfg.as_ref().map(|ex| ex.mode),
        Some(ExchangeMode::LivePaper)
    );
    if exec_config.paper_trading && exec_config.paper.is_none() {
        exec_config.paper = Some(PaperExecutionConfig::default());
    }
    if exec_config.live_trading_enabled {
        match trading_mode {
            TradingMode::LiveOkx => {
                if let Some(okx_cfg) = exchange_cfg
                    .clone()
                    .filter(|ex| ex.name.eq_ignore_ascii_case("okx"))
                {
                    if matches!(
                        okx_cfg.mode,
                        ExchangeMode::LivePaper | ExchangeMode::LiveReal
                    ) {
                        if let Ok(credentials) = okx_cfg.load_credentials_from_env() {
                            exec_config.okx = Some(OkxExecutionConfig {
                                rest_base_url: okx_cfg.rest_base_url,
                                ws_private_url: okx_cfg.ws_private_url,
                                credentials,
                            });
                        } else {
                            warn!("live OKX mode requested but credentials missing; using simulated mode");
                            exec_config.live_trading_enabled = false;
                            exec_config.mode = TradingMode::Simulated;
                        }
                    } else {
                        warn!("OKX exchange config not in live mode; using simulated execution");
                        exec_config.live_trading_enabled = false;
                        exec_config.mode = TradingMode::Simulated;
                    }
                } else {
                    warn!("no OKX exchange config found; using simulated execution");
                    exec_config.mode = TradingMode::Simulated;
                }
            }
            TradingMode::LiveBinance => {
                if let Some(_binance_cfg) = exchange_cfg
                    .clone()
                    .filter(|ex| ex.name.eq_ignore_ascii_case("binance"))
                {
                    warn!("Binance exchange configuration present but credential loading not implemented for new format; using simulated mode");
                    exec_config.live_trading_enabled = false;
                    exec_config.mode = TradingMode::Simulated;
                } else {
                    warn!("no Binance exchange config found; using simulated execution");
                    exec_config.mode = TradingMode::Simulated;
                }
            }
            TradingMode::Simulated => {
                exec_config.mode = TradingMode::Simulated;
            }
        }
    } else {
        exec_config.mode = TradingMode::Simulated;
        exec_config.okx = None;
        exec_config.binance = None;
        exec_config.paper_trading = false;
        exec_config.execution_mode = ExecutionMode::Sim;
    }

    if matches!(exec_config.execution_mode, ExecutionMode::Live)
        && !exec_config.live_trading_enabled
    {
        exec_config.execution_mode = ExecutionMode::Sim;
    }

    if matches!(exec_config.mode, TradingMode::Simulated)
        && !matches!(exec_config.execution_mode, ExecutionMode::Sim)
    {
        exec_config.execution_mode = ExecutionMode::Sim;
    }

    let armed_exchanges: Vec<String> = {
        let mut exchanges = Vec::new();
        if exec_config.live_trading_enabled {
            if matches!(exec_config.mode, TradingMode::LiveOkx) && exec_config.okx.is_some() {
                exchanges.push("OKX".to_string());
            }
            if matches!(exec_config.mode, TradingMode::LiveBinance) && exec_config.binance.is_some()
            {
                exchanges.push("Binance".to_string());
            }
        }
        exchanges
    };

    let initial_trading_state = match exec_config.execution_mode {
        ExecutionMode::Sim => TradingState::Simulating,
        ExecutionMode::Paper => TradingState::LiveDryRun,
        ExecutionMode::Live => TradingState::LiveArmed,
    };

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

    let exchange_profile = exchange_cfg
        .as_ref()
        .map(|ex| (ex.name.clone(), format!("{:?}", ex.mode)));

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
        trading_mode.clone(),
        armed_exchanges.clone(),
        exchange_profile,
        subscribed_symbols,
        node_id.0.clone(),
        env_label.clone(),
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
    kernel.set_trading_state(initial_trading_state);

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
