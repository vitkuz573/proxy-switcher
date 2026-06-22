use proxy_core::config::TunConfig;
use proxy_core::dns::DnsCache;
use proxy_core::pool::ProxyPool;
use proxy_core::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use proxy_core::router::Router;
use proxy_core::tun_manager::{run_forwarding_loop, TunManager};
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

const PROXY_HOST: &str = "116.101.75.173";
const PROXY_PORT: u16 = 2079;
const TUN_ADDR: &str = "10.99.0.1";
const HTTPBIN_IP: &str = "52.5.245.178";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "proxy=debug".into()),
        )
        .init();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        println!("=== E2E: TUN + Router + Forwarder + Proxy ===");

        // Pool with known proxy
        let pool = Arc::new(ProxyPool::new());
        let proxy = ProxyInfo {
            id: format!("{PROXY_HOST}:{PROXY_PORT}"),
            host: PROXY_HOST.into(),
            port: PROXY_PORT,
            protocol: ProxyProtocol::Http,
            anonymity: Anonymity::Unknown,
            latency_ms: Some(500),
            country: None,
            last_checked: None,
            score: 100.0,
        };
        pool.update(vec![proxy]).await;
        pool.set_active(&format!("{PROXY_HOST}:{PROXY_PORT}"))
            .await;
        println!("Proxy: {}", pool.active().await.unwrap().id);

        // DNS cache with httpbin.org mapping
        let dns_cache = Arc::new(DnsCache::new());
        dns_cache.insert_str(HTTPBIN_IP, "httpbin.org").await;
        println!("DNS cache: {HTTPBIN_IP} -> httpbin.org");

        // Router with shared DNS cache
        let router = Arc::new(Router::with_dns_cache(pool.clone(), dns_cache.clone()));

        // TUN
        let mgr = TunManager::new(TunConfig {
            name: "ps-tun0".into(),
            address: TUN_ADDR.into(),
            mtu: 1500,
            disable_ipv6: true,
        });
        mgr.create().await.expect("TUN create failed");
        let dev = mgr.take_device().await.expect("No TUN device");
        drop(mgr);

        // Forwarding loop
        let router_fwd = router.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(run_forwarding_loop(dev, router_fwd, 1500));
        });

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Route httpbin through TUN
        let out = std::process::Command::new("ip")
            .args(["route", "add", HTTPBIN_IP, "dev", "ps-tun0"])
            .output()
            .expect("ip route failed");
        if !out.status.success() {
            eprintln!("Route error: {}", String::from_utf8_lossy(&out.stderr));
        } else {
            println!("Route: {HTTPBIN_IP} -> ps-tun0");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        println!("\n=== curl httpbin.org through TUN ===");

        let out = std::process::Command::new("timeout")
            .args(["30", "curl", "-v", "--max-time", "25", "--resolve", &format!("httpbin.org:80:{HTTPBIN_IP}"), "http://httpbin.org/ip"])
            .output()
            .expect("curl failed");

        let exit_code = out.status.code().unwrap_or(-1);
        println!("curl exit code: {exit_code}");
        if !out.stdout.is_empty() {
            let body = String::from_utf8_lossy(&out.stdout);
            println!("stdout: {body}");
            if body.contains(PROXY_HOST) || body.contains("origin") {
                println!("\n*** E2E PASS ***");
            }
        }
        if !out.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let lines: Vec<&str> = stderr.trim().lines().collect();
            let tail = lines.len().saturating_sub(20);
            for line in &lines[tail..] {
                eprintln!("[curl] {line}");
            }
        }

        // Direct test
        println!("\n=== Direct request ===");
        let out = std::process::Command::new("curl")
            .args(["-s", "--max-time", "10", "http://httpbin.org/ip"])
            .output()
            .expect("curl direct failed");
        if out.status.success() {
            println!("Direct: {}", String::from_utf8_lossy(&out.stdout));
        }
    });
}
