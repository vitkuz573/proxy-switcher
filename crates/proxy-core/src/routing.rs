use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Which destinations are routed through a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    /// Nothing is routed through a proxy.
    ///
    /// The default. Routing everything through a proxy means one dead proxy
    /// takes the whole host's internet access with it, and the engine cannot
    /// even scrape or check anything to recover. A proxy is an opt-in tool
    /// for specific traffic, not a default route.
    #[default]
    None,
    /// Only destinations that match the rules go through a proxy.
    AllowList,
    /// Everything except the destinations that match the rules does.
    DenyList,
}

/// Where a connection should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Through the active proxy.
    Proxy,
    /// Straight to the destination.
    Direct,
}

/// Which traffic a proxy applies to.
///
/// The rules are matched against the destination address, the destination port
/// and the hostname when the router knows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePolicy {
    pub mode: RouteMode,
    /// Address ranges, as `1.2.3.0/24` or a bare `1.2.3.4`.
    #[serde(default)]
    pub cidrs: Vec<String>,
    /// Hostnames or domain suffixes, as `example.com` or `.example.com`.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Destination ports. Empty means every port.
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Addresses that never go through a proxy, whatever the mode says.
    ///
    /// The engine's own control plane lives here, and so does the proxy
    /// itself: a connection to the proxy that is routed through a proxy is a
    /// loop.
    #[serde(default)]
    pub always_direct: Vec<String>,
}

impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            mode: RouteMode::None,
            cidrs: Vec::new(),
            domains: Vec::new(),
            ports: Vec::new(),
            always_direct: Vec::new(),
        }
    }
}

impl RoutePolicy {
    /// True when the policy routes anything at all.
    pub fn is_enabled(&self) -> bool {
        self.mode != RouteMode::None
    }

    /// Decide how a connection should be routed.
    ///
    /// `hostname` is used when the router resolved the destination itself; the
    /// address is always checked as well, so a rule on a hostname still
    /// applies to the address it resolved to.
    pub fn decide(&self, ip: IpAddr, port: u16, hostname: Option<&str>) -> Decision {
        // Addresses that must never be proxied, checked first and without
        // exception.
        if is_local(ip) {
            return Decision::Direct;
        }

        if self.matches_any(ip, port, &self.always_direct) {
            return Decision::Direct;
        }

        let matched = self.matches_any(ip, port, &self.cidrs);

        match self.mode {
            RouteMode::None => Decision::Direct,
            RouteMode::AllowList => {
                if matched || self.matches_domain(hostname) {
                    Decision::Proxy
                } else {
                    Decision::Direct
                }
            }
            RouteMode::DenyList => {
                if matched || self.matches_domain(hostname) {
                    Decision::Direct
                } else {
                    Decision::Proxy
                }
            }
        }
    }

    /// True when the destination is one that never goes through a proxy.
    pub fn matches_always_direct(&self, ip: IpAddr, port: u16) -> bool {
        self.matches_any(ip, port, &self.always_direct)
    }

    /// True when the destination matches one of the given rules.
    fn matches_any(&self, ip: IpAddr, port: u16, cidrs: &[String]) -> bool {
        if !self.ports.is_empty() && !self.ports.contains(&port) {
            return false;
        }
        cidrs.iter().any(|rule| matches_cidr(rule, ip))
    }

    fn matches_domain(&self, hostname: Option<&str>) -> bool {
        let Some(hostname) = hostname else {
            return false;
        };
        self.domains
            .iter()
            .any(|rule| matches_domain(rule, hostname))
    }
}

/// True for addresses that must never leave through a proxy.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                // 100.64.0.0/10, carrier-grade NAT.
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local addresses.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Match an address against a `1.2.3.0/24` or bare `1.2.3.4` rule.
pub fn matches_cidr(rule: &str, ip: IpAddr) -> bool {
    let rule = rule.trim();
    if rule.is_empty() {
        return false;
    }

    match rule.split_once('/') {
        Some((network, prefix)) => {
            let Ok(network) = network.parse::<IpAddr>() else {
                return false;
            };
            let Ok(prefix) = prefix.parse::<u8>() else {
                return false;
            };
            match (network, ip) {
                (IpAddr::V4(net), IpAddr::V4(addr)) => {
                    let net = u32::from(net);
                    let addr = u32::from(addr);
                    if prefix == 0 {
                        return true;
                    }
                    if prefix > 32 {
                        return false;
                    }
                    let mask = u32::MAX << (32 - prefix);
                    (net & mask) == (addr & mask)
                }
                (IpAddr::V6(net), IpAddr::V6(addr)) => {
                    let net = u128::from(net);
                    let addr = u128::from(addr);
                    if prefix == 0 {
                        return true;
                    }
                    if prefix > 128 {
                        return false;
                    }
                    let mask = u128::MAX << (128 - prefix);
                    (net & mask) == (addr & mask)
                }
                _ => false,
            }
        }
        None => rule.parse::<IpAddr>().map(|r| r == ip).unwrap_or(false),
    }
}

