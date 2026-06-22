pub mod connection;

use crate::forwarder::Forwarder;
use crate::packet::build_response_packet;
use crate::packet::ParsedPacket;
use crate::pool::ProxyPool;
use connection::{ConnectionTracker, FlowKey};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

pub struct Router {
    pub tracker: ConnectionTracker,
    pool: Arc<ProxyPool>,
}

impl Router {
    pub fn new(pool: Arc<ProxyPool>) -> Self {
        Self { tracker: ConnectionTracker::new(), pool }
    }

    pub async fn handle_outgoing(&self, packet: &ParsedPacket) {
        if !packet.is_tcp_syn() {
            return;
        }
        let ip = &packet.ip;
        let tcp = packet.tcp.as_ref().unwrap();

        let key = FlowKey {
            src_ip: ip.source,
            src_port: tcp.source_port,
            dst_ip: ip.destination,
            dst_port: tcp.destination_port,
        };

        let proxy = match self.pool.active().await {
            Some(p) => p,
            None => {
                warn!("No active proxy");
                return;
            }
        };

        debug!(
            "New conn: {}:{} -> {}:{}",
            key.src_ip, key.src_port, key.dst_ip, key.dst_port
        );

        match Forwarder::connect_to(&proxy, &key.dst_ip.to_string(), key.dst_port).await {
            Ok(conn) => self.tracker.insert(key, conn).await,
            Err(e) => warn!("Proxy connect failed: {e}"),
        }
    }

    pub async fn handle_data(&self, packet: &ParsedPacket) {
        let ip = &packet.ip;
        let tcp = match &packet.tcp {
            Some(t) => t,
            None => return,
        };

        let key = FlowKey {
            src_ip: ip.source,
            src_port: tcp.source_port,
            dst_ip: ip.destination,
            dst_port: tcp.destination_port,
        };

        if let Some(conn) = self.tracker.get(&key).await {
            if !packet.payload.is_empty() {
                let mut guard = conn.write().await;
                if let Err(e) = guard.write_all(&packet.payload).await {
                    debug!("Write error, closing: {e}");
                    self.tracker.remove(&key).await;
                }
            }
        }

        if packet.is_tcp_fin() {
            self.tracker.remove(&key).await;
        }
    }

    /// Read data from all proxy connections and build response packets
    pub async fn pump_responses(&self) -> Vec<Vec<u8>> {
        let mut responses = Vec::new();
        for key in self.tracker.keys().await {
            let conn = match self.tracker.get(&key).await {
                Some(c) => c,
                None => continue,
            };
            let mut guard = conn.write().await;
            let mut buf = vec![0u8; 65536];
            match guard.read(&mut buf).await {
                Ok(0) => {
                    drop(guard);
                    self.tracker.remove(&key).await;
                }
                Ok(n) => {
                    buf.truncate(n);
                    let fake_packet = ParsedPacket {
                        ip: crate::packet::ip::IpHeader {
                            version: 4,
                            ihl: 20,
                            total_length: 0,
                            identification: 0,
                            flags: 0,
                            fragment_offset: 0,
                            ttl: 0,
                            protocol: 6,
                            checksum: 0,
                            source: key.dst_ip,
                            destination: key.src_ip,
                        },
                        tcp: Some(crate::packet::tcp::TcpHeader {
                            source_port: key.dst_port,
                            destination_port: key.src_port,
                            sequence_number: 0,
                            acknowledgment_number: 0,
                            data_offset: 20,
                            flags: crate::packet::tcp::TcpFlags {
                                ack: true,
                                psh: true,
                                ..Default::default()
                            },
                            window_size: 65535,
                            checksum: 0,
                            urgent_pointer: 0,
                        }),
                        payload: buf.clone(),
                    };
                    let ip_pkt = build_response_packet(&fake_packet, &buf);
                    responses.push(ip_pkt);
                }
                Err(_) => {
                    drop(guard);
                    self.tracker.remove(&key).await;
                }
            }
        }
        responses
    }
}
