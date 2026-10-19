use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub tun: TunConfig,
    pub scraper: ScraperConfig,
    pub health: HealthConfig,
    pub pool: PoolConfig,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScraperConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    pub concurrency: usize,
    pub timeout_secs: u64,
    pub check_interval_secs: u64,
    pub target_url: String,
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
                concurrency: 100,
                timeout_secs: 5,
                // Re-probing every minute churned the pool for no benefit; a
                // public proxy that works now usually still works in ten
                // minutes, and the streak counter handles genuine decay.
                check_interval_secs: 600,
                target_url: "http://httpbin.org/ip".into(),
                max_consecutive_failures: default_failure_threshold(),
                unverified_ttl_secs: default_unverified_ttl(),
            },
            pool: PoolConfig {
                max_proxies: 1000,
                min_score: 0.0,
                auto_rotate: false,
                rotate_interval_secs: 120,
            },
        }
    }
}
