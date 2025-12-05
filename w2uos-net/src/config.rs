use std::{fmt, str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetMode {
    Direct,
    HttpProxy,
    Socks5Proxy,
    Tor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionReusePolicy {
    AlwaysReuse,
    RandomReuse,
    NoReuse,
}

impl Default for NetMode {
    fn default() -> Self {
        Self::Direct
    }
}

impl fmt::Display for NetMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetMode::Direct => write!(f, "Direct"),
            NetMode::HttpProxy => write!(f, "HttpProxy"),
            NetMode::Socks5Proxy => write!(f, "Socks5Proxy"),
            NetMode::Tor => write!(f, "Tor"),
        }
    }
}

impl FromStr for NetMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "direct" => Ok(NetMode::Direct),
            "httpproxy" | "http_proxy" => Ok(NetMode::HttpProxy),
            "socks5" | "socks5proxy" | "socks5_proxy" => Ok(NetMode::Socks5Proxy),
            "tor" => Ok(NetMode::Tor),
            other => anyhow::bail!("unknown net mode: {other}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetProfile {
    pub mode: NetMode,
    pub proxy_url: Option<String>,
    pub request_timeout: Option<Duration>,
    pub connect_timeout: Option<Duration>,
    pub max_idle_per_host: Option<usize>,
    pub max_retries: Option<u32>,
    pub request_jitter_ms: Option<(u64, u64)>,
    pub max_parallel_requests_per_target: Option<usize>,
    pub connection_reuse_policy: Option<ConnectionReusePolicy>,
}

impl Default for NetProfile {
    fn default() -> Self {
        Self {
            mode: NetMode::Direct,
            proxy_url: None,
            request_timeout: Some(Duration::from_secs(30)),
            connect_timeout: Some(Duration::from_secs(10)),
            max_idle_per_host: Some(8),
            max_retries: Some(0),
            request_jitter_ms: None,
            max_parallel_requests_per_target: None,
            connection_reuse_policy: Some(ConnectionReusePolicy::AlwaysReuse),
        }
    }
}
