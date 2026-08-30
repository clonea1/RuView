#!/usr/bin/env python3
"""Summarise a link recording: per-link resting scale, distribution, and the
quiet segment.

Two jobs:

1. Find the empty-room segment. The recording starts with the occupant still
   present, so the file contains an occupied->empty transition. Rather than
   trusting a wall clock, the segment is found from the data: a long stretch
   where total across-link motion sits at its lowest sustained level.

2. Describe each link over that segment. The mean is the resting scale that
   `rti::normalise_response` divides by; the high percentiles are what a
   detection threshold should be built on, because "louder than the room ever
   was while empty" is a far better test than "louder than a number I chose".

Any link whose empty-room variance is high is being perturbed by something
that was in the room the whole time. With people and the free-roaming rabbit
out, that means the caged chinchillas, and the affected links are the ones
whose line passes near their cages.

Usage: analyse_baseline.py <recording.jsonl> [--from UNIX] [--to UNIX]
"""

import json
import sys


def load(path):
    rows = []
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                d = json.loads(line)
            except ValueError:
                continue
            if "payload" not in d:
                continue  # a failed poll; leaves a visible gap in t
            rows.append(d)
    return rows


def series(rows):
    """{link_label: [(t, raw_motion), ...]} plus the total-motion trace."""
    per = {}
    total = []
    for d in rows:
        t = d["t"]
        s = 0.0
        for l in d["payload"].get("links", []):
            per.setdefault(l["label"], []).append((t, l["raw_motion"]))
            s += l["raw_motion"]
        total.append((t, s))
    return per, total


def quietest_window(total, width_s=300):
    """Lowest-mean contiguous window of `width_s`, by start time.

    A plain global minimum would land on a single quiet second. Requiring a
    sustained window is what distinguishes "the room is empty" from "nobody
    moved for a moment".
    """
    if not total:
        return None
    best = None
    for i, (t0, _) in enumerate(total):
        window = [v for (t, v) in total[i:] if t <= t0 + width_s]
        if len(window) < width_s * 0.6:  # too few samples; ran off the end
            continue
        m = sum(window) / len(window)
        if best is None or m < best[0]:
            best = (m, t0, t0 + width_s, len(window))
    return best


def pct(vals, p):
    if not vals:
        return float("nan")
    s = sorted(vals)
    k = (len(s) - 1) * p
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    path = sys.argv[1]
    t_from = t_to = None
    if "--from" in sys.argv:
        t_from = float(sys.argv[sys.argv.index("--from") + 1])
    if "--to" in sys.argv:
        t_to = float(sys.argv[sys.argv.index("--to") + 1])

    rows = load(path)
    if not rows:
        print("no usable rows in", path)
        return 1
    span = rows[-1]["t"] - rows[0]["t"]
    print("%d rows spanning %.1f min" % (len(rows), span / 60.0))

    per_all, total = series(rows)

    if t_from is None:
        win = quietest_window(total, width_s=min(300, max(60, span * 0.25)))
        if win:
            mean, t_from, t_to, n = win
            print(
                "quietest sustained window: %.0f s wide, %d samples, "
                "mean total motion %.2f" % (t_to - t_from, n, mean)
            )
            print("  offset into file: %.1f .. %.1f min"
                  % ((t_from - rows[0]["t"]) / 60.0, (t_to - rows[0]["t"]) / 60.0))
    if t_from is None:
        t_from, t_to = rows[0]["t"], rows[-1]["t"]

    sel = [d for d in rows if t_from <= d["t"] <= (t_to or 1e18)]
    per, _ = series(sel)
    print("\nper-link over the selected segment (%d samples):" % len(sel))
    print("  %-14s %8s %8s %8s %8s %8s  %s"
          % ("link", "mean", "sd", "p50", "p95", "max", "sd/mean"))
    for label in sorted(per, key=lambda k: -sum(v for _, v in per[k])):
        vals = [v for _, v in per[label]]
        n = len(vals)
        mean = sum(vals) / n
        sd = (sum((v - mean) ** 2 for v in vals) / n) ** 0.5
        rel = sd / mean if mean > 0 else float("nan")
        print("  %-14s %8.3f %8.3f %8.3f %8.3f %8.3f  %6.2f%s"
              % (label, mean, sd, pct(vals, 0.5), pct(vals, 0.95), max(vals), rel,
                 "   <-- restless" if rel > 0.35 else ""))

    print("\nmean is the resting scale for rti::normalise_response.")
    print("p95 is the honest floor for 'this link is more perturbed than the")
    print("empty room ever was'. A high sd/mean means something in the room was")
    print("moving on that link the whole time.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
