mod api;

use anyhow::Context;
use clap::Parser;
use proxy_core::config::Config;
use proxy_core::health::HealthChecker;
use proxy_core::pool::{EvictionPolicy, ProxyPool};
use proxy_core::router::Router;
use proxy_core::scraper::Scraper;
use proxy_core::storage::Storage;
use proxy_core::tun_manager::{run_forwarding_loop, TunManager};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "proxy-daemon", about = "Proxy Switcher daemon")]
struct Args {
    #[arg(short, long, default_value = "/etc/proxy-switcher/config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "proxy=info".into()),
        )
        .init();

    let args = Args::parse();
    let config_path = &args.config;

    let config: Config = if std::path::Path::new(config_path).exists() {
        let content = tokio::fs::read_to_string(config_path)
            .await
            .context("Failed to read config")?;
        toml::from_str(&content).context("Failed to parse config")?
    } else {
        info!("No config found at {config_path}, using defaults");
        Config::default()
    };

    let pool = Arc::new(ProxyPool::new());
    let _storage = if let Some(parent) = config.daemon.data_dir.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
        let db_path = config.daemon.data_dir.join("proxy-switcher.db");
        match Storage::open(&db_path) {
            Ok(s) => Some(s),
            Err(e) => {
                error!("Failed to open storage: {e}");
                None
            }
        }
    } else {
        None
    };

    // Initialize TUN
    let tun = TunManager::new(config.tun.clone());
    let tun_ok = tun.create().await.is_ok();

    // Take device and start forwarding loop
    let router = Arc::new(Router::new(pool.clone()));

    if tun_ok {
        if let Some(dev) = tun.take_device().await {
            let router_clone = router.clone();
            let mtu = config.tun.mtu as usize;
            let shutdown = tun.shutdown_handle();
            tokio::spawn(async move {
                run_forwarding_loop(dev, router_clone, mtu, shutdown).await;
            });
        }
    }

    // Health checker
    let health = Arc::new(HealthChecker::new(
        config.health.concurrency,
        config.health.timeout_secs,
        config.health.target_url.clone(),
    ));

    // Shared state for API
    let sources = Arc::new(RwLock::new(config.scraper.sources.clone()));
    let scraper = Arc::new(Scraper::new(sources.clone()));
    // Shared health-check bookkeeping: a live progress counter and a guard so
    // only one batch runs at a time.
    let check_progress = Arc::new(AtomicUsize::new(0));
    let check_in_flight = Arc::new(AtomicBool::new(false));
    let eviction_policy = EvictionPolicy {
        max_consecutive_failures: config.health.max_consecutive_failures,
        unverified_ttl_secs: config.health.unverified_ttl_secs,
    };

    let scrape_state = Arc::new(RwLock::new(api::ScrapeState::new(check_progress.clone())));

    let app_state = api::AppState {
        pool: pool.clone(),
        router: router.clone(),
        scraper: scraper.clone(),
        health: health.clone(),
        scrape_state: scrape_state.clone(),
        sources: sources.clone(),
        check_progress: check_progress.clone(),
        check_in_flight: check_in_flight.clone(),
        eviction_policy,
    };

    // Automatic scrape loop
    if config.scraper.enabled {
        let pool_clone = pool.clone();
        let health_clone = health.clone();
        let scraper_clone = scraper.clone();
        let scrape_state_clone = scrape_state.clone();
        let interval = config.scraper.interval_secs;
        // Pre-clone so the spawned loop owns its own handles.
        let loop_progress = check_progress.clone();
        let loop_in_flight = check_in_flight.clone();

        tokio::spawn(async move {
            loop {
                let mut s = scrape_state_clone.write().await;
                s.running = true;
                s.errors.clear();
                drop(s);

                let proxies = match scraper_clone.scrape_all().await {
                    Ok(p) => p,
                    Err(e) => {
                        error!("Auto scrape cycle failed: {e}");
                        let mut s = scrape_state_clone.write().await;
                        s.running = false;
                        s.errors.push(format!("{e}"));
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        continue;
                    }
                };

                {
                    let mut s = scrape_state_clone.write().await;
                    s.running = false;
                    s.last_run = Some(chrono::Utc::now());
                    s.proxies_found = proxies.len();
                }

                info!("Auto scrape collected {} proxies", proxies.len());

                // Merge rather than replace: proxies already known keep their
                // latency, score and failure streak. Replacing the pool here is
                // what made the healthy count drop to zero on every cycle.
                let added = pool_clone.merge(proxies).await;
                let pending = pool_clone.unverified().await;
                let pool_total = pool_clone.len().await;

                {
                    let mut s = scrape_state_clone.write().await;
                    s.proxies_found = pool_total;
                }

                info!(
                    added,
                    pool_total,
                    unverified = pending.len(),
                    "scrape merged into pool"
                );

                let hc = health_clone.clone();
                let pc = pool_clone.clone();
                let sc = scrape_state_clone.clone();
                let progress = loop_progress.clone();
                let in_flight = loop_in_flight.clone();
                let policy = eviction_policy;

                if pending.is_empty() {
                    info!("no unverified proxies to check");
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    continue;
                }

                // Only one batch at a time: a second one would contend for the
                // same concurrency permits and delay the first.
                if in_flight
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_err()
                {
                    warn!("Health check already in progress, deferring unverified batch");
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    continue;
                }

                {
                    let mut s = sc.write().await;
                    s.check_running = true;
                    s.check_total = pending.len();
                }

                tokio::spawn(async move {
                    let results = hc.check_batch(&pending, progress).await;
                    pc.apply_health_results(results, policy).await;
                    in_flight.store(false, std::sync::atomic::Ordering::SeqCst);
                    let mut s = sc.write().await;
                    s.check_running = false;
                    info!("Unverified batch done: pool has {}", pc.len().await);
                });

                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            }
        });
    }

    // Periodic re-check of the proxies already proven to work. Unverified
    // candidates are handled by the scrape loop, so this only has to confirm
    // the working set is still working.
    {
        let pool_clone = pool.clone();
        let health_clone = health.clone();
        let health_config = config.health.clone();
        let progress = check_progress.clone();
        let in_flight = check_in_flight.clone();
        let scrape_state_clone = scrape_state.clone();
        let policy = eviction_policy;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    health_config.check_interval_secs,
                ))
                .await;

                let working = pool_clone.working().await;
                if working.is_empty() {
                    continue;
                }

                // Yield to a batch that is already running instead of
                // duplicating the sweep.
                if in_flight
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_err()
                {
                    info!("Health check already in progress, skipping re-check");
                    continue;
                }

                info!("Re-checking {} working proxies", working.len());
                {
                    let mut s = scrape_state_clone.write().await;
                    s.check_running = true;
                    s.check_total = working.len();
                }

                let results = health_clone.check_batch(&working, progress.clone()).await;
                let outcome = pool_clone.apply_health_results(results, policy).await;
                in_flight.store(false, std::sync::atomic::Ordering::SeqCst);

                let healthy = pool_clone.healthy_count().await;
                let mut s = scrape_state_clone.write().await;
                s.check_running = false;
                s.healthy_count = healthy;

                info!(
                    working = outcome.working,
                    failed = outcome.failed,
                    evicted = outcome.evicted,
                    healthy,
                    "re-check complete"
                );
            }
        });
    }

    // Start API server
    let addr = format!("{}:{}", config.daemon.api_host, config.daemon.api_port);
    let router = api::build_router(app_state);

    info!("Starting API server on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Failed to bind API address")?;

    // Signal the forwarding loop before the server unwinds, so its blocking
    // task returns and the runtime can actually be dropped.
    let tun_for_shutdown = tun.shutdown_handle();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tun_for_shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await
        .context("API server error")?;

    tun.request_shutdown();
    tun.cleanup().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutting down...");
}
