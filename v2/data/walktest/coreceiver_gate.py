"""THE GATE EXPERIMENT for the cross-receiver plan.

Question: when two nodes capture the SAME packet, do their CSI deviations
covary? Everything downstream — grouping receivers into fields, drawing a glow
— assumes they do, and nobody has checked.

Design, and why it needs no ground truth:

  Take each receiver's deviation from ITS OWN mean channel shape, so the static
  per-link channel (which dominates raw amplitude and is different for every
  link) is removed and only the change remains. Then correlate those deviations
  between two receivers of one packet.

  The control is the same computation on receivers from DIFFERENT packets. If
  same-packet correlation exceeds shuffled-pair correlation, the excess is a
  shared cause — but NOT necessarily one in the room.

  MEASURED 2026-09-01, and this is why the original reading of this test was
  wrong: two receivers decoding the SAME packet also see the same transmitted
  symbols. Different packets carry different data, so the transmitted spectrum
  varies packet to packet and both receivers see that identical variation.
  Subtracting each link's mean channel shape removes static per-link structure
  but does NOT remove per-packet transmit content. So a large same-vs-shuffled
  excess is produced by the transmitter alone, with nobody in the house.

  Measured that night: sweeping (a person actively moving in one room) gave
  mean r 0.1495; a quiet control minutes later — everyone still or two floors
  away — gave 0.1216, still 10.0 sd above shuffled. The excess barely moved.
  The motion-attributable increment was 0.028, about 1.9 sd: not established.

  So this test alone CANNOT support the cross-receiver architecture. It detects
  that a shared cause exists; attributing that cause to a disturbance in the
  room requires a second run under quiet conditions and a comparison between
  the two. Run it both ways and pass --label so the runs can be compared.

Only same-width vectors are compared. A 64-bin and a 256-bin frame sample
different frequency grids and are not elementwise comparable — the same rule
the link table now enforces.

HOW TO USE IT, and the mistake to avoid
---------------------------------------
Run it TWICE and compare, because a single run cannot tell you what you want
to know:

  1. Empty house, nobody moving. This is the null. MEASURED 2026-09-01 with
     97% of node-samples reading "present_still": same-packet r = 0.29 against
     a shuffled control of 0.04, 20.5 standard errors apart — and the
     correlation was FLAT against receiver separation (0.25 / 0.31 / 0.30 /
     0.27 across 10-50 ft).

  2. Someone deliberately moving, in one known spot, for a few minutes.

The number that matters is not the headline correlation, it is the SHAPE of
the distance curve. In a still house the only shared cause available is
transmitter-side — it rides on the common waveform and reaches every receiver
equally, so a flat curve is what an empty house MUST produce and proves
nothing about geometry. A disturbance in the room is local: receivers whose
sensitive regions overlap it should covary more than distant ones, and the
curve should slope.

So: flat in run 1 and sloping in run 2 means the cross-receiver premise holds
and receivers can be grouped into fields. Flat in BOTH means the shared signal
is transmitter-side only and the grouping step should not be built.

Do not read a single empty-house run as a negative result. That mistake was
made here first and caught by checking /api/v1/nodes motion levels.
"""
import json
import math
import random
import time
import urllib.request
from collections import defaultdict

import os
import sys

# Label this run so an ACTIVE and a QUIET run can be compared afterwards --
# the comparison, not either run alone, is what attributes the shared cause.
LABEL = sys.argv[1] if len(sys.argv) > 1 else 'unlabelled'
RUNS = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gate_runs.json')

B = 'http://192.168.1.66:3000'
TARGET = 1200          # snapshots to gather
POLL_LIMIT = 200


def get(path):
    return json.load(urllib.request.urlopen(B + path, timeout=25))


snaps = {}
t0 = time.time()
while len(snaps) < TARGET and time.time() - t0 < 240:
    try:
        d = get('/api/v1/fusion/snapshots?limit=%d' % POLL_LIMIT)
    except Exception:
        time.sleep(2)
        continue
    for s in d['snapshots']:
        snaps[s['seq']] = s
    time.sleep(2)

print('snapshots gathered: %d' % len(snaps))
multi = [s for s in snaps.values() if len(s['receivers']) >= 2]
print('with 2+ receivers  : %d' % len(multi))
print('with 3+ receivers  : %d' % len([s for s in multi if len(s['receivers']) >= 3]))

