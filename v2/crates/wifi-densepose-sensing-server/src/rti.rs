//! Link-line ("radio tomographic") position estimation from per-link motion.
//!
//! # Why this exists
//!
//! The shipping estimators — [`motion_weighted_centroid`] and its Doppler
//! sibling — compute `sum(w_i * pos_i) / sum(w_i)` over node positions with all
//! `w_i >= 0`. That is a convex combination, so the estimate is *mathematically
//! confined* to the convex hull of the nodes. On the 2026-08-28 layout the node
//! triangle encloses 5.23 m² of a 138.98 m² room: the dot can only ever appear
//! in 3.8% of the space, at a fixed height, no matter where the person is. It is
//! not a weak estimator, it is a structurally incapable one.
//!
//! This module drops the "position is near the loud node" heuristic for a
//! forward model: a person perturbs a link when they are near the *line* between
//! its two endpoints. Both endpoints are known, so each link constrains position
//! to a region rather than voting for a point, and the estimate is free to land
//! anywhere the links actually cross.
//!
//! # Model
//!
//! For a cell `c` and a link from `tx` to `rx`, the excess path length
//! `|tx-c| + |c-rx| - |tx-rx|` is zero on the direct line and grows with
//! distance from it; its level sets are ellipses with the endpoints as foci.
//! The predicted weight is `exp(-excess / ELLIPSE_WIDTH_M)` — the usual
//! elliptical RTI kernel, smoothed rather than hard-thresholded so the score
//! surface has no cliffs for the peak search to catch on.
//!
//! Cells are scored by the **Pearson correlation** between the observed
//! per-link responses and the weights that cell predicts. Correlation, not a
//! plain weighted sum, for two reasons that both matter at six links:
//!
//! - A plain sum rewards any cell sitting near several links, whatever the
//!   observations say. Correlation asks whether the *pattern* of which links are
//!   unusually perturbed matches the pattern this cell would produce.
//! - Centering makes the score invariant to overall room activity, so the
//!   estimate does not swing with how vigorously the person is moving.
//!
//! # What this does not fix
//!
//! Escaping the convex hull is a real gain but it is not accuracy. Six
//! independent links (nine directed links, but reciprocal pairs measure the same
//! physical channel) sample a 139 m² room very sparsely, and the three
//! node-to-node links are the *edges* of the node triangle, so they do not cross
//! its interior. Sparse geometry produces genuinely ambiguous score surfaces,
//! which is why [`RtiEstimate::spread_m`] is computed and reported rather than
//! hidden: a multi-modal surface must be visible as one, not silently collapsed
//! to whichever mode happened to win.
//!
//! Nothing in here is calibrated against ground truth. Any accuracy figure for
//! this tier is `CLAIMED` until a labelled walk says otherwise.

/// One link's geometry and its normalised response this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkObservation {
    /// Receiver position, metres, room frame.
    pub rx: [f64; 2],
    /// Transmitter position, metres, room frame.
    pub tx: [f64; 2],
    /// Response, normalised so links of different intrinsic sensitivity are
    /// comparable. See [`normalise_response`] — passing raw motion here is a
    /// bug, not a shortcut: measured live, AP links run 0.9-4.0 while
    /// node-to-node links run 0.02-0.55, so raw values let three links decide
    /// everything and silence the three with the useful parallax.
    pub response: f64,
}

/// Search-grid and kernel parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RtiConfig {
    pub width_m: f64,
    pub depth_m: f64,
    /// Grid pitch. 0.25 m over a 13.4x10.4 m room is ~2200 cells; at six links
    /// and a 2 Hz tick the whole search is negligible next to the CSI work.
    pub cell_m: f64,
    /// Kernel decay constant for excess path length.
    pub ellipse_width_m: f64,
}

impl Default for RtiConfig {
    fn default() -> Self {
        Self {
            width_m: 10.0,
            depth_m: 10.0,
            cell_m: 0.25,
            // A person is roughly half a metre wide and perturbs well beyond
            // the first Fresnel zone (~0.36 m at mid-span on a 4 m link at
            // 2.4 GHz). Tighter than this and a real target falls outside its
            // own link's kernel between grid cells.
            ellipse_width_m: 0.6,
        }
    }
}

/// A position estimate plus the honesty metrics needed to decide whether to
/// believe it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RtiEstimate {
    pub x: f64,
    pub y: f64,
    /// Peak Pearson correlation, `[-1, 1]`. How well the best cell explains the
    /// observed pattern — not a probability, and not calibrated.
    pub confidence: f64,
    /// RMS spread of the near-peak cells, metres. Small means the surface has
    /// one clear mode; large means the links admit several separated
    /// explanations and the reported point is one of them arbitrarily. This is
    /// the number that reveals sparse-geometry ambiguity, so callers should
    /// gate on it rather than on `confidence` alone.
    pub spread_m: f64,
    /// Links that contributed.
    pub links_used: usize,
}

