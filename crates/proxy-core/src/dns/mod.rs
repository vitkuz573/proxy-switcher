use anyhow::Result;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use std::net::IpAddr;

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
