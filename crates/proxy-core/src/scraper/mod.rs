pub mod parse;

use crate::proxy::{ProxyInfo, ProxyProtocol};
use anyhow::Result;
use parse::Format;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

pub struct Scraper {
    client: reqwest::Client,
    sources: Arc<RwLock<Vec<String>>>,
}

/// What a source's format does not say about the proxies in it.
#[derive(Clone, Copy)]
enum ParseStrategy {
    /// The list is about one protocol, whatever shape it has.
    Protocol(ProxyProtocol),
    /// The list names the protocol of each entry.
    Detect,
}

const BUILTIN_SOURCES: &[(&str, ParseStrategy)] = &[
    ("https://free-proxy-list.net", ParseStrategy::Detect),
    ("https://www.sslproxies.org", ParseStrategy::Detect),
    ("https://www.us-proxy.org", ParseStrategy::Detect),
    ("https://www.proxy-list.download/api/v1/get?type=http", ParseStrategy::Protocol(ProxyProtocol::Http)),
    ("https://www.proxy-list.download/api/v1/get?type=https", ParseStrategy::Protocol(ProxyProtocol::Https)),
    ("https://www.proxy-list.download/api/v1/get?type=socks4", ParseStrategy::Protocol(ProxyProtocol::Socks4)),
    ("https://www.proxy-list.download/api/v1/get?type=socks5", ParseStrategy::Protocol(ProxyProtocol::Socks5)),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/socks5.txt", ParseStrategy::Protocol(ProxyProtocol::Socks5)),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/socks4.txt", ParseStrategy::Protocol(ProxyProtocol::Socks4)),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/http.txt", ParseStrategy::Protocol(ProxyProtocol::Http)),
    ("https://raw.githubusercontent.com/clarketm/proxy-list/master/proxy-list-raw.txt", ParseStrategy::Protocol(ProxyProtocol::Http)),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=http&timeout=10000&country=all", ParseStrategy::Protocol(ProxyProtocol::Http)),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks5&timeout=10000&country=all", ParseStrategy::Protocol(ProxyProtocol::Socks5)),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks4&timeout=10000&country=all", ParseStrategy::Protocol(ProxyProtocol::Socks4)),
];

fn resolve_strategy(url: &str) -> ParseStrategy {
    BUILTIN_SOURCES
        .iter()
        .find(|(u, _)| *u == url)
        .map(|(_, s)| *s)
        .unwrap_or(ParseStrategy::Detect)
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

fn builtin_urls() -> Vec<String> {
    BUILTIN_SOURCES.iter().map(|(u, _)| u.to_string()).collect()
}

impl Scraper {
    pub fn new(sources: Arc<RwLock<Vec<String>>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
            .build()
            .expect("Failed to create HTTP client");

        Self { client, sources }
    }

    pub async fn scrape_all(&self) -> Result<Vec<ProxyInfo>> {
        let urls = self.sources.read().await;
        let sources: Vec<(String, ParseStrategy)> = if urls.is_empty() {
            builtin_urls()
                .into_iter()
                .map(|u| {
                    let s = resolve_strategy(&u);
                    (u, s)
                })
                .collect()
        } else {
            urls.iter()
                .map(|u| {
                    let s = resolve_strategy(u);
                    (u.clone(), s)
                })
                .collect()
        };
        drop(urls);

        let mut all = Vec::new();
        let mut seen = HashSet::new();

        for (source, strategy) in &sources {
            match self.scrape_source(source, *strategy).await {
                Ok(proxies) => {
                    info!("Scraped {} proxies from {}", proxies.len(), source);
                    for p in proxies {
                        if seen.insert(p.id.clone()) {
                            all.push(p);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to scrape {source}: {e}");
                }
            }
        }

        info!("Total unique proxies collected: {}", all.len());
        Ok(all)
    }

    /// Fetch a source and parse it by its shape.
    ///
    /// The format is detected from the body rather than configured per source,
    /// so a list that changes its layout keeps working and a new source needs
    /// no entry anywhere.
    async fn scrape_source(&self, url: &str, strategy: ParseStrategy) -> Result<Vec<ProxyInfo>> {
        let hint = protocol_hint(url, strategy);

        let response = self.client.get(url).send().await?;
        let body = response.text().await?;

        let mut proxies = parse::parse(&body, hint);

        // A list that needs a table and did not get one may still be readable
        // as plain text: several sites serve a table to a browser and a plain
        // list to everything else.
        if proxies.is_empty() && parse::detect(&body) == Format::Html {
            proxies = parse::parse_plain_text(&body, hint);
        }

        Ok(proxies)
    }
}
