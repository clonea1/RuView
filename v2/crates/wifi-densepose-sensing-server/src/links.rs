//! Per-link CSI state, keyed by (receiver, transmitter).
//!
//! The rest of the server models one link per node: `NodeState` holds a single
//! `frame_history` per `node_id`. That was accurate while the only labelled
//! traffic was AP -> node. It is not accurate for what the radios actually
//! hear: a node in promiscuous MGMT+DATA mode receives CSI from the AP, from
//! every peer node's ESP-NOW beacon, and from unrelated household traffic —
//! measured 2026-08-28 at roughly 75% non-AP on a normal home channel.
//!
//! Interleaving those into one history mixes links with completely different
//! geometry. Until wire v2 carried the transmitter MAC there was no way to
//! separate them, so the only remedy was `filter_mac`, which fixes the mixing
//! by throwing ~75% of the measurements away.
//!
//! This module keeps them instead. Each (receiver, transmitter) pair is its own
//! link with its own history and its own motion metric.
//!
//! # Why this matters for position
//!
//! Three AP -> node links leave a single distant transmitter within a ~36 degree
//! fan and converge on the node cluster, which is very little parallax to
//! localize with. The node <-> node links between the same three boards cross
//! the room from three sides, ~109 degrees apart. Link count also grows as
//! `N*(N-1)/2`, so adding boards adds links quadratically: 3 nodes give 3
//! links, 6 give 15, 13 give 78.
//!
//! Everything here is **amplitude-derived**. That is deliberate: CSI phase on
//! this hardware was measured to be uniform-random per packet (see
//! `phase_diag`), so nothing that depends on phase coherence can be built on
//! it. Link-based localization needs only per-link signal perturbation.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// One RF link: frames received by `rx_node` that were sent by `tx_mac`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct LinkId {
    pub rx_node: u8,
    pub tx_mac: [u8; 6],
}

impl LinkId {
    /// Compact label for logs: receiver plus the last three octets of the
    /// transmitter, which is enough to tell the AP from peers and neighbours.
    pub fn label(&self) -> String {
        format!(
            "{}<-{:02x}:{:02x}:{:02x}",
            self.rx_node, self.tx_mac[3], self.tx_mac[4], self.tx_mac[5]
        )
    }
}

/// How many amplitude frames to retain per link.
///
/// At the ~35-50 fps a single transmitter sustains this is 1.3-1.8 s, enough
/// for a motion statistic without holding a large multiple of the node count in
/// memory once link counts grow quadratically.
const LINK_HISTORY: usize = 32;

/// Maximum links tracked at once. A busy channel presents many transmitters and
/// most are irrelevant; this is a sensing surface, not a registry. Links are
/// admitted first-come and expired by staleness, so a full table still turns
/// over rather than wedging permanently.
///
/// Raised 256 -> 1024 on 2026-09-01. Until then the per-node grid gate in
/// `main.rs` discarded every transmitter that did not match the AP's format, so
/// the table only ever held ~30 links. With that gate moved per-link, a node
/// admits everything it hears: MEASURED 24 transmitters at one node, so nine
/// nodes is ~216 links before churn — inside 256, but with no headroom, and
/// admission fails SILENTLY when full. Memory is the trade: a link holds up to
/// `LINK_HISTORY` amplitude vectors, so 1024 links of 64-bin frames is ~17 MB
/// and the realistic mixed case is well under 30 MB.
pub const MAX_LINKS: usize = 1024;

/// A link with no frame for this long is dropped, freeing its slot.
const LINK_STALE_AFTER: Duration = Duration::from_secs(30);

/// EMA rate for a link's resting-state baseline. Matches the amplitude
/// pipeline's existing slow-baseline approach rather than inventing a second
/// convention.
const BASELINE_ALPHA: f64 = 0.02;

/// How far above its own resting level a link may sit and still update the
/// baseline. Above this the baseline holds — see the freeze rationale in
/// `LinkTable::observe`.
const BASELINE_FREEZE_RATIO: f64 = 1.5;

/// Fraction of the learned baseline subtracted from the raw metric. Mirrors
/// `BASELINE_SUBTRACTION_FRACTION` in the per-node path, which live testing
/// moved from 0.7 to 0.85 after 30% of the noise floor kept surviving as
/// residual "motion" in an empty room.
const BASELINE_SUBTRACTION: f64 = 0.85;

/// Frames required before a link reports a motion metric at all.
const MIN_FRAMES_FOR_METRIC: usize = 8;

/// Which percentile over subcarriers `agc_normalised_motion` reports. See the
/// rationale at the aggregation step — the mean dilutes a minority response.
///
/// Note the sensitivity floor this implies: a nearest-rank p90 only moves when
/// **more than 10% of subcarriers** respond. A perturbation confined to fewer
/// bins than that lands below the rank and reads as quiet, exactly as the
/// `p90_is_not_the_maximum...` test asserts. That is the intended trade — it is
/// what makes the statistic robust to a single noisy bin — but it means the
/// metric is tuned for a body-sized response across the grid, not a pinpoint one.
const MOTION_PERCENTILE: f64 = 0.90;

