use crate::proxy::{Anonymity, ProxyInfo, ProxyProtocol};

/// The shape of a scraped list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// An HTML page with a table of proxies.
    Html,
    /// One proxy per line, as `address:port`.
    PlainText,
    /// Comma separated values.
    Csv,
    /// A JSON document.
    Json,
    /// A list of proxy links.
    Links,
}

/// What a source says about the protocol, when the format itself does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolHint {
    pub protocol: ProxyProtocol,
}

impl Default for ProtocolHint {
    fn default() -> Self {
        Self {
            protocol: ProxyProtocol::Http,
        }
    }
}

/// Guess the format of a response body.
///
/// The list formats in the wild are not consistent, and a source that changes
/// its layout should keep working. The detection is by shape rather than by
/// configuration, so a new source needs no entry anywhere.
pub fn detect(body: &str) -> Format {
    let trimmed = body.trim_start();

    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Format::Json;
    }

    // A list of links: every line starts with a scheme.
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if !lines.is_empty()
        && lines.iter().all(|l| {
            l.contains("://") && l.trim_start().starts_with("trojan")
                || l.trim_start().starts_with("ss://")
                || l.trim_start().starts_with("socks5://")
                || l.trim_start().starts_with("socks4://")
        })
    {
        return Format::Links;
    }

    let lower = body.to_ascii_lowercase();
    if lower.contains("<table") || lower.contains("<html") || lower.contains("<tbody") {
        return Format::Html;
    }

    // A comma on the first line means the list is separated by commas.
    if let Some(first) = body.lines().find(|l| !l.trim().is_empty()) {
        if first.contains(',') && first.split(',').count() >= 2 {
            return Format::Csv;
        }
    }

    Format::PlainText
}

/// Parse a body into proxies, whatever the format.
pub fn parse(body: &str, hint: ProtocolHint) -> Vec<ProxyInfo> {
    match detect(body) {
        Format::Links => crate::scraper::links::parse_links(body),
        Format::Json => parse_json(body, hint),
        Format::Csv => parse_csv(body, hint),
        Format::PlainText => parse_plain_text(body, hint),
        Format::Html => parse_html(body, hint),
    }
}

/// Parse a plain list of `address:port` lines.
pub fn parse_plain_text(body: &str, hint: ProtocolHint) -> Vec<ProxyInfo> {
    let mut proxies = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Some lists write "address:port protocol" on one line.
        let mut parts = line.split_whitespace();
        let Some(address) = parts.next() else {
            continue;
        };
        let protocol = parts
            .next()
            .and_then(parse_protocol)
            .unwrap_or(hint.protocol);

        if let Some((host, port)) = split_address(address) {
            if let Some(proxy) = build(&host, port, protocol, None) {
                proxies.push(proxy);
            }
        }
    }

    proxies
}

/// Parse comma separated values: `address,port[,protocol,...]`.
pub fn parse_csv(body: &str, hint: ProtocolHint) -> Vec<ProxyInfo> {
    let mut proxies = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields: Vec<&str> = line.split(',').map(|f| f.trim()).collect();
        if fields.len() < 2 {
            continue;
        }

        // A header row is not a proxy.
        let Ok(port) = fields[1].parse::<u16>() else {
            continue;
        };

        let protocol = fields
            .iter()
            .skip(2)
            .find_map(|f| parse_protocol(f))
            .unwrap_or(hint.protocol);

        // The port is its own field here, and the host is on its own.
        let host = fields[0].trim();
        if host.is_empty() || host.contains(' ') {
            continue;
        }

        // The country is usually the last field.
        let country = fields
            .iter()
            .skip(2)
            .find(|f| !f.is_empty() && parse_protocol(f).is_none())
            .map(|f| f.to_string());

        if let Some(proxy) = build(host, port, protocol, country) {
            proxies.push(proxy);
        }
    }

    proxies
}

/// Parse a JSON document.
///
/// The shapes differ per API, so rather than a fixed set of field names the
/// document is searched for objects that carry an address and a port. The
/// field names in use are `ip`, `host`, `address` and `addr` for the address
/// and `port` for the port.
pub fn parse_json(body: &str, hint: ProtocolHint) -> Vec<ProxyInfo> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let mut proxies = Vec::new();
    collect_json(&value, hint, &mut proxies);
    proxies
}

/// Walk a JSON value, collecting anything that looks like a proxy.
fn collect_json(value: &serde_json::Value, hint: ProtocolHint, out: &mut Vec<ProxyInfo>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(proxy) = proxy_from_object(map, hint) {
                out.push(proxy);
                return;
            }
            for nested in map.values() {
                collect_json(nested, hint, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json(item, hint, out);
            }
        }
        _ => {}
    }
}

