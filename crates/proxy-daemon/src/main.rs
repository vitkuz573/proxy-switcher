mod api;
mod policy;

use anyhow::Context;
use clap::Parser;
use proxy_core::config::Config;
use proxy_core::health::HealthChecker;
use proxy_core::pool::{EvictionPolicy, ProbeSchedule, ProxyPool};
use proxy_core::proxy::ProxyProtocol;
use proxy_core::router::Router;
use proxy_core::scraper::{Scraper, SourceSpec};
use proxy_core::storage::Storage;
use proxy_core::tun_manager::{run_forwarding_loop, TunManager};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[derive(Parser)]
#[command(name = "proxy-daemon", about = "Proxy Switcher daemon")]
struct Args {
    #[arg(short, long, default_value = "/etc/proxy-switcher/config.toml")]
    config: String,
}

/// Load the country database, fetching it when it is missing or stale.
///
/// The fetch is the daemon's own traffic, so it bypasses the device the
/// daemon itself created: asking a proxy for the proxy list would be asking
/// the thing that is broken for a way to get the thing that is broken.
async fn load_geo_database(
    config: &proxy_core::config::GeoConfig,
) -> Option<proxy_core::geo::GeoDatabase> {
    if !config.enabled {
        return None;
    }

    let fresh = std::fs::metadata(&config.database)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .map(|elapsed| elapsed.as_secs() < config.update_interval_secs)
        .unwrap_or(false);

    if !fresh {
        if let Err(e) = download_geo_database(config).await {
            warn!("could not refresh the country database: {e}");
        }
    }

    match proxy_core::geo::GeoDatabase::load(&config.database) {
        Ok(database) => Some(database),
        Err(e) => {
            warn!("could not read the country database: {e}");
            None
        }
    }
}

/// Fetch the country database.
async fn download_geo_database(config: &proxy_core::config::GeoConfig) -> anyhow::Result<()> {
    use std::io::Write;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.download_timeout_secs))
        .user_agent("proxy-switcher/1.0")
        .build()?;

    info!(url = %config.update_url, "fetching the country database");
    let body = client
        .get(&config.update_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    if let Some(parent) = config.database.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Written beside the target and moved into place, so an interrupted
    // download never leaves a half-written database behind.
    let temporary = config.database.with_extension("download");
    {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &config.database)?;

    info!(
        bytes = body.len(),
        file = %config.database.display(),
        "country database fetched"
    );
    Ok(())
}

/// Read a file of sources.
///
/// The file is TOML, so a source can carry a comment explaining why it is
/// there, and the protocol it is about. The format is deliberately not
/// compiled into the binary: where to look for proxies changes far more often
/// than the code does.
fn load_source_file(path: &std::path::Path) -> anyhow::Result<Vec<SourceSpec>> {
    let body = std::fs::read_to_string(path)?;

    // Both shapes are accepted: a TOML document of `[[source]]` entries, and
    // a plain list of addresses, one per line.
    if body.contains("[[source]]") {
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(default)]
            source: Vec<Entry>,
        }

        #[derive(serde::Deserialize)]
        struct Entry {
            url: String,
            protocol: Option<String>,
        }

        let file: File = toml::from_str(&body)?;
        return Ok(file
            .source
            .into_iter()
            .filter_map(|entry| {
                let url = entry.url.trim().to_string();
                if url.is_empty() {
                    return None;
                }
                Some(SourceSpec::new(
                    url,
                    parse_protocol(entry.protocol.as_deref()),
                ))
            })
            .collect());
    }

    let mut sources = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // A line may be "url" or "url,protocol".
        let url = line.split(',').next().unwrap_or(line).trim();
        if url.is_empty() {
            continue;
        }

        let protocol = line
            .split_once(',')
            .map(|(_, protocol)| protocol.trim().to_string());

        let spec = SourceSpec::new(url, parse_protocol(protocol.as_deref()));
        if !sources.contains(&spec) {
            sources.push(spec);
        }
    }

    Ok(sources)
}

/// Parse a protocol name.
fn parse_protocol(name: Option<&str>) -> Option<ProxyProtocol> {
    match name?.trim().to_ascii_lowercase().as_str() {
        "http" => Some(ProxyProtocol::Http),
        "https" => Some(ProxyProtocol::Https),
        "socks4" => Some(ProxyProtocol::Socks4),
        "socks5" => Some(ProxyProtocol::Socks5),
        "trojan" => Some(ProxyProtocol::Trojan),
        _ => None,
    }
}

