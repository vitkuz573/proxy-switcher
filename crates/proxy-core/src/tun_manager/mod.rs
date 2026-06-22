use crate::packet::ParsedPacket;
use crate::router::Router;
use anyhow::{Context, Result};
use std::io::Read;
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
/// pumps responses back. Designed to run in a spawned task.
///
/// TUN devices use blocking I/O, so this runs the I/O portion in a
/// spawn_blocking task and bridges to async routing via Handle::block_on.
pub async fn run_forwarding_loop(
    dev: tun::Device,
    router: Arc<Router>,
    mtu: usize,
) {
    info!("Starting TUN forwarding loop (MTU={mtu})");
    let handle = tokio::runtime::Handle::current();

    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut buf = vec![0u8; mtu];
        let mut dev = dev;

        loop {
            let n = match dev.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("TUN read error: {e}");
                    break;
                }
            };

            let data = &buf[..n];
            match ParsedPacket::parse(data) {
                Ok(packet) => {
                    handle.block_on(async {
                        if let Some(syn_ack) = router.handle_outgoing(&packet).await {
                            let _ = dev.write_all(&syn_ack);
                        }
                        router.handle_data(&packet).await;
                    });
                }
                Err(e) => tracing::trace!("Parse error: {e}"),
            }

            handle.block_on(async {
                let responses = router.pump_responses().await;
                for pkt in &responses {
                    let _ = dev.write_all(pkt);
                }
            });
        }
    })
    .await
    .expect("TUN loop panicked");

    info!("TUN forwarding loop ended");
}
