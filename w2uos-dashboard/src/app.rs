use std::collections::HashMap;

use futures_util::StreamExt;
use gloo_console::log;
use gloo_net::{
    http::Request,
    websocket::{futures::WebSocket, Message},
};
use gloo_timers::callback::Interval;
use js_sys::Date;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::panels::{DashboardLayout, DashboardPanelConfig, PanelDataSource, PanelType};

const API_BASE: &str = "http://localhost:8080";
const WS_URL: &str = "ws://localhost:8080/ws/stream";
const LAYOUT_STORAGE_KEY: &str = "w2uos_dashboard_layout_v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeStatusDto {
    pub kernel_state: String,
    pub services: Vec<ServiceStatusDto>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatusDto {
    pub id: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatencySummaryDto {
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionDto {
    pub symbol: String,
    pub base: String,
    pub quote: String,
    pub position_base: f64,
    pub balance_usdt: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogSource {
    Kernel,
    MarketData,
    Execution,
    Strategy,
    Api,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogDto {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
    pub fields: serde_json::Value,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExchangeId {
    Okx,
    Binance,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolDto {
    pub base: String,
    pub quote: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub exchange: ExchangeId,
    pub symbol: SymbolDto,
    pub last: f64,
    pub bid: f64,
    pub ask: f64,
    pub volume_24h: f64,
    pub trace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    Filled,
    Rejected(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderResult {
    pub command_id: String,
    pub exchange: Option<String>,
    pub symbol: Option<MarketSymbol>,
    #[serde(default)]
    pub side: Option<String>,
    pub status: OrderStatus,
    pub filled_size_quote: f64,
    pub avg_price: f64,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub trace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketSymbol {
    pub base: String,
    pub quote: String,
}

#[derive(Clone, Debug, PartialEq)]
enum StreamPayload {
    Market(MarketSnapshot),
    Order(OrderResult),
    Log(LogDto),
}

fn parse_stream_payload(value: &Value) -> Option<StreamPayload> {
    if let Ok(market) = serde_json::from_value::<MarketSnapshot>(value.clone()) {
        return Some(StreamPayload::Market(market));
    }
    if let Ok(order) = serde_json::from_value::<OrderResult>(value.clone()) {
        if order.filled_size_quote != 0.0 || matches!(order.status, OrderStatus::Rejected(_)) {
            return Some(StreamPayload::Order(order));
        }
    }
    serde_json::from_value::<LogDto>(value.clone())
        .ok()
        .map(StreamPayload::Log)
}

fn subject_from_payload(value: &Value) -> Option<String> {
    match parse_stream_payload(value)? {
        StreamPayload::Market(m) => Some(format!(
            "market.{}.{}{}",
            format_exchange(&m.exchange),
            m.symbol.base,
            m.symbol.quote
        )),
        StreamPayload::Order(_) => Some("orders.result".to_string()),
        StreamPayload::Log(_) => Some("log.event".to_string()),
    }
}

fn matches_subject(pattern: &str, subject: &str) -> bool {
    if pattern.ends_with('*') {
        let prefix = pattern.trim_end_matches('*');
        subject.starts_with(prefix)
    } else {
        pattern == subject
    }
}

fn now_time_string() -> String {
    Date::new_0()
        .to_locale_time_string("en-US")
        .as_string()
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn load_layout_from_storage() -> Option<DashboardLayout> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(LAYOUT_STORAGE_KEY).ok().flatten())
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn persist_layout(layout: &DashboardLayout) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        if let Ok(text) = serde_json::to_string(layout) {
            let _ = storage.set_item(LAYOUT_STORAGE_KEY, &text);
        }
    }
}

#[function_component(App)]
pub fn app() -> Html {
    let layout = use_state(|| load_layout_from_storage().unwrap_or_else(DashboardLayout::default));
    let layout_mode = use_state(|| false);
    let rest_data = use_state(HashMap::<String, Value>::new);
    let ws_data = use_state(HashMap::<String, Vec<Value>>::new);
    let clock = use_state(now_time_string);

    {
        let layout = layout.clone();
        use_effect_with(layout.clone(), move |layout_cfg| {
            persist_layout(layout_cfg);
            || ()
        });
    }

    {
        let layout_snapshot = (*layout).clone();
        let rest_data = rest_data.clone();
        use_effect_with(layout_snapshot, move |layout_cfg| {
            for panel in &layout_cfg.panels {
                if let PanelDataSource::Rest { path } = &panel.data_source {
                    let path = path.clone();
                    let rest_data = rest_data.clone();
                    spawn_local(async move {
                        if let Ok(resp) =
                            Request::get(&format!("{}{}", API_BASE, path)).send().await
                        {
                            if let Ok(json) = resp.json::<Value>().await {
                                rest_data.set({
                                    let mut map = (*rest_data).clone();
                                    map.insert(path.clone(), json);
                                    map
                                });
                            }
                        }
                    });
                }
            }
            let rest_data = rest_data.clone();
            let layout_inner = layout_cfg.clone();
            let interval = Interval::new(5000, move || {
                for panel in &layout_inner.panels {
                    if let PanelDataSource::Rest { path } = &panel.data_source {
                        let path = path.clone();
                        let rest_data = rest_data.clone();
                        spawn_local(async move {
                            if let Ok(resp) =
                                Request::get(&format!("{}{}", API_BASE, path)).send().await
                            {
                                if let Ok(json) = resp.json::<Value>().await {
                                    rest_data.set({
                                        let mut map = (*rest_data).clone();
                                        map.insert(path.clone(), json);
                                        map
                                    });
                                }
                            }
                        });
                    }
                }
            });
            move || drop(interval)
        });
    }

    {
        let ws_data = ws_data.clone();
        let patterns: Vec<String> = layout
            .iter()
            .flat_map(|cfg| cfg.panels.iter())
            .filter_map(|p| match &p.data_source {
                PanelDataSource::WsSubject { subject } => Some(subject.clone()),
                _ => None,
            })
            .collect();
        use_effect_with((), move |_| {
            let ws = WebSocket::open(WS_URL);
            match ws {
                Ok(ws) => {
                    let (_write, mut read) = ws.split();
                    spawn_local(async move {
                        while let Some(Ok(Message::Text(txt))) = read.next().await {
                            if let Ok(value) = serde_json::from_str::<Value>(&txt) {
                                if let Some(subject) = subject_from_payload(&value) {
                                    for pattern in &patterns {
                                        if matches_subject(pattern, &subject) {
                                            ws_data.set({
                                                let mut map = (*ws_data).clone();
                                                let entry = map.entry(pattern.clone()).or_default();
                                                entry.push(value.clone());
                                                if entry.len() > 200 {
                                                    let excess = entry.len() - 200;
                                                    entry.drain(0..excess);
                                                }
                                                map
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
                Err(err) => {
                    log!(format!("ws connection failed: {err:?}"));
                }
            }
            || ()
        });
    }

    {
        let clock = clock.clone();
        use_effect_with((), move |_| {
            let interval = Interval::new(1000, move || clock.set(now_time_string()));
            move || drop(interval)
        });
    }

    let move_panel = {
        let layout = layout.clone();
        Callback::from(move |(index, direction): (usize, isize)| {
            layout.set({
                let mut updated = (*layout).clone();
                let len = updated.panels.len();
                if direction < 0 && index > 0 {
                    updated.panels.swap(index, index - 1);
                } else if direction > 0 && index + 1 < len {
                    updated.panels.swap(index, index + 1);
                }
                updated
            });
        })
    };

    let header = {
        let layout_mode = layout_mode.clone();
        html! {
            <div class="flex items-center justify-between border border-green-700 bg-green-950/40 px-4 py-3 shadow-[0_0_10px_rgba(16,185,129,0.4)]">
                <div class="text-2xl font-bold tracking-wide text-amber-300">{"W2UOS"} <span class="text-cyan-400">{"Dashboard"}</span></div>
                <div class="flex space-x-4 items-center text-sm">
                    <button class="px-2 py-1 border border-cyan-600 rounded text-cyan-200 hover:bg-cyan-900/40"
                        onclick={Callback::from(move |_| layout_mode.set(!*layout_mode))}>
                        { if *layout_mode { "Exit layout" } else { "Layout mode" } }
                    </button>
                    <div class="text-cyan-300">{ (*clock).clone() }</div>
                </div>
            </div>
        }
    };

    html! {
        <div class="min-h-screen bg-black text-green-300 font-mono p-4 space-y-4">
            { header }
            <div class="grid md:grid-cols-2 lg:grid-cols-3 gap-4">
                { for layout.panels.iter().enumerate().map(|(idx, panel)| {
                    let panel_cfg = panel.clone();
                    let rest = match &panel_cfg.data_source {
                        PanelDataSource::Rest { path } => rest_data.get(path).cloned(),
                        _ => None,
                    };
                    let ws = match &panel_cfg.data_source {
                        PanelDataSource::WsSubject { subject } => ws_data.get(subject).cloned(),
                        _ => None,
                    };
                    let move_panel = move_panel.clone();
                    let layout_mode = *layout_mode;
                    html! { <PanelView
                        key={panel_cfg.id.clone()}
                        config={panel_cfg}
                        rest_data={rest}
                        ws_data={ws.unwrap_or_default()}
                        layout_mode={layout_mode}
                        on_move_up={Callback::from(move |_| move_panel.emit((idx, -1)))}
                        on_move_down={Callback::from(move |_| move_panel.emit((idx, 1)))}
                    /> }
                }) }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct PanelViewProps {
    config: DashboardPanelConfig,
    rest_data: Option<Value>,
    ws_data: Vec<Value>,
    layout_mode: bool,
    on_move_up: Callback<()>,
    on_move_down: Callback<()>,
}

#[function_component(PanelView)]
fn panel_view(props: &PanelViewProps) -> Html {
    let content = render_panel_content(&props.config, props.rest_data.clone(), &props.ws_data);
    html! {
        <div class="border border-green-800 bg-green-950/30 p-3 shadow-[0_0_10px_rgba(16,185,129,0.25)] flex flex-col space-y-2">
            <div class="flex items-center justify-between">
                <div class="text-lg text-amber-200">{ props.config.title.clone() }</div>
                <div class="flex space-x-2">
                    { if props.layout_mode {
                        html! {
                            <>
                                <button class="px-2 py-1 text-xs border border-cyan-700 rounded hover:bg-cyan-900/50" onclick={props.on_move_up.clone()}>{{"↑"}}</button>
                                <button class="px-2 py-1 text-xs border border-cyan-700 rounded hover:bg-cyan-900/50" onclick={props.on_move_down.clone()}>{{"↓"}}</button>
                            </>
                        }
                    } else { Html::default() } }
                </div>
            </div>
            { content }
        </div>
    }
}

fn render_panel_content(
    config: &DashboardPanelConfig,
    rest_data: Option<Value>,
    ws_data: &[Value],
) -> Html {
    match config.panel_type {
        PanelType::Table => match config.id.as_str() {
            "node_status" => render_status(rest_data),
            "markets" => render_markets(ws_data),
            "positions" => render_positions(rest_data),
            _ => render_generic_table(rest_data, ws_data),
        },
        PanelType::Log => render_logs(ws_data),
        PanelType::Chart => match config.id.as_str() {
            "performance" => render_latency(rest_data),
            _ => render_generic_chart(ws_data),
        },
        PanelType::Custom => render_generic_table(rest_data, ws_data),
    }
}

fn render_status(rest_data: Option<Value>) -> Html {
    let Some(value) = rest_data else {
        return html! {<div class="text-xs text-cyan-200">{"Loading status..."}</div>};
    };
    let dto: Option<NodeStatusDto> = serde_json::from_value(value).ok();
    let Some(dto) = dto else {
        return html! {<div class="text-xs text-red-300">{"Invalid status payload"}</div>};
    };
    html! {
        <div class="space-y-2 text-sm">
            <div class="flex items-center space-x-2">
                <span class="text-amber-200">{"Kernel"}</span>
                <span class="px-2 py-1 rounded border border-green-700 bg-green-900/60">{dto.kernel_state}</span>
            </div>
            <div class="space-y-1">
                { for dto.services.iter().map(|svc| html! {
                    <div class="flex justify-between border border-green-900/50 rounded px-2 py-1">
                        <span class="text-cyan-200">{svc.id.clone()}</span>
                        <span class="text-green-300">{svc.state.clone()}</span>
                    </div>
                }) }
            </div>
        </div>
    }
}

fn render_positions(rest_data: Option<Value>) -> Html {
    let Some(value) = rest_data else {
        return html! {<div class="text-xs text-orange-200">{"Loading positions..."}</div>};
    };
    let list: Vec<PositionDto> = serde_json::from_value(value).unwrap_or_default();
    if list.is_empty() {
        return html! {<div class="text-xs text-orange-300">{"No open positions"}</div>};
    }
    html! {
        <div class="space-y-1 text-sm">
            { for list.iter().map(|pos| html! {
                <div class="flex justify-between border border-orange-900/50 rounded px-2 py-1">
                    <span class="text-amber-200">{pos.symbol.clone()}</span>
                    <span class="text-cyan-300">{format!("{:.4} {}", pos.position_base, pos.base)}</span>
                    <span class="text-green-200">{format!("USDT {:.2}", pos.balance_usdt)}</span>
                </div>
            }) }
        </div>
    }
}

fn render_markets(ws_data: &[Value]) -> Html {
    let mut latest: HashMap<String, MarketSnapshot> = HashMap::new();
    for val in ws_data.iter() {
        if let Ok(snap) = serde_json::from_value::<MarketSnapshot>(val.clone()) {
            let key = format!("{}{}", snap.symbol.base, snap.symbol.quote);
            latest.insert(key, snap);
        }
    }
    if latest.is_empty() {
        return html! {<div class="text-xs text-cyan-200">{"Awaiting ticks..."}</div>};
    }
    let mut rows: Vec<_> = latest.into_values().collect();
    rows.sort_by(|a, b| format_symbol(&a.symbol).cmp(&format_symbol(&b.symbol)));
    html! {
        <div class="text-sm space-y-1">
            <div class="grid grid-cols-5 text-xs text-green-400 border-b border-green-800 pb-1 mb-2">
                <span>{"Exchange"}</span><span>{"Symbol"}</span><span class="text-right">{"Last"}</span><span class="text-right">{"Bid/Ask"}</span><span class="text-right">{"Vol 24h"}</span>
            </div>
            { for rows.iter().map(|row| html! {
                <div class="grid grid-cols-5 border border-green-900/60 rounded px-2 py-1 hover:border-cyan-500/70 transition">
                    <span class="text-amber-200">{format_exchange(&row.exchange)}</span>
                    <span>{format_symbol(&row.symbol)}</span>
                    <span class="text-right text-green-200">{format!("{:.2}", row.last)}</span>
                    <span class="text-right text-cyan-300">{format!("{:.2} / {:.2}", row.bid, row.ask)}</span>
                    <span class="text-right text-orange-300">{format!("{:.1}", row.volume_24h)}</span>
                </div>
            }) }
        </div>
    }
}

fn render_logs(ws_data: &[Value]) -> Html {
    let mut lines: Vec<LogDto> = Vec::new();
    for val in ws_data.iter() {
        if let Ok(evt) = serde_json::from_value::<LogDto>(val.clone()) {
            lines.push(evt);
        }
    }
    lines.sort_by_key(|l| l.ts);
    html! {
        <div class="h-48 overflow-y-auto space-y-1 pr-2 text-xs">
            { for lines.iter().rev().map(|line| render_log_line(line)) }
        </div>
    }
}

fn render_latency(rest_data: Option<Value>) -> Html {
    let Some(value) = rest_data else {
        return html! {<div class="text-xs text-emerald-200">{"Loading latency..."}</div>};
    };
    let dto: Option<LatencySummaryDto> = serde_json::from_value(value).ok();
    let Some(dto) = dto else {
        return html! {<div class="text-xs text-red-300">{"Invalid latency payload"}</div>};
    };
    html! {
        <div class="grid grid-cols-2 gap-2 text-sm">
            <div class="flex flex-col">
                <span class="text-emerald-200">{"Avg"}</span>
                <span class="text-cyan-300">{ dto.avg_ms.map(|v| format!("{:.1} ms", v)).unwrap_or_else(|| "--".into()) }</span>
            </div>
            <div class="flex flex-col">
                <span class="text-emerald-200">{"Min"}</span>
                <span class="text-cyan-300">{ dto.min_ms.map(|v| format!("{:.1} ms", v)).unwrap_or_else(|| "--".into()) }</span>
            </div>
            <div class="flex flex-col">
                <span class="text-emerald-200">{"p95"}</span>
                <span class="text-cyan-300">{ dto.p95_ms.map(|v| format!("{:.1} ms", v)).unwrap_or_else(|| "--".into()) }</span>
            </div>
            <div class="flex flex-col">
                <span class="text-emerald-200">{"p99"}</span>
                <span class="text-cyan-300">{ dto.p99_ms.map(|v| format!("{:.1} ms", v)).unwrap_or_else(|| "--".into()) }</span>
            </div>
        </div>
    }
}

fn render_generic_table(rest_data: Option<Value>, ws_data: &[Value]) -> Html {
    if let Some(rest) = rest_data {
        return html! {<pre class="text-xs text-green-200 whitespace-pre-wrap">{format!("{}", rest)}</pre>};
    }
    if !ws_data.is_empty() {
        return html! {<pre class="text-xs text-green-200 whitespace-pre-wrap">{format!("{}", serde_json::to_string_pretty(ws_data).unwrap_or_default())}</pre>};
    }
    html! {<div class="text-xs text-cyan-200">{"Awaiting data..."}</div>}
}

fn render_generic_chart(ws_data: &[Value]) -> Html {
    let numeric: Vec<f64> = ws_data.iter().filter_map(|v| v.as_f64()).collect();
    if numeric.is_empty() {
        return html! {<div class="text-xs text-cyan-200">{"No chart data"}</div>};
    }
    let max = numeric.iter().cloned().fold(f64::MIN, f64::max);
    let min = numeric.iter().cloned().fold(f64::MAX, f64::min);
    let range = (max - min).max(1.0);
    let width = 280.0;
    let height = 120.0;
    let step = width / (numeric.len().saturating_sub(1) as f64);
    let mut d = String::new();
    for (i, val) in numeric.iter().enumerate() {
        let x = step * i as f64;
        let y = height - ((val - min) / range) * height;
        if i == 0 {
            d.push_str(&format!("M {:.1} {:.1}", x, y));
        } else {
            d.push_str(&format!(" L {:.1} {:.1}", x, y));
        }
    }
    html! {
        <svg viewBox={format!("0 0 {} {}", width, height)} class="w-full h-32 bg-black/40 border border-cyan-800">
            <path d={d} fill="none" stroke="#22d3ee" stroke-width="2" />
        </svg>
    }
}

fn render_log_line(line: &LogDto) -> Html {
    let color = match line.level {
        LogLevel::Error => "text-red-400",
        LogLevel::Warn => "text-yellow-300",
        _ => "text-green-300",
    };
    html! {
        <div class={classes!("border-b", "border-green-900/40", color)}>
            <span class="text-green-500">{format!("[{}]", line.ts)}</span>
            <span class="text-cyan-300 ml-2">{format_source(&line.source)}</span>
            <span class="ml-2">{line.message.clone()}</span>
        </div>
    }
}

fn format_source(src: &LogSource) -> String {
    match src {
        LogSource::Kernel => "Kernel".into(),
        LogSource::MarketData => "MarketData".into(),
        LogSource::Execution => "Execution".into(),
        LogSource::Strategy => "Strategy".into(),
        LogSource::Api => "Api".into(),
        LogSource::Other(v) => v.clone(),
    }
}

fn format_exchange(ex: &ExchangeId) -> String {
    match ex {
        ExchangeId::Okx => "OKX".into(),
        ExchangeId::Binance => "Binance".into(),
        ExchangeId::Other(v) => v.clone(),
    }
}

fn format_symbol(sym: &SymbolDto) -> String {
    format!("{}{}", sym.base, sym.quote)
}

#[cfg(target_arch = "wasm32")]
#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn app_renders() {
        let document = web_sys::window().unwrap().document().unwrap();
        let root = document.create_element("div").unwrap();
        document.body().unwrap().append_child(&root).unwrap();
        yew::Renderer::<App>::with_root(root).render();
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn wildcard_subject_matching() {
        assert!(matches_subject("market.*", "market.OKX.BTCUSDT"));
        assert!(matches_subject("log.event", "log.event"));
        assert!(!matches_subject("orders.result", "log.event"));
    }

    #[test]
    fn subject_detection_from_payload() {
        let mkt = serde_json::json!({
            "ts": "2024-01-01T00:00:00Z",
            "exchange": "Okx",
            "symbol": {"base": "BTC", "quote": "USDT"},
            "last": 10.0,
            "bid": 9.9,
            "ask": 10.1,
            "volume_24h": 1.0
        });
        assert_eq!(
            subject_from_payload(&mkt),
            Some("market.OKX.BTCUSDT".to_string())
        );

        let log_evt = serde_json::json!({
            "ts": "2024-01-01T00:00:00Z",
            "level": "Info",
            "source": "Kernel",
            "message": "hello",
            "fields": {},
            "correlation_id": null
        });
        assert_eq!(
            subject_from_payload(&log_evt),
            Some("log.event".to_string())
        );
    }
}
