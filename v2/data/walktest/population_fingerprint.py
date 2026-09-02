"""Transmitter-population fingerprint: is this window comparable to that one?

WHY THIS EXISTS. On 2026-09-02 at 03:55 both APs rebooted. Every client
re-associated, the distribution across the two APs shuffled, and the set of
transmitters the fleet could overhear swapped wholesale -- six devices went
from under 20% presence to ~100%, four went from ~100% to near zero. Every
scalar metric moved with it: fast links +47%, mean motion +12%, 4-sigma
excursions doubled, and the paired-CSI gate statistic stepped 0.15 -> 0.23.

Nothing in the pipeline noticed. The overnight run was collected as a "quiet
baseline" and contains a regime change in its middle. Had the active and quiet
gate runs happened to fall on opposite sides of it, the artifact would have
looked like a occupancy effect of exactly the size we were hunting.

And this is not a one-off. Joe pinned both APs to channel 11 and disabled
auto-channel, but UniFi still shunts load between APs and clients roam on
their own -- so the population drifts continuously. Worse, because BOTH APs
sit on channel 11, a client moving between them does not change channel at
all: recording `rx_ctrl.channel` would have shown a flat 11 straight through
and detected nothing. The population itself is the only observable that moves.

WHAT IT MEASURES. Per window, each transmitter's mean number of simultaneous
live links -- how many of the nine nodes were hearing it, averaged over the
polls in the window. Deliberately not a simple presence fraction: a device
audible to one node and the same device audible to eight are different facts
about the fleet, and the second is what makes it useful as an illuminator. The
value therefore runs 0..9, not 0..1.

Two windows are compared by cosine distance over that vector, which is
scale-invariant, so a window with more total traffic is not automatically
"different" -- only a change in the SHAPE of the population counts.

THE TEST OF THE DETECTOR, not of the data: it is given no timestamps and no
hint. If it independently flags ~03:55 as the largest discontinuity of the
night, it works. If it flags something else, or nothing, it does not.
"""
import io
import json
import math
import sys
import datetime as dt
from collections import defaultdict

SRC = sys.argv[1] if len(sys.argv) > 1 else "links_overnight.jsonl"
WIN_S = 300.0          # 5-minute windows
MAX_SPAN = 25.0        # only links whose metric can actually track the window

f = io.open(SRC, encoding="utf-8")
meta = json.loads(f.readline())
LBL = {e["mac"]: (e["label"] or e["mac"]) for e in meta["emitters"]}
if meta.get("ap_mac"):
    LBL[meta["ap_mac"]] = "ACCESS POINT"
rows = [json.loads(l) for l in f if l.strip()]
t0 = rows[0]["u"]
print("%s: %d polls, %s -> %s"
      % (SRC, len(rows),
         dt.datetime.fromtimestamp(rows[0]["u"]).strftime("%H:%M"),
         dt.datetime.fromtimestamp(rows[-1]["u"]).strftime("%H:%M")))

wins = defaultdict(lambda: {"n": 0, "tx": defaultdict(int)})
for r in rows:
    w = int((r["u"] - t0) // WIN_S)
    b = wins[w]
    b["n"] += 1
    for k, v in r["l"].items():
        if v[1] is None or v[1] > MAX_SPAN:
            continue
        b["tx"][k.split("|")[1]] += 1

keys = sorted(wins)
alltx = sorted({m for w in wins.values() for m in w["tx"]})
print("windows: %d of %.0f s   transmitters seen: %d\n" % (len(keys), WIN_S, len(alltx)))


def vec(w):
    b = wins[w]
    n = max(b["n"], 1)
    return [b["tx"].get(m, 0) / n for m in alltx]


def cosdist(a, b):
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(x * x for x in b))
    if na < 1e-12 or nb < 1e-12:
        return 1.0
    return 1.0 - sum(x * y for x, y in zip(a, b)) / (na * nb)


V = {w: vec(w) for w in keys}
steps = []
for i in range(1, len(keys)):
    a, b = keys[i - 1], keys[i]
    if wins[a]["n"] < WIN_S / 4 or wins[b]["n"] < WIN_S / 4:
        continue
    steps.append((cosdist(V[a], V[b]), b))

vals = sorted(d for d, _ in steps)
med = vals[len(vals) // 2]
p90 = vals[int(len(vals) * 0.9)]
mad = sorted(abs(d - med) for d, _ in steps)[len(steps) // 2]
thresh = med + 8 * (mad if mad > 1e-9 else 1e-3)

print("consecutive-window population change (cosine distance)")
print("   median %.4f   p90 %.4f   max %.4f" % (med, p90, vals[-1]))
print("   flag threshold (median + 8*MAD): %.4f\n" % thresh)

flagged = sorted([s for s in steps if s[0] > thresh], reverse=True)
if not flagged:
    print("NO REGIME CHANGE DETECTED.")
else:
    print("REGIME CHANGES DETECTED, largest first:")
    print("   %-10s %10s   %s" % ("clock", "distance", "x median"))
    for d, w in flagged[:6]:
        clk = dt.datetime.fromtimestamp(t0 + w * WIN_S).strftime("%H:%M:%S")
        print("   %-10s %10.4f   %.0fx" % (clk, d, d / med if med > 0 else 0))

    d, w = flagged[0]
    a = keys[keys.index(w) - 1]
    print("\n   what moved across the largest one:")
    va, vb = V[a], V[w]
    ch = sorted(((vb[i] - va[i], alltx[i]) for i in range(len(alltx))),
                key=lambda t: -abs(t[0]))
    print("   %-34s %10s %10s" % ("transmitter", "links bef", "links aft"))
    for delta, m in ch[:8]:
        if abs(delta) < 0.15:
            break
        i = alltx.index(m)
        print("   %-34s %10.1f %10.1f" % (LBL.get(m, m)[:34], va[i], vb[i]))

print()
print("=" * 70)
print("HOW TO USE THIS")
print("=" * 70)
print("Before comparing any two measurement windows, compare their population")
print("vectors. A cosine distance above the flag threshold means the fleet was")
print("listening to a different set of radios, and the two numbers are not")
print("comparable however carefully each was computed.")
