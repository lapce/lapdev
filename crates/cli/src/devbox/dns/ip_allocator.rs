use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr},
};

/// Allocates synthetic loopback IPs from a reserved subnet (127.77.0.0/24)
pub struct SyntheticIpAllocator {
    /// Base IP: 127.77.0.0
    base: Ipv4Addr,
    /// Next available offset (0-255)
    next_offset: u8,
    /// Allocated IPs mapped to their service keys (name.namespace:port)
    allocated: HashMap<String, IpAddr>,
    /// Reverse mapping from IP to service key
    reverse: HashMap<IpAddr, String>,
    /// Freed IPs that can be reused
    free_pool: HashSet<u8>,
}

impl Default for SyntheticIpAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntheticIpAllocator {
    pub fn new() -> Self {
        Self {
            base: Ipv4Addr::new(127, 77, 0, 0),
            next_offset: 1, // Start from .1, skip .0
            allocated: HashMap::new(),
            reverse: HashMap::new(),
            free_pool: HashSet::new(),
        }
    }

    /// Allocate or reuse an IP for a service endpoint
    pub fn allocate(&mut self, service_name: &str, namespace: &str, port: u16) -> Option<IpAddr> {
        let key = format!("{}.{}:{}", service_name, namespace, port);

        // If already allocated, return existing IP
        if let Some(ip) = self.allocated.get(&key) {
            return Some(*ip);
        }

        // Try to allocate from free pool first
        let offset = if let Some(offset) = self.free_pool.iter().next().copied() {
            self.free_pool.remove(&offset);
            offset
        } else {
            // Allocate new IP
            if self.next_offset == 0 {
                // Wrapped around, no more IPs available
                return None;
            }
            let offset = self.next_offset;
            self.next_offset = self.next_offset.wrapping_add(1);
            offset
        };

        let ip = IpAddr::V4(Ipv4Addr::new(
            self.base.octets()[0],
            self.base.octets()[1],
            self.base.octets()[2],
            offset,
        ));

        self.allocated.insert(key.clone(), ip);
        self.reverse.insert(ip, key);
        Some(ip)
    }

    /// Deallocate an IP and return it to the free pool
    pub fn deallocate(&mut self, service_name: &str, namespace: &str, port: u16) {
        let key = format!("{}.{}:{}", service_name, namespace, port);
        if let Some(ip) = self.allocated.remove(&key) {
            self.reverse.remove(&ip);
            if let IpAddr::V4(ipv4) = ip {
                let offset = ipv4.octets()[3];
                self.free_pool.insert(offset);
            }
        }
    }

    /// Deallocate all IPs and reset
    pub fn clear(&mut self) {
        self.allocated.clear();
        self.reverse.clear();
        self.free_pool.clear();
        self.next_offset = 1;
    }

    /// Get the service key for an IP address
    pub fn lookup_ip(&self, ip: &IpAddr) -> Option<&str> {
        self.reverse.get(ip).map(|s| s.as_str())
    }

    /// Get all allocated IPs
    pub fn allocated_ips(&self) -> Vec<IpAddr> {
        self.allocated.values().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_and_reuse() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip1 = allocator.allocate("service1", "default", 80).unwrap();
        let ip2 = allocator.allocate("service1", "default", 80).unwrap();

        assert_eq!(ip1, ip2); // Should reuse same IP

        let ip3 = allocator.allocate("service2", "default", 80).unwrap();
        assert_ne!(ip1, ip3); // Different service should get different IP
    }

    #[test]
    fn test_deallocate_and_reuse() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip1 = allocator.allocate("service1", "default", 80).unwrap();
        allocator.deallocate("service1", "default", 80);

        let ip2 = allocator.allocate("service2", "default", 80).unwrap();
        assert_eq!(ip1, ip2); // Should reuse deallocated IP
    }

    #[test]
    fn test_lookup() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip = allocator.allocate("service1", "default", 80).unwrap();
        let key = allocator.lookup_ip(&ip).unwrap();
        assert_eq!(key, "service1.default:80");
    }
}
