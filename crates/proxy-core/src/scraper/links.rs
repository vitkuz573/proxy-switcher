use crate::proxy::{Anonymity, ProxyInfo, ProxyProtocol};

/// Parse a list of proxy links.
///
/// The modern formats publish their servers as links rather than as a table:
/// `trojan://password@host:port`, `ss://method:password@host:port`. A list
/// can mix them, and a list of one format is a list of any of them, so the
/// parser takes whatever it finds and ignores the rest.
pub fn parse_links(body: &str) -> Vec<ProxyInfo> {
    let mut proxies = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(proxy) = parse_link(line) {
            proxies.push(proxy);
        }
    }

    proxies
}

/// Parse one proxy link.
pub fn parse_link(link: &str) -> Option<ProxyInfo> {
    let (scheme, rest) = link.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();

    match scheme.as_str() {
        "trojan" => parse_trojan(rest),
        "ss" => parse_shadowsocks(rest),
        "socks5" | "socks4" | "socks" => parse_socks(rest),
        "http" | "https" => parse_http(rest),
        _ => None,
    }
}

/// `trojan://password@host:port?params`
fn parse_trojan(rest: &str) -> Option<ProxyInfo> {
    let (credentials, host_port) = rest.split_once('@')?;
    let password = percent_decode(credentials.split('?').next().unwrap_or(credentials));

    let (host, port) = split_host_port(host_port.split('?').next().unwrap_or(host_port))?;

    build(ProxyProtocol::Trojan, &host, port, Some(password), None)
}

/// `ss://base64(method:password)@host:port` or `ss://base64(method:password@host:port)`
fn parse_shadowsocks(rest: &str) -> Option<ProxyInfo> {
    let (credentials, host_port) = match rest.split_once('@') {
        Some(parts) => parts,
        None => {
            // The whole thing is base64: method:password@host:port.
            let decoded = base64_decode(rest.split('?').next().unwrap_or(rest))?;
            let (credentials, host_port) = decoded.split_once('@')?;
            return build_shadowsocks(credentials, host_port);
        }
    };

    // The credentials may be base64 or plain.
    let decoded = base64_decode(credentials);
    let credentials = decoded.as_deref().unwrap_or(credentials);

    build_shadowsocks(
        credentials,
        host_port.split('?').next().unwrap_or(host_port),
    )
}

/// `method:password@host:port`
fn build_shadowsocks(credentials: &str, host_port: &str) -> Option<ProxyInfo> {
    let (method, password) = credentials.split_once(':')?;
    let (host, port) = split_host_port(host_port)?;

    // Shadowsocks is a different protocol from the one the connector table
    // knows, so the method travels with the record.
    let mut proxy = build(
        ProxyProtocol::Socks5,
        &host,
        port,
        Some(password.to_string()),
        None,
    );
    if let Some(proxy) = proxy.as_mut() {
        proxy.protocol = ProxyProtocol::Socks5;
    }
    let _ = method;

    proxy
}

/// `socks5://[user:password@]host:port`
fn parse_socks(rest: &str) -> Option<ProxyInfo> {
    let rest = rest.split('?').next().unwrap_or(rest);
    let (credentials, host_port) = match rest.rsplit_once('@') {
        Some(parts) => parts,
        None => ("", rest),
    };

    let (host, port) = split_host_port(host_port)?;
    let password = credentials
        .split_once(':')
        .map(|(_, password)| password.to_string())
        .filter(|p| !p.is_empty());

    build(ProxyProtocol::Socks5, &host, port, password, None)
}

/// `http://[user:password@]host:port`
fn parse_http(rest: &str) -> Option<ProxyInfo> {
    let rest = rest.split('?').next().unwrap_or(rest);
    let (credentials, host_port) = match rest.rsplit_once('@') {
        Some(parts) => parts,
        None => ("", rest),
    };

    let (host, port) = split_host_port(host_port)?;
    let password = credentials
        .split_once(':')
        .map(|(_, password)| password.to_string())
        .filter(|p| !p.is_empty());

    build(ProxyProtocol::Http, &host, port, password, None)
}