# Mean channel shape per (node, tx, width). This is the static part to remove.
sums = defaultdict(lambda: None)
counts = defaultdict(int)
for s in multi:
    for r in s['receivers']:
        k = (r['node_id'], s['tx_mac'], r['n_subcarriers'])
        a = r['amplitudes']
        if sums[k] is None:
            sums[k] = list(a)
        else:
            acc = sums[k]
            for i, v in enumerate(a):
                acc[i] += v
        counts[k] += 1

MIN_SAMPLES = 8          # a mean from fewer frames is mostly the frame itself
means = {k: [v / counts[k] for v in sums[k]] for k in sums if counts[k] >= MIN_SAMPLES}
print('channel means built for %d (node,tx,width) combinations' % len(means))


def deviation(r, tx):
    k = (r['node_id'], tx, r['n_subcarriers'])
    m = means.get(k)
    if m is None:
        return None
    a = r['amplitudes']
    if len(a) != len(m):
        return None
    d = [a[i] - m[i] for i in range(len(a))]
    # Drop guard/null subcarriers: always zero, they would inflate correlation
    # by adding matching constant terms to both vectors.
    keep = [i for i in range(len(d)) if m[i] > 0.5]
    return [d[i] for i in keep] if len(keep) >= 16 else None


def corr(a, b):
    n = min(len(a), len(b))
    if n < 16:
        return None
    ma = sum(a[:n]) / n
    mb = sum(b[:n]) / n
    num = va = vb = 0.0
    for i in range(n):
        da = a[i] - ma
        db = b[i] - mb
        num += da * db
        va += da * da
        vb += db * db
    if va <= 1e-9 or vb <= 1e-9:
        return None
    return num / math.sqrt(va * vb)


# --- same-packet pairs ------------------------------------------------------
same = []
same_pairs = []
pool = defaultdict(list)          # width -> [(deviation, node)] for the control
for s in multi:
    devs = []
    for r in s['receivers']:
        d = deviation(r, s['tx_mac'])
        if d is not None:
            devs.append((r['n_subcarriers'], r['node_id'], d))
    for i in range(len(devs)):
        pool[devs[i][0]].append((devs[i][2], devs[i][1]))
        for j in range(i + 1, len(devs)):
            if devs[i][0] != devs[j][0]:
                continue          # different grids are not comparable
            c = corr(devs[i][2], devs[j][2])
            if c is not None:
                same.append(c)
                same_pairs.append((devs[i][1], devs[j][1], c))

# --- shuffled control: receivers from DIFFERENT packets ---------------------
random.seed(7)
ctrl = []
for width, items in pool.items():
    if len(items) < 4:
        continue
    for _ in range(len(items) * 2):
        (a, na), (b, nb) = random.sample(items, 2)
        if na == nb:
            continue              # same node against itself is not a pair
        c = corr(a, b)
        if c is not None:
            ctrl.append(c)


