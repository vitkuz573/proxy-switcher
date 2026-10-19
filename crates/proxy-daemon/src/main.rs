mod api;

use anyhow::Context;
use clap::Parser;
use proxy_core::config::Config;
use proxy_core::health::HealthChecker;
use proxy_core::pool::{EvictionPolicy, ProbeSchedule, ProxyPool};
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
    let db_path = if let Some(parent) = config.daemon.data_dir.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
        Some(config.daemon.data_dir.join("proxy-switcher.db"))
    } else {
        None
    };

    // Restore the pool before anything else runs.
    //
    // Without this the pool starts empty on every boot: the first scrape
    // repopulates it, but `last_success` is lost, so every proxy reads as
    // unverified and the whole pool is re-probed from scratch. Restoring keeps
    // the post-restart batch down to whatever the scrape actually discovered.
    if let Some(path) = db_path.as_ref() {
        let restored = Storage::open(path).and_then(|s| s.load_proxies());
        match restored {
            Ok(restored) if !restored.is_empty() => {
                pool.restore(restored).await;
            }
            Ok(_) => info!("storage holds no proxies yet"),
            Err(e) => error!("Failed to restore pool: {e}"),
        }
    }

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
        config.health.connect_timeout_secs,
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
    // Per-proxy probe schedule. The pool decides what is due; the health
    // checker only works on that set.
    let probe_schedule = ProbeSchedule {
        working_interval_secs: config.health.check_interval_secs,
        failed_backoff_secs: config.health.failed_backoff_secs.clone(),
        inconclusive_backoff_secs: config.health.inconclusive_backoff_secs,
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
        probe_schedule: probe_schedule.clone(),
    };

    // One scheduler for the whole pool.
    //
    // Discovery and probing used to be two independent loops on different
    // cadences, and each of them swept the entire unverified pool. Every
    // cycle therefore re-probed every proxy that had never worked, so the
    // cost of a cycle grew with the pool — and the pool grew with every
    // scrape. Now the pool decides which proxies are due, per proxy, with a
    // backoff, and the loop sleeps until the next one comes due.
    {
        let pool_clone = pool.clone();
        let health_clone = health.clone();
        let scraper_clone = scraper.clone();
        let scrape_state_clone = scrape_state.clone();
        let scraper_enabled = config.scraper.enabled;
        let scrape_interval = config.scraper.interval_secs;
        let progress = check_progress.clone();
        let in_flight = check_in_flight.clone();
        let policy = eviction_policy;
        let schedule = probe_schedule.clone();
        let db_path = db_path.clone();
        let max_proxies = config.pool.max_proxies;

        tokio::spawn(async move {
            // The clock is anchored on the last scrape, so the loop wakes for
            // whichever comes first: the next scrape, or the next probe due.
            let mut last_scrape =
                chrono::Utc::now() - chrono::Duration::seconds(scrape_interval as i64);

            loop {
                // ── Discovery ────────────────────────────────────────────
                let now = chrono::Utc::now();
                let scrape_due = (now - last_scrape).num_seconds() as u64 >= scrape_interval;

                if scraper_enabled && scrape_due {
                    let mut s = scrape_state_clone.write().await;
                    s.running = true;
                    s.errors.clear();
                    drop(s);

                    match scraper_clone.scrape_all().await {
                        Ok(proxies) => {
                            last_scrape = chrono::Utc::now();
                            {
                                let mut s = scrape_state_clone.write().await;
                                s.running = false;
                                s.last_run = Some(last_scrape);
                            }
                            info!("Auto scrape collected {} proxies", proxies.len());

                            // Merge rather than replace: proxies already known
                            // keep their latency, score and failure streak.
                            let added = pool_clone.merge(proxies).await;
                            // The capacity was a configuration field nothing
                            // enforced, so the pool grew with every scrape.
                            let dropped = pool_clone.enforce_capacity(max_proxies).await;
                            let pool_total = pool_clone.len().await;
                            info!(added, dropped, pool_total, "scrape merged into pool");
                        }
                        Err(e) => {
                            error!("Auto scrape cycle failed: {e}");
                            let mut s = scrape_state_clone.write().await;
                            s.running = false;
                            s.errors.push(format!("{e}"));
                        }
                    }
                }

                // ── Probing: everything the pool says is due ─────────────
                let due = pool_clone.due_for_check(chrono::Utc::now()).await;
                let pool_total = pool_clone.len().await;

                {
                    let mut s = scrape_state_clone.write().await;
                    s.proxies_found = pool_total;
                }

                if due.is_empty() {
                    info!(pool_total, "nothing due for a probe");
                } else {
                    // Only one batch at a time: a second one would contend for
                    // the same concurrency permits and delay the first.
                    if in_flight
                        .compare_exchange(
                            false,
                            true,
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                        )
                        .is_err()
                    {
                        warn!("Health check already in progress, deferring batch");
                    } else {
                        {
                            let mut s = scrape_state_clone.write().await;
                            s.check_running = true;
                            s.check_total = due.len();
                        }

                        let hc = health_clone.clone();
                        let pc = pool_clone.clone();
                        let sc = scrape_state_clone.write().await;
                        drop(sc);

                        let batch_progress = progress.clone();
                        let batch_in_flight = in_flight.clone();
                        let batch_policy = policy;
                        let batch_schedule = schedule.clone();
                        let batch_path = db_path.clone();
                        let batch_state = scrape_state_clone.clone();

                        tokio::spawn(async move {
                            // Stream each result into the pool as it arrives, so
                            // confirmed proxies become routable during the batch
                            // instead of only once the last probe finishes.
                            let counts = {
                                let pc = pc.clone();
                                hc.check_batch(&due, batch_progress, move |result| {
                                    let pc = pc.clone();
                                    let schedule = batch_schedule.clone();
                                    async move {
                                        pc.apply_health_result(&result, batch_policy, &schedule)
                                            .await;
                                    }
                                })
                                .await
                            };

                            batch_in_flight.store(false, std::sync::atomic::Ordering::SeqCst);

                            // One sweep for the whole batch: removing a proxy
                            // rebuilds the id index, so doing it per proxy would
                            // cost thousands of full rebuilds.
                            let evicted_count = pc.evict_pending().await;

                            // Resolve before the macro: an `.await` inside
                            // tracing arguments keeps its temporary alive
                            // across the await and makes the future !Send.
                            let healthy = pc.healthy_count().await;
                            let pool_size = pc.len().await;

                            {
                                let mut s = batch_state.write().await;
                                s.check_running = false;
                                s.healthy_count = healthy;
                            }

                            info!(
                                checked = counts.working + counts.failed + counts.inconclusive,
                                working = counts.working,
                                failed = counts.failed,
                                inconclusive = counts.inconclusive,
                                evicted = evicted_count,
                                healthy,
                                pool = pool_size,
                                "probe batch done"
                            );

                            // Persist once the batch settles. Writing per
                            // result would mean thousands of full-table
                            // rewrites. The connection is not `Sync`, so it is
                            // opened for the write and dropped again.
                            if let Some(path) = batch_path {
                                let snapshot = pc.all().await;
                                match tokio::task::spawn_blocking(move || {
                                    Storage::open(&path)?.replace_proxies(&snapshot)
                                })
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(e)) => error!("Failed to persist pool: {e}"),
                                    Err(e) => error!("Persist task failed: {e}"),
                                }
                            }
                        });
                    }
                }

                // ── Sleep until the next event ─────────────────────────────
                // The next probe due, or the next scrape, whichever is first.
                // Sleeping on the schedule rather than on a fixed period is
                // what keeps a quiet pool quiet.
                let now = chrono::Utc::now();
                let next_probe = pool_clone.next_due_at().await;
                let until_probe = next_probe
                    .map(|t| (t - now).num_seconds().max(0) as u64)
                    .unwrap_or(scrape_interval);
                let until_scrape =
                    ((last_scrape + chrono::Duration::seconds(scrape_interval as i64)) - now)
                        .num_seconds()
                        .max(0) as u64;

                let wait = until_probe.min(until_scrape).max(1).min(scrape_interval);
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
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
