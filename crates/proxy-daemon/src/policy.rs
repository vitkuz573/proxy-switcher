use std::collections::BTreeMap;
use std::net::IpAddr;

use anyhow::Result;
use tracing::{info, warn};

use proxy_core::routing::RoutePolicy;
use proxy_core::tun_manager::TunManager;

/// The destinations the policy sends into the device.
///
/// A policy names addresses and names. The addresses are used as they are; the
/// names are resolved, and the addresses they come to are used as well. The
/// resolution is repeated, because the addresses behind a name change.
#[derive(Debug, Default)]
pub struct PolicyRoutes {
    /// The addresses the policy covers, and the name behind each where there
    /// is one.
    pub destinations: BTreeMap<IpAddr, Option<String>>,
}

impl PolicyRoutes {
    /// How many destinations the policy covers.
    pub fn len(&self) -> usize {
        self.destinations.len()
    }
}

/// Resolve the policy into the destinations it covers.
///
/// The names are resolved with the system resolver, so the operator's own
/// resolver and any VPN it carries are the ones that answer. A name that does
/// not resolve is reported and left out: it covers nothing today, and the next
/// round will try again.
pub async fn resolve_policy(policy: &RoutePolicy) -> PolicyRoutes {
    let mut destinations: BTreeMap<IpAddr, Option<String>> = BTreeMap::new();

    // The addresses the policy names are used as they are.
    for rule in &policy.cidrs {
        match parse_destination(rule) {
            Some(address) => {
                destinations.insert(address, None);
            }
            None => warn!("the policy names {rule}, which is not an address"),
        }
    }

    // The names are resolved, and every address behind them is covered.
    for name in &policy.domains {
        let host = name.trim().trim_start_matches('.').to_lowercase();
        if host.is_empty() {
            continue;
        }

        match tokio::net::lookup_host((host.clone(), 443u16)).await {
            Ok(addresses) => {
                let mut found = false;
                for address in addresses {
                    // A local address is never proxied, and never routed
                    // into the device either.
                    if proxy_core::routing::is_local(address.ip()) {
                        continue;
                    }
                    destinations.insert(address.ip(), Some(host.clone()));
                    found = true;
                }
                if !found {
                    warn!("the policy names {host}, but it resolved to nothing usable");
                }
            }
            Err(e) => warn!("the policy names {host}, and it did not resolve: {e}"),
        }
    }

    PolicyRoutes { destinations }
}

/// Parse a destination the policy names.
///
/// A bare address is an address; anything with a prefix length is a range.
fn parse_destination(rule: &str) -> Option<IpAddr> {
    let rule = rule.trim();
    if let Ok(address) = rule.parse::<IpAddr>() {
        return Some(address);
    }
    None
}

/// The routes to put into the routing table for a policy.
///
/// A range is used as it stands; a single address becomes a single-address
/// route, which is more specific than the default route and therefore wins.
pub fn routes_for(policy: &RoutePolicy, resolved: &PolicyRoutes) -> Vec<String> {
    let mut routes: Vec<String> = Vec::new();

    for rule in &policy.cidrs {
        let rule = rule.trim();
        if rule.is_empty() {
            continue;
        }
        // A range is a route as it stands. A bare address needs a prefix
        // length, or the kernel reads it as a network rather than a host.
        if rule.contains('/') {
            routes.push(rule.to_string());
        } else if let Ok(address) = rule.parse::<IpAddr>() {
            routes.push(match address {
                IpAddr::V4(v4) => format!("{v4}/32"),
                IpAddr::V6(v6) => format!("{v6}/128"),
            });
        }
    }

    for address in resolved.destinations.keys() {
        let route = match address {
            IpAddr::V4(v4) => format!("{v4}/32"),
            IpAddr::V6(v6) => format!("{v6}/128"),
        };
        if !routes.contains(&route) {
            routes.push(route);
        }
    }

    routes
}

/// Put the policy's routes into the routing table.
pub async fn apply(
    tun: &TunManager,
    policy: &RoutePolicy,
    resolved: &PolicyRoutes,
) -> Result<usize> {
    let routes = routes_for(policy, resolved);
    let count = routes.len();

    let installed = tun.sync_policy_routes(&routes).await?;
    info!(
        destinations = count,
        installed = installed.len(),
        "policy routes applied"
    );

    Ok(installed.len())
}