# --- does correlation depend on how far apart the two receivers are? --------
#
# A transmitter-side artifact rides on the shared waveform and correlates EVERY
# receiver pair equally, whatever the geometry. A disturbance in the room is
# local, so receivers whose sensitive regions overlap should covary more than
# distant ones. Opposite predictions, one test — and the same test decides
# whether "separated receivers form their own fields of observation" is real.
cfg = get('/api/v1/config/room')
npos = {n['id']: (n['x'], n['y'], n['z']) for n in cfg['nodes']}
FT = 0.3048
bins = defaultdict(list)
for a, b, c in same_pairs:
    if a in npos and b in npos:
        d = math.dist(npos[a], npos[b]) / FT
        bins[int(d // 10) * 10].append(c)
print()
print('same-packet correlation vs distance between the two receivers')
print('(bin means alone are misleading here — three runs on 2026-09-01 gave')
print(' decaying, flat and RISING profiles from the same conditions, so the')
print(' fitted slope and its standard error are what to read.)')
print('%-12s %7s %9s %9s' % ('separation', 'n', 'mean r', 'median'))
prev = None
for lo in sorted(bins):
    v = bins[lo]
    if len(v) < 25:
        continue
    m = sum(v) / len(v)
    sv = sorted(v)
    print('%-12s %7d %9.4f %9.4f' % ('%d-%d ft' % (lo, lo + 10), len(v), m, sv[len(sv) // 2]))
    prev = m


def _slope(rows):
    """Least-squares slope of correlation on separation, with its standard
    error. A transmitter-side artifact rides on the shared waveform and
    correlates every pair equally whatever the geometry, so a significantly
    NEGATIVE slope is the only thing here that argues for a local cause."""
    n = len(rows)
    if n < 30:
        return None
    mx = sum(r[0] for r in rows) / n
    my = sum(r[1] for r in rows) / n
    sxx = sum((r[0] - mx) ** 2 for r in rows)
    if sxx <= 1e-9:
        return None
    b = sum((r[0] - mx) * (r[1] - my) for r in rows) / sxx
    a = my - b * mx
    resid = [r[1] - (a + b * r[0]) for r in rows]
    se = math.sqrt((sum(x * x for x in resid) / (n - 2)) / sxx)
    return b, se


def stats(v):
    if not v:
        return None
    v = sorted(v)
    n = len(v)
    mean = sum(v) / n
    sd = math.sqrt(sum((x - mean) ** 2 for x in v) / n)
    return n, mean, v[n // 2], sd, v[int(n * 0.9)]


ss, cs = stats(same), stats(ctrl)
print()
print('%-22s %7s %8s %8s %8s %8s' % ('', 'n', 'mean r', 'median', 'sd', 'p90'))
if ss:
    print('%-22s %7d %8.4f %8.4f %8.4f %8.4f' % ('SAME packet', ss[0], ss[1], ss[2], ss[3], ss[4]))
if cs:
    print('%-22s %7d %8.4f %8.4f %8.4f %8.4f' % ('shuffled control', cs[0], cs[1], cs[2], cs[3], cs[4]))
if ss and cs:
    # Welch-style separation of the two means, in standard errors.
    se = math.sqrt(ss[3] ** 2 / ss[0] + cs[3] ** 2 / cs[0])
    z = (ss[1] - cs[1]) / se if se > 0 else 0.0
    print()
    prev = []
    if os.path.exists(RUNS):
        try:
            prev = json.load(open(RUNS))
        except Exception:
            prev = []
    prev.append({'label': LABEL, 'time': time.strftime('%Y-%m-%d %H:%M:%S'),
                 'same_mean': ss[1], 'same_n': ss[0],
                 'ctrl_mean': cs[1], 'ctrl_n': cs[0]})
    try:
        json.dump(prev, open(RUNS, 'w'), indent=2)
    except Exception:
        pass
    if len(prev) > 1:
        print('previous runs on record:')
        for p in prev[-6:]:
            print('   %-22s %s  same r=%.4f  shuffled r=%.4f'
                  % (p['label'], p['time'], p['same_mean'], p['ctrl_mean']))
        print()
    print('difference in mean r : %+.4f' % (ss[1] - cs[1]))
    print('separation           : %.1f standard errors' % z)
    print()
    geo = [(d, c) for a, b, c in same_pairs
           for d in ([math.dist(npos[a], npos[b]) / FT] if a in npos and b in npos else [])]
    sl = _slope(geo)
    if sl:
        print('separation slope     : %+.6f r per ft  (%.1f sd from zero, n=%d)'
              % (sl[0], abs(sl[0]) / sl[1] if sl[1] > 0 else 0, len(geo)))
    print()
    if abs(z) < 3:
        print('VERDICT: no detectable shared signal between receivers of one packet.')
        print('  Nothing here to build on. Note this is a WEAKER claim than it')
        print('  looks: the test cannot see a room disturbance that is smaller')
        print('  than the transmit-content artifact it also measures.')
    elif ss[1] > cs[1]:
        print('VERDICT: a shared cause is present, and it is NOT YET ATTRIBUTED.')
        print()
        print('  Same-packet receivers covary well beyond chance. That is real,')
        print('  and it reproduces. It is ALSO exactly what two receivers')
        print('  decoding the same transmitted symbols produce with an empty')
        print('  house -- MEASURED 2026-09-01: a quiet control gave r=0.1216')
        print('  against r=0.1495 while a person swept the room, a difference')
        print('  of only 1.9 sd.')
        print()
        if sl and sl[0] < 0 and abs(sl[0]) / sl[1] > 3:
            print('  The separation slope IS significantly negative here, which a')
            print('  transmitter-side artifact cannot produce. That argues for a')
            print('  local cause -- but confirm it against a quiet control before')
            print('  building: the slope was unstable across three runs.')
        else:
            print('  The separation slope is not significantly negative, so nothing')
            print('  here distinguishes a room disturbance from the transmit')
            print('  artifact. DO NOT read this as support for the cross-receiver')
            print('  architecture. Re-run with the house quiet and compare.')
    else:
        print('VERDICT: same-packet receivers covary LESS than chance -- anti-correlated.')
        print('  Real structure, but not the structure the plan assumed. Investigate')
        print('  before building anything on it.')