/// A 2D position needs at least three links before the geometry constrains
/// anything; with two, the near-peak set is a curve, not a point.
pub const MIN_LINKS: usize = 3;

/// Cells whose total predicted weight is below this are not observed by any
/// link, so their correlation score is a comparison of two noise vectors and
/// can peak anywhere. Excluding them keeps estimates inside the region the
/// links actually illuminate instead of letting an empty corner of the house
/// win on an accident of sign.
pub const MIN_CELL_OBSERVABILITY: f64 = 0.05;

/// Cells scoring at least this fraction of the peak join the centroid that
/// refines the estimate below grid pitch, and define the spread.
const NEAR_PEAK_FRACTION: f64 = 0.9;

/// Predicted response of a link to a target at `cell`.
///
/// `exp(-excess / width)`, where excess is the extra path length through the
/// cell over the direct line. Exactly 1 on the line, decaying with distance
/// from it; the level sets are ellipses focused on the endpoints.
///
/// Deliberately carries no `1/sqrt(link_length)` term. The classic RTI
/// formulation includes one, but every observation here is already normalised
/// per link, so a constant per-link factor cancels out of the correlation.
pub fn link_weight(rx: [f64; 2], tx: [f64; 2], cell: [f64; 2], ellipse_width_m: f64) -> f64 {
    let direct = dist(rx, tx);
    if direct <= f64::EPSILON || ellipse_width_m <= 0.0 {
        return 0.0;
    }
    let through = dist(tx, cell) + dist(cell, rx);
    // Clamped at zero: floating-point error can make `through` a hair under
    // `direct` for a cell exactly on the line, which would otherwise return a
    // weight above 1.
    let excess = (through - direct).max(0.0);
    (-excess / ellipse_width_m).exp()
}

