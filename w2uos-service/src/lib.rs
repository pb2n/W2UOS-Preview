use anyhow::Result;

pub type ServiceId = String;

#[async_trait::async_trait]
pub trait Service: Send + Sync {
    fn id(&self) -> &ServiceId;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn health_check(&self) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TraceId(pub String);

impl TraceId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
