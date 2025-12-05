use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use rand::{distributions::Uniform, Rng};
use reqwest::Client;
use tokio::{sync::Semaphore, time::sleep};
use tracing::debug;

use crate::client::build_http_client;
use crate::config::{ConnectionReusePolicy, NetProfile};

#[derive(Clone)]
pub struct TrafficShaper {
    client: Client,
    profile: NetProfile,
    semaphores: Arc<tokio::sync::Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl TrafficShaper {
    pub fn new(client: Client, profile: NetProfile) -> Self {
        Self {
            client,
            profile,
            semaphores: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    fn jitter_delay_ms(&self) -> Option<u64> {
        self.profile.request_jitter_ms.as_ref().map(|(min, max)| {
            let mut rng = rand::thread_rng();
            rng.sample(Uniform::new_inclusive(*min, *max))
        })
    }

    fn select_client(&self, reuse_randomly: Option<bool>) -> Result<Client> {
        match self
            .profile
            .connection_reuse_policy
            .as_ref()
            .unwrap_or(&ConnectionReusePolicy::AlwaysReuse)
        {
            ConnectionReusePolicy::AlwaysReuse => Ok(self.client.clone()),
            ConnectionReusePolicy::NoReuse => build_http_client(&self.profile),
            ConnectionReusePolicy::RandomReuse => {
                if reuse_randomly.unwrap_or_else(|| rand::thread_rng().gen_bool(0.5)) {
                    Ok(self.client.clone())
                } else {
                    build_http_client(&self.profile)
                }
            }
        }
    }

    async fn acquire_permit(&self, tag: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let limit = self.profile.max_parallel_requests_per_target?;
        let mut guard = self.semaphores.lock().await;
        let sem = guard
            .entry(tag.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(limit)))
            .clone();
        Some(sem.acquire_owned().await.expect("semaphore closed"))
    }

    pub async fn execute<F, Fut, T>(&self, tag: &str, f: F) -> Result<T>
    where
        F: FnOnce(&Client) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send,
        T: Send + 'static,
    {
        let reuse_randomly = matches!(
            self.profile
                .connection_reuse_policy
                .as_ref()
                .unwrap_or(&ConnectionReusePolicy::AlwaysReuse),
            ConnectionReusePolicy::RandomReuse
        )
        .then(|| rand::thread_rng().gen_bool(0.5));

        if let Some(delay_ms) = self.jitter_delay_ms() {
            if delay_ms > 0 {
                debug!(%tag, delay_ms, "applying outbound jitter");
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }

        let permit = self.acquire_permit(tag).await;
        if permit.is_some() {
            debug!(%tag, "traffic shaper acquired concurrency slot");
        }

        let client = self.select_client(reuse_randomly)?;
        let result = f(&client).await;
        drop(permit);
        result
    }
}

pub fn build_client_and_shaper(profile: &NetProfile) -> Result<(Client, TrafficShaper)> {
    let client = build_http_client(profile)?;
    let shaper = TrafficShaper::new(client.clone(), profile.clone());
    Ok((client, shaper))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{sleep, Duration, Instant};

    #[tokio::test]
    async fn serializes_when_limit_one() {
        let profile = NetProfile {
            max_parallel_requests_per_target: Some(1),
            ..NetProfile::default()
        };
        let (_, shaper) = build_client_and_shaper(&profile).expect("client builds");
        let current = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let run = |shaper: TrafficShaper| {
            let current = Arc::clone(&current);
            let max_seen = Arc::clone(&max_seen);
            async move {
                shaper
                    .execute("tag", move |_client| {
                        let current = Arc::clone(&current);
                        let max_seen = Arc::clone(&max_seen);
                        async move {
                            let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                            let _ = max_seen.fetch_max(now, Ordering::SeqCst);
                            sleep(Duration::from_millis(50)).await;
                            current.fetch_sub(1, Ordering::SeqCst);
                            Ok(())
                        }
                    })
                    .await
                    .unwrap();
            }
        };

        let t1 = tokio::spawn(run(shaper.clone()));
        let t2 = tokio::spawn(run(shaper.clone()));
        let _ = tokio::join!(t1, t2);

        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn jitter_applies_delay_within_range() {
        let profile = NetProfile {
            request_jitter_ms: Some((20, 40)),
            ..NetProfile::default()
        };
        let (_, shaper) = build_client_and_shaper(&profile).expect("client builds");

        let start = Instant::now();
        shaper
            .execute("jitter", |_client| async { Ok::<(), anyhow::Error>(()) })
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(20));
        assert!(elapsed <= Duration::from_millis(80));
    }

    #[tokio::test]
    async fn limit_enforces_serial_execution_over_time() {
        let profile = NetProfile {
            max_parallel_requests_per_target: Some(1),
            ..NetProfile::default()
        };
        let (_, shaper) = build_client_and_shaper(&profile).expect("client builds");

        let start = Instant::now();
        let f1 = shaper.execute("tag", |_client| async {
            sleep(Duration::from_millis(40)).await;
            Ok::<(), anyhow::Error>(())
        });
        let f2 = shaper.execute("tag", |_client| async {
            sleep(Duration::from_millis(40)).await;
            Ok::<(), anyhow::Error>(())
        });

        let _ = tokio::join!(f1, f2);
        let elapsed = start.elapsed();

        assert!(elapsed >= Duration::from_millis(70));
    }
}
