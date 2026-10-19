use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ProxyPool;
use crate::proxy::{Anonymity, ProxyProtocol};

/// How many proxies there are, and how many of them work.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PoolStatistics {
    pub pool_size: usize,
    /// Confirmed working.
    pub working: usize,
    /// Working and able to carry a tunnel.
    pub tunnel_capable: usize,
    /// Never confirmed to work.
    pub unverified: usize,
    /// Confirmed working, but too slow to be worth selecting.
    pub below_threshold: usize,
    /// Proxies waiting for their next probe.
    pub due: usize,
    /// The distribution by protocol.
    pub by_protocol: Vec<ProtocolCount>,
    /// The distribution by what the proxy reveals about the client.
    pub by_anonymity: Vec<AnonymityCount>,
    /// The distribution by country, when the list said.
    pub by_country: Vec<CountryCount>,
    /// The distribution of measured latency.
    pub latency: Vec<LatencyBucket>,
    /// The exit addresses the proxies were seen to use.
    ///
    /// Many entries in a list are the same endpoint published under many
    /// names, and this is where that shows.
    pub distinct_exit_addresses: usize,
}

/// A protocol and how many proxies use it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProtocolCount {
    pub protocol: ProxyProtocol,
    pub count: usize,
    pub working: usize,
}

/// An anonymity class and how many proxies fall into it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnonymityCount {
    pub anonymity: Anonymity,
    pub count: usize,
    pub working: usize,
}

/// A country and how many proxies come from it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CountryCount {
    pub country: String,
    pub count: usize,
    pub working: usize,
}

/// A latency range and how many proxies fall into it.
///
/// The boundaries are numbers rather than a label: the engine does not know
/// what language the reader has, and a label baked into the answer would be
/// shown in the wrong one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatencyBucket {
    /// The lower bound, in milliseconds.
    pub from_ms: u64,
    /// The upper bound, in milliseconds. `None` for the open-ended range.
    pub to_ms: Option<u64>,
    pub count: usize,
}

/// One sample of the pool over time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistorySample {
    /// When the sample was taken.
    pub at: chrono::DateTime<chrono::Utc>,
    pub pool_size: usize,
    pub working: usize,
    pub due: usize,
}

/// The latency ranges the distribution is counted in.
const LATENCY_BUCKETS: &[(u64, Option<u64>)] = &[
    (0, Some(100)),
    (100, Some(300)),
    (300, Some(600)),
    (600, Some(1000)),
    (1000, Some(2000)),
    (2000, Some(5000)),
    (5000, None),
];

impl ProxyPool {
    /// The distribution of the pool.
    ///
    /// Counted in one pass under a read lock, so it is a consistent snapshot
    /// rather than several that disagree with each other.
    pub async fn statistics(
        &self,
        min_score: f64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PoolStatistics {
        let inner = self.inner.read().await;

        let mut protocols: HashMap<ProxyProtocol, (usize, usize)> = HashMap::new();
        let mut anonymities: HashMap<Anonymity, (usize, usize)> = HashMap::new();
        let mut countries: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut latency = vec![0usize; LATENCY_BUCKETS.len()];
        let mut exit_addresses: HashMap<String, ()> = HashMap::new();

        let mut working = 0usize;
        let mut tunnel_capable = 0usize;
        let mut unverified = 0usize;
        let mut below_threshold = 0usize;
        let mut due = 0usize;

        for proxy in &inner.proxies {
            let is_working = proxy.last_success.is_some() && proxy.consecutive_failures == 0;

            if is_working {
                working += 1;
                if proxy.supports_tunnel && proxy.score >= min_score {
                    tunnel_capable += 1;
                } else {
                    below_threshold += 1;
                }
            } else {
                unverified += 1;
            }

            if proxy.next_probe_at <= now {
                due += 1;
            }

            let entry = protocols.entry(proxy.protocol).or_insert((0, 0));
            entry.0 += 1;
            if is_working {
                entry.1 += 1;
            }

            let entry = anonymities.entry(proxy.anonymity).or_insert((0, 0));
            entry.0 += 1;
            if is_working {
                entry.1 += 1;
            }

            if let Some(country) = proxy.country.as_deref() {
                let entry = countries.entry(country).or_insert((0, 0));
                entry.0 += 1;
                if is_working {
                    entry.1 += 1;
                }
            }

            if let Some(latency_ms) = proxy.latency_ms {
                let index = LATENCY_BUCKETS
                    .iter()
                    .position(|(from, to)| {
                        latency_ms >= *from && to.is_none_or(|upper| latency_ms < upper)
                    })
                    .unwrap_or(LATENCY_BUCKETS.len() - 1);
                latency[index] += 1;
            }

            if let Some(exit) = proxy.exit_ip.as_ref() {
                exit_addresses.insert(exit.to_string(), ());
            }
        }

        let mut by_protocol: Vec<ProtocolCount> = protocols
            .into_iter()
            .map(|(protocol, (count, working))| ProtocolCount {
                protocol,
                count,
                working,
            })
            .collect();
        by_protocol.sort_by_key(|p| std::cmp::Reverse(p.count));

        let mut by_anonymity: Vec<AnonymityCount> = anonymities
            .into_iter()
            .map(|(anonymity, (count, working))| AnonymityCount {
                anonymity,
                count,
                working,
            })
            .collect();
        by_anonymity.sort_by_key(|a| std::cmp::Reverse(a.count));

        // Only the countries worth a row: a list of two hundred thousand has
        // hundreds, and the tail is noise.
        let mut by_country: Vec<CountryCount> = countries
            .into_iter()
            .map(|(country, (count, working))| CountryCount {
                country: country.to_string(),
                count,
                working,
            })
            .collect();
        by_country.sort_by_key(|c| std::cmp::Reverse(c.count));
        by_country.truncate(20);

        PoolStatistics {
            pool_size: inner.proxies.len(),
            working,
            tunnel_capable,
            unverified,
            below_threshold,
            due,
            by_protocol,
            by_anonymity,
            by_country,
            latency: LATENCY_BUCKETS
                .iter()
                .zip(latency)
                .map(|((from, to), count)| LatencyBucket {
                    from_ms: *from,
                    to_ms: *to,
                    count,
                })
                .collect(),
            distinct_exit_addresses: exit_addresses.len(),
        }
    }
}
