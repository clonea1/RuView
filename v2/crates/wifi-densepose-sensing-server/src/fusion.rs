//! Cross-node frame pairing, keyed on `(transmitter, 802.11 rx_seq)`.
//!
//! # Why this exists
//!
//! Fusing measurements from several receivers needs a way to say "these two
//! observations are of the *same event*". Everything else the server has fails
//! at that:
//!
//! * The `sequence` field is a counter private to each node. Two nodes that
//!   captured the same packet report unrelated numbers.
//! * Arrival timestamps at the sink measure network jitter, not capture time.
//!   Two nodes can be milliseconds apart on the wire for one transmission.
//! * The transmitter MAC (wire v2) identifies a *link*, not a *transmission* —
//!   it says which channel, not which packet.
//!
//! `rx_seq` is the 802.11 sequence control field, assigned by the transmitter
//! and identical at every receiver. So `(tx_mac, rx_seq)` names one
//! transmission fleet-wide with no coordination between receivers: no shared
//! clock, no negotiation, no round trip. That is the whole reason wire v3
//! exists.
//!
//! # What it measures
//!
//! For each transmission, how many distinct nodes captured it. That yields the
//! number that decides whether fusion is viable at all — not "does the code
//! run" but "do independent receivers actually hold measurements of the same
//! packet, and how often".
//!
//! Measured 2026-08-30 over serial with two boards side by side: 72% of frames
//! heard by one were heard by the other, but only 25% of frames that survived
//! each node's rate gate were common, because the gate has independent phase
//! per node. This module measures that ratio continuously across the whole
//! fleet instead of through a diagnostic build and a USB cable.
//!
//! # Windowing, and why a short one is correct
//!
//! `rx_seq` is 16 bits and wraps every 65536 frames, so a key is only unique
//! for a bounded time. Worse, a *fragment* number occupies the low 4 bits, so
//! the raw field wraps its sequence part every 4096 transmissions — a few
//! minutes on a busy channel. Entries therefore live for `WINDOW`, which is
//! far shorter than any wrap, and are finalised on eviction.
//!
//! A short window is not a limitation here: two nodes that captured the same
//! packet saw it within microseconds of each other. Anything arriving a second
//! later is a different transmission that happens to share a number.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a `(tx, rx_seq)` key stays open for other nodes to report it.
///
/// Receivers capture the same packet within microseconds; the spread we
/// actually tolerate is sink-side network jitter. 750 ms is generously beyond
/// that while staying far below any `rx_seq` wrap.
const WINDOW: Duration = Duration::from_millis(750);

/// Highest `node_id` tracked in the pairwise matrix. Nine boards today; the
/// matrix is `MAX_NODES^2` counters, so this is cheap to oversize and
/// expensive to under-size (a node above the bound is counted in the totals
/// but silently missing from pair statistics).
pub const MAX_NODES: usize = 16;

/// Cap on simultaneously open keys, so a misbehaving or spoofed transmitter
/// cannot grow this map without bound. At ~50 fps across 9 nodes a 750 ms
/// window holds a few hundred entries; 20k is far above any legitimate load.
const MAX_OPEN: usize = 20_000;

struct Open {
    first: Instant,
    /// Bitmask of node_ids that reported this transmission.
    nodes: u32,
    count: u8,
}

/// Rolling pairing statistics.
#[derive(Clone, Debug, Default)]
pub struct FusionStats {
    /// Frames offered that carried an `rx_seq` (i.e. wire v3).
    pub observations: u64,
    /// Distinct transmissions finalised.
    pub transmissions: u64,
    /// `by_receivers[n]` = transmissions captured by exactly `n` nodes.
    /// Index 0 is unused; index 1 is "only one node heard it".
    pub by_receivers: [u64; MAX_NODES + 1],
    /// `pairs[a][b]` (a < b) = transmissions captured by BOTH node a and b.
    /// This is the matrix that says which node pairs are actually fusable.
    pub pairs: [[u64; MAX_NODES]; MAX_NODES],
    /// Frames dropped because the open-key map was at capacity.
    pub overflow: u64,
}

impl FusionStats {
    /// Transmissions captured by two or more nodes — the fusable ones.
    pub fn paired(&self) -> u64 {
        self.by_receivers.iter().skip(2).sum()
    }

    /// Fraction of finalised transmissions that more than one node captured.
    ///
    /// This is the headline number. A fleet where every node hears its own
    /// private traffic scores near 0 no matter how healthy each node looks.
    pub fn paired_fraction(&self) -> f64 {
        if self.transmissions == 0 {
            0.0
        } else {
            self.paired() as f64 / self.transmissions as f64
        }
    }
}

/// Index of in-flight transmissions plus the statistics they finalise into.
pub struct FusionIndex {
    open: HashMap<([u8; 6], u16), Open>,
    stats: FusionStats,
}

impl Default for FusionIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl FusionIndex {
    pub fn new() -> Self {
        Self {
            open: HashMap::new(),
            stats: FusionStats::default(),
        }
    }

    /// Record that `node_id` captured the transmission `(tx, rx_seq)`.
    ///
    /// Frames without an `rx_seq` (wire v1/v2) must not be offered here: they
    /// have no transmission identity, and keying them on a placeholder would
    /// pair every older node's frames with every other node's.
    pub fn observe(&mut self, node_id: u8, tx: [u8; 6], rx_seq: u16, now: Instant) {
        self.stats.observations += 1;

        let key = (tx, rx_seq);
        if let Some(e) = self.open.get_mut(&key) {
            let bit = 1u32 << (node_id as usize).min(31);
            if (node_id as usize) < MAX_NODES && e.nodes & bit == 0 {
                e.nodes |= bit;
                e.count = e.count.saturating_add(1);
            }
            return;
        }

        if self.open.len() >= MAX_OPEN {
            self.stats.overflow += 1;
            return;
        }
        let bit = if (node_id as usize) < MAX_NODES {
            1u32 << node_id
        } else {
            0
        };
        self.open.insert(
            key,
            Open {
                first: now,
                nodes: bit,
                count: u8::from(bit != 0),
            },
        );
    }

