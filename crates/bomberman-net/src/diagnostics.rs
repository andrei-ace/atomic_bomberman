//! Credential-free, read-only gameplay telemetry. No transport tuning lives here.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrafficCounters {
    pub received_packets: u64,
    pub received_bytes: u64,
    pub sent_packets: u64,
    pub sent_bytes: u64,
    pub send_failures: u64,
    pub queue_evictions: u64,
    pub queue_rejections: u64,
    pub superseded_packets: u64,
    pub send_buffer_pressure: u64,
    pub incoming_high_water: usize,
    pub outgoing_high_water: usize,
}

/// Fixed memory, 100us buckets up to 100ms; the last bucket is overflow.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ForwardingHistogram {
    pub buckets: Vec<u64>,
    pub samples: u64,
    pub max_us: u64,
}
impl ForwardingHistogram {
    pub fn record(&mut self, elapsed: Duration) {
        if self.buckets.is_empty() {
            self.buckets.resize(1001, 0);
        }
        let us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.buckets[(us / 100).min(1000) as usize] += 1;
        self.samples += 1;
        self.max_us = self.max_us.max(us);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransportSnapshot {
    pub udp_tx_packets: u64,
    pub udp_tx_bytes: u64,
    pub udp_rx_packets: u64,
    pub udp_rx_bytes: u64,
    pub rtt_us: u64,
    pub congestion_window: u64,
    pub congestion_events: u64,
    /// QUIC sender estimate; this is not receiver/application packet loss.
    pub sender_lost_packets: u64,
    pub sender_sent_packets: u64,
    pub send_buffer_free: usize,
}
impl TransportSnapshot {
    pub(crate) fn of(connection: &quinn::Connection) -> Self {
        let stats = connection.stats();
        Self {
            udp_tx_packets: stats.udp_tx.datagrams,
            udp_tx_bytes: stats.udp_tx.bytes,
            udp_rx_packets: stats.udp_rx.datagrams,
            udp_rx_bytes: stats.udp_rx.bytes,
            rtt_us: stats.path.rtt.as_micros() as u64,
            congestion_window: stats.path.cwnd,
            congestion_events: stats.path.congestion_events,
            sender_lost_packets: stats.path.lost_packets,
            sender_sent_packets: stats.path.sent_packets,
            send_buffer_free: connection.datagram_send_buffer_space(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameplayDiagnostics {
    pub room_tag: String,
    pub peer: u32,
    pub generation: u64,
    pub traffic: TrafficCounters,
    pub transport: TransportSnapshot,
    pub forwarding: ForwardingHistogram,
}

/// Warn once per gap and at most once per second. A generation must first
/// carry gameplay data; lobby probes alone never arm the monitor.
#[derive(Default)]
pub(crate) struct ReceiveGap {
    last: Option<Instant>,
    warned: bool,
    last_warning: Option<Instant>,
}
impl ReceiveGap {
    pub(crate) fn received(&mut self, now: Instant) -> Option<u64> {
        let recovery = self
            .last
            .filter(|_| self.warned)
            .map(|last| now.duration_since(last).as_millis() as u64);
        self.last = Some(now);
        self.warned = false;
        recovery
    }
    pub(crate) fn check(&mut self, now: Instant) -> Option<u64> {
        let gap = now.duration_since(self.last?);
        if gap > Duration::from_millis(250)
            && !self.warned
            && self
                .last_warning
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
        {
            self.warned = true;
            self.last_warning = Some(now);
            Some(gap.as_millis() as u64)
        } else {
            None
        }
    }
}

pub fn log_record(value: &serde_json::Value) {
    // Callers construct records from an allowlist, never arbitrary packets,
    // invitations, errors with endpoints, or Debug implementations.
    eprintln!("{value}");
}
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaps_ignore_idle_and_warn_once_then_report_recovery() {
        let t = Instant::now();
        let mut monitor = ReceiveGap::default();
        assert_eq!(monitor.check(t + Duration::from_secs(20)), None);
        monitor.received(t);
        assert_eq!(monitor.check(t + Duration::from_millis(250)), None);
        assert_eq!(monitor.check(t + Duration::from_millis(251)), Some(251));
        assert_eq!(monitor.check(t + Duration::from_secs(2)), None);
        assert_eq!(monitor.received(t + Duration::from_secs(2)), Some(2000));
    }
}
