mod api;

use anyhow::Context;
use clap::Parser;
use proxy_core::config::Config;
use proxy_core::health::HealthChecker;
use proxy_core::pool::ProxyPool;
use proxy_core::router::Router;
use proxy_core::scraper::Scraper;
use proxy_core::storage::Storage;
use proxy_core::tun_manager::{run_forwarding_loop, TunManager};
use std::sync::Arc;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{error, info};

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
            tokio::spawn(async move {
                run_forwarding_loop(dev, router_clone, mtu).await;
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
    let scrape_state = Arc::new(RwLock::new(api::ScrapeState {
        running: false,
        last_run: None,
        proxies_found: 0,
        healthy_count: 0,
        errors: Vec::new(),
    }));

    let app_state = api::AppState {
        pool: pool.clone(),
        router: router.clone(),
        scraper: scraper.clone(),
        health: health.clone(),
        scrape_state: scrape_state.clone(),
        sources: sources.clone(),
    };

    // Automatic scrape loop
    if config.scraper.enabled {
        let pool_clone = pool.clone();
        let health_clone = health.clone();
        let scraper_clone = scraper.clone();
        let scrape_state_clone = scrape_state.clone();
        let interval = config.scraper.interval_secs;

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

                info!("Auto scrape: {} proxies, running health check...", proxies.len());
                let results = health_clone.check_batch(&proxies).await;
                let healthy_count = results.iter().filter(|r| r.alive).count();
                pool_clone.apply_health_results(results).await;

                {
                    let mut s = scrape_state_clone.write().await;
                    s.running = false;
                    s.last_run = Some(chrono::Utc::now());
                    s.proxies_found = proxies.len();
                    s.healthy_count = healthy_count;
                }

                info!("Auto scrape done: {healthy_count} alive, {} in pool", pool_clone.all().await.len());

                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
            }
        });
    }

    // Periodic pool re-check
    {
        let pool_clone = pool.clone();
        let health_clone = health.clone();
        let health_config = config.health.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(
                    health_config.check_interval_secs,
                ))
                .await;

                let proxies = pool_clone.all().await;
                if proxies.is_empty() {
                    continue;
                }

                info!("Re-checking {} pool proxies...", proxies.len());
                let results = health_clone.check_batch(&proxies).await;
                pool_clone.apply_health_results(results).await;

                let healthy = pool_clone.healthy_count().await;
                info!("Re-check complete: {healthy} alive");
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

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("API server error")?;

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
