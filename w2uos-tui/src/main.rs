#![allow(dead_code)]

use std::io::stdout;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event as CEvent, KeyCode};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::time::interval;

type UiFrame<'a> = Frame<'a>;

#[derive(Clone, Debug)]
struct AppConfig {
    base_url: String,
    api_key: Option<String>,
    symbol: String,
    summary_limit: usize,
    refresh: Duration,
}

impl AppConfig {
    fn from_env() -> Self {
        let base_url =
            std::env::var("W2UOS_API_BASE").unwrap_or_else(|_| "http://localhost:8080".to_string());
        let api_key = std::env::var("W2UOS_API_KEY").ok();
        let symbol = std::env::var("W2UOS_TUI_SYMBOL").unwrap_or_else(|_| "BTC/USDT".to_string());
        let summary_limit = std::env::var("W2UOS_TUI_LIMIT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        let refresh_hz = std::env::var("W2UOS_TUI_HZ")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|hz| hz.max(1).min(20))
            .unwrap_or(8);
        let refresh = Duration::from_millis(1000 / refresh_hz);

        Self {
            base_url,
            api_key,
            symbol,
            summary_limit,
            refresh,
        }
    }
}

#[derive(Clone)]
struct ApiClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl ApiClient {
    fn new(base_url: String, api_key: Option<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;
        Ok(Self {
            base_url,
            api_key,
            client,
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut req = self.client.get(url);
        if let Some(key) = &self.api_key {
            req = req.header("X-API-KEY", key);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json().await?)
    }

    async fn status(&self) -> Result<NodeStatusDto> {
        self.get_json("status").await
    }

    async fn control_state(&self) -> Result<ControlStateDto> {
        self.get_json("control/state").await
    }

    async fn health(&self) -> Result<HealthStatusDto> {
        self.get_json("health").await
    }

    async fn system_metrics(&self) -> Result<SystemMetricsDto> {
        self.get_json("metrics/system").await
    }

    async fn market_metrics(&self, symbol: &str, limit: usize) -> Result<Option<MarketMetricsDto>> {
        let path = format!("metrics/market?symbol={}&limit={}", symbol, limit);
        self.get_json(&path).await.map(Some).or_else(|e| {
            if e.to_string().contains("404") {
                Ok(None)
            } else {
                Err(e)
            }
        })
    }

    async fn live_positions(&self) -> Result<Vec<PositionDto>> {
        self.get_json("/live/positions").await
    }

    async fn live_status(&self) -> Result<LiveStatusDto> {
        self.get_json("/live/status").await
    }
}

#[derive(Default, Clone)]
struct AppState {
    status: Option<NodeStatusDto>,
    control_state: Option<ControlStateDto>,
    health: Option<HealthStatusDto>,
    system_metrics: Option<SystemMetricsDto>,
    market_metrics: Option<MarketMetricsDto>,
    live_status: Option<LiveStatusDto>,
    positions: Vec<PositionDto>,
    last_error: Option<String>,
    last_snapshot_ts: Option<DateTime<Utc>>,
    last_snapshot_instant: Option<Instant>,
    snapshot_rate: Option<f64>,
}

impl AppState {
    fn record_snapshot(&mut self, ts: Option<DateTime<Utc>>) {
        let Some(new_ts) = ts else { return };

        if self
            .last_snapshot_ts
            .map(|prev| prev != new_ts)
            .unwrap_or(true)
        {
            if let Some(prev_instant) = self.last_snapshot_instant.take() {
                let elapsed = prev_instant.elapsed().as_secs_f64();
                if elapsed > 0.0 {
                    self.snapshot_rate = Some(1.0 / elapsed);
                }
            }
            self.last_snapshot_ts = Some(new_ts);
            self.last_snapshot_instant = Some(Instant::now());
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct NodeStatusDto {
    kernel_state: String,
    kernel_mode: String,
    trading_mode: String,
    trading_state: String,
    node_id: String,
    env: String,
    armed_exchanges: Vec<String>,
    risk_profile: String,
    max_leverage: i32,
    max_notional_usdt: f64,
    max_concurrent_positions: i32,
    services: Vec<ServiceStatusDto>,
}

#[derive(Debug, Deserialize, Clone)]
struct ServiceStatusDto {
    id: String,
    state: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ControlStateDto {
    trading_state: String,
    risk_profile: String,
    live_switch_armed: bool,
    live_switch_reason: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct HealthStatusDto {
    kernel_state: String,
    market_connected: bool,
    execution_connected: bool,
    ws_public_alive: bool,
    ws_private_alive: bool,
}

#[derive(Debug, Deserialize, Clone)]
struct SystemMetricsDto {
    node: NodeInfoDto,
    runtime: RuntimeInfoDto,
    strategy: StrategyInfoDto,
    risk: RiskInfoDto,
    ibmq: IbmqInfoDto,
    x402: X402InfoDto,
}

#[derive(Debug, Deserialize, Clone)]
struct NodeInfoDto {
    node_id: String,
    env: String,
    version: String,
    uptime_seconds: u64,
    hostname: String,
    trading_state: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RuntimeInfoDto {
    cpu_load_pct: f64,
    mem_used_mb: f64,
    mem_total_mb: f64,
    disk_used_gb: f64,
    disk_total_gb: f64,
}

#[derive(Debug, Deserialize, Clone)]
struct StrategyInfoDto {
    active_strategies: Vec<String>,
    signals_per_minute: f64,
    avg_decision_latency_ms: f64,
    error_count_last_10m: u64,
}

#[derive(Debug, Deserialize, Clone)]
struct RiskInfoDto {
    risk_profile: String,
    max_leverage: i32,
    max_notional_usdt: f64,
    max_concurrent_positions: i32,
    current_notional_usdt: f64,
    open_positions_count: i32,
    intraday_drawdown_pct: f64,
}

#[derive(Debug, Deserialize, Clone)]
struct IbmqInfoDto {
    enabled: bool,
    role: String,
    last_job_ts: Option<String>,
    last_job_status: Option<String>,
    last_solution_score: Option<f64>,
    schedule_mode: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct X402InfoDto {
    enabled: bool,
    role: String,
    registered_agents: u32,
    pending_settlements: u32,
    last_settlement_ts: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct MarketMetricsDto {
    symbol: String,
    exchange: String,
    last_price: f64,
    bid: f64,
    ask: f64,
    spread_bp: f64,
    volume_24h: f64,
    funding_rate: Option<f64>,
    open_interest: Option<f64>,
    volatility_1h_pct: Option<f64>,
    volatility_24h_pct: Option<f64>,
    orderbook: OrderbookSummaryDto,
    recent_trades: Vec<RecentTradeDto>,
}

#[derive(Debug, Deserialize, Clone)]
struct OrderbookSummaryDto {
    best_bid_size: f64,
    best_ask_size: f64,
    bid_depth_usdt: f64,
    ask_depth_usdt: f64,
}

#[derive(Debug, Deserialize, Clone)]
struct RecentTradeDto {
    ts: String,
    side: String,
    price: f64,
    size_quote: f64,
}

#[derive(Debug, Deserialize, Clone)]
struct PositionDto {
    symbol: String,
    base: String,
    quote: String,
    position_base: f64,
    balance_usdt: f64,
}

#[derive(Debug, Deserialize, Clone)]
struct LiveStatusDto {
    exchange: String,
    mode: String,
    symbols: Vec<String>,
    market_connected: bool,
    last_snapshot_ts: Option<DateTime<Utc>>,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        stdout()
            .execute(EnterAlternateScreen)
            .context("switch to alternate screen")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = AppConfig::from_env();
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let client = ApiClient::new(cfg.base_url.clone(), cfg.api_key.clone())?;

    if let Err(err) = run_tui(&mut terminal, client, cfg).await {
        eprintln!("TUI error: {err:?}");
    }

    Ok(())
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    client: ApiClient,
    cfg: AppConfig,
) -> Result<()> {
    let mut state = AppState::default();
    let mut tick = interval(cfg.refresh);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(err) = refresh(&client, &mut state, &cfg).await {
                    state.last_error = Some(err.to_string());
                }
                terminal.draw(|f| draw_ui(f, &state, &cfg))?;
            }
            _ = tokio::signal::ctrl_c() => break,
            maybe_evt = poll_key_event() => {
                if let Some(evt) = maybe_evt? {
                    if matches!(evt, CEvent::Key(k) if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)) {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn poll_key_event() -> Result<Option<CEvent>> {
    tokio::task::spawn_blocking(|| {
        if event::poll(Duration::from_millis(10))? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    })
    .await?
}

async fn refresh(client: &ApiClient, state: &mut AppState, cfg: &AppConfig) -> Result<()> {
    let mut errors = Vec::new();
    let status = client.status().await;
    if let Ok(value) = status {
        state.status = Some(value);
    } else if let Err(err) = status {
        errors.push(format!("status: {err}"));
    }

    let control = client.control_state().await;
    if let Ok(value) = control {
        state.control_state = Some(value);
    } else if let Err(err) = control {
        errors.push(format!("control: {err}"));
    }

    let health = client.health().await;
    if let Ok(value) = health {
        state.health = Some(value);
    } else if let Err(err) = health {
        errors.push(format!("health: {err}"));
    }

    let system_metrics = client.system_metrics().await;
    if let Ok(metrics) = system_metrics {
        state.system_metrics = Some(metrics);
    } else if let Err(err) = system_metrics {
        errors.push(format!("metrics: {err}"));
    }

    match client.market_metrics(&cfg.symbol, cfg.summary_limit).await {
        Ok(Some(market)) => state.market_metrics = Some(market),
        Ok(None) => errors.push("symbol not found".to_string()),
        Err(err) => errors.push(format!("market: {err}")),
    }

    let live_status = client.live_status().await;
    if let Ok(live) = live_status {
        state.record_snapshot(live.last_snapshot_ts);
        state.live_status = Some(live);
    } else if let Err(err) = live_status {
        errors.push(format!("live: {err}"));
    }

    let positions = client.live_positions().await;
    if let Ok(pos) = positions {
        state.positions = pos;
    } else if let Err(err) = positions {
        errors.push(format!("positions: {err}"));
    }

    state.last_error = errors.into_iter().reduce(|a, b| format!("{a}; {b}"));

    Ok(())
}

fn draw_ui(frame: &mut UiFrame, state: &AppState, cfg: &AppConfig) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(40)])
        .split(frame.size());

    draw_system_panel(frame, chunks[0], state, cfg);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(chunks[1]);

    draw_market_panel(frame, right[0], state, cfg);
    draw_positions_panel(frame, right[1], state);
}

fn draw_system_panel(frame: &mut UiFrame, area: Rect, state: &AppState, cfg: &AppConfig) {
    let block = Block::default()
        .title("SYSTEM")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let status = state.status.as_ref();
    let control = state.control_state.as_ref();
    let health = state.health.as_ref();
    let metrics = state.system_metrics.as_ref();

    let node_id = status.map(|s| s.node_id.as_str()).unwrap_or("n/a");
    let kernel_mode = status.map(|s| s.kernel_mode.as_str()).unwrap_or("n/a");
    let execution_mode = status.map(|s| s.trading_mode.as_str()).unwrap_or("n/a");
    let exchange = status
        .and_then(|s| s.armed_exchanges.first())
        .map(|s| s.as_str())
        .unwrap_or("n/a");
    let live_switch = control
        .map(|c| if c.live_switch_armed { "ARMED" } else { "OFF" })
        .unwrap_or("n/a");
    let ws_status = health
        .map(|h| {
            format!(
                "pub:{} priv:{}",
                flag(h.ws_public_alive),
                flag(h.ws_private_alive)
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let rest_status = health
        .map(|h| {
            format!(
                "market:{} exec:{}",
                flag(h.market_connected),
                flag(h.execution_connected)
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let uptime = metrics
        .map(|m| format_duration(m.node.uptime_seconds))
        .unwrap_or_else(|| "n/a".to_string());
    let last_error = state
        .last_error
        .clone()
        .unwrap_or_else(|| "none".to_string());
    let ibmq = metrics
        .map(|m| {
            format!(
                "{} ({})",
                if m.ibmq.enabled { "on" } else { "off" },
                m.ibmq.role
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let x402 = metrics
        .map(|m| {
            format!(
                "{} ({})",
                if m.x402.enabled { "on" } else { "off" },
                m.x402.role
            )
        })
        .unwrap_or_else(|| "n/a".to_string());

    let content = vec![
        Line::from(vec![Span::styled("Node ID: ", bold()), Span::raw(node_id)]),
        Line::from(vec![
            Span::styled("Kernel Mode: ", bold()),
            Span::raw(kernel_mode),
        ]),
        Line::from(vec![
            Span::styled("Exchange: ", bold()),
            Span::raw(exchange),
        ]),
        Line::from(vec![
            Span::styled("Execution Mode: ", bold()),
            Span::raw(execution_mode),
        ]),
        Line::from(vec![
            Span::styled("LiveSwitch: ", bold()),
            Span::styled(live_switch, live_switch_style(live_switch)),
        ]),
        Line::from(vec![
            Span::styled("WebSocket: ", bold()),
            Span::raw(ws_status),
        ]),
        Line::from(vec![Span::styled("REST: ", bold()), Span::raw(rest_status)]),
        Line::from(vec![Span::styled("Uptime: ", bold()), Span::raw(uptime)]),
        Line::from(vec![
            Span::styled("Snapshot rate: ", bold()),
            Span::raw(
                state
                    .snapshot_rate
                    .map(|r| format!("{r:.2} msg/s"))
                    .unwrap_or_else(|| "n/a".to_string()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Last error: ", bold()),
            Span::raw(last_error),
        ]),
        Line::from(vec![Span::styled("IBMQ: ", bold()), Span::raw(ibmq)]),
        Line::from(vec![Span::styled("X402: ", bold()), Span::raw(x402)]),
        Line::from(vec![
            Span::styled("API: ", bold()),
            Span::raw(cfg.base_url.clone()),
        ]),
    ];

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_market_panel(frame: &mut UiFrame, area: Rect, state: &AppState, _cfg: &AppConfig) {
    let block = Block::default()
        .title("MARKET")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    if let Some(mkt) = state.market_metrics.as_ref() {
        let ws_latency = state
            .live_status
            .as_ref()
            .and_then(|ls| ls.last_snapshot_ts)
            .map(|ts| (Utc::now() - ts).num_milliseconds())
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "n/a".to_string());

        let lines = vec![
            Line::from(vec![
                Span::styled("Exchange: ", bold()),
                Span::raw(&mkt.exchange),
            ]),
            Line::from(vec![
                Span::styled("Symbol: ", bold()),
                Span::raw(&mkt.symbol),
            ]),
            Line::from(vec![
                Span::styled("Last: ", bold()),
                Span::raw(format_price(mkt.last_price)),
            ]),
            Line::from(vec![
                Span::styled("Bid/Ask: ", bold()),
                Span::raw(format!(
                    "{} / {}",
                    format_price(mkt.bid),
                    format_price(mkt.ask)
                )),
            ]),
            Line::from(vec![
                Span::styled("Spread bp: ", bold()),
                Span::raw(format!("{:.2}", mkt.spread_bp)),
            ]),
            Line::from(vec![
                Span::styled("24h Vol: ", bold()),
                Span::raw(format!("{:.2}", mkt.volume_24h)),
            ]),
            Line::from(vec![
                Span::styled("WS Latency: ", bold()),
                Span::raw(ws_latency),
            ]),
            Line::from(vec![
                Span::styled("Snapshot rate: ", bold()),
                Span::raw(
                    state
                        .snapshot_rate
                        .map(|r| format!("{r:.2} msg/s"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
            ]),
        ];

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    } else {
        let paragraph = Paragraph::new("Waiting for market data...").block(block);
        frame.render_widget(paragraph, area);
    }
}

fn draw_positions_panel(frame: &mut UiFrame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title("POSITIONS")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green));

    let header = Row::new(vec![
        Cell::from("Symbol"),
        Cell::from("Side"),
        Cell::from("Size"),
        Cell::from("Entry"),
        Cell::from("Mark"),
        Cell::from("Unrealized"),
        Cell::from("Realized"),
        Cell::from("Margin"),
        Cell::from("Risk"),
        Cell::from("OrderId"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let mut rows = Vec::new();
    for pos in &state.positions {
        let side = if pos.position_base >= 0.0 {
            "Long"
        } else {
            "Short"
        };
        rows.push(Row::new(vec![
            Cell::from(pos.symbol.clone()),
            Cell::from(side),
            Cell::from(format!("{:.4}", pos.position_base)),
            Cell::from("n/a"),
            Cell::from(
                state
                    .market_metrics
                    .as_ref()
                    .filter(|m| {
                        m.symbol
                            .replace('-', "/")
                            .eq_ignore_ascii_case(&pos.symbol.replace('-', "/"))
                    })
                    .map(|m| format_price(m.last_price))
                    .unwrap_or_else(|| "n/a".to_string()),
            ),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
        ]));
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
            Cell::from("—"),
        ]));
    }

    let widths = [
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(12),
    ];

    let table = Table::new(rows, widths).header(header).block(block);

    frame.render_widget(table, area);
}

fn format_price(price: f64) -> String {
    if price.abs() >= 1_000.0 {
        format!("{price:.2}")
    } else {
        format!("{price:.6}")
    }
}

fn flag(value: bool) -> &'static str {
    if value {
        "ok"
    } else {
        "down"
    }
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, seconds)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn live_switch_style(state: &str) -> Style {
    match state.to_ascii_uppercase().as_str() {
        "ARMED" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Gray),
    }
}