#[derive(Debug, Clone)]
struct LinkState {
    history: VecDeque<Vec<f64>>,
    /// Arrival time of each frame in `history`, pushed and popped in lockstep.
    ///
    /// `history` is a fixed 64-frame window, so its *wall-clock span* is set by
    /// how fast the link is currently delivering. Measured on the 2026-08-28
    /// baseline, per-link delivery swung 0.70x to 4.83x within one session
    /// (one link went 1.79 -> 8.66 fps), which stretched or shrank the window
    /// behind `raw_motion` and `rssi` by the same factor. Two readings of the
    /// same link are therefore only comparable when their spans are — so the
    /// span is measured and reported rather than assumed constant.
    stamps: VecDeque<Instant>,
    last_seen: Instant,
    frames: u64,
    rssi_ema: f64,
    baseline: f64,
    baseline_samples: u64,
    /// Subcarrier width this link's history is locked to. 0 until the first
    /// frame. Densest-wins: a denser frame clears the history and re-locks.
    grid: usize,
    /// Frames dropped for arriving on a sparser grid than `grid`. Counted, not
    /// silent — an invisible drop is what hid every foreign transmitter until
    /// 2026-09-01.
    sparser_skipped: u64,
}

impl LinkState {
    /// Wall-clock seconds between the oldest and newest frame in the window.
    fn window_span_s(&self) -> f64 {
        match (self.stamps.front(), self.stamps.back()) {
            (Some(a), Some(b)) => b.duration_since(*a).as_secs_f64(),
            _ => 0.0,
        }
    }

    /// Delivery rate over the window. `n - 1` intervals span `n` frames, and
    /// a zero span (a single frame, or several inside the clock's resolution)
    /// has no rate to report.
    fn delivery_fps(&self) -> f64 {
        let span = self.window_span_s();
        if span <= 0.0 || self.stamps.len() < 2 {
            return 0.0;
        }
        (self.stamps.len() as f64 - 1.0) / span
    }
}

/// One row of [`LinkTable::inventory`] — a link as the table holds it, before
/// any scoring decides whether it is renderable.
#[derive(Debug, Clone)]
pub struct LinkInventory {
    pub id: LinkId,
    pub frames: u64,
    pub rssi: f64,
    /// Frames currently in the rolling history (cap `LINK_HISTORY`).
    pub history_len: usize,
    /// `(subcarrier_width, frames)` seen in that history, most frequent first.
    /// A transmitter that interleaves PPDU formats shows several entries.
    pub widths: Vec<(usize, usize)>,
    pub modal_width: usize,
    pub frames_at_modal: usize,
    /// Whether this link currently survives into `metrics()`.
    pub visible: bool,
    /// Why not, when `visible` is false.
    pub reason: &'static str,
    pub age_ms: u64,
    pub baseline_samples: u64,
    /// Delivery rate over the link's own window. The single best triage
    /// number: a loud transmitter at 0.02 fps cannot form a history and will
    /// never be usable, however strong it looks.
    pub fps: f64,
    /// Wall-clock span of the history window, seconds. `fps` is only
    /// comparable between links whose spans are comparable.
    pub window_span_s: f64,
    /// Subcarrier width this link is locked to.
    pub grid: usize,
    /// Frames refused for arriving on a sparser grid.
    pub sparser_skipped: u64,
}

/// Per-link motion, as reported to callers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkMetric {
    pub id: LinkId,
    /// Frames observed on this link since it was admitted.
    pub frames: u64,
    /// EMA of received signal strength, dBm.
    pub rssi: f64,
    /// Raw perturbation: p90 over subcarriers of the temporal standard
    /// deviation of AGC-normalised amplitude.
    pub raw_motion: f64,
    /// `raw_motion` with the link's learned resting baseline subtracted, floored
    /// at zero. This is the quantity a localiser should consume.
    pub motion: f64,
    /// The link's learned resting level. Exposed because it is the only scale
    /// on which links of wildly different intrinsic sensitivity can be
    /// compared: measured live, an AP link rests near 20 while a peer link
    /// rests near 1.3, and a solver fed raw values lets the loud links decide
    /// everything. See `rti::normalise_response`.
    pub baseline: f64,
    /// How many frames the baseline EMA has seen. Below a few hundred it has
    /// not converged and the normalised response it produces is unreliable.
    pub baseline_samples: u64,
    /// Wall-clock seconds spanned by the frames currently in the window.
    ///
    /// `raw_motion` is a statistic over that span, so this is the quantity
    /// that makes two samples comparable. A large change here means the
    /// window moved, not the room.
    pub window_span_s: f64,
    /// Current delivery rate on this link, frames per second, measured over
    /// the window rather than assumed. A link whose rate collapses while its
    /// peers surge is showing transport contention, not motion — on the
    /// 2026-08-28 baseline that pattern produced the night's *largest*
    /// apparent motion excursions with nobody in the room.
    pub fps: f64,
}

/// All currently tracked links.
#[derive(Debug, Default)]
pub struct LinkTable {
    links: BTreeMap<LinkId, LinkState>,
}

