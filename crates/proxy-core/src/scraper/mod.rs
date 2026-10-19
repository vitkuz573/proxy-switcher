pub mod links;
pub mod parse;

use crate::proxy::{ProxyInfo, ProxyProtocol};
use anyhow::Result;
use parse::Format;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub struct Scraper {
    client: reqwest::Client,
    sources: Arc<RwLock<Vec<SourceSpec>>>,
}

/// Where to look for proxies, and what the list is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub url: String,
    /// The protocol the list is about, when the list does not say.
    pub protocol: Option<ProxyProtocol>,
}

impl SourceSpec {
    pub fn new(url: impl Into<String>, protocol: Option<ProxyProtocol>) -> Self {
        Self {
            url: url.into(),
            protocol,
        }
    }
}

/// What a source's format does not say about the proxies in it.
#[derive(Clone, Copy)]
enum ParseStrategy {
    /// The list is about one protocol, whatever shape it has.
    Protocol(ProxyProtocol),
    /// The list names the protocol of each entry.
    Detect,
}

/// What a source's format does not say about the proxies in it.
///
/// The protocol is taken from the address, because that is where the lists put
/// it: a file called `socks5.txt` holds SOCKS5 proxies.
fn resolve_strategy(spec: &SourceSpec) -> ParseStrategy {
    if let Some(protocol) = spec.protocol {
        return ParseStrategy::Protocol(protocol);
    }

    let lower = spec.url.to_ascii_lowercase();

    if lower.contains("socks5") {
        ParseStrategy::Protocol(ProxyProtocol::Socks5)
    } else if lower.contains("socks4") {
        ParseStrategy::Protocol(ProxyProtocol::Socks4)
    } else if lower.contains("https") {
        ParseStrategy::Protocol(ProxyProtocol::Https)
    } else {
        ParseStrategy::Detect
    }
}

/// The protocol a source is about, when the format does not say.
fn protocol_hint(url: &str, strategy: ParseStrategy) -> parse::ProtocolHint {
    let from_strategy = match strategy {
        ParseStrategy::Protocol(proto) => Some(proto),
        ParseStrategy::Detect => None,
    };

    if let Some(protocol) = from_strategy {
        return parse::ProtocolHint { protocol };
    }

    let lower = url.to_ascii_lowercase();
    let protocol = if lower.contains("socks5") {
        ProxyProtocol::Socks5
    } else if lower.contains("socks4") {
        ProxyProtocol::Socks4
    } else if lower.contains("https") {
        ProxyProtocol::Https
    } else {
        ProxyProtocol::Http
    };

    parse::ProtocolHint { protocol }
}

impl Scraper {
    pub fn new(sources: Arc<RwLock<Vec<SourceSpec>>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
            .build()
            .expect("Failed to create HTTP client");

        Self { client, sources }
    }

    /// Scrape every source into the pool.
    ///
    /// The results go straight into the pool rather than through a list of
    /// their own. A list of two hundred thousand proxies is two hundred
    /// thousand records held twice over — once in the list, once in the pool —
    /// and a set of identifiers to deduplicate them. The pool already has an
    /// index, so it does that work itself, and the list is never built.
    ///
    /// The sources are scraped in parallel, and that is not a matter of speed
    /// alone. In a network where some hosts are blocked or intercepted, a
    /// source that never answers costs its whole timeout. Fetched one after
    /// another, thirty such sources would hold the cycle for minutes, and the
    /// health check would wait for all of them. Fetched together, they cost
    /// one timeout in total and the working sources arrive as they answer.
    ///
    /// One source failing does not stop the others: the whole point of having
    /// many sources is that most of them still answer.
    pub async fn scrape_into(&self, pool: &crate::pool::ProxyPool) -> Result<usize> {
        let specs = self.sources.read().await.clone();
        let sources: Vec<(String, ParseStrategy)> = specs
            .iter()
            .map(|spec| (spec.url.clone(), resolve_strategy(spec)))
            .collect();

        let mut added = 0usize;
        let mut failed: Vec<String> = Vec::new();

        // A bound on the number of requests in flight, so a long list does not
        // open a hundred sockets at once.
        let permits = Arc::new(tokio::sync::Semaphore::new(16));

        let mut tasks = Vec::with_capacity(sources.len());
        for (source, strategy) in sources {
            let client = self.client.clone();
            let permits = permits.clone();
            let source_for_task = source.clone();

            tasks.push(tokio::spawn(async move {
                let _permit = permits.acquire_owned().await.expect("the semaphore");
                let result = fetch_and_parse(&client, &source, strategy).await;
                (source_for_task, result)
            }));
        }

        for task in tasks {
            let (source, result) = match task.await {
                Ok(outcome) => outcome,
                Err(e) => {
                    failed.push(format!("a scrape task failed: {e}"));
                    continue;
                }
            };

            match result {
                Ok(proxies) => {
                    info!("Scraped {} proxies from {}", proxies.len(), source);
                    // Merged as they arrive, so the pool is never behind the
                    // scrape and nothing is held twice.
                    added += pool.merge(proxies).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to scrape {source}: {e}");
                    failed.push(source);
                }
            }
        }

        info!(added, failed = failed.len(), "Scrape cycle complete");

        Ok(added)
    }
}

/// Fetch a source and parse it by its shape.
///
/// The format is detected from the body rather than configured per source, so a
/// list that changes its layout keeps working and a new source needs no entry
/// anywhere.
async fn fetch_and_parse(
    client: &reqwest::Client,
    url: &str,
    strategy: ParseStrategy,
) -> Result<Vec<ProxyInfo>> {
    let hint = protocol_hint(url, strategy);

    let response = client.get(url).send().await?;
    let body = response.text().await?;

    let mut proxies = parse::parse(&body, hint);

    // A list that needs a table and did not get one may still be readable as
    // plain text: several sites serve a table to a browser and a plain list to
    // everything else.
    if proxies.is_empty() && parse::detect(&body) == Format::Html {
        proxies = parse::parse_plain_text(&body, hint);
    }

    Ok(proxies)
}
