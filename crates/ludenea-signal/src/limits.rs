//! What the listener grants a client, and the count of refused joins per address.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// The bounds the listener holds every client to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// The rooms the listener holds at once; a host beyond them is refused with 503.
    pub max_rooms: usize,
    /// The rooms one client address may host at once; beyond them it is refused with 429.
    /// An address is the TCP peer's: an IPv4 address, or an IPv6 address by its /64 network.
    pub rooms_per_address: usize,
    /// The refused joins (404, 409 and 410) after which an address is refused with 429 until
    /// a minute after its first refusal.
    pub refused_joins_per_minute: usize,
    /// How long a connection may go without sending a frame (a message, a keep-alive, a ping
    /// or a pong) before it is closed with 1001. A write to any client (a member, or one
    /// loading a page file) that stays blocked this long ends its connection too.
    pub idle: Duration,
    /// The largest message, and frame, a client may send; a larger one closes with 1009.
    pub max_message_bytes: usize,
    /// The messages queued for one member, at least 1; a member that falls further behind is
    /// closed with 1013.
    pub queue: usize,
    /// How long a client may take to send the headers of a request.
    pub header_timeout: Duration,
    /// The connections served at once, page loads and room members together, at least 1;
    /// further clients wait to be accepted until one ends.
    pub max_connections: usize,
}

impl Default for Limits {
    /// 64 rooms, 4 per address, 30 refused joins a minute, 30 seconds idle, messages of
    /// 16 KiB, 64 queued messages, 10 seconds for headers, and 512 connections: enough for
    /// every member of 64 full rooms and as many page loads.
    fn default() -> Limits {
        Limits {
            max_rooms: 64,
            rooms_per_address: 4,
            refused_joins_per_minute: 30,
            idle: Duration::from_secs(30),
            max_message_bytes: 16 * 1024,
            queue: 64,
            header_timeout: Duration::from_secs(10),
            max_connections: 512,
        }
    }
}

/// The address the per-address limits count a client under: an IPv4 address as is (also
/// when mapped into IPv6), an IPv6 address by its /64 network, which one client usually owns
/// whole.
pub(crate) fn client_key(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let network = u128::from(v6) & !((1u128 << 64) - 1);
                IpAddr::V6(network.into())
            }
        },
    }
}

/// How long the refusals of an address count.
const WINDOW: Duration = Duration::from_secs(60);

/// The addresses whose refusals are remembered at once.
// ponytail: a fixed cap with eviction of the oldest window, so a flood from many addresses
// (an IPv6 prefix) stays bounded at the price of forgetting some; count by prefix if that
// ever matters.
const MAX_ADDRESSES: usize = 4096;

/// The refused joins of each address, counted from the first one for [`WINDOW`].
#[derive(Debug, Default)]
pub(crate) struct Refusals {
    windows: BTreeMap<IpAddr, (Instant, usize)>,
}

impl Refusals {
    /// The refusals `address` collected in its current window at `now`.
    pub(crate) fn count(&self, address: IpAddr, now: Instant) -> usize {
        match self.windows.get(&address) {
            Some(&(start, count)) if now.saturating_duration_since(start) < WINDOW => count,
            _ => 0,
        }
    }

    /// Counts one more refusal of `address` at `now`.
    pub(crate) fn add(&mut self, address: IpAddr, now: Instant) {
        let count = self.count(address, now);
        if count == 0 {
            if self.windows.len() >= MAX_ADDRESSES && !self.windows.contains_key(&address) {
                self.windows
                    .retain(|_, &mut (start, _)| now.saturating_duration_since(start) < WINDOW);
            }
            if self.windows.len() >= MAX_ADDRESSES
                && let Some(oldest) = self
                    .windows
                    .iter()
                    .min_by_key(|(_, (start, _))| *start)
                    .map(|(address, _)| *address)
            {
                self.windows.remove(&oldest);
            }
            self.windows.insert(address, (now, 1));
        } else if let Some(window) = self.windows.get_mut(&address) {
            window.1 = count + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_counts_by_its_ipv4_address_or_its_ipv6_network() {
        let key = |text: &str| client_key(text.parse().unwrap());
        assert_eq!(key("10.0.0.1"), key("::ffff:10.0.0.1"));
        assert_ne!(key("10.0.0.1"), key("10.0.0.2"));
        assert_eq!(key("2001:db8:1:2::1"), key("2001:db8:1:2:ffff::9"));
        assert_eq!(
            key("2001:db8:1:2::1"),
            "2001:db8:1:2::".parse::<IpAddr>().unwrap()
        );
        assert_ne!(key("2001:db8:1:2::1"), key("2001:db8:1:3::1"));
    }

    #[test]
    fn refusals_count_for_a_minute_from_the_first() {
        let mut refusals = Refusals::default();
        let address: IpAddr = "10.0.0.1".parse().unwrap();
        #[expect(clippy::disallowed_methods, reason = "a test of network pacing")]
        let start = Instant::now();
        refusals.add(address, start);
        refusals.add(address, start + Duration::from_secs(59));
        assert_eq!(refusals.count(address, start + Duration::from_secs(59)), 2);
        assert_eq!(refusals.count(address, start + WINDOW), 0);
        refusals.add(address, start + WINDOW);
        assert_eq!(refusals.count(address, start + WINDOW), 1);
    }

    #[test]
    fn the_remembered_addresses_are_bounded() {
        let mut refusals = Refusals::default();
        #[expect(clippy::disallowed_methods, reason = "a test of network pacing")]
        let start = Instant::now();
        for index in 0..MAX_ADDRESSES + 10 {
            let address = IpAddr::from(u32::try_from(index).unwrap().to_be_bytes());
            refusals.add(address, start + Duration::from_millis(index as u64));
        }
        assert_eq!(refusals.windows.len(), MAX_ADDRESSES);
        // The oldest windows made room for the newest.
        assert_eq!(refusals.count(IpAddr::from([0, 0, 0, 0]), start), 0);
        let newest = IpAddr::from(u32::try_from(MAX_ADDRESSES + 9).unwrap().to_be_bytes());
        assert_eq!(refusals.count(newest, start + WINDOW / 2), 1);
    }
}