/// Build a proxy from a JSON object, if it has an address and a port.
fn proxy_from_object(
    map: &serde_json::Map<String, serde_json::Value>,
    hint: ProtocolHint,
) -> Option<ProxyInfo> {
    let address = ["ip", "host", "address", "addr", "ip_address"]
        .iter()
        .find_map(|key| map.get(*key).and_then(|v| v.as_str()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?;

    // The port may be its own field or part of the address.
    let declared_port = map
        .get("port")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .and_then(|p| u16::try_from(p).ok());

    let protocol = map
        .get("protocol")
        .or_else(|| map.get("type"))
        .or_else(|| map.get("scheme"))
        .and_then(|v| v.as_str())
        .and_then(parse_protocol)
        .unwrap_or(hint.protocol);

    let country = map
        .get("country")
        .or_else(|| map.get("country_code"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // The address may be "host" with the port in its own field, or
    // "host:port" in one.
    match (split_address(address), declared_port) {
        (Some((host, port)), _) => build(&host, port, protocol, country),
        (None, Some(port)) => build(address, port, protocol, country),
        (None, None) => None,
    }
}

/// Split an address into a host and a port.
fn split_address(address: &str) -> Option<(String, u16)> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }

    // "host:port", and nothing with a scheme or a path.
    let (host, port) = address.rsplit_once(':')?;
    let port: u16 = port.trim().parse().ok()?;

    Some((host.trim().to_string(), port))
}

/// Build a proxy from a host, a port and a protocol.
fn build(
    host: &str,
    port: u16,
    protocol: ProxyProtocol,
    country: Option<String>,
) -> Option<ProxyInfo> {
    let host = host.trim();
    if host.is_empty() || host.contains('/') || host.contains(' ') {
        return None;
    }
    // The host has to be an address or a name, not a sentence.
    if host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':' || c == '_')
    {
        return None;
    }

    Some(ProxyInfo {
        id: format!("{host}:{port}"),
        host: host.to_string(),
        port,
        protocol,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: None,
        country_reported: country.map(Into::into),
        last_checked: None,
        last_success: None,
        first_seen: chrono::Utc::now(),
        next_probe_at: chrono::Utc::now(),
        supports_tunnel: true,
        exit_ip: None,
        password: None,
        consecutive_failures: 0,
        score: 0.0,
    })
}

/// Parse a protocol name.
pub fn parse_protocol(name: &str) -> Option<ProxyProtocol> {
    match name.trim().to_ascii_lowercase().as_str() {
        "http" | "https://" => Some(ProxyProtocol::Http),
        "https" | "tls" | "ssl" => Some(ProxyProtocol::Https),
        "socks4" | "socks 4" | "socks4a" => Some(ProxyProtocol::Socks4),
        "socks5" | "socks 5" | "socks" => Some(ProxyProtocol::Socks5),
        _ => None,
    }
}

/// Parse an HTML table of proxies.
///
/// The column layout differs per site, so the columns are found by their
/// content rather than by their position: the address is the cell that parses
/// as an address, the port is the cell that parses as a port.
pub fn parse_html(body: &str, hint: ProtocolHint) -> Vec<ProxyInfo> {
    let document = scraper::Html::parse_document(body);

    let Ok(row_selector) = scraper::Selector::parse("tr") else {
        return Vec::new();
    };
    let Ok(cell_selector) = scraper::Selector::parse("td") else {
        return Vec::new();
    };

    let mut proxies = Vec::new();

    for row in document.select(&row_selector) {
        let cells: Vec<String> = row
            .select(&cell_selector)
            .map(|c| c.text().collect::<String>().trim().to_string())
            .collect();

        if cells.is_empty() {
            continue;
        }

        // Find the address and the port among the cells.
        let mut address: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut protocol = hint.protocol;
        let mut country: Option<String> = None;
        let mut anonymity = Anonymity::Unknown;

        for cell in &cells {
            if let Ok(value) = cell.parse::<u16>() {
                if port.is_none() && (1..=65535).contains(&value) {
                    port = Some(value);
                    continue;
                }
            }

            if let Some(parsed) = parse_protocol(cell) {
                // Only take a protocol from a cell that is nothing but one.
                if cell.split_whitespace().count() == 1 {
                    protocol = parsed;
                    continue;
                }
            }

            if address.is_none() && looks_like_address(cell) {
                address = Some(cell.clone());
                continue;
            }

            let lower = cell.to_ascii_lowercase();
            if lower.contains("transparent") {
                anonymity = Anonymity::Transparent;
            } else if lower.contains("anonymous") {
                anonymity = Anonymity::Anonymous;
            } else if lower.contains("elite") {
                anonymity = Anonymity::Elite;
            } else if country.is_none() && looks_like_country(cell) {
                country = Some(cell.clone());
            }
        }

        let Some(address) = address else { continue };
        let Some(port) = port else { continue };

        // The address and the port are separate cells here.
        let Some(mut proxy) = build(&address, port, protocol, country) else {
            continue;
        };
        proxy.anonymity = anonymity;
        proxies.push(proxy);
    }

    proxies
}

/// Whether a cell looks like an address rather than a word.
fn looks_like_address(cell: &str) -> bool {
    let cell = cell.trim();
    if cell.is_empty() || cell.contains(' ') {
        return false;
    }
    if cell.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    // A name: labels, dots and dashes, and it looks nothing like a sentence.
    cell.len() <= 253
        && cell
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        && cell.contains('.')
}

/// Whether a cell looks like a country label.
fn looks_like_country(cell: &str) -> bool {
    let cell = cell.trim();
    !cell.is_empty()
        && cell.len() <= 32
        && cell.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
        && cell.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_format_of_a_body() {
        assert_eq!(detect("[{\"ip\":\"1.2.3.4\"}]"), Format::Json);
        assert_eq!(
            detect("<html><table><tr><td>1.2.3.4</td></tr></table></html>"),
            Format::Html
        );
        assert_eq!(detect("1.2.3.4:8080\n5.6.7.8:3128"), Format::PlainText);
        assert_eq!(detect("1.2.3.4,8080\n5.6.7.8,3128"), Format::Csv);
    }

    #[test]
    fn parses_plain_text() {
        let proxies = parse_plain_text(
            "1.2.3.4:8080\n# a comment\n5.6.7.8:3128 socks5\n",
            ProtocolHint::default(),
        );
        assert_eq!(proxies.len(), 2);
        assert_eq!(proxies[0].id, "1.2.3.4:8080");
        assert_eq!(proxies[1].protocol, ProxyProtocol::Socks5);
    }

    #[test]
    fn parses_csv_with_a_header() {
        let proxies = parse_csv(
            "ip,port,protocol,country\n1.2.3.4,8080,socks5,NL\n5.6.7.8,3128,http,DE\n",
            ProtocolHint::default(),
        );
        assert_eq!(proxies.len(), 2);
        assert_eq!(proxies[0].protocol, ProxyProtocol::Socks5);
        // The list's claim is kept; the measured country comes from the exit
        // address once the proxy is verified.
        assert_eq!(proxies[0].country_reported.as_deref(), Some("NL"));
        assert_eq!(proxies[0].country, None);
    }

    #[test]
    fn parses_json_in_several_shapes() {
        let shapes = [
            r#"[{"ip":"1.2.3.4","port":8080,"protocol":"socks5","country":"NL"}]"#,
            r#"{"data":[{"host":"1.2.3.4","port":"8080"}]}"#,
            r#"{"proxies":[{"address":"1.2.3.4:8080"}]}"#,
        ];

        for shape in shapes {
            let proxies = parse_json(shape, ProtocolHint::default());
            assert_eq!(proxies.len(), 1, "{shape}");
            assert_eq!(proxies[0].host, "1.2.3.4");
        }
    }

    #[test]
    fn ignores_a_json_document_without_proxies() {
        assert!(parse_json(r#"{"status":"ok","count":0}"#, ProtocolHint::default()).is_empty());
    }

    #[test]
    fn finds_the_columns_of_a_table_by_content() {
        let html = "<table><tbody>\
            <tr><td>1.2.3.4</td><td>8080</td><td>United States</td><td>HTTP</td><td>elite proxy</td></tr>\
            </tbody></table>";
        let proxies = parse_html(html, ProtocolHint::default());
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].id, "1.2.3.4:8080");
        assert_eq!(proxies[0].protocol, ProxyProtocol::Http);
        assert_eq!(proxies[0].anonymity, Anonymity::Elite);
    }

    #[test]
    fn rejects_addresses_that_are_not_addresses() {
        assert!(split_address("a sentence with spaces").is_none());
        assert!(split_address("1.2.3.4:notaport").is_none());
        assert!(build("1.2.3.4", 80, ProxyProtocol::Http, None).is_some());
        assert!(build("http://1.2.3.4", 80, ProxyProtocol::Http, None).is_none());
    }
}
