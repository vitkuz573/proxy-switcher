use anyhow::Result;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct DnsHandler {
    resolver: TokioAsyncResolver,
}

impl DnsHandler {
    pub fn new() -> Self {
        let resolver = TokioAsyncResolver::tokio(
            ResolverConfig::default(),
            ResolverOpts::default(),
        );
        Self { resolver }
    }

    pub async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>> {
        let response = self.resolver.lookup_ip(host).await?;
        Ok(response.into_iter().collect())
    }
}

impl Default for DnsHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe cache of IP → hostname mappings, populated from DNS responses
/// intercepted in the TUN loop.
#[derive(Clone, Default)]
pub struct DnsCache {
    inner: Arc<RwLock<HashMap<u32, String>>>,
}

impl DnsCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, ip: Ipv4Addr, host: String) {
        self.inner.write().await.insert(u32::from(ip), host);
    }

    pub async fn insert_str(&self, ip: &str, host: &str) {
        if let Ok(addr) = ip.parse::<Ipv4Addr>() {
            self.insert(addr, host.to_string()).await;
        }
    }

    pub async fn lookup(&self, ip: Ipv4Addr) -> Option<String> {
        self.inner.read().await.get(&u32::from(ip)).cloned()
    }
}

/// Parse a DNS response payload (after UDP header) and extract A record mappings.
/// Returns (hostname, ip) pairs.
pub fn parse_dns_response(data: &[u8]) -> Vec<(String, Ipv4Addr)> {
    if data.len() < 12 {
        return vec![];
    }

    let flags = u16::from_be_bytes([data[2], data[3]]);
    if (flags >> 15) & 1 != 1 {
        return vec![];
    }

    let questions = u16::from_be_bytes([data[4], data[5]]) as usize;
    let answers = u16::from_be_bytes([data[6], data[7]]) as usize;
    if questions == 0 || answers == 0 {
        return vec![];
    }

    let mut offset = 12;

    for _ in 0..questions {
        match skip_name(data, offset) {
            Some(new) => offset = new,
            None => return vec![],
        }
        if offset + 4 > data.len() {
            return vec![];
        }
        offset += 4;
    }

    let mut results = Vec::new();
    for _ in 0..answers {
        match skip_name(data, offset) {
            Some(new) => offset = new,
            None => return results,
        }
        if offset + 10 > data.len() {
            return results;
        }
        let atype = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let aclass = u16::from_be_bytes([data[offset + 2], data[offset + 3]]);
        let rdlength = u16::from_be_bytes([data[offset + 8], data[offset + 9]]) as usize;
        offset += 10;

        if atype == 1 && aclass == 1 && rdlength == 4 && offset + 4 <= data.len() {
            let ip = Ipv4Addr::new(data[offset], data[offset + 1], data[offset + 2], data[offset + 3]);
            // Try to extract the original query name from the answer name
            results.push((ip.to_string(), ip));
            offset += rdlength;
        } else {
            offset += rdlength;
        }
    }

    results
}

fn skip_name(data: &[u8], start: usize) -> Option<usize> {
    let mut offset = start;
    loop {
        if offset >= data.len() {
            return None;
        }
        let len = data[offset];
        if len == 0 {
            return Some(offset + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(offset + 2);
        }
        offset += 1 + len as usize;
        if offset > data.len() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dns_response() {
        // Craft a minimal DNS response: header + question + A record
        let pkt = vec![
            0x00, 0x01, // ID
            0x81, 0x80, // flags: QR=1, response
            0x00, 0x01, // 1 question
            0x00, 0x01, // 1 answer
            0x00, 0x00, // 0 authority
            0x00, 0x00, // 0 additional
            // Question: httpbin.org = 7httpbin3org0
            0x07, b'h', b't', b't', b'p', b'b', b'i', b'n',
            0x03, b'o', b'r', b'g',
            0x00, // end of name
            0x00, 0x01, // type A
            0x00, 0x01, // class IN
            // Answer: pointer to name (0xC0 0x0C = offset 12), A record
            0xC0, 0x0C, // name pointer
            0x00, 0x01, // type A
            0x00, 0x01, // class IN
            0x00, 0x00, 0x00, 0x3C, // TTL = 60
            0x00, 0x04, // rdlength = 4
            0x34, 0x05, 0xF5, 0xB2, // 52.5.245.178
        ];

        let results = parse_dns_response(&pkt);
        assert!(!results.is_empty(), "should have parsed at least one record");
        assert_eq!(results[0].1, Ipv4Addr::new(52, 5, 245, 178));
    }
}
