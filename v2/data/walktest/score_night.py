#!/usr/bin/env python3
"""Score a link recording for motion, rejecting transport artifacts.

Why this exists
---------------
On the 2026-08-28 overnight baseline the *largest* apparent motion excursions
of the night happened with the room empty. During them one link's delivery
collapsed to 0.1-1.4 fps while its peers surged to 22 fps. A body blocking a
path reduces delivery on that path; it cannot raise delivery on the others.
Total offered load is conserved, so that signature is contention
redistribution inside the ESP-NOW mesh, not a person.

It fools the motion metric because `raw_motion` is a statistic over a fixed
64-frame window (`links.rs`, LINK_HISTORY). When a link's delivery rate
changes 5x, that window's wall-clock span changes 5x with it -- 36 s at
1.79 fps versus 7 s at 8.66 fps -- so the variance it measures changes for
reasons that have nothing to do with the room.

RETRACTED 2026-08-29: an earlier version of this script gated alarms on
per-bin delivery deviation and reported that it rejected 56% of them as
artifacts. That gate does not work and the claim is withdrawn.

Scored against a precisely-labelled 138 s human entry (11:19:40-11:21:58,
marked in real time), the gate threw away 5 of 8 true detections. And the
known artifact window itself shows a max deviation of only 1.00, while the
human reaches 4.0 -- the person perturbs delivery MORE than the artifact, so
no threshold separates them.

The error was conflating two quantities: the 14 dB event is a *persistent
regime change* between window means, whereas this measured *instantaneous*
deviation from each link's overall median -- which a persistent change barely
moves, because the new regime becomes the median.

The instrumentation is still worth having, so delivery rate and window span
are reported per episode for a human to judge. They are NOT used to filter.

    python score_night.py <recording.jsonl> [--marks room-marks.json]

Recordings made before 2026-08-29 carry no `fps` field; delivery rate is then
derived from successive `frames` counters, which is why both paths exist.
"""

import argparse
import collections
import io
import json
import statistics
import sys
import time

BIN_S = 10          # analysis bin; long enough to hold a 64-frame window
Z_ALARM = 6.0       # robust-z on raw_motion for a bin to be an alarm
RATE_DEV_PCTL = 0.90  # which quantile of per-link rate deviation to gate on


