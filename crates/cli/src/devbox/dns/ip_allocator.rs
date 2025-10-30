use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr},
};

/// Allocates synthetic loopback IPs from a reserved subnet (127.77.0.0/16)
pub struct SyntheticIpAllocator {
    /// Base IP: 127.77.0.0
    base: Ipv4Addr,
    /// Next available offset (0-65535) tracked across the last two octets
    next_offset: u16,
    /// Allocated IPs mapped to their service keys (name.namespace)
    allocated: HashMap<String, IpAddr>,
    /// Reverse mapping from IP to service key
    reverse: HashMap<IpAddr, String>,
    /// Freed IPs that can be reused
    free_pool: HashSet<u16>,
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
            next_offset: 1, // Start from offset 1, skip 0
            allocated: HashMap::new(),
            reverse: HashMap::new(),
            free_pool: HashSet::new(),
        }
    }

    /// Allocate or reuse an IP for a service across all ports
    pub fn allocate(&mut self, service_name: &str, namespace: &str) -> Option<IpAddr> {
        let key = format!("{}.{}", service_name, namespace);

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

        let base_octets = self.base.octets();
        let high = (offset >> 8) as u8;
        let low = (offset & 0xFF) as u8;
        let ip = IpAddr::V4(Ipv4Addr::new(
            base_octets[0],
            base_octets[1],
            base_octets[2].wrapping_add(high),
            low,
        ));

        self.allocated.insert(key.clone(), ip);
        self.reverse.insert(ip, key);
        Some(ip)
    }

    /// Deallocate an IP and return it to the free pool
    pub fn deallocate(&mut self, service_name: &str, namespace: &str) {
        let key = format!("{}.{}", service_name, namespace);
        if let Some(ip) = self.allocated.remove(&key) {
            self.reverse.remove(&ip);
            if let IpAddr::V4(ipv4) = ip {
                let ip_octets = ipv4.octets();
                let base_octets = self.base.octets();
                let high = (ip_octets[2] as u16).saturating_sub(base_octets[2] as u16);
                let low = ip_octets[3] as u16;
                let offset = (high << 8) | low;
                if offset != 0 {
                    self.free_pool.insert(offset);
                }
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

        let ip1 = allocator.allocate("service1", "default").unwrap();
        let ip2 = allocator.allocate("service1", "default").unwrap();

        assert_eq!(ip1, ip2); // Should reuse same IP

        let ip3 = allocator.allocate("service2", "default").unwrap();
        assert_ne!(ip1, ip3); // Different service should get different IP
    }

    #[test]
    fn test_deallocate_and_reuse() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip1 = allocator.allocate("service1", "default").unwrap();
        allocator.deallocate("service1", "default");

        let ip2 = allocator.allocate("service2", "default").unwrap();
        assert_eq!(ip1, ip2); // Should reuse deallocated IP
    }

    #[test]
    fn test_lookup() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip = allocator.allocate("service1", "default").unwrap();
        let key = allocator.lookup_ip(&ip).unwrap();
        assert_eq!(key, "service1.default");
    }

    #[test]
    fn test_multi_port_same_ip() {
        let mut allocator = SyntheticIpAllocator::new();

        let ip_http = allocator.allocate("service1", "default").unwrap();
        let ip_https = allocator.allocate("service1", "default").unwrap();

        assert_eq!(ip_http, ip_https);
    }

    #[test]
    fn test_allocate_beyond_24_bit_range() {
        let mut allocator = SyntheticIpAllocator::new();

        for i in 0..300 {
            let service = format!("service{}", i);
            assert!(
                allocator.allocate(&service, "default").is_some(),
                "allocation failed at index {i}"
            );
        }
    }
}
