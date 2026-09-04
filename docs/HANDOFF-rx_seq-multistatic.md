# Handoff: rx_seq frame selection and multistatic fusion

Written 2026-09-04 to close a long session. **Read this first**, then
`docs/BACKLOG.md` section "STOPPED: the seq gate is INERT as written".

---

## The question

Cross-node CSI fusion pairs frames by `(tx_mac, rx_seq)` — transmission
identity, not time. `paired_fraction` currently sits around **29%**: only
29% of observed transmissions are heard by two or more receivers.

An unmerged commit claims to know why it is not higher.

## The claim under test

`1822108a`, on local branch **`phase2-rxseq-fusion`**, never merged, never
pushed, in **no** pull request. Its argument:

> The time-based rate gate has independent phase on every node, so two nodes
> hearing the same transmitter keep DIFFERENT subsets of its frames — node A at
> t=0,20,40 ms while node B is at 7,27,47. Harmless for per-node sensing, fatal
> for cross-node fusion: there is nothing common left to combine. That is very
> likely the real reason multistatic alignment has never worked on this
> hardware, for anyone.

It further claims four upstream issues (**#1049, #1374, #1703, #1710**) blame
timestamp spread and reach for guard intervals, when the guard is not the
binding constraint — our sync is already ~40x better than the soft guard asks
and cohorts were still demoted 58% of the time.

Its fix: keep a frame only when `rx_seq % PERIOD == 0`, so every node
independently selects the *same* frames with no clocks and no coordination.

**Status: UNTESTED.** Not supported, not refuted.

## Why it is untested: the implementation is inert

Deployed to four nodes on 2026-09-04 and measured:

    gated   (n0,n3,n7,n8)  18.3 fps mean
    control (n1,n2,n4,n5)  21.8 fps mean      ratio 0.84

A 1-in-4 gate must cut transmitted frames ~75% (ratio ~0.25). It changed
nothing. Not a build problem — `CONFIG_CSI_SEQ_GATE=y` and `PERIOD=4` were
confirmed in the generated `build/config/sdkconfig.h`, and all four nodes
reported `0.8.10-seqgate`.

### Leading hypothesis, NOT confirmed

    if ((info->rx_seq % (uint16_t)CONFIG_CSI_SEQ_GATE_PERIOD) != 0) return;

802.11 Sequence Control is 16 bits: **bits 0–3 fragment number, bits 4–15
sequence number.** If ESP-IDF's `wifi_csi_info_t.rx_seq` returns the raw field,
every unfragmented frame is `seq << 4` — always a multiple of 16, therefore of
4 — so the test is always true and nothing is dropped.

The IDF header is unhelpfully terse:
`esp_wifi_types_native.h:129` → `uint16_t rx_seq; /**< rx sequence number */`

Candidate fix, **do not apply blind**:

    ((info->rx_seq >> 4) % CONFIG_CSI_SEQ_GATE_PERIOD) != 0

## Next step: confirm what rx_seq contains

Raw values are exposed nowhere today.

- `/api/v1/fusion` keys on `rx_seq` but reports only aggregates.
- The `seq` column in `v2/data/mesh/phase_raw_*.csv` is the NODE's own frame
  counter (values in the millions), **not** the 802.11 field. Do not be misled
  by it — that cost time once already.

Two ways to get it:

1. **Cable a node** (`CH343`, 115200; open with `DtrEnable=$false;
   RtsEnable=$false` or esptool's RTS will reset it). `csi_collector.c` around
   line 445 already has an ADR-345 probe that logs `mac` and `rx_seq` per frame.
   Cheapest option if a node is already on USB.
2. **Server-side histogram** of `rx_seq % 16` over a few thousand frames. If the
   distribution is uniform the hypothesis is wrong; if everything lands on 0 it
   is confirmed. Costs a rebuild (~5 min) plus a 2 s bounce using the
   rename-then-build trick in the runbook.

Once confirmed and fixed, the experiment design is already written and worth
reusing: **`scratchpad/seqgate_run.py`** does A/B/A — gate n0,n3,n7,n8 for 2 h,
ungate all for 2 h, gate again for 2 h, sampling `/api/v1/fusion` every 5 min.
A/B/A matters because fleet-wide pairing rates drift ~1.2x between windows, and
a single-window comparison cannot separate that from the effect.

**Before trusting any future run, verify the gate bites**: gated nodes' fps must
drop to roughly a quarter of control. That check is what caught this, and it
takes two minutes.

## State at handoff

- **All nine nodes on v0.8.9**, no experimental firmware anywhere.
- Bedroom LEDs (n1 guest, n2 kids, n3 fuzzies, n4 master) **off**.
- Recovery bootloader: **n3 and n6 confirmed present**, **n7 confirmed absent**,
  rest unknown — checkable remotely via `pending_verify` after an OTA.
- `main` clean. `experiment/seq-gate` holds the (inert) gate plus its config,
  committed so it is not lost again.
- Seven PRs open upstream (#1791–#1797); none contains the seq gate.
- `contrib/mesh-aligned-rate-gate` — the *other*, working solution to the same
  problem — is submitted and unaffected by any of this.

## Baselines already captured

In the session scratchpad (session-scoped; copy out if wanted):

    seq_T0.json          fusion pairs after 108 min, all nodes ungated
    fusion_baseline_timegate.json
    prod-0.8.9.bin       production image
    seqgate-0.8.10.bin   inert gate image
    seqgate_run.py       the A/B/A harness

Reference figures, all nodes ungated, ~108 min:

    observations 1,422,950   paired 274,592   paired_fraction 28.76%
    strongest pairs: n1-n8 60,580 | n2-n7 58,633 | n0-n8 47,985