/// Match a hostname against a rule.
///
/// A rule of `example.com` matches that host and any subdomain of it; a rule
/// starting with a dot matches subdomains only.
pub fn matches_domain(rule: &str, hostname: &str) -> bool {
    let raw = rule.trim();
    let hostname = hostname.trim().trim_end_matches('.').to_ascii_lowercase();

    if raw.is_empty() || hostname.is_empty() {
        return false;
    }

    // A leading dot means "subdomains only": `.example.com` matches
    // `www.example.com` but not `example.com` itself.
    if let Some(suffix) = raw.strip_prefix('.') {
        let suffix = suffix.to_ascii_lowercase();
        return hostname
            .strip_suffix(&suffix)
            .is_some_and(|prefix| prefix.ends_with('.'));
    }

    let rule = raw.to_ascii_lowercase();
    if hostname == rule {
        return true;
    }
    hostname
        .strip_suffix(&rule)
        .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn nothing_is_routed_by_default() {
        let policy = RoutePolicy::default();
        assert_eq!(policy.decide(ip("8.8.8.8"), 443, None), Decision::Direct);
        assert!(!policy.is_enabled());
    }

    #[test]
    fn an_allow_list_routes_only_what_it_names() {
        let policy = RoutePolicy {
            mode: RouteMode::AllowList,
            cidrs: vec!["1.2.3.0/24".to_string()],
            ..Default::default()
        };
        assert_eq!(policy.decide(ip("1.2.3.4"), 443, None), Decision::Proxy);
        assert_eq!(policy.decide(ip("8.8.8.8"), 443, None), Decision::Direct);
    }

    #[test]
    fn a_deny_list_routes_everything_but_what_it_names() {
        let policy = RoutePolicy {
            mode: RouteMode::DenyList,
            cidrs: vec!["10.0.0.0/8".to_string()],
            ..Default::default()
        };
        assert_eq!(policy.decide(ip("8.8.8.8"), 443, None), Decision::Proxy);
        assert_eq!(policy.decide(ip("10.1.2.3"), 443, None), Decision::Direct);
    }

    #[test]
    fn local_addresses_never_go_through_a_proxy() {
        let policy = RoutePolicy {
            mode: RouteMode::DenyList,
            ..Default::default()
        };
        for address in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "169.254.1.1"] {
            assert_eq!(
                policy.decide(ip(address), 443, None),
                Decision::Direct,
                "{address} must stay local"
            );
        }
    }

    #[test]
    fn always_direct_overrides_the_mode() {
        let policy = RoutePolicy {
            mode: RouteMode::DenyList,
            always_direct: vec!["8.8.8.8".to_string()],
            ..Default::default()
        };
        assert_eq!(policy.decide(ip("8.8.8.8"), 443, None), Decision::Direct);
    }

    #[test]
    fn ports_narrow_the_rules() {
        let policy = RoutePolicy {
            mode: RouteMode::AllowList,
            cidrs: vec!["1.2.3.0/24".to_string()],
            ports: vec![443],
            ..Default::default()
        };
        assert_eq!(policy.decide(ip("1.2.3.4"), 443, None), Decision::Proxy);
        assert_eq!(policy.decide(ip("1.2.3.4"), 80, None), Decision::Direct);
    }

    #[test]
    fn domains_match_subdomains() {
        assert!(matches_domain("example.com", "example.com"));
        assert!(matches_domain("example.com", "www.example.com"));
        assert!(!matches_domain("example.com", "notexample.com"));
        assert!(matches_domain(".example.com", "www.example.com"));
        assert!(!matches_domain(".example.com", "example.com"));
    }

    #[test]
    fn cidr_rules_match_ranges() {
        assert!(matches_cidr("10.0.0.0/8", ip("10.1.2.3")));
        assert!(!matches_cidr("10.0.0.0/8", ip("11.1.2.3")));
        assert!(matches_cidr("192.168.1.1", ip("192.168.1.1")));
        assert!(!matches_cidr("192.168.1.1", ip("192.168.1.2")));
        assert!(matches_cidr("0.0.0.0/0", ip("8.8.8.8")));
    }
}
