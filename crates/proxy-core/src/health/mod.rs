use crate::forwarder::Forwarder;
use crate::proxy::ProxyInfo;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tracing::warn;

pub struct HealthChecker {
    concurrency: Arc<Semaphore>,
    target_url: String,
    timeout: std::time::Duration,
}

#[derive(Debug)]
pub struct HealthResult {
    pub proxy_id: String,
    pub alive: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}

impl HealthChecker {
    pub fn new(concurrency: usize, timeout_secs: u64, target_url: String) -> Self {
        Self {
            concurrency: Arc::new(Semaphore::new(concurrency)),
            target_url,
            timeout: std::time::Duration::from_secs(timeout_secs),
        }
    }

    pub async fn check(&self, proxy: &ProxyInfo) -> HealthResult {
        let _permit = self.concurrency.acquire().await.expect("Semaphore closed");
        let start = Instant::now();

        // Parse target URL into host:port
        let target = self.target_url.trim_start_matches("http://")
            .trim_start_matches("https://");
        let (host, port) = match target.split_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().unwrap_or(80)),
            None => (target, 80u16),
        };

        let result = Forwarder::connect_to(proxy, host, port).await;

        match result {
            Ok(mut conn) => {
                // Send HTTP request through proxy
                let request = format!(
                    "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
                );
                let write_result = tokio::time::timeout(self.timeout, conn.write_all(request.as_bytes())).await;

                match write_result {
                    Ok(Ok(_)) => {
                        let mut buf = Vec::new();
                        let read_result = tokio::time::timeout(self.timeout, conn.read_to_end(&mut buf)).await;
                        match read_result {
                            Ok(Ok(_)) => {
                                let latency = start.elapsed().as_millis() as u64;
                                let response = String::from_utf8_lossy(&buf);
                                let alive = response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200");
                                HealthResult {
                                    proxy_id: proxy.id.clone(),
                                    alive,
                                    latency_ms: latency,
                                    error: None,
                                }
                            }
                            Ok(Err(e)) => HealthResult {
                                proxy_id: proxy.id.clone(),
                                alive: false,
                                latency_ms: 0,
                                error: Some(format!("read error: {e}")),
                            },
                            Err(_) => HealthResult {
                                proxy_id: proxy.id.clone(),
                                alive: false,
                                latency_ms: 0,
                                error: Some("read timeout".into()),
                            },
                        }
                    }
                    Ok(Err(e)) => HealthResult {
                        proxy_id: proxy.id.clone(),
                        alive: false,
                        latency_ms: 0,
                        error: Some(format!("write error: {e}")),
                    },
                    Err(_) => HealthResult {
                        proxy_id: proxy.id.clone(),
                        alive: false,
                        latency_ms: 0,
                        error: Some("write timeout".into()),
                    },
                }
            }
            Err(e) => HealthResult {
                proxy_id: proxy.id.clone(),
                alive: false,
                latency_ms: 0,
                error: Some(format!("connect error: {e}")),
            },
        }
    }

    pub async fn check_batch(&self, proxies: &[ProxyInfo]) -> Vec<HealthResult> {
        let mut results = Vec::with_capacity(proxies.len());

        let permits = self.concurrency.clone();
        let mut handles = Vec::new();

        for proxy in proxies {
            let proxy = proxy.clone();
            let permit = permits.clone().acquire_owned().await.expect("Semaphore closed");
            let target_url = self.target_url.clone();
            let timeout = self.timeout;

            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let start = Instant::now();
                let target = target_url.trim_start_matches("http://")
                    .trim_start_matches("https://");
                let (host, port) = match target.split_once(':') {
                    Some((h, p)) => (h, p.parse::<u16>().unwrap_or(80)),
                    None => (target, 80u16),
                };

                let result = tokio::time::timeout(timeout, Forwarder::connect_to(&proxy, host, port)).await;
                match result {
                    Ok(Ok(mut conn)) => {
                        let latency = start.elapsed().as_millis() as u64;
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(1),
                            conn.write_all(b"GET / HTTP/1.0\r\nHost: healthcheck\r\nConnection: close\r\n\r\n"),
                        ).await;
                        let alive = latency < timeout.as_millis() as u64;
                        HealthResult {
                            proxy_id: proxy.id.clone(),
                            alive,
                            latency_ms: latency,
                            error: None,
                        }
                    }
                    Ok(Err(e)) => HealthResult {
                        proxy_id: proxy.id.clone(),
                        alive: false,
                        latency_ms: 0,
                        error: Some(format!("connect error: {e}")),
                    },
                    Err(_) => HealthResult {
                        proxy_id: proxy.id.clone(),
                        alive: false,
                        latency_ms: 0,
                        error: Some("timeout".into()),
                    },
                }
            }));
        }

        for handle in handles {
            match handle.await {
                Ok(r) => results.push(r),
                Err(e) => warn!("Health check task failed: {e}"),
            }
        }

        results
    }
}