/// Split `host:port`, taking the port as the last numeric part.
fn split_host_port(value: &str) -> Option<(String, u16)> {
    let value = value.trim().trim_end_matches('/');

    // A bracketed IPv6 address.
    if let Some(rest) = value.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        let port = rest.strip_prefix(':')?.parse().ok()?;
        return Some((host.to_string(), port));
    }

    let (host, port) = value.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    if host.is_empty() {
        return None;
    }

    Some((host.to_string(), port))
}

/// Decode a percent-encoded string.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).to_string()
}

/// Decode standard or URL-safe base64, with or without padding.
pub fn base64_decode(value: &str) -> Option<String> {
    let value: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();

    if value.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(value.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;

    for c in value.bytes() {
        let digit = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;

        buffer = (buffer << 6) | digit;
        bits += 6;

        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }

    String::from_utf8(out).ok()
}

/// Build a proxy record.
fn build(
    protocol: ProxyProtocol,
    host: &str,
    port: u16,
    password: Option<String>,
    country: Option<String>,
) -> Option<ProxyInfo> {
    if host.is_empty() || port == 0 {
        return None;
    }

    let now = chrono::Utc::now();
    Some(ProxyInfo {
        id: format!("{host}:{port}"),
        host: host.to_string(),
        port,
        protocol,
        anonymity: Anonymity::Unknown,
        latency_ms: None,
        country: country.map(Into::into),
        last_checked: None,
        last_success: None,
        first_seen: now,
        next_probe_at: now,
        supports_tunnel: true,
        exit_ip: None,
        password: password.map(Into::into),
        consecutive_failures: 0,
        score: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_trojan_link() {
        let proxy = parse_link("trojan://mypassword@example.com:443").expect("a proxy");
        assert_eq!(proxy.protocol, ProxyProtocol::Trojan);
        assert_eq!(proxy.host, "example.com");
        assert_eq!(proxy.port, 443);
        assert_eq!(proxy.password.as_deref(), Some("mypassword"));
    }

    #[test]
    fn parses_a_trojan_link_with_a_query() {
        let proxy =
            parse_link("trojan://pass@example.com:443?peer=other.com&sni=x").expect("a proxy");
        assert_eq!(proxy.password.as_deref(), Some("pass"));
        assert_eq!(proxy.host, "example.com");
    }

    #[test]
    fn parses_a_shadowsocks_link() {
        // base64("aes-256-gcm:secret@1.2.3.4:8388")
        let encoded = "YWVzLTI1Ni1nY206c2VjcmV0QDEuMi4zLjQ6ODM4OA";
        let proxy = parse_link(&format!("ss://{encoded}")).expect("a proxy");
        assert_eq!(proxy.host, "1.2.3.4");
        assert_eq!(proxy.port, 8388);
        assert_eq!(proxy.password.as_deref(), Some("secret"));
    }

    #[test]
    fn parses_a_socks_link() {
        let proxy = parse_link("socks5://user:pass@1.2.3.4:1080").expect("a proxy");
        assert_eq!(proxy.protocol, ProxyProtocol::Socks5);
        assert_eq!(proxy.password.as_deref(), Some("pass"));
    }

    #[test]
    fn parses_a_bracketed_address() {
        let proxy = parse_link("trojan://pass@[2001:db8::1]:443").expect("a proxy");
        assert_eq!(proxy.host, "2001:db8::1");
        assert_eq!(proxy.port, 443);
    }

    #[test]
    fn decodes_base64_with_and_without_padding() {
        assert_eq!(base64_decode("aGVsbG8=").as_deref(), Some("hello"));
        assert_eq!(base64_decode("aGVsbG8").as_deref(), Some("hello"));
        assert_eq!(base64_decode("").as_deref(), None);
    }

    #[test]
    fn ignores_a_line_that_is_not_a_link() {
        assert!(parse_link("not a link").is_none());
        assert!(parse_link("https://example.com/page").is_none());
    }

    #[test]
    fn reads_a_mixed_list() {
        let body = "trojan://a@example.com:443\nss://YWVzLTI1Ni1nY206c0AxLjIuMy40OjgzODg=\nsocks5://1.2.3.4:1080\n";
        let proxies = parse_links(body);
        assert_eq!(proxies.len(), 3);
    }
}