/// High-percentile over subcarriers of the temporal standard deviation, after
/// removing each frame's own mean across subcarriers.
///
/// The per-frame mean removal is essential, not cosmetic: the ESP32 applies
/// automatic gain control per packet, so the absolute amplitude level jumps
/// between packets for reasons that have nothing to do with the channel. A
/// statistic built on the raw level is dominated by AGC. Removing each frame's
/// mean cancels that scalar gain and leaves the response *shape*, whose
/// variation over time is real channel perturbation.
pub fn agc_normalised_motion(history: &VecDeque<Vec<f64>>) -> Option<f64> {
    if history.len() < MIN_FRAMES_FOR_METRIC {
        return None;
    }
    // Lock to the *most common* width in the window, not the newest frame's.
    //
    // A C6 on an 11ax AP interleaves formats: measured upstream at 84% HE-SU
    // (256 bins, 78.125 kHz tone spacing) with an HT minority (64 bins,
    // 312.5 kHz) — see ruvnet/RuView#1005. Keying off `history.back()` meant
    // that whenever the newest frame happened to be one of the HT minority,
    // every HE frame in the window was discarded and the metric was computed
    // from the handful of HT frames instead. Those sample a different grid at
    // a quarter the frequency resolution, so the number that came out was not
    // a noisier version of the same measurement — it was a different one,
    // reported under the same name, in roughly one sample in six.
    //
    // The mode follows a genuine, lasting format change (the window refills
    // with the new width and the mode moves) while ignoring a transient
    // minority, which is what "lock the densest grid, re-warm on upgrade"
    // amounts to without needing an explicit upgrade state machine.
    let mut width_counts: BTreeMap<usize, usize> = BTreeMap::new();
    for f in history.iter() {
        if !f.is_empty() {
            *width_counts.entry(f.len()).or_insert(0) += 1;
        }
    }
    // Ties break toward the denser grid: it carries more information, and on
    // this hardware the dense format is the one the AP actually negotiated.
    let n_sc = width_counts
        .iter()
        .max_by_key(|(width, count)| (**count, **width))
        .map(|(width, _)| *width)?;
    if n_sc == 0 {
        return None;
    }
    let frames: Vec<&Vec<f64>> = history.iter().filter(|f| f.len() == n_sc).collect();
    if frames.len() < MIN_FRAMES_FOR_METRIC {
        return None;
    }

    let normalised: Vec<Vec<f64>> = frames
        .iter()
        .map(|f| {
            let mean = f.iter().sum::<f64>() / n_sc as f64;
            f.iter().map(|a| a - mean).collect()
        })
        .collect();

    let n = normalised.len() as f64;
    let mut per_sc: Vec<f64> = Vec::with_capacity(n_sc);
    for sc in 0..n_sc {
        let mean: f64 = normalised.iter().map(|f| f[sc]).sum::<f64>() / n;
        let var: f64 = normalised
            .iter()
            .map(|f| {
                let d = f[sc] - mean;
                d * d
            })
            .sum::<f64>()
            / n;
        per_sc.push(var.sqrt());
    }

    // High percentile over subcarriers, not the mean.
    //
    // A body perturbs a *minority* of subcarriers strongly and leaves the rest
    // near noise, so any statistic averaged across all bins is diluted by the
    // unperturbed majority and floors close to the resting level. Measured
    // upstream on C6 HE20 256-bin captures (ruvnet/RuView#1015): median |z|
    // moved 0.40 -> 0.75 between an empty room and a person moving, while p90
    // |z| moved 1.30 -> 2.27 on the same data. Same signal, roughly three
    // times the separation, because the percentile follows the bins that
    // actually responded.
    //
    // p90 rather than the max: the maximum is a single bin and rides on
    // whatever noise spike happened to be largest, whereas the 90th percentile
    // still requires ~10% of the grid to agree. Nearest-rank, so it is a real
    // observed value and needs no interpolation.
    per_sc.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((MOTION_PERCENTILE * (per_sc.len() as f64 - 1.0)).round() as usize)
        .min(per_sc.len() - 1);
    Some(per_sc[idx])
}

