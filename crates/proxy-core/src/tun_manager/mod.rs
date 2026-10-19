use crate::packet::ParsedPacket;
use crate::router::Router;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

use std::io::Write;

pub struct TunManager {
    config: crate::config::TunConfig,
    dev: Arc<Mutex<Option<tun::Device>>>,
    /// Set during shutdown so the blocking forwarding loop can return.
    shutdown: Arc<AtomicBool>,
}

fn parse_ipv4(addr: &str) -> Result<(u8, u8, u8, u8)> {
    let parts: Vec<&str> = addr.split('.').collect();
    if parts.len() != 4 {
        anyhow::bail!("Invalid IPv4 address: {addr}");
    }
    let octets: Result<Vec<u8>, _> = parts.iter().map(|p| p.parse::<u8>()).collect();
    let octets = octets.map_err(|e| anyhow::anyhow!("Invalid IPv4 address {addr}: {e}"))?;
    Ok((octets[0], octets[1], octets[2], octets[3]))
}

impl TunManager {
    pub fn new(config: crate::config::TunConfig) -> Self {
        Self {
            config,
            dev: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Handle the forwarding loop can poll to learn it should stop.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        self.shutdown.clone()
    }

    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    pub async fn create(&self) -> Result<()> {
        let addr = parse_ipv4(&self.config.address)?;

        let mut cfg = tun::Configuration::default();
        cfg.tun_name(&self.config.name)
            .address(addr)
            .netmask((255, 255, 255, 0))
            .mtu(self.config.mtu)
            .up();

        let dev = tokio::task::spawn_blocking(move || tun::create(&cfg))
            .await
            .context("Failed to spawn blocking task for TUN creation")?
            .context("Failed to create TUN device")?;

        info!(
            "TUN device {} created at {}",
            self.config.name, self.config.address
        );

        let mut guard = self.dev.lock().await;
        *guard = Some(dev);
        Ok(())
    }

    /// Route all IPv4 traffic through the TUN device (0.0.0.0/0).
    pub async fn set_default_route(&self) -> Result<()> {
        let output = tokio::process::Command::new("ip")
            .args(["route", "replace", "default", "dev", &self.config.name])
            .output()
            .await
            .context("Failed to set default route")?;

        if !output.status.success() {
            anyhow::bail!(
                "ip route failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        info!("Default route set to {}", self.config.name);
        Ok(())
    }

    /// Detect the current default gateway and interface before TUN overrides
    /// the route table.
    pub async fn detect_gateway(&self) -> Result<(String, String)> {
        let out = tokio::process::Command::new("sh")
            .args(["-c", "ip route show default | awk '{print $3, $5}'"])
            .output()
            .await
            .context("Failed to detect default gateway")?;
        let output = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let parts: Vec<&str> = output.split_whitespace().collect();
        if parts.len() < 2 {
            anyhow::bail!("No default gateway/interface found");
        }
        Ok((parts[0].to_string(), parts[1].to_string()))
    }

    /// Route the proxy's own IP through the real gateway instead of TUN to
    /// avoid routing loops. Must be called with the gateway captured BEFORE
    /// set_default_route().
    pub async fn add_exclude_route(&self, proxy_ip: &str, gateway: &str) -> Result<()> {
        let output = tokio::process::Command::new("ip")
            .args(["route", "add", proxy_ip, "via", gateway])
            .output()
            .await
            .context("Failed to add exclude route")?;

        if !output.status.success() {
            warn!(
                "add_exclude_route (may already exist): {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            info!("Exclude route: {proxy_ip} via {gateway}");
        }
        Ok(())
    }

    /// Set up a policy routing rule so that marked sockets bypass the TUN and
    /// follow the same path as ordinary traffic.
    ///
    /// The bypass table is a copy of the main table rather than a single
    /// default route. That matters: the main table is where an installed VPN
    /// keeps its routes, and a bypass table with one route through the
    /// physical gateway sends the daemon's traffic around the VPN as well.
    /// Losing the VPN means losing the very endpoints the daemon has to
    /// reach — the proxies it tests and the targets it verifies them against.
    ///
    /// Only routes through the TUN device itself are left out, which is the
    /// whole point: the daemon's own traffic never re-enters the device.
    pub async fn setup_fwmark_routing(
        gateway: &str,
        iface: &str,
        table: u32,
        mark: u32,
    ) -> Result<()> {
        // Clear anything left by a previous run: `ip rule add` happily
        // accumulates duplicates, and every restart would leave another one.
        // The delete removes one rule at a time, so it is repeated until it
        // stops finding any.
        loop {
            let out = tokio::process::Command::new("ip")
                .args([
                    "rule",
                    "del",
                    "fwmark",
                    &mark.to_string(),
                    "table",
                    &table.to_string(),
                ])
                .output()
                .await?;

            if !out.status.success() {
                break;
            }
        }

        // Copy the main table, minus the device this daemon owns.
        let routes = tokio::process::Command::new("sh")
            .args([
                "-c",
                "ip route show table main | grep -v ' dev ps-tun' || true",
            ])
            .output()
            .await
            .context("Failed to read the main routing table")?;

        let mut copied = 0usize;
        for line in String::from_utf8_lossy(&routes.stdout).lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // `ip route show` prints attributes that `ip route add` refuses.
            let mut args: Vec<String> = Vec::new();
            let mut skip_next = false;
            for token in line.split_whitespace() {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if token == "proto" {
                    skip_next = true;
                    continue;
                }
                args.push(token.to_string());
            }

            if args.is_empty() {
                continue;
            }

            let mut command = vec!["route".to_string(), "replace".to_string()];
            command.push("table".to_string());
            command.push(table.to_string());
            command.extend(args);

            let out = tokio::process::Command::new("ip")
                .args(&command)
                .output()
                .await?;

            if out.status.success() {
                copied += 1;
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stderr.contains("File exists") {
                    warn!("could not copy a route into the bypass table: {stderr}");
                }
            }
        }

        // A default route through the physical gateway is the fallback for
        // anything the main table does not cover.
        let out = tokio::process::Command::new("ip")
            .args([
                "route",
                "replace",
                "default",
                "via",
                gateway,
                "dev",
                iface,
                "table",
                &table.to_string(),
            ])
            .output()
            .await
            .context("Failed to add fwmark routing table")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("File exists") {
                warn!("fwmark table add: {stderr}");
            }
        }

        // Add rule to route marked packets through the custom table
        let out = tokio::process::Command::new("ip")
            .args([
                "rule",
                "add",
                "fwmark",
                &mark.to_string(),
                "table",
                &table.to_string(),
            ])
            .output()
            .await
            .context("Failed to add fwmark rule")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("File exists") {
                warn!("fwmark rule add: {stderr}");
            }
        }

        info!(
            "FWmark routing: mark {mark} -> table {table} ({copied} routes copied) via {gateway} dev {iface}"
        );
        Ok(())
    }

    /// Remove the fwmark routing rule and table.
    pub async fn cleanup_fwmark(table: u32, mark: u32) {
        let _ = tokio::process::Command::new("ip")
            .args([
                "rule",
                "del",
                "fwmark",
                &mark.to_string(),
                "table",
                &table.to_string(),
            ])
            .output()
            .await;
        let _ = tokio::process::Command::new("ip")
            .args(["route", "flush", "table", &table.to_string()])
            .output()
            .await;
    }

    /// Remove the exclude route for the proxy IP.
    pub async fn remove_exclude_route(&self, proxy_ip: &str) {
        if let Err(e) = std::process::Command::new("ip")
            .args(["route", "del", proxy_ip])
            .output()
        {
            warn!("Failed to remove exclude route: {e}");
        }
    }

    pub async fn cleanup(&self) {
        let mut guard = self.dev.lock().await;
        *guard = None;
        if let Err(e) = std::process::Command::new("ip")
            .args(["route", "del", "default", "dev", &self.config.name])
            .output()
        {
            warn!("Failed to remove route: {e}");
        }
        info!("TUN cleanup complete");
    }

    /// Take ownership of the TUN device (leaves None in the option).
    pub async fn take_device(&self) -> Option<tun::Device> {
        self.dev.lock().await.take()
    }
}

/// Enterprise-grade TUN forwarding loop.
///
/// - Reads all IPv4 packets from TUN
/// - Routes TCP through proxy (with retransmitted-SYN guard)
/// - Forwards UDP directly to destinations (non-proxied)
/// - Intercepts DNS responses for hostname caching
/// - Non-blocking response pumping for both TCP and UDP
/// - Periodic cleanup of stale UDP flows
pub async fn run_forwarding_loop(
    dev: tun::Device,
    router: Arc<Router>,
    mtu: usize,
    shutdown: Arc<AtomicBool>,
) {
    info!("Starting TUN forwarding loop (MTU={mtu})");
    let handle = tokio::runtime::Handle::current();

    let result = tokio::task::spawn_blocking(move || {
        use std::io::{ErrorKind, Read, Write};
        let mut buf = vec![0u8; mtu];
        let mut dev = dev;
        let mut packet_count = 0u64;
        let mut udp_cleanup_counter = 0u64;

        // Non-blocking reads are what make this loop stoppable. A blocking
        // `read` sits in the kernel until a packet arrives, so a blocking
        // task could never return and the runtime could never be dropped —
        // every restart ended in systemd SIGKILL after the 90s timeout.
        if let Err(e) = dev.set_nonblock() {
            warn!("TUN non-blocking mode unavailable ({e}); shutdown may hang");
        }

        loop {
            if shutdown.load(Ordering::SeqCst) {
                info!("TUN forwarding loop stopping on shutdown signal");
                break;
            }

            let n = match dev.read(&mut buf) {
                Ok(0) => {
                    warn!("TUN read returned 0 (EOF)");
                    break;
                }
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    // Nothing queued. Yield the blocking thread so shutdown
                    // stays responsive instead of spinning on the CPU.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    udp_cleanup_counter += 1;
                    continue;
                }
                Err(e) => {
                    warn!("TUN read error: {e}");
                    break;
                }
            };

            packet_count += 1;
            let data = &buf[..n];
            trace_packet(n, packet_count);

            match ParsedPacket::parse(data) {
                Ok(packet) => {
                    let protocol = packet.ip.protocol;
                    if protocol == 6 {
                        handle_tcp(&packet, &router, &mut dev, &handle);
                    } else if protocol == 17 {
                        handle_udp(&packet, &router, &handle);
                    }
                }
                Err(_e) => {
                    // Non-IPv4 packets (ARP, IPv6, etc.) are silently ignored
                }
            }

            // Non-blocking pump: TCP responses
            handle.block_on(async {
                let responses = router.pump_responses().await;
                for pkt in &responses {
                    let _ = dev.write_all(pkt);
                }
            });

            // Non-blocking pump: UDP responses + DNS caching
            handle.block_on(async {
                let responses = router.pump_udp().await;
                for pkt in &responses {
                    let _ = dev.write_all(pkt);
                }
            });

            // Periodic cleanup of stale UDP flows
            udp_cleanup_counter += 1;
            if udp_cleanup_counter.is_multiple_of(1000) {
                handle.block_on(async {
                    router.cleanup_udp().await;
                });
            }
        }

        info!("TUN forwarding loop exited after {packet_count} packets");
    })
    .await;

    if let Err(e) = result {
        warn!("TUN forwarding task failed: {e}");
    }
}

fn trace_packet(n: usize, packet_count: u64) {
    tracing::trace!("TUN read: {n} bytes (packet #{packet_count})");
}

fn handle_tcp(
    packet: &ParsedPacket,
    router: &Arc<Router>,
    dev: &mut tun::Device,
    handle: &tokio::runtime::Handle,
) {
    handle.block_on(async {
        if let Some(syn_ack) = router.handle_outgoing(packet).await {
            tracing::trace!("Writing SYN-ACK ({} bytes)", syn_ack.len());
            let _ = dev.write_all(&syn_ack);
        }
        router.handle_data(packet).await;
    });
}

fn handle_udp(packet: &ParsedPacket, router: &Arc<Router>, handle: &tokio::runtime::Handle) {
    handle.block_on(async {
        router.handle_udp(packet).await;
    });
}
