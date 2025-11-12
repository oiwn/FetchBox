use super::models::{ProxyConfig, ProxyEndpoint, ProxyPoolConfig, ResolvedProxyPool};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("Proxy pool '{0}' not found")]
    PoolNotFound(String),

    #[error("Cycle detected in proxy fallback chain: {0}")]
    CycleDetected(String),
}

/// Proxy graph for resolving fallback chains
pub struct ProxyGraph<'a> {
    pools: &'a HashMap<String, ProxyPoolConfig>,
}

impl<'a> ProxyGraph<'a> {
    /// Create a new proxy graph from config
    pub fn new(config: &'a ProxyConfig) -> Self {
        Self {
            pools: &config.pools,
        }
    }

    /// Resolve a proxy pool into tiered fallback structure
    /// Tier 0 = primary proxies
    /// Tier 1+ = fallback tiers in order
    pub fn resolve(&self, pool_name: &str) -> Result<ResolvedProxyPool, ResolverError> {
        let mut visited = HashSet::new();
        let mut tiers = Vec::new();

        self.resolve_recursive(pool_name, &mut visited, &mut tiers)?;

        Ok(ResolvedProxyPool { tiers })
    }

    fn resolve_recursive(
        &self,
        current: &str,
        visited: &mut HashSet<String>,
        tiers: &mut Vec<Vec<ProxyEndpoint>>,
    ) -> Result<(), ResolverError> {
        // Normalize pool name (strip "pools/" prefix if present)
        let pool_name = current.strip_prefix("pools/").unwrap_or(current);

        // Cycle detection
        if visited.contains(pool_name) {
            return Err(ResolverError::CycleDetected(pool_name.to_string()));
        }

        visited.insert(pool_name.to_string());

        // Get pool config
        let pool = self
            .pools
            .get(pool_name)
            .ok_or_else(|| ResolverError::PoolNotFound(pool_name.to_string()))?;

        // Add primary proxies as current tier
        let endpoints: Vec<ProxyEndpoint> = pool
            .primary
            .iter()
            .map(|uri| ProxyEndpoint { uri: uri.clone() })
            .collect();

        tiers.push(endpoints);

        // Recursively resolve fallbacks
        for fallback in &pool.fallbacks {
            self.resolve_recursive(fallback, visited, tiers)?;
        }

        Ok(())
    }

    /// Resolve all pools and return a cached map
    pub fn resolve_all(&self) -> Result<HashMap<String, ResolvedProxyPool>, ResolverError> {
        let mut resolved = HashMap::new();

        for pool_name in self.pools.keys() {
            let resolved_pool = self.resolve(pool_name)?;
            resolved.insert(pool_name.clone(), resolved_pool);
        }

        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::*;
    use super::*;

    #[test]
    fn test_resolve_simple_pool() {
        let mut pools = HashMap::new();
        pools.insert(
            "default".to_string(),
            ProxyPoolConfig {
                primary: vec![
                    "http://proxy-a:8080".to_string(),
                    "http://proxy-b:8080".to_string(),
                ],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);
        let resolved = graph.resolve("default").unwrap();

        assert_eq!(resolved.tiers.len(), 1);
        assert_eq!(resolved.tiers[0].len(), 2);
        assert_eq!(resolved.tiers[0][0].uri, "http://proxy-a:8080");
        assert_eq!(resolved.tiers[0][1].uri, "http://proxy-b:8080");
    }

    #[test]
    fn test_resolve_with_fallback() {
        let mut pools = HashMap::new();

        pools.insert(
            "primary".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://primary:8080".to_string()],
                fallbacks: vec!["fallback".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        pools.insert(
            "fallback".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://fallback:8080".to_string()],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);
        let resolved = graph.resolve("primary").unwrap();

        assert_eq!(resolved.tiers.len(), 2);
        assert_eq!(resolved.tiers[0][0].uri, "http://primary:8080");
        assert_eq!(resolved.tiers[1][0].uri, "http://fallback:8080");
    }

    #[test]
    fn test_resolve_multi_tier_fallback() {
        let mut pools = HashMap::new();

        pools.insert(
            "tier1".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://tier1-a:8080".to_string()],
                fallbacks: vec!["tier2".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        pools.insert(
            "tier2".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://tier2-a:8080".to_string()],
                fallbacks: vec!["tier3".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        pools.insert(
            "tier3".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://tier3-a:8080".to_string()],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);
        let resolved = graph.resolve("tier1").unwrap();

        assert_eq!(resolved.tiers.len(), 3);
        assert_eq!(resolved.tiers[0][0].uri, "http://tier1-a:8080");
        assert_eq!(resolved.tiers[1][0].uri, "http://tier2-a:8080");
        assert_eq!(resolved.tiers[2][0].uri, "http://tier3-a:8080");
    }

    #[test]
    fn test_resolve_pools_prefix() {
        let mut pools = HashMap::new();

        pools.insert(
            "primary".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://primary:8080".to_string()],
                fallbacks: vec!["pools/fallback".to_string()], // with prefix
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        pools.insert(
            "fallback".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://fallback:8080".to_string()],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);
        let resolved = graph.resolve("primary").unwrap();

        assert_eq!(resolved.tiers.len(), 2);
    }

    #[test]
    fn test_resolve_nonexistent_pool() {
        let pools = HashMap::new();
        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);

        let result = graph.resolve("nonexistent");
        assert!(matches!(result, Err(ResolverError::PoolNotFound(_))));
    }

    #[test]
    fn test_resolve_all() {
        let mut pools = HashMap::new();

        pools.insert(
            "pool_a".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://a:8080".to_string()],
                fallbacks: vec![],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        pools.insert(
            "pool_b".to_string(),
            ProxyPoolConfig {
                primary: vec!["http://b:8080".to_string()],
                fallbacks: vec!["pool_a".to_string()],
                retry_backoff_ms: 500,
                max_retries: 3,
            },
        );

        let config = ProxyConfig { pools };
        let graph = ProxyGraph::new(&config);
        let resolved_all = graph.resolve_all().unwrap();

        assert_eq!(resolved_all.len(), 2);
        assert!(resolved_all.contains_key("pool_a"));
        assert!(resolved_all.contains_key("pool_b"));

        assert_eq!(resolved_all["pool_a"].tiers.len(), 1);
        assert_eq!(resolved_all["pool_b"].tiers.len(), 2);
    }
}
