use crate::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use anyhow::Result;
use std::collections::HashSet;
use tracing::info;

pub struct Scraper {
    client: reqwest::Client,
    sources: Vec<(String, ParseStrategy)>,
}

#[derive(Clone, Copy)]
enum ParseStrategy {
    Table {
        table: &'static str,
        col_ip: usize,
        col_port: usize,
        col_country: usize,
        col_proto: usize,
        col_anon: usize,
    },
    PlainText { proto: ProxyProtocol },
}

const BUILTIN_SOURCES: &[(&str, ParseStrategy)] = &[
    ("https://free-proxy-list.net", ParseStrategy::Table { table: "table.table", col_ip: 0, col_port: 1, col_country: 3, col_proto: 4, col_anon: 5 }),
    ("https://www.sslproxies.org", ParseStrategy::Table { table: "table.table", col_ip: 0, col_port: 1, col_country: 3, col_proto: 4, col_anon: 5 }),
    ("https://www.us-proxy.org", ParseStrategy::Table { table: "table.table", col_ip: 0, col_port: 1, col_country: 3, col_proto: 4, col_anon: 5 }),
    ("https://www.proxy-list.download/api/v1/get?type=http", ParseStrategy::PlainText { proto: ProxyProtocol::Http }),
    ("https://www.proxy-list.download/api/v1/get?type=https", ParseStrategy::PlainText { proto: ProxyProtocol::Https }),
    ("https://www.proxy-list.download/api/v1/get?type=socks4", ParseStrategy::PlainText { proto: ProxyProtocol::Socks4 }),
    ("https://www.proxy-list.download/api/v1/get?type=socks5", ParseStrategy::PlainText { proto: ProxyProtocol::Socks5 }),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/socks5.txt", ParseStrategy::PlainText { proto: ProxyProtocol::Socks5 }),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/socks4.txt", ParseStrategy::PlainText { proto: ProxyProtocol::Socks4 }),
    ("https://raw.githubusercontent.com/TheSpeedX/SOCKS-List/master/http.txt", ParseStrategy::PlainText { proto: ProxyProtocol::Http }),
    ("https://raw.githubusercontent.com/clarketm/proxy-list/master/proxy-list-raw.txt", ParseStrategy::PlainText { proto: ProxyProtocol::Http }),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=http&timeout=10000&country=all", ParseStrategy::PlainText { proto: ProxyProtocol::Http }),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks5&timeout=10000&country=all", ParseStrategy::PlainText { proto: ProxyProtocol::Socks5 }),
    ("https://api.proxyscrape.com/v2/?request=getproxies&protocol=socks4&timeout=10000&country=all", ParseStrategy::PlainText { proto: ProxyProtocol::Socks4 }),
];

impl Scraper {
    pub fn new(sources: Vec<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
            .build()
            .expect("Failed to create HTTP client");

        let sources: Vec<(String, ParseStrategy)> = if sources.is_empty() {
            BUILTIN_SOURCES.iter().map(|(url, s)| (url.to_string(), *s)).collect()
        } else {
            sources.into_iter().map(|url| {
                let strategy = BUILTIN_SOURCES.iter().find(|(u, _)| *u == url).map(|(_, s)| *s);
                (url, strategy.unwrap_or(ParseStrategy::PlainText { proto: ProxyProtocol::Http }))
            }).collect()
        };

        Self { client, sources }
    }

    pub async fn scrape_all(&self) -> Result<Vec<ProxyInfo>> {
        let mut all = Vec::new();
        let mut seen = HashSet::new();

        for (source, strategy) in &self.sources {
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

    async fn scrape_source(&self, url: &str, strategy: ParseStrategy) -> Result<Vec<ProxyInfo>> {
        match strategy {
            ParseStrategy::Table { .. } => {
                self.scrape_table(url, strategy).await
            }
            ParseStrategy::PlainText { proto } => {
                self.scrape_plaintext(url, proto).await
            }
        }
    }

    async fn scrape_table(
        &self,
        url: &str,
        strategy: ParseStrategy,
    ) -> Result<Vec<ProxyInfo>> {
        let (table_sel, col_ip, col_port, col_country, col_proto, col_anon) = match strategy {
            ParseStrategy::Table { table, col_ip, col_port, col_country, col_proto, col_anon } => {
                (table, col_ip, col_port, col_country, col_proto, col_anon)
            }
            _ => return Ok(Vec::new()),
        };

        let html = self.client.get(url).send().await?.text().await?;
        let doc = scraper::Html::parse_document(&html);

        let table_sel = scraper::Selector::parse(table_sel)
            .map_err(|e| anyhow::anyhow!("Selector error: {e}"))?;
        let row_sel = scraper::Selector::parse("tbody tr")
            .map_err(|e| anyhow::anyhow!("Selector error: {e}"))?;
        let td_sel = scraper::Selector::parse("td")
            .map_err(|e| anyhow::anyhow!("Selector error: {e}"))?;

        let mut proxies = Vec::new();

        if let Some(table) = doc.select(&table_sel).next() {
            for row in table.select(&row_sel) {
                let cells: Vec<String> = row
                    .select(&td_sel)
                    .map(|c| c.text().collect::<String>().trim().to_string())
                    .collect();

                if cells.len() <= col_port {
                    continue;
                }

                let host = cells[col_ip].trim().to_string();
                if host.is_empty() {
                    continue;
                }

                let port: u16 = match cells[col_port].trim().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                let protocol = if cells.len() > col_proto {
                    match cells[col_proto].to_lowercase().as_str() {
                        "socks4" | "socks 4" => ProxyProtocol::Socks4,
                        "socks5" | "socks 5" => ProxyProtocol::Socks5,
                        "https" => ProxyProtocol::Https,
                        _ => ProxyProtocol::Http,
                    }
                } else {
                    ProxyProtocol::Http
                };

                let anonymity = if cells.len() > col_anon {
                    match cells[col_anon].to_lowercase().as_str() {
                        "elite proxy" | "elite" => Anonymity::Elite,
                        "anonymous" => Anonymity::Anonymous,
                        _ => Anonymity::Transparent,
                    }
                } else {
                    Anonymity::Unknown
                };

                proxies.push(ProxyInfo {
                    id: format!("{host}:{port}"),
                    host,
                    port,
                    protocol,
                    anonymity,
                    latency_ms: None,
                    country: cells.get(col_country).cloned(),
                    last_checked: None,
                    score: 0.0,
                });
            }
        }

        Ok(proxies)
    }

    async fn scrape_plaintext(&self, url: &str, proto: ProxyProtocol) -> Result<Vec<ProxyInfo>> {
        let text = self.client.get(url).send().await?.text().await?;
        let mut proxies = Vec::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }

            if let Some((host, port_str)) = line.split_once(':') {
                let host = host.trim();
                let port: u16 = match port_str.trim().parse() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if host.is_empty() {
                    continue;
                }

                proxies.push(ProxyInfo {
                    id: format!("{host}:{port}"),
                    host: host.to_string(),
                    port,
                    protocol: proto,
                    anonymity: Anonymity::Unknown,
                    latency_ms: None,
                    country: None,
                    last_checked: None,
                    score: 0.0,
                });
            }
        }

        Ok(proxies)
    }
}
