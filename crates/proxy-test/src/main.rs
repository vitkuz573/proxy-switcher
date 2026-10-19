use proxy_core::forwarder::Forwarder;
use proxy_core::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() {
    // Test with known-working proxy found via curl:
    // Direct IP: 109.195.134.55
    // Through proxy 116.101.75.173:2079: 116.101.75.173
    let forwarder = Forwarder::new();
    let proxy = ProxyInfo {
        id: "116.101.75.173:2079".into(),
        host: "116.101.75.173".into(),
        port: 2079,
        protocol: ProxyProtocol::Http,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: None,
        last_checked: None,
        last_success: None,
        first_seen: chrono::Utc::now(),
        next_probe_at: chrono::Utc::now(),
        exit_ip: None,
        password: None,
        supports_tunnel: true,
        consecutive_failures: 0,
        score: 0.0,
    };

    println!(
        "=== Testing Forwarder through proxy {}:{} ===",
        proxy.host, proxy.port
    );

    match forwarder.connect_to(&proxy, "example.com", 80, None).await {
        Ok(mut conn) => {
            let request = b"GET /ip HTTP/1.1\r\nHost: httpbin.org\r\nConnection: close\r\n\r\n";
            if let Err(e) = conn.write_all(request).await {
                eprintln!("Write error: {e}");
                return;
            }
            let mut resp = String::new();
            let mut buf = vec![0u8; 4096];
            loop {
                match conn.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => resp.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(e) => {
                        eprintln!("Read error: {e}");
                        break;
                    }
                }
            }
            println!("Response through proxy:\n{resp}");
            if resp.contains("\"origin\"") {
                println!("\n*** SUCCESS: Forwarder works! IP changed through proxy. ***\n");
            }
        }
        Err(e) => {
            eprintln!("Forwarder connect failed: {e}");
        }
    }

    println!("\n=== Direct connection (no proxy) ===");
    match forwarder.connect_direct("example.com", 80, None).await {
        Ok(mut conn) => {
            let request = b"GET /ip HTTP/1.1\r\nHost: httpbin.org\r\nConnection: close\r\n\r\n";
            if let Err(e) = conn.write_all(request).await {
                eprintln!("Write error: {e}");
                return;
            }
            let mut resp = String::new();
            let mut buf = vec![0u8; 4096];
            loop {
                match conn.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => resp.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(e) => {
                        eprintln!("Read error: {e}");
                        break;
                    }
                }
            }
            println!("Direct response:\n{resp}");
        }
        Err(e) => eprintln!("Direct connect failed: {e}"),
    }
}
