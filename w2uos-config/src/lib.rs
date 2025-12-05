use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use w2uos_data::service::MarketHistoryConfig;
use w2uos_data::{DataMode, ExchangeId, MarketDataConfig, MarketDataSubscription, Symbol};
use w2uos_exec::ExecutionConfig;
use w2uos_net::{NetMode, NetProfile};
use wru_strategy::BotConfig;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum KernelMode {
    Live,
    Backtest,
}

impl Default for KernelMode {
    fn default() -> Self {
        KernelMode::Live
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum NodeRole {
    Data,
    Exec,
    Strategy,
    AllInOne,
}

impl Default for NodeRole {
    fn default() -> Self {
        NodeRole::AllInOne
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GlobalRiskConfig {
    pub max_daily_loss_pct: f64,
    pub max_daily_loss_abs: f64,
    pub max_total_notional: f64,
    pub max_open_positions: usize,
    pub max_error_rate_per_minute: Option<f64>,
}

impl Default for GlobalRiskConfig {
    fn default() -> Self {
        Self {
            max_daily_loss_pct: 5.0,
            max_daily_loss_abs: 10_000.0,
            max_total_notional: 250_000.0,
            max_open_positions: 25,
            max_error_rate_per_minute: Some(10.0),
        }
    }
}

pub type NodeId = String;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoggingConfig {
    pub level: String,
    pub log_path: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            log_path: Some("logs".to_string()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ApiUserConfig {
    pub id: String,
    pub role: String,
    pub api_key_hash: String,
}

impl ApiUserConfig {
    pub fn new_hashed(id: impl Into<String>, role: impl Into<String>, api_key: &str) -> Self {
        Self {
            id: id.into(),
            role: role.into(),
            api_key_hash: hash_api_key(api_key),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ApiSection {
    pub bind: String,
    pub users: Vec<ApiUserConfig>,
}

impl Default for ApiSection {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:8080".to_string(),
            users: vec![ApiUserConfig::new_hashed("admin", "Admin", "dev-key")],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExchangeKind {
    Okx,
    Binance,
    Dex(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkEnv {
    Mainnet,
    Testnet,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExchangeCredentials {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExchangeConfig {
    pub kind: ExchangeKind,
    pub env: NetworkEnv,
    pub rest_base_url: String,
    pub ws_public_url: String,
    pub ws_private_url: Option<String>,
    pub credentials: Option<ExchangeCredentials>,
    pub weight_per_second: Option<u32>,
}

impl ExchangeConfig {
    pub fn okx_defaults(env: NetworkEnv) -> Self {
        let (rest, ws_pub, ws_priv) = match env {
            NetworkEnv::Mainnet => (
                "https://www.okx.com".to_string(),
                "wss://ws.okx.com:8443/ws/v5/public".to_string(),
                Some("wss://ws.okx.com:8443/ws/v5/private".to_string()),
            ),
            NetworkEnv::Testnet => (
                "https://www.okx.com".to_string(),
                "wss://wspap.okx.com:8443/ws/v5/public".to_string(),
                Some("wss://wspap.okx.com:8443/ws/v5/private".to_string()),
            ),
        };

        Self {
            kind: ExchangeKind::Okx,
            env,
            rest_base_url: rest,
            ws_public_url: ws_pub,
            ws_private_url: ws_priv,
            credentials: None,
            weight_per_second: None,
        }
    }

    pub fn binance_defaults(env: NetworkEnv) -> Self {
        let (rest, ws_pub) = match env {
            NetworkEnv::Mainnet => (
                "https://api.binance.com".to_string(),
                "wss://stream.binance.com:9443/ws".to_string(),
            ),
            NetworkEnv::Testnet => (
                "https://testnet.binance.vision".to_string(),
                "wss://testnet.binance.vision/ws".to_string(),
            ),
        };

        Self {
            kind: ExchangeKind::Binance,
            env,
            rest_base_url: rest,
            ws_public_url: ws_pub,
            ws_private_url: None,
            credentials: None,
            weight_per_second: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct StrategySection {
    pub bot_config: BotConfig,
    pub config_path: Option<String>,
}

impl Default for StrategySection {
    fn default() -> Self {
        Self {
            bot_config: BotConfig::default(),
            config_path: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NodeConfig {
    pub net_profile: NetProfile,
    pub node_id: NodeId,
    pub node_role: NodeRole,
    pub kernel_mode: KernelMode,
    pub logging: LoggingConfig,
    pub api: ApiSection,
    #[serde(default = "default_exchange_configs")]
    pub exchanges: Vec<ExchangeConfig>,
    pub market_data: MarketDataConfig,
    pub execution: ExecutionConfig,
    pub strategy: StrategySection,
    pub global_risk: GlobalRiskConfig,
    pub history: Option<MarketHistoryConfig>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let default_subscription = MarketDataSubscription {
            exchange: ExchangeId::Okx,
            symbol: Symbol {
                base: "BTC".to_string(),
                quote: "USDT".to_string(),
            },
            ws_url: "wss://dummy".to_string(),
        };

        Self {
            net_profile: NetProfile::default(),
            node_id: "node-1".to_string(),
            node_role: NodeRole::AllInOne,
            kernel_mode: KernelMode::Live,
            logging: LoggingConfig::default(),
            api: ApiSection::default(),
            exchanges: default_exchange_configs(),
            market_data: MarketDataConfig {
                subscriptions: vec![default_subscription],
                net_profile: NetProfile::default(),
                history: None,
                mode: DataMode::Simulated,
            },
            execution: ExecutionConfig::default(),
            strategy: StrategySection::default(),
            global_risk: GlobalRiskConfig::default(),
            history: None,
        }
    }
}

impl NodeConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        let value = load_value(path)?;
        let mut cfg: NodeConfig = value.try_into()?;
        apply_env_overrides(&mut cfg);
        Ok(cfg)
    }

    pub fn load_with_env(base_path: &Path, env_name: Option<String>) -> Result<Self> {
        let mut merged = load_value(base_path)?;
        let env_overlay = env_name.or_else(|| std::env::var("NODE_ENV").ok());
        if let Some(env) = env_overlay {
            let env_path = env_config_path(base_path, &env);
            if env_path.exists() {
                let overlay = load_value(&env_path)?;
                merge_toml(&mut merged, overlay);
            }
        }

        let mut cfg: NodeConfig = merged.try_into()?;
        apply_env_overrides(&mut cfg);
        Ok(cfg)
    }

    pub fn redacted(&self) -> Self {
        let mut cloned = self.clone();
        for user in &mut cloned.api.users {
            user.api_key_hash = "***".to_string();
        }
        for ex in &mut cloned.exchanges {
            if let Some(creds) = &mut ex.credentials {
                creds.api_key = "***".to_string();
                creds.api_secret = "***".to_string();
                if creds.passphrase.is_some() {
                    creds.passphrase = Some("***".to_string());
                }
            }
        }
        cloned
    }

    pub fn find_exchange(&self, kind: ExchangeKind, env: NetworkEnv) -> Option<&ExchangeConfig> {
        self.exchanges
            .iter()
            .find(|cfg| cfg.kind == kind && cfg.env == env)
    }

    pub fn is_live_trading(&self) -> bool {
        self.exchanges.iter().any(|cfg| cfg.credentials.is_some())
    }
}

fn load_value(path: &Path) -> Result<toml::Value> {
    let contents = std::fs::read_to_string(path)?;
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        match ext {
            "yaml" | "yml" => {
                let json_value: JsonValue = serde_yaml::from_str(&contents)?;
                let toml_string = toml::to_string(&json_value)?;
                Ok(toml::from_str(&toml_string)?)
            }
            _ => Ok(toml::from_str(&contents)?),
        }
    } else {
        Ok(toml::from_str(&contents)?)
    }
}

fn env_config_path(base_path: &Path, env: &str) -> PathBuf {
    let mut env_path = base_path.to_path_buf();
    if let Some(parent) = base_path.parent() {
        env_path = parent.join(format!("config.{}.toml", env));
    } else {
        env_path.set_file_name(format!("config.{}.toml", env));
    }
    env_path
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    use toml::Value;
    match (base, overlay) {
        (Value::Table(base_map), Value::Table(overlay_map)) => {
            for (k, v) in overlay_map {
                match base_map.get_mut(&k) {
                    Some(base_val) => merge_toml(base_val, v),
                    None => {
                        base_map.insert(k, v);
                    }
                }
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val;
        }
    }
}

fn apply_env_overrides(cfg: &mut NodeConfig) {
    if let Ok(api_key) = std::env::var("W2UOS_API_KEY") {
        cfg.api.users = vec![ApiUserConfig::new_hashed("env-admin", "Admin", &api_key)];
    }
    if let Ok(bind) = std::env::var("W2UOS_API_BIND") {
        cfg.api.bind = bind;
    }
    if let Ok(node_id) = std::env::var("W2UOS_NODE_ID") {
        cfg.node_id = node_id;
    }
    if let Ok(role) = std::env::var("W2UOS_NODE_ROLE") {
        cfg.node_role = match role.to_lowercase().as_str() {
            "data" => NodeRole::Data,
            "exec" => NodeRole::Exec,
            "strategy" => NodeRole::Strategy,
            _ => NodeRole::AllInOne,
        };
    }
    if let Ok(mode) = std::env::var("W2UOS_KERNEL_MODE") {
        cfg.kernel_mode = match mode.to_lowercase().as_str() {
            "backtest" => KernelMode::Backtest,
            _ => KernelMode::Live,
        };
    }
    if let Ok(level) = std::env::var("W2UOS_LOG_LEVEL") {
        cfg.logging.level = level;
    }
    if let Ok(net_mode) = std::env::var("W2UOS_NET_MODE") {
        if let Ok(parsed) = NetMode::from_str(&net_mode) {
            cfg.net_profile.mode = parsed.clone();
            cfg.market_data.net_profile.mode = parsed;
        }
    }
    if let Ok(proxy_url) = std::env::var("W2UOS_PROXY_URL") {
        cfg.net_profile.proxy_url = Some(proxy_url.clone());
        cfg.market_data.net_profile.proxy_url = Some(proxy_url);
    }
    if let Ok(history_conn) = std::env::var("W2UOS_HISTORY_URL") {
        cfg.history = Some(MarketHistoryConfig {
            connection_string: history_conn.clone(),
        });
        cfg.market_data.history = Some(MarketHistoryConfig {
            connection_string: history_conn,
        });
    }
    if let Ok(exec_balance) = std::env::var("W2UOS_EXEC_BALANCE") {
        if let Ok(parsed) = exec_balance.parse::<f64>() {
            cfg.execution.initial_balance_usdt = parsed;
        }
    }

    for exchange in &mut cfg.exchanges {
        if let Some(creds) = &mut exchange.credentials {
            creds.api_key = resolve_secret(&creds.api_key);
            creds.api_secret = resolve_secret(&creds.api_secret);
            if let Some(pass) = &creds.passphrase {
                creds.passphrase = Some(resolve_secret(pass));
            }
        }
    }
}

pub fn hash_api_key(api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let digest = hasher.finalize();
    format!("{:x}", digest)
}

fn default_exchange_configs() -> Vec<ExchangeConfig> {
    vec![
        ExchangeConfig::okx_defaults(NetworkEnv::Testnet),
        ExchangeConfig::binance_defaults(NetworkEnv::Testnet),
    ]
}

fn resolve_secret(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(stripped).unwrap_or_else(|_| raw.to_string())
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn loads_from_toml_file() {
        let mut file = NamedTempFile::new().unwrap();
        let contents = br#"
net_profile = { mode = "Direct" }
node_id = "node-test"
node_role = "AllInOne"
kernel_mode = "Live"

[logging]
level = "debug"

[api]
bind = "127.0.0.1:8080"
[[api.users]]
id = "admin"
role = "Admin"
api_key_hash = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"

[market_data.net_profile]
mode = "Direct"

[[market_data.subscriptions]]
exchange = "Okx"
symbol = { base = "BTC", quote = "USDT" }
ws_url = "wss://mock"

[execution.net_profile]
mode = "Direct"
[execution]
initial_balance_usdt = 50000.0

[strategy.bot_config]
name = "bot"
pairs = []

[global_risk]
max_daily_loss_pct = 5.0
max_daily_loss_abs = 1000.0
max_total_notional = 250000.0
max_open_positions = 10
"#;
        std::io::Write::write_all(&mut file, contents).unwrap();

        let cfg = NodeConfig::from_file(file.path()).unwrap();
        assert_eq!(cfg.api.bind, "127.0.0.1:8080");
        assert_eq!(cfg.market_data.subscriptions.len(), 1);
        assert_eq!(cfg.logging.level, "debug");
        assert_eq!(cfg.api.users.len(), 1);
    }

    #[test]
    fn applies_env_overlay_and_env_vars() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");
        std::fs::write(
            &base_path,
            r#"
net_profile = { mode = "Direct" }
node_id = "base-node"
node_role = "AllInOne"
kernel_mode = "Live"

[api]
bind = "127.0.0.1:8080"
[[api.users]]
id = "base"
role = "Admin"
api_key_hash = "75109f8c2467d9664da34450616a245f3bffeb8560d78b44df944a0a26d78799"

[logging]
level = "info"

[[market_data.subscriptions]]
exchange = "Okx"
symbol = { base = "BTC", quote = "USDT" }
ws_url = "wss://mock"

[market_data.net_profile]
mode = "Direct"

[execution.net_profile]
mode = "Direct"
[execution]
initial_balance_usdt = 50000.0

[strategy.bot_config]
name = "bot"
pairs = []

[global_risk]
max_daily_loss_pct = 5.0
max_daily_loss_abs = 1000.0
max_total_notional = 250000.0
max_open_positions = 10
"#,
        )
        .unwrap();

        let overlay_path = dir.path().join("config.dev.toml");
        std::fs::write(
            &overlay_path,
            r#"
node_id = "dev-node"

[api]
[[api.users]]
id = "overlay"
role = "Trader"
api_key_hash = "13a9458e5c08f218d18f799fb3ec284e292d172457b8bbd6616f98b6e4178f55"
"#,
        )
        .unwrap();

        std::env::set_var("NODE_ENV", "dev");
        std::env::set_var("W2UOS_API_KEY", "env-key");
        std::env::set_var("W2UOS_NODE_ROLE", "Exec");

        let cfg = NodeConfig::load_with_env(&base_path, None).unwrap();
        assert_eq!(cfg.api.users.len(), 1);
        assert_eq!(cfg.api.users[0].api_key_hash, hash_api_key("env-key"));
        assert_eq!(cfg.node_id, "dev-node");
        assert_eq!(cfg.node_role, NodeRole::Exec);

        std::env::remove_var("NODE_ENV");
        std::env::remove_var("W2UOS_API_KEY");
        std::env::remove_var("W2UOS_NODE_ROLE");
    }

    #[test]
    fn provides_default_exchange_configs() {
        let cfg = NodeConfig::default();
        assert!(cfg
            .find_exchange(ExchangeKind::Okx, NetworkEnv::Testnet)
            .is_some());
        assert!(cfg
            .find_exchange(ExchangeKind::Binance, NetworkEnv::Testnet)
            .is_some());
    }

    #[test]
    fn resolves_exchange_credentials_from_env_refs() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");
        std::fs::write(
            &base_path,
            r#"
net_profile = { mode = "Direct" }
node_id = "node-test"
node_role = "AllInOne"
kernel_mode = "Live"

[api]
bind = "127.0.0.1:8080"
[[api.users]]
id = "admin"
role = "Admin"
api_key_hash = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"

[[exchanges]]
kind = "Okx"
env = "Testnet"
rest_base_url = "https://example"
ws_public_url = "wss://example/public"
ws_private_url = "wss://example/private"
[exchanges.credentials]
api_key = "${OKX_API_KEY}"
api_secret = "${OKX_API_SECRET}"
passphrase = "${OKX_PASSPHRASE}"

[[market_data.subscriptions]]
exchange = "Okx"
symbol = { base = "BTC", quote = "USDT" }
ws_url = "wss://mock"

[market_data.net_profile]
mode = "Direct"

[execution.net_profile]
mode = "Direct"
[execution]
initial_balance_usdt = 50000.0

[strategy.bot_config]
name = "bot"
pairs = []

[logging]
level = "info"

[global_risk]
max_daily_loss_pct = 5.0
max_daily_loss_abs = 1000.0
max_total_notional = 250000.0
max_open_positions = 10
"#,
        )
        .unwrap();

        std::env::set_var("OKX_API_KEY", "key123");
        std::env::set_var("OKX_API_SECRET", "secret456");
        std::env::set_var("OKX_PASSPHRASE", "pass789");

        let cfg = NodeConfig::from_file(&base_path).unwrap();
        let ex = cfg
            .find_exchange(ExchangeKind::Okx, NetworkEnv::Testnet)
            .and_then(|c| c.credentials.clone())
            .expect("credentials loaded");
        assert_eq!(ex.api_key, "key123");
        assert_eq!(ex.api_secret, "secret456");
        assert_eq!(ex.passphrase.as_deref(), Some("pass789"));

        std::env::remove_var("OKX_API_KEY");
        std::env::remove_var("OKX_API_SECRET");
        std::env::remove_var("OKX_PASSPHRASE");
    }
}
