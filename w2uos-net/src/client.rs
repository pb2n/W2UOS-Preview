use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::{Client, Proxy};

use crate::config::{NetMode, NetProfile};

pub fn build_http_client(profile: &NetProfile) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent("w2uos-net/0.1")
        .tcp_keepalive(Some(Duration::from_secs(30)));

    if let Some(timeout) = profile.request_timeout {
        builder = builder.timeout(timeout);
    }

    if let Some(timeout) = profile.connect_timeout {
        builder = builder.connect_timeout(timeout);
    }

    if let Some(max_idle) = profile.max_idle_per_host {
        builder = builder.pool_max_idle_per_host(max_idle);
    }

    match profile.mode {
        NetMode::Direct => {}
        NetMode::HttpProxy => {
            let proxy = profile
                .proxy_url
                .as_ref()
                .ok_or_else(|| anyhow!("proxy_url required for HttpProxy mode"))?;
            builder = builder.proxy(Proxy::http(proxy)?);
        }
        NetMode::Socks5Proxy | NetMode::Tor => {
            let proxy = profile
                .proxy_url
                .as_ref()
                .ok_or_else(|| anyhow!("proxy_url required for SOCKS/Tor mode"))?;
            builder = builder.proxy(Proxy::all(proxy)?);
        }
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_profile_builds() {
        let profile = NetProfile::default();
        let client = build_http_client(&profile);
        assert!(client.is_ok());
    }

    #[test]
    fn proxy_profile_without_url_errors() {
        let profile = NetProfile {
            mode: NetMode::HttpProxy,
            proxy_url: None,
            ..Default::default()
        };

        let client = build_http_client(&profile);
        assert!(client.is_err());
    }
}
