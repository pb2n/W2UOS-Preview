//! Message bus abstraction for W2UOS.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use tokio::sync::{mpsc, RwLock};

#[derive(Clone, Debug)]
pub struct BusMessage(pub Vec<u8>);

pub struct Subscription {
    pub subject: String,
    pub receiver: mpsc::Receiver<BusMessage>,
}

#[async_trait::async_trait]
pub trait MessageBus: Send + Sync {
    async fn publish(&self, subject: &str, msg: BusMessage) -> Result<()>;
    async fn subscribe(&self, subject: &str) -> Result<Subscription>;
}

#[derive(Default)]
struct LocalBusInner {
    subscribers: HashMap<String, Vec<mpsc::Sender<BusMessage>>>,
}

/// In-process message bus implementation using Tokio channels.
pub struct LocalBus {
    inner: Arc<RwLock<LocalBusInner>>,
}

impl LocalBus {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(LocalBusInner::default())),
        }
    }
}

impl Default for LocalBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MessageBus for LocalBus {
    async fn publish(&self, subject: &str, msg: BusMessage) -> Result<()> {
        let inner = self.inner.read().await;
        if let Some(subscribers) = inner.subscribers.get(subject) {
            for sender in subscribers {
                // Ignore send errors if receiver was dropped.
                let _ = sender.send(msg.clone()).await;
            }
        }
        Ok(())
    }

    async fn subscribe(&self, subject: &str) -> Result<Subscription> {
        let (sender, receiver) = mpsc::channel(1024);
        let mut inner = self.inner.write().await;
        inner
            .subscribers
            .entry(subject.to_string())
            .or_default()
            .push(sender);

        Ok(Subscription {
            subject: subject.to_string(),
            receiver,
        })
    }
}

pub fn new_boxed_local() -> Arc<dyn MessageBus> {
    Arc::new(LocalBus::new())
}

pub struct NatsBus {
    // TODO: NATS backend implementation
}

pub struct ZmqBus {
    // TODO: ZeroMQ backend implementation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_bus_publish_subscribe() {
        let bus = LocalBus::new();
        let mut sub = bus.subscribe("test.subject").await.unwrap();

        bus.publish("test.subject", BusMessage(b"hello".to_vec()))
            .await
            .unwrap();

        let received = sub.receiver.recv().await.unwrap();
        assert_eq!(received.0, b"hello");
    }
}