    /// Finalise every key older than `WINDOW` into the statistics.
    ///
    /// Finalising on eviction rather than on arrival is what makes the counts
    /// meaningful: a transmission is only "seen by two nodes" once both have
    /// had the chance to report it.
    pub fn expire(&mut self, now: Instant) {
        let mut done: Vec<([u8; 6], u16)> = Vec::new();
        for (k, e) in self.open.iter() {
            if now.duration_since(e.first) >= WINDOW {
                done.push(*k);
            }
        }
        for k in done {
            if let Some(e) = self.open.remove(&k) {
                self.stats.transmissions += 1;
                let n = (e.count as usize).min(MAX_NODES);
                self.stats.by_receivers[n] += 1;
                if e.count >= 2 {
                    for a in 0..MAX_NODES {
                        if e.nodes & (1 << a) == 0 {
                            continue;
                        }
                        for b in (a + 1)..MAX_NODES {
                            if e.nodes & (1 << b) != 0 {
                                self.stats.pairs[a][b] += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn stats(&self) -> &FusionStats {
        &self.stats
    }

    /// Keys still awaiting other receivers.
    pub fn open_len(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX: [u8; 6] = [0x8c, 0x30, 0x66, 0x86, 0xa4, 0x21];

    fn drained(idx: &mut FusionIndex, t0: Instant) -> FusionStats {
        idx.expire(t0 + WINDOW + Duration::from_millis(1));
        idx.stats().clone()
    }

    #[test]
    fn two_nodes_capturing_one_transmission_pair() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        idx.observe(1, TX, 0x1234, t0);
        idx.observe(5, TX, 0x1234, t0 + Duration::from_millis(3));
        let s = drained(&mut idx, t0);
        assert_eq!(s.transmissions, 1);
        assert_eq!(s.by_receivers[2], 1, "one transmission, two receivers");
        assert_eq!(s.pairs[1][5], 1, "pair (1,5) credited");
        assert_eq!(s.pairs[1][2], 0, "uninvolved pairs untouched");
        assert!((s.paired_fraction() - 1.0).abs() < 1e-9);
    }

    /// The failure this whole layer is built to detect: every node healthy,
    /// every node capturing, and no two of them ever holding the same packet.
    #[test]
    fn disjoint_captures_score_zero_despite_healthy_nodes() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        for i in 0..50u16 {
            idx.observe(1, TX, i * 2, t0);
            idx.observe(2, TX, i * 2 + 1, t0);
        }
        let s = drained(&mut idx, t0);
        assert_eq!(s.observations, 100, "both nodes were busy");
        assert_eq!(s.transmissions, 100);
        assert_eq!(s.paired(), 0, "but nothing is fusable");
        assert_eq!(s.paired_fraction(), 0.0);
    }

    /// A node reporting the same key twice (a retransmission, or the same
    /// frame counted on two paths) must not look like a second receiver.
    #[test]
    fn duplicate_report_from_one_node_is_not_a_pair() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        idx.observe(3, TX, 7, t0);
        idx.observe(3, TX, 7, t0 + Duration::from_millis(10));
        let s = drained(&mut idx, t0);
        assert_eq!(s.by_receivers[1], 1);
        assert_eq!(s.paired(), 0);
    }

    /// Same key, but far enough apart to be a different transmission after an
    /// rx_seq wrap. Counting those as a pair would inflate the headline number
    /// with coincidences.
    #[test]
    fn same_key_outside_the_window_is_a_separate_transmission() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        idx.observe(1, TX, 42, t0);
        idx.expire(t0 + WINDOW + Duration::from_millis(1));
        idx.observe(2, TX, 42, t0 + WINDOW + Duration::from_millis(2));
        idx.expire(t0 + WINDOW * 3);
        let s = idx.stats().clone();
        assert_eq!(s.transmissions, 2, "two separate transmissions");
        assert_eq!(s.paired(), 0, "not paired across the window boundary");
    }

    #[test]
    fn different_transmitters_never_pair() {
        let t0 = Instant::now();
        let other = [0x9e, 0x30, 0x66, 0x86, 0xa4, 0x21];
        let mut idx = FusionIndex::new();
        idx.observe(1, TX, 99, t0);
        idx.observe(2, other, 99, t0);
        let s = drained(&mut idx, t0);
        assert_eq!(s.transmissions, 2);
        assert_eq!(s.paired(), 0, "same seq from different transmitters is not one event");
    }

    #[test]
    fn three_receivers_credit_all_three_pairs() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        for n in [0u8, 4, 7] {
            idx.observe(n, TX, 5, t0);
        }
        let s = drained(&mut idx, t0);
        assert_eq!(s.by_receivers[3], 1);
        assert_eq!(s.pairs[0][4], 1);
        assert_eq!(s.pairs[0][7], 1);
        assert_eq!(s.pairs[4][7], 1);
        assert_eq!(s.paired(), 1, "one transmission, not three");
    }

    #[test]
    fn open_key_map_is_bounded() {
        let t0 = Instant::now();
        let mut idx = FusionIndex::new();
        for i in 0..(MAX_OPEN as u32 + 500) {
            let tx = [(i >> 8) as u8, i as u8, 0, 0, 0, 0];
            idx.observe(1, tx, i as u16, t0);
        }
        assert!(idx.open_len() <= MAX_OPEN);
        assert!(idx.stats().overflow > 0, "overflow is counted, not silent");
    }
}
