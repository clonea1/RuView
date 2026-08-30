#!/usr/bin/env python3
"""Score a ground-truth walk against the empty-room calibration.

Three questions, in the order they have to be answered. Each one is a
precondition for the next, so a failure at any stage makes the later numbers
meaningless rather than merely worse:

  1. REPRODUCIBILITY — does standing on the same spot twice produce the same
     per-link pattern? If not, nothing downstream can work, and no estimator
     is at fault.
  2. DISCRIMINABILITY — are different spots more different from each other than
     the same spot is from itself? This is the whole question. An instrument
     that responds strongly but identically everywhere carries presence, not
     position.
  3. POSITION — only if 1 and 2 hold, does the link-line solve land near truth,
     and specifically does it place the two flank spots outside the node
     triangle where the shipping estimator arithmetically cannot.

Each link is scored as |z| against its recorded empty-room mean and standard
deviation. Signed rather than absolute deviation would be wrong: the walk-back
measurement showed a body makes some links noisier and others quieter (it can
shadow a multipath-rich path into stability), so direction carries geometry we
have no model for, while magnitude carries "this link is perturbed".

Normalising by the empty-room sd also fixes the AP links for free. They rest an
order of magnitude louder than the peer links but barely move for a person, so
their large sd demotes them exactly as much as their unresponsiveness deserves.
"""

import json
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DWELL_S = 40
SETTLE_LEAD_S = 5   # discard while the 64-frame metric window refills


def load_jsonl(path):
    rows = []
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            d = json.loads(line)
            if "payload" in d:
                rows.append(d)
    return rows


def load_markers(path, server_start_unix):
    """(unix_time, label) for every dwell marker."""
    out = []
    with open(path) as fh:
        next(fh, None)
        for line in fh:
            parts = line.strip().split(",", 1)
            if len(parts) != 2:
                continue
            try:
                t_s = float(parts[0])
            except ValueError:
                continue
            out.append((t_s + server_start_unix, parts[1]))
    return out


def parse_dwell(label):
    """dwellP4pass1along-24out82 -> ('P4', 1, -24, 82)"""
    if not label.startswith("dwell"):
        return None
    rest = label[len("dwell"):]
    spot = rest[:2]
    rest = rest[2:]
    p = rest.index("pass")
    pas = int(rest[p + 4])
    a = rest.index("along")
    o = rest.index("out")
    return spot, pas, int(rest[a + 5:o]), int(rest[o + 3:])


def mean(v):
    return sum(v) / len(v) if v else float("nan")


def zvec(rows, cal, links):
    """Mean |z| per link over the given rows."""
    acc = {k: [] for k in links}
    for r in rows:
        for l in r["payload"]["links"]:
            if l["label"] in acc:
                acc[l["label"]].append(l["raw_motion"])
    out = []
    for k in links:
        c = cal["links"][k]
        m = mean(acc[k]) if acc[k] else float("nan")
        sd = c["sd"] if c["sd"] > 1e-9 else 1e-9
        out.append(abs(m - c["mean"]) / sd)
    return out


def cosine(a, b):
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    if na < 1e-12 or nb < 1e-12:
        return float("nan")
    return sum(x * y for x, y in zip(a, b)) / (na * nb)


def euclid(a, b):
    return math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))


def main():
    rec = sys.argv[1]
    markers_path = sys.argv[2]
    server_start = float(os.path.basename(markers_path).split("_")[-1].split(".")[0])

    cal = json.load(open(os.path.join(HERE, "empty_room_calibration.json")))
    links = sorted(cal["links"].keys())

    rows = load_jsonl(rec)
    marks = load_markers(markers_path, server_start)

    dwells = []
    for t, label in marks:
        p = parse_dwell(label)
        if p:
            dwells.append((t, p))
    print("%d dwells found\n" % len(dwells))

    obs = {}
    for t, (spot, pas, along, outd) in dwells:
        sel = [r for r in rows if t + SETTLE_LEAD_S <= r["t"] <= t + DWELL_S]
        if len(sel) < 15:
            print("  !! %s pass%d: only %d samples, skipping" % (spot, pas, len(sel)))
            continue
        obs[(spot, pas)] = {"z": zvec(sel, cal, links), "n": len(sel),
                            "x": 48 + along, "y": outd}

    # ---- 1. Reproducibility ------------------------------------------------
    print("1. REPRODUCIBILITY  — same spot, pass 1 vs pass 2")
    print("   %-5s %8s %8s   %s" % ("spot", "cosine", "L2", "peak |z| links (pass1 / pass2)"))
    spots = sorted({s for s, _ in obs})
    repro = []
    for s in spots:
        a, b = obs.get((s, 1)), obs.get((s, 2))
        if not a or not b:
            continue
        c = cosine(a["z"], b["z"])
        repro.append(euclid(a["z"], b["z"]))
        top = lambda z: links[max(range(len(z)), key=lambda i: z[i])]
        print("   %-5s %8.3f %8.2f   %s / %s" % (s, c, euclid(a["z"], b["z"]),
                                                 top(a["z"]), top(b["z"])))

    # ---- 2. Discriminability ----------------------------------------------
    print("\n2. DISCRIMINABILITY — is between-spot bigger than within-spot?")
    within = mean(repro)
    between = []
    for i, s1 in enumerate(spots):
        for s2 in spots[i + 1:]:
            for p in (1, 2):
                a, b = obs.get((s1, p)), obs.get((s2, p))
                if a and b:
                    between.append(euclid(a["z"], b["z"]))
    print("   within-spot  (pass1 vs pass2)  mean L2 = %.2f" % within)
    print("   between-spot (same pass)       mean L2 = %.2f" % mean(between))
    ratio = mean(between) / within if within > 1e-9 else float("inf")
    print("   ratio = %.2f  %s" % (ratio,
          "-- positions are separable" if ratio > 1.5 else
          "-- NOT separable: spots look alike compared to their own repeat"))

    # ---- per-spot signature -----------------------------------------------
    print("\n   mean |z| per link, averaged over both passes:")
    print("   %-14s %s" % ("link", "  ".join("%6s" % s for s in spots)))
    for i, k in enumerate(links):
        cells = []
        for s in spots:
            vals = [obs[(s, p)]["z"][i] for p in (1, 2) if (s, p) in obs]
            cells.append("%6.1f" % mean(vals))
        kind = cal["links"][k]["kind"][:4]
        print("   %-14s %s   (%s)" % (k, "  ".join(cells), kind))

    json.dump({("%s_p%d" % k): v for k, v in obs.items()},
              open(os.path.join(HERE, "walk_observations.json"), "w"), indent=2)
    print("\n   per-dwell vectors -> walk_observations.json")


if __name__ == "__main__":
    main()
