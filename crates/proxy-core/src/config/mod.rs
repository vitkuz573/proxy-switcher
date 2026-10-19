use crate::routing::RoutePolicy;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub tun: TunConfig,
    pub scraper: ScraperConfig,
    pub health: HealthConfig,
    pub pool: PoolConfig,
    /// Which traffic a proxy applies to.
    #[serde(default)]
    pub routing: RoutePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub api_host: String,
    pub api_port: u16,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunConfig {
    pub name: String,
    pub address: String,
    pub mtu: u16,
    pub disable_ipv6: bool,
    /// Point the default route at the TUN device.
    ///
    /// Off by default: it changes how the whole host reaches the internet.
    /// The routing policy decides which traffic a proxy applies to, and this
    /// decides whether the device sees the traffic at all.
    #[serde(default)]
    pub default_route: bool,
    /// Firewall mark for the daemon's own traffic.
    ///
    /// The daemon creates a TUN device and can point the default route at it.
    /// Its own connections — to the proxies it tests, to the sources it
    /// scrapes — would then be captured by that device and routed through a
    /// proxy, which is a loop, and which takes the daemon offline the moment
    /// the active proxy dies. Marked traffic bypasses the device.
    #[serde(default = "default_bypass_mark")]
    pub bypass_mark: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScraperConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Maximum probes in flight at once.
    ///
    /// The batch is I/O bound, so this is the main throughput lever: the pool
    /// reaches tens of thousands of proxies and the default was low enough
    /// that a full sweep took minutes.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Budget for writing the request and reading the response.
    pub timeout_secs: u64,
    /// Budget for the proxy handshake alone.
    ///
    /// Separate from `timeout_secs` because a proxy that will not complete a
    /// handshake is dead, and dead proxies are the majority of the pool.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// How often a proxy known to work is re-checked.
    pub check_interval_secs: u64,
    /// Wait after each consecutive failure, in seconds. The last entry applies
    /// to every further failure.
    #[serde(default = "default_failed_backoff")]
    pub failed_backoff_secs: Vec<u64>,
    /// Wait before retrying a proxy whose probe was inconclusive.
    #[serde(default = "default_inconclusive_backoff")]
    pub inconclusive_backoff_secs: u64,
    /// Probe targets, tried in order.
    ///
    /// The target is a third party: it can be blocked, overloaded or down, and
    /// in some networks most of them are unreachable. More than one, and the
    /// engine verifies a target directly before using it, so an unreachable
    /// target never marks the pool dead.
    #[serde(default = "default_probe_targets")]
    pub target_urls: Vec<String>,
    /// Consecutive failed probes a proxy survives before it leaves the pool.
    #[serde(default = "default_failure_threshold")]
    pub max_consecutive_failures: u32,
    /// How long a proxy that has never worked is kept while unverified.
    #[serde(default = "default_unverified_ttl")]
    pub unverified_ttl_secs: u64,
}

fn default_failure_threshold() -> u32 {
    3
}

fn default_probe_targets() -> Vec<String> {
    vec![
        "http://icanhazip.com".to_string(),
        "http://api.ipify.org".to_string(),
        "http://ifconfig.me/ip".to_string(),
    ]
}

fn default_bypass_mark() -> u32 {
    crate::net::ENGINE_MARK
}

fn default_concurrency() -> usize {
    500
}

fn default_connect_timeout() -> u64 {
    3
}

fn default_failed_backoff() -> Vec<u64> {
    vec![60, 300, 900, 3600, 10_800, 21_600]
}

fn default_inconclusive_backoff() -> u64 {
    300
}

fn default_unverified_ttl() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub max_proxies: usize,
    pub min_score: f64,
    pub auto_rotate: bool,
    pub rotate_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig {
                api_host: "127.0.0.1".into(),
                api_port: 8080,
                data_dir: PathBuf::from("/var/lib/proxy-switcher"),
            },
            tun: TunConfig {
                name: "ps-tun0".into(),
                address: "10.0.0.1".into(),
                mtu: 1500,
                disable_ipv6: true,
                default_route: false,
                bypass_mark: default_bypass_mark(),
            },
            scraper: ScraperConfig {
                enabled: true,
                interval_secs: 300,
                sources: vec![
                    "https://free-proxy-list.net".into(),
                    "https://www.sslproxies.org".into(),
                    "https://www.us-proxy.org".into(),
                ],
            },
            health: HealthConfig {
                concurrency: default_concurrency(),
                timeout_secs: 5,
                connect_timeout_secs: default_connect_timeout(),
                failed_backoff_secs: default_failed_backoff(),
                inconclusive_backoff_secs: default_inconclusive_backoff(),
                // Re-probing every minute churned the pool for no benefit; a
                // public proxy that works now usually still works in ten
                // minutes, and the streak counter handles genuine decay.
                check_interval_secs: 600,
                // Address echo services. The probe needs the address the
                // target sees, because that is the proof that the request
                // travelled through the proxy: an endpoint that answers HTTP
                // is not a proxy, and only the address tells them apart.
                target_urls: default_probe_targets(),
                max_consecutive_failures: default_failure_threshold(),
                unverified_ttl_secs: default_unverified_ttl(),
            },
            routing: RoutePolicy::default(),
            pool: PoolConfig {
                max_proxies: 1000,
                min_score: 0.0,
                auto_rotate: false,
                rotate_interval_secs: 120,
            },
        }
    }
}
