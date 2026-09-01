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
  same-packet correlation exceeds shuffled-pair correlation, the excess is
  shared physical cause — something in the room both receivers saw at that
  instant. If the two distributions sit on top of each other, there is no
  shared signal to build on and the architecture does not work as designed.

  That comparison is self-calibrating: it needs no labelled walk, no knowledge
  of whether anyone was moving, and no forward model.

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
    print('difference in mean r : %+.4f' % (ss[1] - cs[1]))
    print('separation           : %.1f standard errors' % z)
    print()
    if abs(z) < 3:
        print('VERDICT: no detectable shared signal between receivers of one packet.')
        print('  The premise the cross-receiver plan rests on is NOT supported by')
        print('  this data. Do not build the grouping or the glow on top of it.')
    elif ss[1] > cs[1]:
        print('VERDICT: same-packet receivers covary MORE than chance.')
        print('  Shared physical cause is present. The premise holds and the next')
        print('  step (grouping receivers into fields) is worth building.')
    else:
        print('VERDICT: same-packet receivers covary LESS than chance — anti-correlated.')
        print('  Real structure, but not the structure the plan assumed. Investigate')
        print('  before building anything on it.')