impl LinkTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one frame against its link.
    ///
    /// Frames with no transmitter identity (wire v1) must not be passed here —
    /// they cannot be attributed to a link, and inventing a placeholder key
    /// would silently recreate the mixing this module exists to undo.
    pub fn observe(
        &mut self,
        rx_node: u8,
        tx_mac: [u8; 6],
        amplitudes: &[f64],
        rssi: i8,
        now: Instant,
    ) {
        if amplitudes.is_empty() {
            return;
        }
        let id = LinkId { rx_node, tx_mac };
        let known = self.links.contains_key(&id);
        if !known && self.links.len() >= MAX_LINKS {
            return; // table full; expiry will free slots
        }
        let st = self.links.entry(id).or_insert_with(|| LinkState {
            history: VecDeque::with_capacity(LINK_HISTORY),
            stamps: VecDeque::with_capacity(LINK_HISTORY),
            last_seen: now,
            frames: 0,
            rssi_ema: rssi as f64,
            baseline: 0.0,
            baseline_samples: 0,
            grid: 0,
            sparser_skipped: 0,
        });
        // ── Per-link subcarrier-grid policy (ADR-110 / issue #1005) ──────────
        //
        // Same rule the per-node gate applied, applied where it belongs. A
        // 64-bin HT frame and a 256-bin HE frame sample different frequency
        // grids (312.5 kHz against 78.125 kHz tone spacing); differencing them
        // is not a noisier measurement, it is a different one reported under
        // the same name.
        //
        // Keyed per LINK rather than per node, because grid is a property of
        // the transmitter. Keying it by node locked every link to whatever the
        // associated AP negotiated and silently discarded every other
        // transmitter in the building — measured 2026-09-01, 24 transmitters
        // seen by a node, 10 admitted fleet-wide.
        //
        // Densest-wins, re-warm on upgrade: a denser grid carries more
        // information, so adopt it and drop the coarser history rather than
        // mixing. A sparser frame still refreshes liveness and RSSI — the link
        // is real and present — it simply contributes no sample.
        let width = amplitudes.len();
        if width > st.grid {
            st.history.clear();
            st.stamps.clear();
            st.grid = width;
            st.baseline = 0.0;
            st.baseline_samples = 0;
        }
        if width < st.grid {
            st.last_seen = now;
            st.frames += 1;
            st.rssi_ema = st.rssi_ema * 0.9 + rssi as f64 * 0.1;
            st.sparser_skipped += 1;
            return;
        }

        st.history.push_back(amplitudes.to_vec());
        st.stamps.push_back(now);
        if st.history.len() > LINK_HISTORY {
            st.history.pop_front();
        }
        if st.stamps.len() > LINK_HISTORY {
            st.stamps.pop_front();
        }
        st.last_seen = now;
        st.frames += 1;
        st.rssi_ema = st.rssi_ema * 0.9 + rssi as f64 * 0.1;

        if let Some(raw) = agc_normalised_motion(&st.history) {
            st.baseline_samples += 1;
            // Track fast while the baseline is still meaningless, then settle to
            // a slow EMA — same warm-up shape as the per-node amplitude path.
            if st.baseline_samples < 40 {
                st.baseline = st.baseline * 0.8 + raw * 0.2;
            } else if raw <= st.baseline * BASELINE_FREEZE_RATIO {
                // Freeze while the link is responding.
                //
                // BASELINE_ALPHA is a ~50-sample time constant, which at the
                // 6-16 fps a link actually runs is 3-8 seconds. An unfrozen
                // EMA therefore absorbs a stationary person into "resting"
                // within seconds, and the reported `motion` — which is
                // `raw - 0.85 * baseline` — decays back to zero while they are
                // still there. That makes the live signal a *change* detector
                // rather than a presence detector, and it is why a seated
                // human is invisible while a walking one is not.
                //
                // Updating only when the link is near rest keeps adaptation to
                // genuine environmental drift (furniture, temperature, a door
                // left open) while refusing to learn away the thing we are
                // trying to detect.
                st.baseline = st.baseline * (1.0 - BASELINE_ALPHA) + raw * BASELINE_ALPHA;
            }
        }
    }

    /// Drop links unseen for [`LINK_STALE_AFTER`].
    pub fn expire(&mut self, now: Instant) {
        self.links
            .retain(|_, st| now.duration_since(st.last_seen) < LINK_STALE_AFTER);
    }

    /// Current per-link metrics, strongest perturbation first.
    pub fn metrics(&self) -> Vec<LinkMetric> {
        let mut out: Vec<LinkMetric> = self
            .links
            .iter()
            .filter_map(|(id, st)| {
                let raw = agc_normalised_motion(&st.history)?;
                Some(LinkMetric {
                    id: *id,
                    frames: st.frames,
                    rssi: st.rssi_ema,
                    raw_motion: raw,
                    motion: (raw - st.baseline * BASELINE_SUBTRACTION).max(0.0),
                    baseline: st.baseline,
                    baseline_samples: st.baseline_samples,
                    window_span_s: st.window_span_s(),
                    fps: st.delivery_fps(),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.motion
                .partial_cmp(&a.motion)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Every link in the table, including the ones [`LinkTable::metrics`]
    /// omits, with the reason each one is missing.
    ///
    /// `metrics()` is a `filter_map` over [`agc_normalised_motion`], so a link
    /// that cannot yet produce a motion value vanishes from `/api/v1/links`,
    /// from `hearing_index`, and from the RTI observation set with no error, no
    /// counter and no log. MEASURED 2026-09-01: a node hears ~24 transmitters
    /// and the server renders 3 of them. That gap was invisible from every
    /// existing endpoint, which is exactly why this one exists — it reports
    /// what the table HOLDS rather than what survived scoring.
    ///
    /// Diagnosis is taken from `agc_normalised_motion` itself for `visible`,
    /// and the supporting fields are recomputed alongside it, so the two can
    /// never drift into disagreeing about the same link.
    pub fn inventory(&self, now: Instant) -> Vec<LinkInventory> {
        let mut out: Vec<LinkInventory> = self
            .links
            .iter()
            .map(|(id, st)| {
                let mut width_counts: BTreeMap<usize, usize> = BTreeMap::new();
                for f in st.history.iter() {
                    if !f.is_empty() {
                        *width_counts.entry(f.len()).or_insert(0) += 1;
                    }
                }
                let modal_width = width_counts
                    .iter()
                    .max_by_key(|(width, count)| (**count, **width))
                    .map(|(w, _)| *w)
                    .unwrap_or(0);
                let frames_at_modal = width_counts.get(&modal_width).copied().unwrap_or(0);

                let visible = agc_normalised_motion(&st.history).is_some();
                let reason = if visible {
                    "ok"
                } else if st.history.len() < MIN_FRAMES_FOR_METRIC {
                    "history_below_min"
                } else if modal_width == 0 {
                    "no_usable_width"
                } else if frames_at_modal < MIN_FRAMES_FOR_METRIC {
                    "modal_width_below_min"
                } else {
                    "motion_undefined"
                };

                let mut widths: Vec<(usize, usize)> = width_counts.into_iter().collect();
                widths.sort_by(|a, b| b.1.cmp(&a.1));

                LinkInventory {
                    id: *id,
                    frames: st.frames,
                    rssi: st.rssi_ema,
                    history_len: st.history.len(),
                    widths,
                    modal_width,
                    frames_at_modal,
                    visible,
                    reason,
                    age_ms: now.duration_since(st.last_seen).as_millis() as u64,
                    baseline_samples: st.baseline_samples,
                    fps: st.delivery_fps(),
                    window_span_s: st.window_span_s(),
                    grid: st.grid,
                    sparser_skipped: st.sparser_skipped,
                }
            })
            .collect();
        // Loudest first: the ones worth keeping as illuminators sort to the top.
        out.sort_by(|a, b| b.rssi.partial_cmp(&a.rssi).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

/// Which of our own nodes transmitted `tx_mac`, inferred from who did *not*
/// hear it.
///
/// Nothing in the system records a node's own MAC — a node reports the
/// addresses it hears, never its own. A radio does not receive its own
/// transmissions, though, and that absence is the signature: given the set of
/// receivers currently reporting links, a transmitter heard by all of them
/// except exactly one is that one receiver's own board. A transmitter heard by
/// every receiver is external infrastructure.
///
/// This is an inference and callers must present it as one. Limits, all of
/// which return `None` rather than a guess:
///
/// - Fewer than three receivers. With two, "heard by all but one" and "heard by
///   exactly one" are the same observation, so the signature carries nothing.
/// - More than one receiver missing — a peer out of range, or newly booted,
///   looks identical to a peer that is simply not ours.
///
/// A wrong answer here mislabels a display row. It never feeds a metric.
pub fn infer_transmitting_node(
    tx_mac: &[u8; 6],
    receivers: &std::collections::BTreeSet<u8>,
    heard_by: &std::collections::BTreeMap<[u8; 6], std::collections::BTreeSet<u8>>,
) -> Option<u8> {
    if receivers.len() < 3 {
        return None;
    }
    let heard = heard_by.get(tx_mac)?;
    let mut missing = receivers.difference(heard);
    let candidate = *missing.next()?;
    // Exactly one receiver missing, or the signature does not hold.
    missing.next().is_none().then_some(candidate)
}

/// Receivers currently reporting, and which receivers heard each transmitter —
/// the two indexes [`infer_transmitting_node`] needs.
#[allow(clippy::type_complexity)]
pub fn hearing_index(
    metrics: &[LinkMetric],
) -> (
    std::collections::BTreeSet<u8>,
    std::collections::BTreeMap<[u8; 6], std::collections::BTreeSet<u8>>,
) {
    let mut receivers = std::collections::BTreeSet::new();
    let mut heard_by: std::collections::BTreeMap<[u8; 6], std::collections::BTreeSet<u8>> =
        std::collections::BTreeMap::new();
    for m in metrics {
        receivers.insert(m.id.rx_node);
        heard_by.entry(m.id.tx_mac).or_default().insert(m.id.rx_node);
    }
    (receivers, heard_by)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AP: [u8; 6] = [0x8c, 0x30, 0x66, 0x86, 0xa4, 0x21];
    const PEER: [u8; 6] = [0xe8, 0xf6, 0x0a, 0xfc, 0xb2, 0xa8];

    fn steady(n_sc: usize, level: f64) -> Vec<f64> {
        vec![level; n_sc]
    }

    /// AGC changes the whole frame's level at once. A motion metric must not
    /// respond to that, or it reports gain changes as movement.
    #[test]
    fn per_frame_gain_changes_do_not_register_as_motion() {
        let mut h = VecDeque::new();
        for i in 0..32 {
            // Same shape, wildly different level each frame.
            let level = 10.0 + (i % 7) as f64 * 25.0;
            h.push_back(steady(56, level));
        }
        let m = agc_normalised_motion(&h).expect("defined");
        assert!(
            m < 1e-9,
            "constant shape under varying gain must read as no motion, got {m}"
        );
    }

    /// A changing response *shape* is real channel perturbation and must
    /// register even if the frame mean stays constant.
    #[test]
    fn shape_change_registers_as_motion() {
        let mut h = VecDeque::new();
        for i in 0..32 {
            // Mean held at 10.0; the tilt across subcarriers varies.
            let tilt = ((i % 5) as f64 - 2.0) * 0.5;
            let f: Vec<f64> = (0..56)
                .map(|k| 10.0 + tilt * (k as f64 - 27.5) / 27.5)
                .collect();
            h.push_back(f);
        }
        let m = agc_normalised_motion(&h).expect("defined");
        assert!(m > 0.05, "shape variation must register, got {m}");
    }

    #[test]
    fn insufficient_history_yields_no_metric() {
        let mut h = VecDeque::new();
        for _ in 0..(MIN_FRAMES_FOR_METRIC - 1) {
            h.push_back(steady(56, 10.0));
        }
        assert!(agc_normalised_motion(&h).is_none());
    }

    /// The whole point of the module: two transmitters heard by one receiver
    /// are two separate links, not one mixed history.
    #[test]
    fn two_transmitters_on_one_receiver_are_separate_links() {
        let mut t = LinkTable::new();
        let now = Instant::now();
        for i in 0..20 {
            t.observe(2, AP, &steady(56, 10.0), -60, now);
            let tilt = (i % 5) as f64;
            let moving: Vec<f64> = (0..56).map(|k| 10.0 + tilt * (k as f64 % 3.0)).collect();
            t.observe(2, PEER, &moving, -70, now);
        }
        assert_eq!(t.len(), 2, "one receiver, two transmitters => two links");
        let m = t.metrics();
        assert_eq!(m.len(), 2);
        let ap = m.iter().find(|x| x.id.tx_mac == AP).expect("AP link");
        let peer = m.iter().find(|x| x.id.tx_mac == PEER).expect("peer link");
        assert!(
            ap.raw_motion < peer.raw_motion,
            "the static link must not inherit the moving link's perturbation \
             (ap={} peer={})",
            ap.raw_motion,
            peer.raw_motion
        );
    }

    #[test]
    fn same_transmitter_on_two_receivers_are_separate_links() {
        let mut t = LinkTable::new();
        let now = Instant::now();
        for _ in 0..12 {
            t.observe(0, AP, &steady(56, 10.0), -60, now);
            t.observe(1, AP, &steady(56, 20.0), -50, now);
        }
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn stale_links_expire_and_free_their_slot() {
        let mut t = LinkTable::new();
        let t0 = Instant::now();
        for _ in 0..12 {
            t.observe(0, AP, &steady(56, 10.0), -60, t0);
        }
        assert_eq!(t.len(), 1);
        t.expire(t0 + LINK_STALE_AFTER + Duration::from_secs(1));
        assert_eq!(t.len(), 0, "stale link must be dropped");
    }

    #[test]
    fn table_is_bounded_against_a_busy_channel() {
        let mut t = LinkTable::new();
        let now = Instant::now();
        for i in 0..(MAX_LINKS as u32 + 40) {
            let mac = [0xAA, 0xBB, 0xCC, (i >> 16) as u8, (i >> 8) as u8, i as u8];
            t.observe(0, mac, &steady(56, 10.0), -70, now);
        }
        assert!(
            t.len() <= MAX_LINKS,
            "must not grow without bound on a busy channel, got {}",
            t.len()
        );
    }

    /// Frames of differing width within one window must not be mixed into the
    /// variance — a transmitter switching PPDU format would otherwise look like
    /// a huge motion event.
    /// A minority of HT frames arriving in a mostly-HE window must not hijack
    /// the measurement. Before the mode-lock this failed: whichever format the
    /// newest frame happened to be decided the whole window.
    #[test]
    fn an_ht_minority_does_not_displace_the_he_majority() {
        let mut h: VecDeque<Vec<f64>> = VecDeque::new();
        for i in 0..40 {
            h.push_back((0..256).map(|k| ((i + k) as f64 * 0.01).sin()).collect());
        }
        // Five HT frames scattered in, and crucially the NEWEST frame is HT.
        for i in 0..5 {
            h.push_back((0..64).map(|k| ((i + k) as f64 * 0.3).sin()).collect());
        }
        let with_ht_newest = agc_normalised_motion(&h).expect("metric");

        // The same history without the HT frames at all.
        let he_only: VecDeque<Vec<f64>> =
            h.iter().filter(|f| f.len() == 256).cloned().collect();
        let pure = agc_normalised_motion(&he_only).expect("metric");

        assert!(
            (with_ht_newest - pure).abs() < 1e-12,
            "HT minority changed the metric: {with_ht_newest} vs {pure}"
        );
    }

    /// A lasting format change must be followed, not ignored forever — the
    /// window refills with the new width and the mode moves with it.
    #[test]
    fn a_sustained_format_change_is_adopted() {
        let mut h: VecDeque<Vec<f64>> = VecDeque::new();
        for i in 0..10 {
            h.push_back((0..256).map(|k| ((i + k) as f64 * 0.01).sin()).collect());
        }
        for i in 0..40 {
            h.push_back((0..64).map(|k| ((i + k) as f64 * 0.02).sin()).collect());
        }
        let got = agc_normalised_motion(&h).expect("metric");
        let ht_only: VecDeque<Vec<f64>> =
            h.iter().filter(|f| f.len() == 64).cloned().collect();
        let pure = agc_normalised_motion(&ht_only).expect("metric");
        assert!((got - pure).abs() < 1e-12, "did not follow the new format");
    }

    #[test]
    fn frames_of_differing_width_do_not_corrupt_the_metric() {
        let mut h = VecDeque::new();
        for _ in 0..20 {
            h.push_back(steady(56, 10.0));
        }
        for _ in 0..4 {
            h.push_back(steady(128, 10.0));
        }
        // The 56-wide majority IS the measurement. The four 128-wide frames are
        // a transient minority and must neither corrupt it nor suppress it.
        //
        // This test previously asserted `is_none()`, encoding the old
        // newest-frame-wins rule: twenty perfectly good frames were thrown away
        // because four frames of another format happened to arrive last. That
        // was the defect, not the contract.
        let m = agc_normalised_motion(&h).expect("the majority width yields a metric");
        assert!(m.abs() < 1e-12, "steady frames must show no motion, got {m}");
    }

    // ---- transmitter-identity inference (ADR-345) ----------------------

    const NODE0: [u8; 6] = [0x9c, 0xcc, 0x01, 0x40, 0x18, 0xb8];
    const NODE1: [u8; 6] = [0xe8, 0xf6, 0x0a, 0xfc, 0xfb, 0x6c];

    /// Build the two indexes from an explicit (rx_node, tx_mac) list, so each
    /// test states the mesh it is describing rather than driving a LinkTable.
    fn indexes(
        pairs: &[(u8, [u8; 6])],
    ) -> (
        std::collections::BTreeSet<u8>,
        std::collections::BTreeMap<[u8; 6], std::collections::BTreeSet<u8>>,
    ) {
        let metrics: Vec<LinkMetric> = pairs
            .iter()
            .map(|&(rx_node, tx_mac)| LinkMetric {
                id: LinkId { rx_node, tx_mac },
                frames: 100,
                rssi: -70.0,
                raw_motion: 1.0,
                motion: 0.5,
                baseline: 1.0,
                baseline_samples: 500,
                window_span_s: 4.0,
                fps: 16.0,
            })
            .collect();
        hearing_index(&metrics)
    }

    #[test]
    fn a_transmitter_every_receiver_hears_is_infrastructure() {
        // The AP is external, so all three nodes hear it and none is excluded.
        let (rx, heard) = indexes(&[(0, AP), (1, AP), (2, AP)]);
        assert_eq!(infer_transmitting_node(&AP, &rx, &heard), None);
    }

    #[test]
    fn the_one_receiver_that_cannot_hear_a_transmitter_is_that_transmitter() {
        // NODE0's beacons reach nodes 1 and 2. Node 0 cannot hear its own
        // radio, and that hole identifies it.
        let (rx, heard) = indexes(&[
            (0, AP),
            (1, AP),
            (2, AP),
            (1, NODE0),
            (2, NODE0),
        ]);
        assert_eq!(infer_transmitting_node(&NODE0, &rx, &heard), Some(0));
    }

    #[test]
    fn two_receivers_missing_is_ambiguous_and_yields_no_guess() {
        // A neighbour's router that only node 2 happens to hear leaves nodes 0
        // and 1 both "missing" — indistinguishable from a peer out of range,
        // so the inference must decline rather than pick one.
        let stranger: [u8; 6] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let (rx, heard) = indexes(&[(0, AP), (1, AP), (2, AP), (2, stranger)]);
        assert_eq!(infer_transmitting_node(&stranger, &rx, &heard), None);
    }

    #[test]
    fn two_receivers_carry_no_signature_at_all() {
        // With only nodes 0 and 1 reporting, "heard by all but one" and "heard
        // by exactly one" are the same observation. Refuse regardless of how
        // suggestive the pattern looks.
        let (rx, heard) = indexes(&[(0, AP), (1, AP), (1, NODE0)]);
        assert_eq!(rx.len(), 2);
        assert_eq!(infer_transmitting_node(&NODE0, &rx, &heard), None);
    }

    #[test]
    fn a_full_three_node_mesh_labels_every_peer_and_leaves_the_ap_external() {
        // The validated 2026-08-28 topology: 3 AP links + 6 directional
        // node-to-node links.
        let (rx, heard) = indexes(&[
            (0, AP),
            (1, AP),
            (2, AP),
            (1, NODE0),
            (2, NODE0),
            (0, NODE1),
            (2, NODE1),
            (0, PEER),
            (1, PEER),
        ]);
        assert_eq!(infer_transmitting_node(&NODE0, &rx, &heard), Some(0));
        assert_eq!(infer_transmitting_node(&NODE1, &rx, &heard), Some(1));
        assert_eq!(infer_transmitting_node(&PEER, &rx, &heard), Some(2));
        assert_eq!(infer_transmitting_node(&AP, &rx, &heard), None);
    }

    // ── Pre-nine-node changes, 2026-08-29 ──────────────────────────────────

    /// Build a history where only `n_hot` of `n_sc` subcarriers carry a
    /// time-varying response and the rest are static.
    fn history_with_minority_response(
        n_sc: usize, n_hot: usize, frames: usize, amp: f64,
    ) -> VecDeque<Vec<f64>> {
        let mut h = VecDeque::new();
        for t in 0..frames {
            let mut f = vec![10.0; n_sc];
            for sc in 0..n_hot {
                // Alternate so the temporal std of a hot bin is exactly `amp`.
                f[sc] = 10.0 + if t % 2 == 0 { amp } else { -amp };
            }
            h.push_back(f);
        }
        h
    }

    #[test]
    fn p90_follows_a_minority_response_that_the_mean_would_dilute() {
        // The #1015 result, as a property: a body perturbs a minority of
        // subcarriers strongly. With 20% of bins hot, the mean over all bins is
        // pulled toward the static majority; p90 sits on the responding bins.
        let h = history_with_minority_response(100, 20, 32, 4.0);
        let got = agc_normalised_motion(&h).expect("metric must be produced");

        // Mean-over-subcarriers, the statistic this replaced, for comparison.
        let mean_stat = {
            let n_sc = 100usize;
            let frames: Vec<&Vec<f64>> = h.iter().collect();
            let n = frames.len() as f64;
            let normalised: Vec<Vec<f64>> = frames.iter().map(|f| {
                let m = f.iter().sum::<f64>() / n_sc as f64;
                f.iter().map(|a| a - m).collect()
            }).collect();
            let mut total = 0.0;
            for sc in 0..n_sc {
                let m: f64 = normalised.iter().map(|f| f[sc]).sum::<f64>() / n;
                let v: f64 = normalised.iter().map(|f| (f[sc] - m).powi(2)).sum::<f64>() / n;
                total += v.sqrt();
            }
            total / n_sc as f64
        };

        assert!(got > mean_stat * 2.0,
            "p90 ({got}) should be well above the diluted mean ({mean_stat})");
    }

    #[test]
    fn p90_is_not_the_maximum_so_a_single_noisy_bin_cannot_drive_it() {
        // One hot bin in 100 is below the 90th percentile, so the metric must
        // stay near the quiet level — this is why p90 and not max.
        let h = history_with_minority_response(100, 1, 32, 50.0);
        let got = agc_normalised_motion(&h).expect("metric must be produced");
        let quiet = agc_normalised_motion(&history_with_minority_response(100, 0, 32, 0.0))
            .expect("metric must be produced");
        assert!(got < quiet + 1.0,
            "a single spiking bin must not drive p90 (got {got}, quiet {quiet})");
    }

    #[test]
    fn baseline_freezes_while_the_link_is_responding() {
        // The change-detector defect: an unfrozen EMA absorbs a stationary
        // person within seconds, so `motion` decays to zero while they are
        // still present. Drive a link to a settled baseline, then hold it high
        // and assert the baseline does not chase.
        let now = Instant::now();
        let mut t = LinkTable::new();
        let quiet = vec![10.0f64; 32];
        for i in 0..300 {
            t.observe(0, [1, 2, 3, 4, 5, 6], &quiet, -60,
                      now + Duration::from_millis(i * 50));
        }
        let settled = t.metrics()[0].baseline;
        assert!(settled >= 0.0);

        // Now a sustained strong response, far above the freeze ratio.
        for i in 300..900 {
            let hot: Vec<f64> = (0..32)
                .map(|sc| if sc < 8 { 10.0 + if i % 2 == 0 { 6.0 } else { -6.0 } } else { 10.0 })
                .collect();
            t.observe(0, [1, 2, 3, 4, 5, 6], &hot, -60,
                      now + Duration::from_millis(i * 50));
        }
        let m = &t.metrics()[0];
        assert!(m.raw_motion > settled * BASELINE_FREEZE_RATIO,
            "test setup: response must exceed the freeze ratio");
        assert!(m.baseline <= settled * BASELINE_FREEZE_RATIO,
            "baseline must not chase a sustained response              (settled {settled}, now {})", m.baseline);
        assert!(m.motion > 0.0,
            "a sustained responder must still report motion, not decay to zero");
    }

    #[test]
    fn table_admits_more_links_than_a_nine_node_mesh_needs() {
        // Nine receivers x (8 peers + AP) = 81 links minimum. The old cap of
        // 64 admitted first-come and silently dropped the rest.
        let now = Instant::now();
        let mut t = LinkTable::new();
        let amp = vec![10.0f64; 16];
        for rx in 0..9u8 {
            for tx in 0..9u8 {
                if rx == tx { continue; }
                let mac = [0xE8, 0xF6, 0x0A, 0, 0, tx];
                for i in 0..MIN_FRAMES_FOR_METRIC + 2 {
                    t.observe(rx, mac, &amp, -70,
                              now + Duration::from_millis(i as u64 * 10));
                }
            }
            let ap = [0x8C, 0x30, 0x66, 0, 0, 1];
            for i in 0..MIN_FRAMES_FOR_METRIC + 2 {
                t.observe(rx, ap, &amp, -70, now + Duration::from_millis(i as u64 * 10));
            }
        }
        assert_eq!(t.len(), 81, "all 81 nine-node links must be admitted");
    }

    // ---- per-link subcarrier-grid policy -----------------------------------

    /// The bug this replaced: the gate was keyed by NODE, so whichever grid the
    /// associated AP used locked out every other transmitter. Two transmitters
    /// on the SAME receiver must be able to hold different grids at once.
    #[test]
    fn two_transmitters_on_one_receiver_keep_independent_grids() {
        let t0 = Instant::now();
        let mut t = LinkTable::new();
        for i in 0..12 {
            let at = t0 + Duration::from_millis(i * 10);
            t.observe(3, AP, &steady(256, 20.0), -70, at);    // dense HE
            t.observe(3, PEER, &steady(64, 20.0), -85, at);   // sparse HT
        }
        let inv = t.inventory(t0 + Duration::from_millis(200));
        let ap = inv.iter().find(|r| r.id.tx_mac == AP).expect("AP link");
        let peer = inv.iter().find(|r| r.id.tx_mac == PEER).expect("peer link");
        assert_eq!(ap.grid, 256, "AP locked to its own grid");
        assert_eq!(peer.grid, 64, "peer keeps ITS grid, not the AP's");
        assert_eq!(peer.history_len, 12, "sparse transmitter still accumulates");
        assert_eq!(peer.sparser_skipped, 0);
    }

    /// A denser frame is more information, so it wins and the coarser history
    /// is discarded rather than mixed — bins from different grids are not
    /// comparable (issue #1005).
    #[test]
    fn a_denser_frame_upgrades_the_link_and_rewarms() {
        let t0 = Instant::now();
        let mut t = LinkTable::new();
        for i in 0..10 {
            t.observe(1, AP, &steady(64, 20.0), -70, t0 + Duration::from_millis(i * 10));
        }
        t.observe(1, AP, &steady(256, 20.0), -70, t0 + Duration::from_millis(200));
        let inv = t.inventory(t0 + Duration::from_millis(210));
        let r = &inv[0];
        assert_eq!(r.grid, 256, "upgraded to the denser grid");
        assert_eq!(r.history_len, 1, "coarse history cleared, not mixed");
        assert_eq!(r.baseline_samples, 0, "baseline re-warms on upgrade");
    }

    /// A sparser frame must not enter the history, but the link is still real:
    /// liveness and frame count keep moving, and the skip is COUNTED. A silent
    /// drop is what hid every foreign transmitter until 2026-09-01.
    #[test]
    fn a_sparser_frame_is_counted_not_silently_dropped() {
        let t0 = Instant::now();
        let mut t = LinkTable::new();
        for i in 0..10 {
            t.observe(1, AP, &steady(256, 20.0), -70, t0 + Duration::from_millis(i * 10));
        }
        t.observe(1, AP, &steady(64, 20.0), -70, t0 + Duration::from_millis(200));
        let inv = t.inventory(t0 + Duration::from_millis(210));
        let r = &inv[0];
        assert_eq!(r.grid, 256);
        assert_eq!(r.history_len, 10, "sparser frame kept out of the history");
        assert_eq!(r.sparser_skipped, 1, "and counted");
        assert_eq!(r.frames, 11, "but it still counts as a frame on the link");
    }

    /// A uniform-grid history is what `agc_normalised_motion` wants, so after
    /// the policy runs the link should actually be renderable.
    #[test]
    fn a_sparse_only_transmitter_becomes_visible() {
        let t0 = Instant::now();
        let mut t = LinkTable::new();
        for i in 0..16 {
            let level = 20.0 + (i % 5) as f64;
            t.observe(2, PEER, &steady(64, level), -84, t0 + Duration::from_millis(i * 10));
        }
        let inv = t.inventory(t0 + Duration::from_millis(200));
        let r = &inv[0];
        assert_eq!(r.grid, 64);
        assert!(r.visible, "a 64-bin-only transmitter must render: {}", r.reason);
    }
}