/// Scale a link's raw motion into a comparable response.
///
/// `raw / scale`, where `scale` is that link's own resting level. The result is
/// "how many times its own quiet state is this link running at", which is
/// comparable across links whose absolute sensitivities differ by two orders of
/// magnitude for reasons — path length, RSSI, beacon size versus data frame —
/// that have nothing to do with where anyone is standing.
///
/// The caller supplies `scale` from a continuously adapting baseline, which has
/// a known weakness: a person who stays still long enough is gradually absorbed
/// into it. A recorded empty-room calibration would replace it with a fixed
/// reference and remove that failure mode entirely. Until then, this tier is
/// blind to a motionless target — the same limitation the rest of the pipeline
/// already has.
pub fn normalise_response(raw_motion: f64, scale: f64) -> f64 {
    if !(scale > f64::EPSILON) || !raw_motion.is_finite() {
        return 0.0;
    }
    (raw_motion / scale).max(0.0)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Pearson correlation. `None` when either input has no variance — for a cell
/// that means every link predicts the same weight, so it explains no pattern
/// and must not score as a perfect match.
fn correlation(a: &[f64], b: &[f64]) -> Option<f64> {
    let n = a.len();
    if n < 2 || b.len() != n {
        return None;
    }
    let inv = 1.0 / n as f64;
    let ma = a.iter().sum::<f64>() * inv;
    let mb = b.iter().sum::<f64>() * inv;
    let (mut num, mut va, mut vb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let da = a[i] - ma;
        let db = b[i] - mb;
        num += da * db;
        va += da * da;
        vb += db * db;
    }
    if va <= f64::EPSILON || vb <= f64::EPSILON {
        return None;
    }
    Some(num / (va * vb).sqrt())
}

/// Search the room for the cell whose predicted link weights best explain the
/// observed responses.
///
/// `None` when there are too few links, when no link shows any response, or
/// when no observed cell produces a positive correlation — all cases where a
/// returned point would be an invention rather than an estimate.
pub fn estimate(observations: &[LinkObservation], cfg: &RtiConfig) -> Option<RtiEstimate> {
    if observations.len() < MIN_LINKS {
        return None;
    }
    if cfg.cell_m <= 0.0 || cfg.width_m <= 0.0 || cfg.depth_m <= 0.0 {
        return None;
    }

    let observed: Vec<f64> = observations.iter().map(|o| o.response).collect();
    if !observed.iter().all(|v| v.is_finite()) {
        return None;
    }
    // A flat observation vector carries no spatial information; correlating
    // against it would return whichever cell won on rounding.
    let mean = observed.iter().sum::<f64>() / observed.len() as f64;
    if observed.iter().all(|v| (v - mean).abs() <= f64::EPSILON) {
        return None;
    }

    let nx = (cfg.width_m / cfg.cell_m).ceil() as usize;
    let ny = (cfg.depth_m / cfg.cell_m).ceil() as usize;
    if nx == 0 || ny == 0 {
        return None;
    }

    let mut weights = vec![0.0_f64; observations.len()];
    let mut best = f64::NEG_INFINITY;
    // Two passes over the grid: one to find the peak, one to collect the cells
    // near it. Cheaper than retaining every score, and the grid is small.
    let mut scored: Vec<([f64; 2], f64)> = Vec::new();

    for iy in 0..ny {
        for ix in 0..nx {
            let cell = [
                (ix as f64 + 0.5) * cfg.cell_m,
                (iy as f64 + 0.5) * cfg.cell_m,
            ];
            let mut mass = 0.0;
            for (i, o) in observations.iter().enumerate() {
                let w = link_weight(o.rx, o.tx, cell, cfg.ellipse_width_m);
                weights[i] = w;
                mass += w;
            }
            if mass < MIN_CELL_OBSERVABILITY {
                continue;
            }
            let Some(score) = correlation(&observed, &weights) else {
                continue;
            };
            if score > best {
                best = score;
            }
            scored.push((cell, score));
        }
    }

    // A negative peak means every observed cell predicts the *opposite* pattern
    // to the one measured. Reporting the least-bad of those would be a fiction.
    if scored.is_empty() || best <= 0.0 {
        return None;
    }

    let cutoff = best * NEAR_PEAK_FRACTION;
    let near: Vec<[f64; 2]> = scored
        .iter()
        .filter(|(_, s)| *s >= cutoff)
        .map(|(c, _)| *c)
        .collect();
    if near.is_empty() {
        return None;
    }

    let n = near.len() as f64;
    let cx = near.iter().map(|c| c[0]).sum::<f64>() / n;
    let cy = near.iter().map(|c| c[1]).sum::<f64>() / n;
    let spread = (near
        .iter()
        .map(|c| {
            let dx = c[0] - cx;
            let dy = c[1] - cy;
            dx * dx + dy * dy
        })
        .sum::<f64>()
        / n)
        .sqrt();

    Some(RtiEstimate {
        x: cx,
        y: cy,
        confidence: best,
        spread_m: spread,
        links_used: observations.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 2026-08-28 layout, from `v2/data/room_config.json`.
    const NODE0: [f64; 2] = [1.2192, 0.0];
    const NODE1: [f64; 2] = [3.81, 0.0];
    const NODE2: [f64; 2] = [2.4384, 4.0386];
    const AP: [f64; 2] = [7.9248, 5.8674];

    fn room() -> RtiConfig {
        RtiConfig {
            width_m: 13.4112,
            depth_m: 10.3632,
            cell_m: 0.25,
            ellipse_width_m: 0.6,
        }
    }

    /// The six independent links of the validated mesh, with responses
    /// synthesised from a known truth position through the same forward model
    /// the solver inverts, plus a flat resting level.
    fn observations_for(truth: [f64; 2], cfg: &RtiConfig) -> Vec<LinkObservation> {
        let pairs = [
            (NODE0, AP),
            (NODE1, AP),
            (NODE2, AP),
            (NODE0, NODE1),
            (NODE0, NODE2),
            (NODE1, NODE2),
        ];
        pairs
            .iter()
            .map(|&(rx, tx)| LinkObservation {
                rx,
                tx,
                response: 1.0 + 4.0 * link_weight(rx, tx, truth, cfg.ellipse_width_m),
            })
            .collect()
    }

    #[test]
    fn a_target_on_a_link_line_produces_unit_weight() {
        let mid = [(NODE0[0] + NODE1[0]) / 2.0, 0.0];
        let w = link_weight(NODE0, NODE1, mid, 0.6);
        assert!((w - 1.0).abs() < 1e-9, "on-line weight should be 1, got {w}");
    }

    #[test]
    fn weight_decays_with_distance_from_the_link_line() {
        let mid = [(NODE0[0] + NODE1[0]) / 2.0, 0.0];
        let near = link_weight(NODE0, NODE1, [mid[0], 0.3], 0.6);
        let far = link_weight(NODE0, NODE1, [mid[0], 1.5], 0.6);
        assert!(near > far, "closer to the line must weigh more");
        assert!(far > 0.0 && near < 1.0);
    }

    #[test]
    fn normalisation_makes_a_weak_and_a_strong_link_comparable() {
        // The measured live spread: an AP link resting near 20 and a peer link
        // resting near 1.3, both at twice their own quiet level.
        let ap = normalise_response(40.0, 20.0);
        let peer = normalise_response(2.6, 1.3);
        assert!((ap - peer).abs() < 1e-12, "{ap} vs {peer}");
    }

    #[test]
    fn normalisation_refuses_a_zero_or_absent_scale() {
        assert_eq!(normalise_response(5.0, 0.0), 0.0);
        assert_eq!(normalise_response(5.0, -1.0), 0.0);
        assert_eq!(normalise_response(f64::NAN, 1.0), 0.0);
    }

    #[test]
    fn a_target_inside_the_node_triangle_is_recovered() {
        let cfg = room();
        let truth = [2.4, 1.4];
        let est = estimate(&observations_for(truth, &cfg), &cfg).expect("should solve");
        let err = ((est.x - truth[0]).powi(2) + (est.y - truth[1]).powi(2)).sqrt();
        assert!(err < 1.0, "error {err:.2} m at {:?}", (est.x, est.y));
    }

    /// The point of the whole module: the existing centroid tiers are a convex
    /// combination of node positions and cannot leave the node triangle. This
    /// one must be able to.
    #[test]
    fn a_target_outside_the_node_hull_is_not_dragged_back_into_it() {
        let cfg = room();
        // On the AP-to-node2 line, well beyond the triangle's far edge.
        let truth = [4.5, 4.6];
        let est = estimate(&observations_for(truth, &cfg), &cfg).expect("should solve");

        // Inside-triangle test by sign-of-cross-product against each edge.
        let inside = {
            let s = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
                (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
            };
            let p = [est.x, est.y];
            let (d1, d2, d3) = (s(NODE0, NODE1, p), s(NODE1, NODE2, p), s(NODE2, NODE0, p));
            !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        };
        assert!(
            !inside,
            "estimate {:?} fell back inside the node triangle",
            (est.x, est.y)
        );
    }

    #[test]
    fn fewer_than_three_links_is_refused() {
        let cfg = room();
        let obs = observations_for([2.4, 1.4], &cfg);
        assert!(estimate(&obs[..2], &cfg).is_none());
    }

    #[test]
    fn a_flat_response_vector_yields_no_estimate() {
        // Nothing is more perturbed than anything else: no spatial information,
        // so any returned point would be arbitrary.
        let cfg = room();
        let obs: Vec<LinkObservation> = observations_for([2.4, 1.4], &cfg)
            .into_iter()
            .map(|o| LinkObservation { response: 1.0, ..o })
            .collect();
        assert!(estimate(&obs, &cfg).is_none());
    }

    #[test]
    fn an_unobserved_corner_of_the_house_cannot_win() {
        // Truth placed where no link passes. The solver must not report that
        // corner confidently; the links carry no evidence about it.
        let cfg = room();
        let est = estimate(&observations_for([12.8, 9.8], &cfg), &cfg);
        if let Some(e) = est {
            let d = ((e.x - 12.8f64).powi(2) + (e.y - 9.8f64).powi(2)).sqrt();
            assert!(
                d > 1.0,
                "solver claimed an unobserved corner at {:?}",
                (e.x, e.y)
            );
        }
    }

    #[test]
    fn ambiguity_is_reported_as_spread_rather_than_hidden() {
        let cfg = room();
        let sharp = estimate(&observations_for([2.4, 1.4], &cfg), &cfg).expect("solves");

        // Three collinear links along one wall: many cells explain the same
        // pattern, so the near-peak set must be visibly spread out.
        let flat = vec![
            LinkObservation { rx: [0.0, 0.0], tx: [10.0, 0.0], response: 3.0 },
            LinkObservation { rx: [0.0, 0.1], tx: [10.0, 0.1], response: 3.0 },
            LinkObservation { rx: [0.0, 0.2], tx: [10.0, 0.2], response: 1.0 },
        ];
        if let Some(amb) = estimate(&flat, &cfg) {
            assert!(
                amb.spread_m > sharp.spread_m,
                "ambiguous geometry {:.2} m must spread wider than sharp {:.2} m",
                amb.spread_m,
                sharp.spread_m
            );
        }
    }

    #[test]
    fn a_degenerate_zero_length_link_contributes_nothing_instead_of_dividing_by_zero() {
        assert_eq!(link_weight(NODE0, NODE0, [1.0, 1.0], 0.6), 0.0);
        assert_eq!(link_weight(NODE0, NODE1, [1.0, 1.0], 0.0), 0.0);
    }
}
