use std::net::IpAddr;

/// Where the `Sharer` learns this machine's LAN address.
/// A one-method trait so tests can pin a deterministic address
/// instead of depending on the host's actual network setup.
pub trait LanAddress: Send + Sync {
    fn local_ip(&self) -> std::io::Result<IpAddr>;
}

/// Production implementation backed by the `local_ip_address` crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLanAddress;

impl LanAddress for SystemLanAddress {
    fn local_ip(&self) -> std::io::Result<IpAddr> {
        local_ip_address::local_ip()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not run by default: depends on the host having a real network.
    /// Run manually on your machine with: cargo test -- --ignored
    #[test]
    #[ignore = "needs a real network interface"]
    fn system_detector_finds_a_usable_ip() {
        let ip = SystemLanAddress.local_ip().expect("no local ip found");
        assert!(!ip.is_unspecified());
        assert!(!ip.is_loopback());
    }
}
