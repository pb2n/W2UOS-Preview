use std::str::FromStr;
use std::sync::Arc;

use actix_web::dev::ServiceRequest;
use actix_web::http::Method;
use chrono::Utc;
use subtle::ConstantTimeEq;
use tracing::warn;
use w2uos_bus::MessageBus;
use w2uos_config::hash_api_key;
use w2uos_log::{
    log_event_via_bus,
    types::{LogEvent, LogLevel, LogSource},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    ReadOnly,
    Trader,
    Admin,
}

impl Role {
    pub fn allows(&self, required: &Role) -> bool {
        matches!(self, Role::Admin)
            || (matches!(self, Role::Trader) && matches!(required, Role::Trader | Role::ReadOnly))
            || (matches!(self, Role::ReadOnly) && matches!(required, Role::ReadOnly))
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "admin" => Ok(Role::Admin),
            "trader" => Ok(Role::Trader),
            _ => Ok(Role::ReadOnly),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiUser {
    pub id: String,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiUserCredential {
    pub id: String,
    pub role: Role,
    pub api_key_hash: String,
}

fn constant_time_eq_hex(expected: &str, candidate: &str) -> bool {
    expected.as_bytes().ct_eq(candidate.as_bytes()).into()
}

pub fn authenticate(api_key: &str, users: &[ApiUserCredential]) -> Option<ApiUser> {
    let candidate_hash = hash_api_key(api_key);
    users
        .iter()
        .find(|user| constant_time_eq_hex(&user.api_key_hash, &candidate_hash))
        .map(|user| ApiUser {
            id: user.id.clone(),
            role: user.role.clone(),
        })
}

pub fn required_role_for_path(req: &ServiceRequest) -> Role {
    let path = req.path();
    let method = req.method();
    if path == "/health" {
        return Role::ReadOnly;
    }

    if path.starts_with("/config") {
        return Role::Admin;
    }

    if path.starts_with("/control/") || path == "/risk/reset_circuit" || path == "/backtest/start" {
        return Role::Trader;
    }

    if method == Method::POST && (path.contains("/risk/") || path.contains("/backtest")) {
        return Role::Trader;
    }

    Role::ReadOnly
}

pub fn record_auth_failure(bus: Arc<dyn MessageBus>, reason: &str, path: &str) {
    let event = LogEvent {
        ts: Utc::now(),
        level: LogLevel::Warn,
        source: LogSource::Api,
        message: "authentication failure".to_string(),
        fields: serde_json::json!({
            "reason": reason,
            "path": path,
        }),
        correlation_id: None,
        trace_id: None,
    };
    actix_rt::spawn(async move {
        if let Err(err) = log_event_via_bus(bus.as_ref(), &event).await {
            warn!(?err, "failed to log authentication failure");
        }
    });
}