/// How many proxies one round of the health check takes.
///
/// A round is what the check holds in memory at once, so it is a memory
/// decision as much as a scheduling one. Large enough that the per-round
/// overhead disappears, small enough that a pool of any size costs the same.
const ROUND_SIZE: usize = 20_000;

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

    let pool = Arc::new(ProxyPool::new().with_min_score(config.pool.min_score));

    // The country database.
    //
    // The engine knows the address every verified proxy exits from, so the
    // country is looked up rather than believed: the lists name a country in
    // words, often the wrong words, and name none at all for most entries.
    let geo_database = match load_geo_database(&config.geo).await {
        Some(database) => {
            info!(
                ranges = database.len(),
                file = %config.geo.database.display(),
                "country database loaded"
            );
            Some(Arc::new(database))
        }
        None => {
            warn!("no country database; the proxies will have no country");
            None
        }
    };

    let db_path = if let Some(parent) = config.daemon.data_dir.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
        Some(config.daemon.data_dir.join("proxy-switcher.db"))
    } else {
        None
    };

    // The active proxy the operator chose, kept in memory as well as on disk.
    //
    // The pool is restored from disk before the first scrape, and the proxy
    // the operator chose may only turn up in that scrape. The choice is kept
    // until the proxy is there.
    let remembered_active = Arc::new(RwLock::new(None::<String>));

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

        // The active proxy is remembered.
        //
        // It is the operator's choice, and a restart that forgets it leaves
        // the routing policy pointing at nothing: every destination the
        // policy covers is refused, and the host quietly loses the traffic it
        // was supposed to have proxied.
        let chosen = Storage::open(path)
            .and_then(|s| s.load_setting("active_proxy"))
            .unwrap_or(None)
            .filter(|id| !id.is_empty());
        if let Some(id) = chosen {
            *remembered_active.write().await = Some(id.clone());
            match pool.set_active(&id).await {
                Some(proxy) => info!(id = %proxy.id, "active proxy restored"),
                None => warn!(id = %id, "the remembered active proxy is not in the pool yet"),
            }
        }
    }

    // The proxies that were condemned are remembered as well. The sources
    // offer them again on every scrape, and without this the pool fills up
    // with proxies that have already been found dead.
    if let Some(path) = db_path.as_ref() {
        let condemned = Storage::open(path)
            .and_then(|storage| storage.load_condemned())
            .unwrap_or_default();

        if !condemned.is_empty() {
            pool.remember_condemned(condemned).await;
        }
    }

    // The pool is geolocated after it is restored, for the proxies that were
    // verified before the last restart.
    if let Some(database) = geo_database.as_ref() {
        let located = pool.geolocate(database).await;
        if located > 0 {
            info!(located, "proxies geolocated on start-up");
        }
    }

    // The daemon's own traffic bypasses the TUN it created.
    //
    // Without this the connections it makes — to the proxies it tests, to the
    // sources it scrapes — are captured by the device and routed through the
    // active proxy. That is a loop, and when the proxy dies the daemon can no
    // longer scrape or check anything to recover.
    let bypass_mark = config.tun.bypass_mark;

    // Which traffic a proxy applies to. The default routes nothing: a proxy
    // is a tool for specific destinations, not a default route for the host.
    let route_policy = config.routing.clone();

    // Initialize TUN
    let tun = TunManager::new(config.tun.clone());
    let tun_ok = tun.create().await.is_ok();

    // The gateway has to be read before the default route is moved, or there
    // is nothing left to point the bypass table at.
    let gateway = tun.detect_gateway().await.ok();

    if tun_ok {
        if let Some((gateway, iface)) = gateway.as_ref() {
            // Marked traffic — everything the daemon itself opens — goes
            // straight out through the real gateway instead of into the
            // device. This is what keeps the daemon reachable when the
            // proxy it is testing is the thing that broke.
            if let Err(e) = TunManager::setup_fwmark_routing(
                gateway,
                iface,
                proxy_core::net::ENGINE_TABLE,
                bypass_mark,
            )
            .await
            {
                warn!("Could not set up the bypass routing table: {e}");
            }
        }

        // The default route is only moved when the operator asks for it: it
        // changes how the whole host reaches the internet.
        if config.tun.default_route {
            if let Err(e) = tun.set_default_route().await {
                error!("Could not point the default route at the TUN: {e}");
            }
        }
    }

    // Take device and start forwarding loop
    let router = Arc::new(Router::with_policy(
        pool.clone(),
        route_policy.clone(),
        Some(bypass_mark),
    ));

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
        config.health.target_urls.clone(),
        Some(bypass_mark),
    ));

    // Shared state for API
    //
    // Three places the list of sources can come from, in this order:
    //
    //   1. the file in the repository or the deployment,
    //   2. the list added at run time through the API, which is remembered,
    //   3. the inline list in the configuration file.
    //
    // Nothing is compiled into the binary. The file is the base list, the API
    // adds to it, and a deployment that wants no list at all simply says so.
    let mut sources: Vec<SourceSpec> = config
        .scraper
        .sources
        .iter()
        .map(|url| SourceSpec::new(url.clone(), None))
        .collect();

    if let Some(path) = config.scraper.sources_file.as_ref() {
        match load_source_file(path) {
            Ok(from_file) => {
                info!(count = from_file.len(), file = %path.display(), "scrape sources read from file");
                for source in from_file {
                    if !sources.contains(&source) {
                        sources.push(source);
                    }
                }
            }
            Err(e) => error!(
                "Could not read the scrape sources from {}: {e}",
                path.display()
            ),
        }
    }

    if let Some(path) = db_path.as_ref() {
        match Storage::open(path).and_then(|s| s.load_sources()) {
            Ok(stored) => {
                if !stored.is_empty() {
                    info!(count = stored.len(), "scrape sources restored from storage");
                    for source in stored {
                        if !sources.contains(&source) {
                            sources.push(source);
                        }
                    }
                }
            }
            Err(e) => error!("Could not read the scrape sources from storage: {e}"),
        }
    }

    let sources = Arc::new(RwLock::new(sources));
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

    // The pool over time: one sample a minute, a day of history.
    //
    // The samples are kept on disk as well: the graph says how the pool has
    // moved, and a restart used to empty it.
    let history = api::PoolHistory::new(24 * 60);
    if let Some(path) = db_path.as_ref() {
        let kept = Storage::open(path)
            .and_then(|storage| storage.load_setting("pool_history"))
            .unwrap_or(None);

        if let Some(kept) = kept {
            match serde_json::from_str::<Vec<proxy_core::pool::statistics::HistorySample>>(&kept) {
                Ok(samples) if !samples.is_empty() => {
                    history.restore(samples);
                    info!(
                        samples = history.samples().len(),
                        "the pool history restored"
                    );
                }
                Ok(_) => {}
                Err(e) => warn!("the pool history could not be read: {e}"),
            }
        }
    }

    let policy_destinations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let policy_routes = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // The credentials the local proxy accepts, and the policies they carry.
    let credentials = match db_path.as_ref() {
        Some(path) => match proxy_core::credentials::CredentialStore::open(path) {
            Ok(store) => {
                let store = Arc::new(store);
                if let Err(e) = store.ensure_builtin_policies().await {
                    error!("could not write the built-in policies: {e}");
                }
                Some(store)
            }
            Err(e) => {
                error!("could not open the credential store: {e}");
                None
            }
        },
        None => None,
    };

    // The local proxy: a port a program can be pointed at.
    let local_proxy = Arc::new(proxy_core::local_proxy::LocalProxy::new(
        proxy_core::local_proxy::LocalProxyConfig {
            enabled: config.local_proxy.enabled,
            host: config.local_proxy.host.clone(),
            port: config.local_proxy.port,
            allow_anonymous: config.local_proxy.allow_anonymous,
            max_attempts: config.local_proxy.max_attempts,
            timeout: std::time::Duration::from_secs(config.local_proxy.timeout_secs),
        },
        pool.clone(),
        Some(bypass_mark),
        credentials.clone(),
    ));

    if config.local_proxy.enabled {
        let proxy = local_proxy.clone();
        tokio::spawn(async move {
            if let Err(e) = proxy.serve().await {
                error!("the local proxy stopped: {e}");
            }
        });
    }

    let app_state = api::AppState {
        history: history.clone(),
        min_score: config.pool.min_score,
        pool: pool.clone(),
        router: router.clone(),
        scraper: scraper.clone(),
        health: health.clone(),
        scrape_state: scrape_state.clone(),
        sources: sources.clone(),
        check_progress: check_progress.clone(),
        check_in_flight: check_in_flight.clone(),
        eviction_policy,
        db_path: db_path.clone(),
        route_policy: route_policy.clone(),
        policy_destinations: policy_destinations.clone(),
        policy_routes: policy_routes.clone(),
        remembered_active: remembered_active.clone(),
        local_proxy: local_proxy.clone(),
        credentials: credentials.clone(),
        bypass_mark,
        default_route: config.tun.default_route,
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
        let remembered = remembered_active.clone();

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

                    // The results go straight into the pool: a list of two
                    // hundred thousand proxies would be held twice over.
                    let added = scraper_clone.scrape_into(&pool_clone).await.unwrap_or(0);
                    last_scrape = chrono::Utc::now();

                    {
                        let mut s = scrape_state_clone.write().await;
                        s.running = false;
                        s.last_run = Some(last_scrape);
                    }

                    // The remembered proxy is applied once the pool knows it.
                    //
                    // The pool is restored from disk before the first scrape,
                    // and the proxy the operator chose may only turn up in
                    // that scrape. The choice is kept until the proxy is
                    // there, so a restart does not quietly leave the routing
                    // policy pointing at nothing.
                    let remembered = remembered.read().await.clone();
                    if let Some(id) = remembered {
                        if pool_clone.active().await.is_none() {
                            if let Some(proxy) = pool_clone.set_active(&id).await {
                                info!(id = %proxy.id, "active proxy restored");
                            }
                        }
                    }

                    // The capacity was a configuration field nothing
                    // enforced, so the pool grew with every scrape.
                    let dropped = pool_clone.enforce_capacity(max_proxies).await;
                    let pool_total = pool_clone.len().await;
                    info!(added, dropped, pool_total, "scrape cycle merged into pool");
                }

                // ── Probing: everything the pool says is due ─────────────
                //
                // The pool is asked for the work in rounds rather than handing
                // over the whole list. A pool of two hundred thousand proxies
                // cloned in one go is two hundred thousand records held twice
                // over, and the copy buys nothing: the pool is already the
                // only place they live.
                let now = chrono::Utc::now();
                let due_total = pool_clone.due_count(now).await;
                let pool_total = pool_clone.len().await;

                {
                    let mut s = scrape_state_clone.write().await;
                    s.proxies_found = pool_total;
                }

                if due_total == 0 {
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
                        // Not worth a log line every second: the batch that
                        // is running is doing the work.
                        tracing::debug!("Health check already in progress, deferring batch");
                    } else {
                        {
                            let mut s = scrape_state_clone.write().await;
                            s.check_running = true;
                            s.check_total = due_total;
                        }

                        // The counter belongs to the batch: the rounds that
                        // follow add to it, so the progress of the whole
                        // batch is what is shown.
                        progress.store(0, std::sync::atomic::Ordering::Relaxed);

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
                        let geo = geo_database.clone();

                        tokio::spawn(async move {
                            // Stream each result into the pool as it arrives, so
                            // confirmed proxies become routable during the batch
                            // instead of only once the last probe finishes.
                            // The reasons are counted as well as the verdicts:
                            // a batch where everything fails at once is a
                            // problem with the probe path, not with thousands
                            // of proxies, and the difference is only visible
                            // in the reasons.
                            let reasons: Arc<
                                std::sync::Mutex<std::collections::HashMap<String, usize>>,
                            > = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

                            let mut working = 0usize;
                            let mut failed = 0usize;
                            let mut inconclusive = 0usize;

                            // When the pool was last written down.
                            let mut last_persisted = std::time::Instant::now();

                            // The round has a deadline.
                            //
                            // The loop used to end when nothing was due, and
                            // that never happened: the proxies that failed
                            // are looked at again after a while, so something
                            // was always due and the round never ended. The
                            // sweep that removes the condemned ran at the end
                            // of the round, so it never ran at all.
                            //
                            // The round now covers the pool once and stops.
                            // A working proxy is looked at again after the
                            // interval, and the next round picks up whatever
                            // is due by then.
                            let deadline = std::time::Instant::now()
                                + std::time::Duration::from_secs(
                                    batch_schedule.working_interval_secs as u64,
                                );

                            // Rounds, until the pool has nothing left that is
                            // due. Each round is a bounded number of proxies,
                            // so the memory a check needs is a round and not
                            // the pool.
                            loop {
                                if std::time::Instant::now() >= deadline {
                                    info!("the round has covered the pool and stops here");
                                    break;
                                }
                                // The pool is written down while the batch
                                // runs.
                                //
                                // A batch of the whole pool takes half an
                                // hour, and the state it collects is worth
                                // more than the state it had before: a
                                // restart in the middle of a batch used to
                                // throw away every confirmation the batch
                                // had made.
                                if last_persisted.elapsed() >= std::time::Duration::from_secs(300) {
                                    last_persisted = std::time::Instant::now();

                                    if let Some(path) = batch_path.clone() {
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
                                }

                                let round = pc.due_batch(chrono::Utc::now(), ROUND_SIZE).await;
                                if round.is_empty() {
                                    break;
                                }

                                let round_size = round.len();
                                let batch_schedule = batch_schedule.clone();
                                let counts = {
                                    let pc = pc.clone();
                                    let reasons = reasons.clone();
                                    // The database is captured by reference:
                                    // the closure runs once per result, and
                                    // moving it in would take it out of the
                                    // round.
                                    let geo = geo.as_ref();
                                    hc.check_batch(&round, batch_progress.clone(), move |result| {
                                        let pc = pc.clone();
                                        let reasons = reasons.clone();
                                        let schedule = batch_schedule.clone();
                                        async move {
                                            if let Some(reason) = result.error.as_ref() {
                                                if let Ok(mut map) = reasons.lock() {
                                                    let key: String =
                                                        reason.chars().take(48).collect();
                                                    *map.entry(key).or_default() += 1;
                                                }
                                            }
                                            pc.apply_health_result(
                                                &result,
                                                batch_policy,
                                                &schedule,
                                            )
                                            .await;

                                            // The exit address is known now,
                                            // so the country can be worked
                                            // out straight away.
                                            if let Some(database) = geo {
                                                pc.geolocate_one(&result.proxy_id, database).await;
                                            }
                                        }
                                    })
                                    .await
                                };

                                working += counts.working;
                                failed += counts.failed;
                                inconclusive += counts.inconclusive;

                                let total = working + failed + inconclusive;
                                if total % 20_000 < round_size {
                                    info!(
                                        checked = total,
                                        working, failed, inconclusive, "probe round"
                                    );
                                }
                            }

                            if let Ok(map) = reasons.lock() {
                                let mut ranked: Vec<(&String, &usize)> = map.iter().collect();
                                ranked.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
                                for (reason, count) in ranked.iter().take(6) {
                                    warn!(reason = %reason, count, "probe failure reason");
                                }
                            }

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
                                checked = working + failed + inconclusive,
                                working,
                                failed,
                                inconclusive,
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
                                let condemned = pc.condemned().await;
                                match tokio::task::spawn_blocking(move || {
                                    let storage = Storage::open(&path)?;
                                    storage.replace_proxies(&snapshot)?;
                                    storage.save_condemned(&condemned)
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

                // While a batch is running there is nothing to do but wait for
                // it, and the next due proxy may be far off. Sleeping a moment
                // rather than a second keeps the loop from spinning.
                let wait = if in_flight.load(std::sync::atomic::Ordering::SeqCst) {
                    5
                } else {
                    until_probe.min(until_scrape).max(1).min(scrape_interval)
                };
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
        });
    }

    // The policy routes.
    //
    // The device is not the default route. The destinations the policy names
    // are pointed at it and everything else keeps the route it had, so a proxy
    // that stops working takes down the destinations it was given and nothing
    // else. The daemon's own traffic bypasses the device as well, so it cannot
    // end up trying to reach the internet through the thing it is testing.
    {
        let tun = tun.clone();
        let router = router.clone();
        let policy = route_policy.clone();
        let policy_destinations = policy_destinations.clone();
        let policy_routes = policy_routes.clone();

        tokio::spawn(async move {
            // The names the policy covered last time. A name that is no
            // longer routed is forgotten, so the cache does not grow a
            // picture of the internet that is not there any more.
            let mut known: Vec<std::net::Ipv4Addr> = Vec::new();

            loop {
                let resolved = policy::resolve_policy(&policy).await;

                // The names the policy covers are put into the resolver's
                // cache, so a connection arriving for one of those addresses
                // is recognised by name — which is what the policy is written
                // in, and what a plain HTTP request needs for its Host header.
                let cache = router.dns_cache();
                let mut current: Vec<std::net::Ipv4Addr> = Vec::new();
                for (address, name) in &resolved.destinations {
                    if let (IpAddr::V4(v4), Some(name)) = (address, name) {
                        cache.insert(*v4, name.clone()).await;
                        current.push(*v4);
                    }
                }
                for stale in known.drain(..) {
                    if !current.contains(&stale) {
                        cache.remove(stale).await;
                    }
                }
                known = current;

                // The router is told what the device is carrying, so a
                // connection that arrives there is proxied even when the
                // router never learned the name behind the address.
                router
                    .set_routed_destinations(
                        resolved
                            .destinations
                            .keys()
                            .filter_map(|address| match address {
                                IpAddr::V4(v4) => Some(*v4),
                                IpAddr::V6(_) => None,
                            })
                            .collect(),
                    )
                    .await;

                match policy::apply(&tun, &policy, &resolved).await {
                    Ok(routes) => {
                        policy_destinations
                            .store(resolved.len(), std::sync::atomic::Ordering::Relaxed);
                        policy_routes.store(routes, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => warn!("could not apply the policy routes: {e}"),
                }

                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // The sampler: one snapshot of the pool a minute, for the history.
    {
        let pool = pool.clone();
        let history = history.clone();
        let min_score = config.pool.min_score;
        let db_path = db_path.clone();
        let remembered = remembered_active.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;

                let now = chrono::Utc::now();
                let statistics = pool.statistics(min_score, now).await;
                history.push(proxy_core::pool::statistics::HistorySample {
                    at: now,
                    pool_size: statistics.pool_size,
                    working: statistics.working,
                    due: statistics.due,
                });

                // The history is written down with it: a restart used to
                // empty the graph.
                if let Some(path) = db_path.clone() {
                    let samples = history.samples();
                    match serde_json::to_string(&samples) {
                        Ok(encoded) => {
                            let written = tokio::task::spawn_blocking(move || {
                                Storage::open(&path)?.save_setting("pool_history", &encoded)
                            })
                            .await;

                            match written {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => warn!("the pool history could not be written: {e}"),
                                Err(e) => warn!("the write-down task failed: {e}"),
                            }
                        }
                        Err(e) => warn!("the pool history could not be encoded: {e}"),
                    }
                }

                // The active proxy is written down when it changes.
                //
                // The engine moves on by itself when the proxy in front dies,
                // and a restart that forgot the move would put the dead proxy
                // back in front.
                let active = pool.active().await.map(|p| p.id);
                let known = remembered.read().await.clone();
                if active != known {
                    if let Some(path) = db_path.as_ref() {
                        let written = match active.as_ref() {
                            Some(id) => proxy_core::storage::Storage::open(path)
                                .and_then(|s| s.save_setting("active_proxy", id)),
                            None => proxy_core::storage::Storage::open(path)
                                .and_then(|s| s.save_setting("active_proxy", "")),
                        };
                        if let Err(e) = written {
                            warn!("could not remember the active proxy: {e}");
                        }
                    }
                    *remembered.write().await = active;
                }
            }
        });
    }

    // Automatic rotation.
    //
    // The setting existed and did nothing: a proxy left selected stays
    // selected until it dies, and a public proxy that works now often stops
    // working within the hour. Rotating on a schedule keeps a fresh proxy in
    // front without anyone having to intervene.
    if config.pool.auto_rotate && config.pool.rotate_interval_secs > 0 {
        let pool = pool.clone();
        let interval = config.pool.rotate_interval_secs;
        let min_score = config.pool.min_score;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;

                let candidates = pool.tunnel_capable(min_score).await;
                if candidates.is_empty() {
                    warn!("Nothing to rotate to");
                    continue;
                }

                // Move to a different proxy than the current one, so the
                // rotation actually rotates.
                let current = pool.active().await.map(|p| p.id);
                let next = candidates
                    .iter()
                    .find(|p| Some(&p.id) != current.as_ref())
                    .or_else(|| candidates.first());

                if let Some(next) = next {
                    match pool.set_active(&next.id).await {
                        Some(selected) => info!(id = %selected.id, "rotated"),
                        None => warn!("could not select {}", next.id),
                    }
                }
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

    // The pool is written down on the way out.
    //
    // The health checks spend half an hour on a full pass, and a restart in
    // the middle of one used to throw away every confirmation the pass had
    // made. The state is worth more than the time it takes to write.
    if let Some(path) = db_path.clone() {
        let snapshot = pool.all().await;
        match tokio::task::spawn_blocking(move || Storage::open(&path)?.replace_proxies(&snapshot))
            .await
        {
            Ok(Ok(())) => info!("the pool was written down on the way out"),
            Ok(Err(e)) => error!("could not write the pool down: {e}"),
            Err(e) => error!("the write-down task failed: {e}"),
        }
    }

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
