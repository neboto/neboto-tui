use crate::aws::region::Region;
use crate::aws::resource::Resource;
use crate::aws::service::ServiceType;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Cache key combining service, region, and an optional variant discriminator.
/// The variant lets a single service cache several distinct result sets — e.g.
/// the Cost service caches each group-by/period combination separately so
/// toggling between them is instant instead of refetching.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    service: ServiceType,
    region: Region,
    variant: Option<String>,
}

pub struct ResourceCache {
    entries: HashMap<CacheKey, CacheEntry>,
    ttl: Duration,
    /// Per-service TTL overrides from config (`cache_ttls`). Checked before
    /// the built-in defaults in `effective_ttl`.
    ttl_overrides: HashMap<ServiceType, Duration>,
}

struct CacheEntry {
    resources: Vec<Box<dyn Resource>>,
    timestamp: Instant,
}

impl ResourceCache {
    pub fn new(ttl: Duration, ttl_overrides: HashMap<ServiceType, Duration>) -> Self {
        Self {
            entries: HashMap::new(),
            ttl,
            ttl_overrides,
        }
    }

    fn cache_region(service: &ServiceType, region: &Region) -> Region {
        if service.is_global() {
            Region::UsEast1
        } else {
            *region
        }
    }

    /// Per-service freshness window: config override first, then the built-in
    /// defaults. Cost Explorer data only refreshes ~daily and is billed per
    /// request, so it gets a much longer TTL than the default.
    fn effective_ttl(&self, service: &ServiceType) -> Duration {
        if let Some(ttl) = self.ttl_overrides.get(service) {
            return *ttl;
        }
        match service {
            ServiceType::Cost => Duration::from_secs(6 * 60 * 60), // 6 hours
            _ => self.ttl,
        }
    }

    pub fn get(
        &self,
        service: &ServiceType,
        region: &Region,
        variant: Option<&str>,
    ) -> Option<Vec<Box<dyn Resource>>> {
        let key = CacheKey {
            service: *service,
            region: Self::cache_region(service, region),
            variant: variant.map(str::to_string),
        };

        let ttl = self.effective_ttl(service);
        self.entries.get(&key).and_then(|entry| {
            if entry.timestamp.elapsed() < ttl {
                Some(entry.resources.iter().map(|r| r.clone()).collect())
            } else {
                None
            }
        })
    }

    pub fn insert(
        &mut self,
        service: ServiceType,
        region: Region,
        variant: Option<String>,
        resources: Vec<Box<dyn Resource>>,
    ) {
        let key = CacheKey {
            service,
            region: Self::cache_region(&service, &region),
            variant,
        };

        self.entries.insert(
            key,
            CacheEntry {
                resources,
                timestamp: Instant::now(),
            },
        );
    }

    /// Borrow the live (within-TTL) entry for this key without cloning — the
    /// `@all` cross-service search scans every cached list on each keystroke
    /// and only clones what it keeps.
    pub fn get_ref(
        &self,
        service: &ServiceType,
        region: &Region,
        variant: Option<&str>,
    ) -> Option<&[Box<dyn Resource>]> {
        let key = CacheKey {
            service: *service,
            region: Self::cache_region(service, region),
            variant: variant.map(str::to_string),
        };
        let ttl = self.effective_ttl(service);
        self.entries.get(&key).and_then(|entry| {
            (entry.timestamp.elapsed() < ttl).then_some(entry.resources.as_slice())
        })
    }

    /// Age of the live (within-TTL) cache entry for this key, if any. Lets the
    /// UI surface how old the data on screen is ("updated 3m ago").
    pub fn age(
        &self,
        service: &ServiceType,
        region: &Region,
        variant: Option<&str>,
    ) -> Option<Duration> {
        let key = CacheKey {
            service: *service,
            region: Self::cache_region(service, region),
            variant: variant.map(str::to_string),
        };
        let ttl = self.effective_ttl(service);
        self.entries.get(&key).and_then(|entry| {
            let age = entry.timestamp.elapsed();
            (age < ttl).then_some(age)
        })
    }

    pub fn invalidate(&mut self, service: &ServiceType, region: &Region, variant: Option<&str>) {
        let key = CacheKey {
            service: *service,
            region: Self::cache_region(service, region),
            variant: variant.map(str::to_string),
        };
        self.entries.remove(&key);
    }
}