def load(path):
    """-> {bin: {label: [raw_motion]}}, {bin: {label: fps}}"""
    motion = collections.defaultdict(lambda: collections.defaultdict(list))
    frames = collections.defaultdict(dict)
    served = collections.defaultdict(dict)
    for line in io.open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        t = rec.get("t")
        if t is None:
            continue
        b = int(t // BIN_S)
        for lk in rec.get("payload", {}).get("links", []):
            lab = lk.get("label")
            if lab is None:
                continue
            motion[b][lab].append(lk.get("raw_motion", 0.0))
            if lk.get("fps") is not None:          # server >= 2026-08-29
                served[b][lab] = lk["fps"]
            f = lk.get("frames")
            if f is not None:                       # fallback for old files
                prev = frames[b].get(lab)
                frames[b][lab] = f if prev is None else max(prev, f)
    return motion, frames, served


def rates(bins, frames, served):
    """Per-bin per-link delivery fps, preferring the server-reported value."""
    out = {}
    for i, b in enumerate(bins):
        if served.get(b):
            out[b] = dict(served[b])
            continue
        if i == 0:
            continue
        p = bins[i - 1]
        if b - p != 1:
            continue
        r = {}
        for lab, f in frames.get(b, {}).items():
            pf = frames.get(p, {}).get(lab)
            if pf is None:
                continue
            d = (f - pf) / float(BIN_S)
            if 0 <= d < 200:                        # guard counter resets
                r[lab] = d
        if r:
            out[b] = r
    return out


def robust_baseline(values):
    med = statistics.median(values)
    mad = statistics.median([abs(v - med) for v in values]) or 1e-9
    return med, 1.4826 * mad


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("recording")
    ap.add_argument("--marks", help="room-marks JSON exported from ui/mark.html")
    ap.add_argument("--z", type=float, default=Z_ALARM)
    args = ap.parse_args()

    motion, frames, served = load(args.recording)
    bins = sorted(motion)
    if len(bins) < 10:
        sys.exit("not enough data in %s" % args.recording)

    seen = collections.Counter()
    for b in bins:
        for lab in motion[b]:
            seen[lab] += 1
    core = sorted([l for l, c in seen.items() if c > 0.9 * len(bins)])
    if not core:
        sys.exit("no link appears in >90%% of bins; recording too fragmented")

    base = {}
    for lab in core:
        base[lab] = robust_baseline(
            [statistics.mean(motion[b][lab]) for b in bins if lab in motion[b]])

    rate = rates(bins, frames, served)
    rate_med = {}
    for lab in core:
        v = [rate[b][lab] for b in rate if lab in rate[b]]
        if v:
            rate_med[lab] = statistics.median(v) or 1e-9

    rows = []
    for b in bins:
        if b not in rate:
            continue
        zs = [(statistics.mean(motion[b][lab]) - base[lab][0]) / base[lab][1]
              for lab in core if lab in motion[b]]
        if not zs:
            continue
        dev = sorted(abs(rate[b][lab] / rate_med[lab] - 1.0)
                     for lab in rate[b] if lab in rate_med)
        if not dev:
            continue
        rows.append((b, max(zs), dev[int(RATE_DEV_PCTL * (len(dev) - 1))]))

    if not rows:
        sys.exit("no bin had both motion and delivery-rate data")

    span_h = (bins[-1] - bins[0]) * BIN_S / 3600.0
    # Reported, never applied — see the retraction in the module docstring.
    gate = sorted(r[2] for r in rows)[int(0.90 * (len(rows) - 1))]
    alarms = [r for r in rows if r[1] > args.z]
    kept = alarms
    rejected = 0

    print("%s" % args.recording)
    print("  %s -> %s  (%.2f h, %d bins of %ds, %d core links)" % (
        time.strftime("%H:%M", time.localtime(bins[0] * BIN_S)),
        time.strftime("%H:%M", time.localtime(bins[-1] * BIN_S)),
        span_h, len(rows), BIN_S, len(core)))
    print("  source of delivery rate: %s" % (
        "server fps field" if served else "derived from frames counter"))
    print()
    print("  bins with z > %.1f          : %d" % (args.z, len(alarms)))
    print("  delivery deviation p90       : %.2f (reported only, not used to"
          " filter -- see docstring)" % gate)

    groups = []
    for b, z, d in sorted(kept):
        if groups and b - groups[-1][-1][0] <= 3:
            groups[-1].append((b, z, d))
        else:
            groups.append([(b, z, d)])

    print("\n  candidate motion episodes:")
    if not groups:
        print("    (none)")
    for g in groups:
        print("    %s -> %s  %4ds  peak z=%5.1f" % (
            time.strftime("%H:%M:%S", time.localtime(g[0][0] * BIN_S)),
            time.strftime("%H:%M:%S", time.localtime(g[-1][0] * BIN_S)),
            len(g) * BIN_S, max(x[1] for x in g)))

    if not args.marks:
        print("\n  no --marks given; episodes are unlabelled. Use ui/mark.html "
              "to stamp entries as they happen -- times recalled afterwards "
              "are too soft to score against.")
        return

    with io.open(args.marks, encoding="utf-8") as fh:
        marks = json.load(fh).get("marks", [])
    spans, open_at = [], None
    for m in sorted(marks, key=lambda x: x["t_unix_ms"]):
        if m["label"] == "room-in":
            open_at = m["t_unix_ms"] / 1000.0
        elif m["label"] == "room-out" and open_at is not None:
            spans.append((open_at, m["t_unix_ms"] / 1000.0))
            open_at = None
    if open_at is not None:
        spans.append((open_at, bins[-1] * BIN_S))

    def occupied(b):
        t = b * BIN_S
        return any(a <= t <= z for a, z in spans)

    occ = [r for r in rows if occupied(r[0])]
    emp = [r for r in rows if not occupied(r[0])]
    keptset = set(x[0] for x in kept)
    print("\n  scored against %d occupancy span(s) from %s:" % (len(spans), args.marks))
    for name, group in (("occupied", occ), ("empty", emp)):
        if not group:
            print("    %-9s no bins" % name)
            continue
        hits = sum(1 for r in group if r[0] in keptset)
        hrs = len(group) * BIN_S / 3600.0
        print("    %-9s %5.2f h   %3d detections   %6.1f /hour" % (
            name, hrs, hits, hits / hrs if hrs else 0.0))


if __name__ == "__main__":
    main()
