use proxy_core::config::TunConfig;
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
const FWMARK_TABLE: u32 = 100;
const FWMARK: u32 = 1;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "proxy=info".into()),
        )
        .init();

    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        println!("=== E2E: Full traffic through TUN + Proxy ===");

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

        // Detect real gateway and interface BEFORE overriding the route table
        let mgr_temp = TunManager::new(TunConfig {
            name: "ps-tun0".into(),
            address: TUN_ADDR.into(),
            mtu: 1500,
            disable_ipv6: true,
        });
        let (gateway, iface) = mgr_temp.detect_gateway().await.expect("detect_gateway failed");
        println!("Real gateway: {gateway}, interface: {iface}");

        // Set up fwmark routing so our UDP sockets bypass the TUN (no loop)
        TunManager::setup_fwmark_routing(&gateway, &iface, FWMARK_TABLE, FWMARK)
            .await
            .expect("setup_fwmark_routing failed");
        println!("FWmark routing: mark {FWMARK} -> table {FWMARK_TABLE} via {gateway}");

        // Router with fwmark for UDP sockets
        let router = Arc::new(Router::with_fwmark(pool.clone(), FWMARK));

        // TUN device
        let mgr = TunManager::new(TunConfig {
            name: "ps-tun0".into(),
            address: TUN_ADDR.into(),
            mtu: 1500,
            disable_ipv6: true,
        });
        mgr.create().await.expect("TUN create failed");

        // Default route: all IPv4 through TUN
        mgr.set_default_route().await.expect("set_default_route failed");
        println!("Default route: 0.0.0.0/0 -> ps-tun0");

        // Exclude route: proxy IP via real gateway (avoids routing loop)
        mgr.add_exclude_route(PROXY_HOST, &gateway)
            .await
            .expect("add_exclude_route failed");
        println!("Exclude route: {PROXY_HOST} -> {gateway}");

        let dev = mgr.take_device().await.expect("No TUN device");
        drop(mgr);

        // Forwarding loop in background thread
        let router_fwd = router.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(run_forwarding_loop(dev, router_fwd, 1500));
        });

        // Wait for routes and forwarding loop to settle
        tokio::time::sleep(Duration::from_millis(1000)).await;

        println!("\n=== curl httpbin.org (full E2E: DNS → TCP → Proxy) ===");

        // Full E2E test: DNS goes through TUN → UDP forwarded → cached →
        // TCP SYN uses hostname → proxy CONNECT → response
        let out = std::process::Command::new("timeout")
            .args(["45", "curl", "-v", "--max-time", "35", "http://httpbin.org/ip"])
            .output()
            .expect("curl failed");

        let exit_code = out.status.code().unwrap_or(-1);
        println!("curl exit code: {exit_code}");

        let passed = if !out.stdout.is_empty() {
            let body = String::from_utf8_lossy(&out.stdout);
            println!("stdout: {body}");
            body.contains("origin")
        } else {
            false
        };

        if passed {
            println!("\n*** E2E PASS ***");
        } else {
            println!("\n*** E2E FAIL ***");
        }

        if !out.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let lines: Vec<&str> = stderr.trim().lines().collect();
            let tail = lines.len().saturating_sub(20);
            for line in &lines[tail..] {
                eprintln!("[curl] {line}");
            }
        }

        // Cleanup fwmark
        TunManager::cleanup_fwmark(FWMARK_TABLE, FWMARK).await;

        if !passed {
            std::process::exit(1);
        }
    });
}
