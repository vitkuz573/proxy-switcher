use crate::dns;
use crate::packet::ParsedPacket;
use crate::router::Router;
use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub struct TunManager {
    config: crate::config::TunConfig,
    dev: Arc<Mutex<Option<tun::Device>>>,
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
        }
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

        info!("TUN device {} created at {}", self.config.name, self.config.address);

        let mut guard = self.dev.lock().await;
        *guard = Some(dev);
        Ok(())
    }

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

    /// Take ownership of the TUN device (leaves None in the option)
    pub async fn take_device(&self) -> Option<tun::Device> {
        self.dev.lock().await.take()
    }
}

/// Standalone forwarding loop — reads IP packets from TUN, routes them,
/// pumps responses back. Runs in spawn_blocking with non-blocking response
/// pumping between each TUN read.
pub async fn run_forwarding_loop(
    dev: tun::Device,
    router: Arc<Router>,
    mtu: usize,
) {
    info!("Starting TUN forwarding loop (MTU={mtu})");
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut buf = vec![0u8; mtu];
        let mut dev = dev;

        let mut packet_count = 0u64;
        loop {
            let n = match dev.read(&mut buf) {
                Ok(0) => {
                    tracing::warn!("TUN read returned 0 (EOF)");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("TUN read error: {e}");
                    break;
                }
            };

            packet_count += 1;
            let data = &buf[..n];
            tracing::info!("TUN read: {n} bytes (packet #{packet_count})");

            match ParsedPacket::parse(data) {
                Ok(packet) => {
                    // DNS response interception (UDP, protocol 17)
                    if packet.ip.protocol == 17 {
                        if packet.payload.len() >= 8 {
                            let sport = u16::from_be_bytes([packet.payload[0], packet.payload[1]]);
                            let dport = u16::from_be_bytes([packet.payload[2], packet.payload[3]]);
                            if dport == 53 || sport == 53 {
                                let dns_payload = &packet.payload[8..];
                                let mappings = dns::parse_dns_response(dns_payload);
                                if !mappings.is_empty() {
                                    let dns_cache = router.dns_cache();
                                    handle.block_on(async {
                                        for (hostname, ip) in &mappings {
                                            tracing::info!("DNS cache: {ip} -> {hostname}");
                                            dns_cache.insert(*ip, hostname.clone()).await;
                                        }
                                    });
                                }
                            }
                        }
                        continue;
                    }

                    tracing::info!(
                        "Parsed: {}:{} -> {}:{} syn={} fin={} payload={}",
                        packet.ip.source, packet.tcp.as_ref().map(|t| t.source_port).unwrap_or(0),
                        packet.ip.destination, packet.tcp.as_ref().map(|t| t.destination_port).unwrap_or(0),
                        packet.is_tcp_syn(), packet.is_tcp_fin(),
                        packet.payload.len(),
                    );
                    handle.block_on(async {
                        if let Some(syn_ack) = router.handle_outgoing(&packet).await {
                            tracing::info!("Writing SYN-ACK ({} bytes)", syn_ack.len());
                            let _ = dev.write_all(&syn_ack);
                        }
                        router.handle_data(&packet).await;
                    });
                }
                Err(_e) => tracing::trace!("Parse error: {_e}"),
            }

            // Non-blocking pump: read whatever proxy data is available and
            // write response packets back to the TUN device.
            handle.block_on(async {
                let responses = router.pump_responses().await;
                if !responses.is_empty() {
                    tracing::info!("Pump: writing {} response packets to TUN", responses.len());
                    for pkt in &responses {
                        tracing::info!("Pump: write packet ({} bytes)", pkt.len());
                        let _ = dev.write_all(pkt);
                    }
                }
            });
        }
    })
    .await
    .expect("TUN loop panicked");

    info!("TUN forwarding loop ended");
}
