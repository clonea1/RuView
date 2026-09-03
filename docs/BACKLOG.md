# RuView backlog

Open work that is not captured by code, tests, or an accepted ADR.

The point of this file is that it survives a conversation ending. Anything
living only in a chat log or an assistant's memory is lost the moment the
session is; anything here is in git and readable by a human. Decisions and
designs belong in `docs/adr/`; this file is for *work items*.

Keep entries short and dated. Delete them when done — git remembers.

## RESUME HERE (state at 2026-09-03, end of session)

`main` is clean and contains everything. Nothing is uncommitted.

**The five existing `contrib/*` branches are STALE and must be re-cut.** They
were cut before the de-personalisation (commits 1c5234e3, 32c00370) and before
the upstream adoptions (16e37318, b8dc73df). `contrib/remote-config` still
contains a hardcoded personal OneDrive path in config_push.py -- verified
present in that branch. Do not submit any of them as they stand.

**Re-cut eight, each one topic, each from `origin/main`, each built standalone
before submitting** (that check is what caught the thermal.c and /calibrate
collisions last time):

    wifi-retry-watchdog   espnow-beacon-scaling   mesh-aligned-rate-gate
    disable-unused-154-radio   remote-config
    thermal   rollback   provisioning-tooling      <- these three are new

Exclude from every branch: `board_index.json`, `provision_conf*.json`,
`partitions_16mb.csv`, `sdkconfig.defaults.16mb`, `case/`.

**Still to execute from the PR sweep** (verdicts agreed with Joe):
- **1760** -- merge their chip-identity binding, keep our secrets-removal on
  top. They still write the WiFi passphrase into the state file; we do not.
  Conflicts in provision.py, so this is a real merge, not a cherry-pick.
- **1593** -- BSSID telemetry for roam detection. Check its airtime cost first:
  it emits a 32-byte packet per node every ~30 s plus on every roam, and the
  2.4 GHz band was at 86% airtime.

**Node 3 is on OLD firmware** (pre-16e37318). It is on USB at COM6 at desk
height. The adopted fixes -- 12 KB httpd stack, 128 TX buffers, auth cleanup --
are built but NOT flashed to any node.

**The 128-TX-buffer change is UNVALIDATED.** Node 3's ENOMEM flood vanished when
it moved from -84 to -70 dBm, so there is no symptom left to test against.
**Node 7 is now at -85 dBm** and is the natural reproducer; alternatively put
node 3 back on the floor for ten minutes. Do not claim the change works until
it is measured against a real symptom.

**The channel split has happened** (family room AP -> ch 1, sensors pinned to
2nd floor on ch 11). First measurement: links 178 -> 147, transmitters 38 -> 33,
fleet-wide illuminators 4 -> 2. RSSI moved hard both ways -- nodes 0/1/8 gained
16-23 dB, nodes 2/7 lost 22-24 dB. **That is a single snapshot and has not been
confirmed.** Re-measure after it settles. Before-snapshot saved at
`scratchpad/links_before_chansplit.json`.

**PR 1647 ratio-normalisation was tested and did not help** (RAW 9%, SUBTRACT
27%, RATIO 18%, majority-class null 50%). n=11 of 28 marks -- underpowered, so
this is "no evidence it helps", not "it does not help". A proper test needs a
tapper session run concurrently with a capture, 40+ marks spread across rooms.

## Programme: repo, install, and upstream (Joe, 2026-09-02)

Priorities below are by *dependency and risk*, not by size. Two items solve
the same problem and one of them is better; two others are blocked until a
third lands. Reasoning is stated so it can be argued with.

### P0 — the keystone

- **P0.1 Fix the full-disk backup. DONE 2026-09-02** — daily images restored,
  running at 09:45.

- **P0.2 Make the server installable independently of the repo.** This is the
  keystone: it unblocks the product install, the backup layout, and removes
  the reason house data sits in a source tree.

  **The scale of the problem, measured 2026-09-02:** the repo is **76 GB**, of
  which `v2/target/` is **37 GB**. The actual runtime payload is the server
  binary (9 MB) plus `ui/` (3.8 MB) — about **13 MB**. A delivered product is
  0.02% of what currently has to be present for the server to start.

  **Why it cannot run outside the repo today**, both concrete and small:
  - `data_dir` is hardcoded to a relative `"data"` (main.rs, `let data_dir =
    PathBuf::from("data")`) with no CLI argument, so it resolves against the
    working directory.
  - `ui_path` defaults to `"../ui"`, i.e. the repo's UI folder seen from `v2/`.

  So the fix is two arguments and their defaults, not a rearchitecture:
  `--data-dir` (new) and a sane `--ui-path` default beside the binary.

### P1 — the product layout (Joe, 2026-09-02)

Three locations, three purposes:

| path | holds | backed up |
|---|---|---|
| `C:\Program Files\RuView\` | the delivered product: exe + `ui/` + sample config | no (reinstallable) |
| `C:\ProgramData\RuView\` | site collateral: room config, emitters, node identity, provisioning profiles, captures, models | yes |
| `C:\src\RuView\` | the repo (development only) | yes, excluding `target/` |

- **P1.1 Ship an installable payload**: binary, `ui/`, a sample room config, and
  a config file instead of a fifteen-argument command line. Run as a service or
  scheduled task rather than a hand-started process.
- **P1.2 Move site data to `C:\ProgramData\RuView\`.** Retires
  `.githooks/pre-push` as load-bearing: that guard exists only because house
  data (nine surveyed positions, interior dimensions, 57 device MACs) lives in
  a source repo at all.
- **P1.3 Move the repo off `C:\temp` to `C:\src\RuView`.**

- **P1.4 Backup by scheduled robocopy, not live sync.**
  `C:\src\RuView` -> `OneDrive\src\RuView`, excluding `target/` — robocopy
  supports `/XD`, which is precisely what OneDrive selective sync cannot do
  inside a synced folder. `C:\ProgramData\RuView` -> `OneDrive\RuView`.
  A scheduled snapshot never holds a lock on a file cargo or git is rewriting,
  which was the objection to syncing the working tree; it does not apply here.

### P2 — upstream: reconcile with open PRs BEFORE submitting

**Method matters here.** A first pass asked the GitHub API how many PRs were
open and was told 5, none touching firmware. The real number is **556 open**,
and Joe found a firmware one by hand within a minute. The reliable method is
git, not a summarised API page:

    git fetch origin "refs/pull/*/head:refs/remotes/pr/*"
    # for each ref: skip if merge-base --is-ancestor <sha> origin/main
    # then: git diff --name-only origin/main...<sha> -- <our files>

That found 40+ unmerged PRs touching `firmware/`, and a dozen touching our
exact files. Findings so far:

- **PR 1760 — OPEN, competing, and we each have the better half.**
  `fix(provision): owner-only state files, chip-identity binding, and
  --ota-psk`. Conflicts with ours in `provision.py`.
  *They* bind provisioning state to the board's MAC via `esptool read-mac`,
  which we do not do inline. *We* remove secrets from the state file entirely;
  1760 still lists `password` and `seed_token` in `MERGEABLE_ATTRS`, so it
  makes owner-only a file that still holds the WiFi passphrase in cleartext.
  **Decision (Joe, 2026-09-03): build on 1760, keep their identity binding,
  layer our secrets-removal on top.** Both improve, and it removes work from
  our side.

- **PR 1734 — OPEN, complementary, merges clean.** Moves the MAC filter above
  the CSI rate gate. Tested: merges cleanly with our mesh-aligned gate, and
  the combined order (filter, then bucket-gate) is correct. Their fix is
  arguably a *prerequisite* for ours: our gate takes the first frame in a
  shared window, so a foreign frame claiming the bucket would reproduce the
  starvation they fix. Say so in our PR; it supports theirs.

- **PR 1683 — CLOSED, unmerged.** Did what our remote-config branch does for
  auth, but cleaner: makes `ota_check_auth` public by dropping `static`,
  rather than our wrapper indirection. **Adopt the technique even though the
  PR is dead.** Note also that 1683 tried to stop provisioning credentials
  leaking into command lines and local storage and was closed without
  merging -- that problem is still open upstream, and our secrets work
  addresses it.

- **PR 1696 — OPEN but stale** (created and last touched 2026-08-24, no
  activity since). Large opt-in BLE fusion path, compile-time disabled by
  default. Conflicts with us in `nvs_config.h`, `stream_sender.c`,
  `provision.py`, but they are additive collisions -- both sides appending --
  so whoever merges second resolves mechanically. Not a blocker.

**Still unexamined**, all unmerged and touching our files: 1717, 1647, 1605,
1594, 1593, 1498, 1418, 1391, 1292, 1288, 1286, 1193, 1159, 1150, 1142, 1129
and roughly twenty more. Check state and overlap before submitting anything
that touches `csi_collector.c`, `main.c`, `nvs_config.*` or `provision.py`.

### P2 — upstream, mostly done

- **P2.1 Fork and open the five PRs.** `contrib/*` branches are cut from
  `origin/main`, firmware-only, verified to build standalone. Needs a GitHub
  fork; `gh` is not installed and `origin` is upstream itself.
- **P2.2 Finish and document remaining firmware work** first, so the PRs land
  as a coherent set rather than a trickle.

### P3 — drift audit for the sensing server

- **P3.1 Establish what has drifted.** `main` is 91 commits ahead of
  `origin/main` and 0 behind, but nobody has audited *what upstream changed in
  the server* while we diverged. The firmware got a careful 0.8.8 merge; the
  server has had no equivalent pass. Deliverable: a list of upstream server
  changes marked take / skip / conflicts-with-ours.

### P4 — side-by-side binary swap

- Already partially proven: renaming the running exe before `cargo build` let
  the server serve continuously through two full compiles on 2026-09-02
  (health returned 200 throughout). The remaining work is a supervisor that
  stages, verifies and swaps — the same shape as the firmware OTA rollback.

## Blocked on a decision or an action outside the repo

- **Fork the repo and open the upstream PRs.** Five `contrib/*` branches are
  cut from `origin/main`, firmware-only, and verified to build standalone:
  `wifi-retry-watchdog`, `espnow-beacon-scaling`, `mesh-aligned-rate-gate`,
  `disable-unused-154-radio`, `remote-config`. `origin` is upstream itself and
  `gh` is not installed, so there is nowhere to push from. (2026-09-02)

## Node management UI

- **DONE 2026-09-02.** All three stages shipped: the server learns node IPs
  from UDP source addresses, proxies config and firmware to nodes with the PSK
  held server-side, and a Nodes tab exposes it. Verified end to end against
  node 3.
- **ADR-351 is now an accepted risk, not a pending question.** Joe elected to
  ship mutating controls unauthenticated and secure them later, so *anyone who
  can reach the web UI can reconfigure or reflash the fleet today*. The PIN is
  outstanding work, and its absence is a live exposure rather than a
  hypothetical one. (2026-09-02)
- **OTA image upload is not proxied.** The UI reads firmware version, partition
  and rollback state, but pushing a new image still goes directly to a node
  with `curl` or `ota_push`. Streaming a 1 MB multipart body through the proxy
  is a separate piece of work. (2026-09-02)

## Server updates without the downtime

- **Adopt a Chrome-style side-by-side binary swap** (Joe, 2026-09-02). Today a
  server rebuild costs several minutes of fleet blackout: Windows locks the
  running `sensing-server.exe`, so `cargo build` fails at the link step with
  "Access is denied", forcing stop -> compile -> start. Measured on
  2026-09-02: ~3 minutes of no ingest.

  The Chrome model is to install the new version alongside the old and swap on
  the next restart, so the compile never blocks the running service.

  **The cheap 90% of this is nearly free and worth doing first.** Windows
  permits *renaming* a running executable, only not deleting or overwriting
  it. So a build wrapper that renames `sensing-server.exe` to
  `sensing-server.old.exe` before invoking cargo lets the full compile happen
  while the old process keeps serving from the renamed file. Downtime then
  collapses from a whole compile to a stop/start — seconds.

  The fuller version adds a supervisor that stages a built binary, verifies it
  starts, and swaps on restart, with the previous binary retained for a manual
  revert. That is the same shape as the firmware OTA rollback landed on
  2026-09-02 (stage, prove, keep or revert), and worth reusing the reasoning
  from rather than designing fresh.

  Note the interaction with the fleet: nodes stream UDP and do not retry, so
  every second of server downtime is sensing data that no longer exists.

## UWB anchors for CSI ground truth (Joe, 2026-09-03)

Both Moto Tags have **UWB**, which changes this from RSSI guesswork to
centimetre-class ranging. No ESP32 has a UWB radio, so the nodes cannot use it
-- but the phone can.

**Inverted arrangement:** place the tags at surveyed fixed points as UWB
*anchors* and carry the phone. That is how UWB positioning is normally built
(fixed anchors, mobile device), and it makes the phone self-locating to a few
centimetres. Log that with timestamps and it is continuous, automatic ground
truth for CSI training -- versus 39 thumb taps, of which only 11 landed inside
a capture window.

This is the fix for what defeated the PR-1647 test: not a better algorithm, a
vastly better label set.

**Open question, hardware vs API:** the tags are capable; whether Android's
`android.uwb` API exposes ranging against Moto Tags to a third-party app, or
whether it is locked to Google's Find My Device precision-finding flow, is
unverified. Check that before buying anything or writing code.

Note this makes tags a *labelling instrument*, not worn infrastructure --
which answers the original objection that nobody wants to carry tech and the
animals will not wear collars. The tags sit on shelves; only the phone moves,
and only during a training session.

## BLE tags as an identity anchor

- **Static-address BLE tags would disambiguate what CSI provably cannot.**
  The recurring blocker is not "is something moving" -- that works -- it is
  "how many, and which one": Joe vs Amy vs a child, one person or two, a
  person or the rabbit. CSI cannot answer that, and the cross-receiver
  approach was closed after an empty house scored higher than an occupied one.

  The ESP32-C6 has Bluetooth 5.3 LE and can scan advertisements passively with
  no pairing. **Phones are useless for this** -- iOS and Android rotate their
  BLE address roughly every 15 minutes precisely to defeat it, as upstream
  ADR-341 states plainly. Cheap BLE tags do not rotate, so one per person and
  one on the rabbit's collar gives a persistent, unambiguous identity signal.

  **RSSI is not position**, so this is identity evidence, not localisation.
  Actual BLE ranging needs Bluetooth 6 Channel Sounding, which no ESP32 can
  do -- ADR-341 routes that through an external companion radio over UART.

  Value even so: labelled ground truth without a phone tapper, a way to score
  every existing capture retrospectively, and a hard answer to "was that one
  person or two" that no amount of CSI processing has produced.

  Cost to check: a handful of tags and a BLE scan task on one node. Watch the
  airtime -- BLE is a separate radio from WiFi on the C6, but they share an
  antenna path, and the 2.4 GHz band is already at 86% airtime.

## Baseline handling: what is actually there (corrected 2026-09-03)

An earlier note in this file claimed the firmware uses a latched
`mean + 3*sigma` threshold and that firmware and server baselines are
unreconciled. **That was a misreading** -- it described the scheme the code
explicitly replaced. Corrected here so the wrong version does not outlive it.

**Firmware (`edge_processing.c`) tracks a leaky minimum.** The
`EDGE_CALIB_FRAMES` (~60 s) warm-up only *seeds* a plausible starting value; it
does not latch. Thereafter:

    if (motion < s_floor)  s_floor = motion;        // descend instantly
    else                   s_floor *= EDGE_FLOOR_LEAK;   // climb slowly

So an empty, calm hour walks the floor down to true quiet and disturbances are
detected sooner -- the behaviour Joe expected and the code delivers. The design
note records the measurement behind it: the room's true floor sits ~14x below
where the animals register, and settling after human activity runs a couple of
hours, which no one-minute average can resolve.

The old latching scheme is described in that comment as unfixable by
construction: the node calibrates in the seconds after a human plugged it in,
so the window is contaminated and "nobody can leave a room fast enough to fix
it". History: `53115fb3` -> revert `4c9a22ae` -> `47731d02`. Per
`reference_espectre_lessons`, ESPectre still ships the original bug.

**A fleet re-baseline button exists.** `ui/components/RoomBuilderTab.js:453`
"Recalibrate All Nodes" posts to `/api/v1/calibrate`, which fans
`POST /calibrate` out to every node. The leave-the-house-and-press-it workflow
is built.

**What remains genuinely open** is narrower than the earlier note claimed: the
server keeps its own separate baseline (`BASELINE_WARMUP = 50` frames,
`alpha = 0.003`, subtracted at 0.85) applied on top of the node's already-floored
output. That layering is real and untested, and it is the thing to control for
in any retest of the ratio-vs-subtract question -- not a conflict between three
rival formulations.

## Contribution branches: eleven topics, not eight

Cut fresh from `origin/main` on 2026-09-03 after the earlier five were found
stale. Joe caught two topics missing from the list and a third branch acting as
a bucket; the audit below is content-level, because **file-level coverage lies**
-- it hid the rollback gap earlier the same evening (`ota_update.c` appeared
"covered" while the rollback commit was in no branch at all).

### Cut and verified

| branch | state |
|---|---|
| `contrib/disable-unused-154-radio` | clean, 1 file |
| `contrib/mesh-aligned-rate-gate` | clean, 1 file |
| `contrib/thermal` | clean, 6 files, **builds standalone** |
| `contrib/adaptive-floor` | hand-separated, build pending |

### Cut but conflicted -- need individual resolution

| branch | conflicts in |
|---|---|
| `contrib/wifi-retry-watchdog` | `Kconfig.projbuild` (stacks on `contrib/thermal`, it edits `thermal.c`) |
| `contrib/remote-config` | `CMakeLists.txt`, `ota_update.c` |
| `contrib/rollback` | `main.c`, `ota_update.c/h`, `sdkconfig.defaults.16mb` (needs `remote-config` beneath it) |
| `contrib/provisioning-tooling` | `.gitignore` -- which should not travel with it at all |

### MUST BE SPLIT -- `contrib/espnow-beacon-scaling` is a bucket

It carries three unrelated topics. Verified content-level: none of the three
exists upstream.

1. **ESP-NOW beacon scaling** -- the actual subject of the branch.
2. **256-bin HE20 subcarrier grids** (`EDGE_MAX_SUBCARRIERS 256`, C6/C5).
   Possibly the most broadly useful contribution in the whole set: upstream
   caps at 128, which silently truncates HE20 CSI on any C6 or C5. Anyone
   running this firmware on newer silicon loses half their subcarriers and has
   no indication of it.
3. **Vitals slots packet** (`EDGE_VITALS_SLOTS_MAGIC 0xC5110009`).

### Deliberately excluded from every branch

`board_index.json`, `provision_conf*.json`, `partitions_16mb.csv`,
`sdkconfig.defaults.16mb`, `case/`.

### Method note

The adaptive-floor branch could not be cherry-picked. Its history is
`53115fb3` (introduce) -> `4c9a22ae` (revert) -> re-introduced inside
`ba454775`, a commit titled "model storeys and walls" -- a firmware fix that
rode in on a room-geometry commit. It was rebuilt by hand from upstream's file
plus only the floor hunks, leaving the 256-bin and vitals changes in the same
file untouched. Expect the same for the other tangled ones.

### Full symbol audit, 2026-09-03 -- fourteen topics, not eight

Joe asked whether a line-level audit was needed. It was. Everything before this
was file-level coverage plus spot-checks, and file-level coverage **lies**: it
reported `edge_processing.c` "covered" while three topics hid inside
it, and reported  covered while the rollback commit sat in no
branch at all. Joe found two missing topics by memory before the audit found
three more.

Method: enumerate every  and function signature added to
 between  and . 71 new
symbols, 14 new files.

Topics found that were NOT on the branch list:

- **ESP-NOW recovery** -- `espnow_recover`, `ESPNOW_STALL_US`,
  `ESPNOW_MAX_INFLIGHT`, `ESPNOW_FAIL_BEFORE_RECOVERY`. A stall detector and
  recovery path, wrongly lumped in with beacon scaling; separate concerns
  sharing a file.
- **CSI wire v3** -- `CSI_MAGIC_V3`, `CSI_HEADER_SIZE_V3`,
  `CSI_MAX_FRAME_SIZE`. Carries the 802.11 rx_seq to the server, which is what
  makes cross-node frame pairing possible at all. Upstream is still on v2.
- **Diagnostics / census** -- `diag_census_record`, `diag_census_dump`,
  `diag_census_dump_one`, `diag_survey_visible_aps`. Opt-in, off by default.

Full topic list (14): beacon-scaling, espnow-recovery, csi-wire-v3,
diagnostics-census, adaptive-floor, subcarrier-grids-256, vitals-slots,
led-runtime-control, remote-config, rollback, thermal, wifi-retry-watchdog,
provisioning-tooling, disable-154-radio.

**Lesson for the remaining work:** verify coverage by SYMBOL, never by
filename. Three of the fourteen were invisible to a filename check.

## Sensing-server audit, first real pass (2026-09-03)

Symbol-level, not spot-checked. Everything outside `firmware/` and `docs/`.

**Scale: 49 files, 21,334 insertions, 3,437 deletions.** The server delta is
roughly seven times the firmware's by symbol count.

    507 new symbols   371 functions, 104 constants, 32 types

### Four entirely new modules (3,332 lines)

| module | lines | pub fns | purpose |
|---|---|---|---|
| `links.rs` | 1357 | 11 | per-link CSI state, keyed by (receiver, transmitter) |
| `rti.rs` | 826 | 5 | link-line radio-tomographic position estimation |
| `phase_diag.rs` | 600 | 12 | phase-channel diagnostics |
| `fusion.rs` | 549 | 9 | cross-node frame pairing on (transmitter, rx_seq) |

`fusion.rs` is the server half of `contrib/csi-wire-v3`. Those two must be
sequenced together, receiver first -- see the ADR-138 note above.

### Tests

We added **268 tests** across these files. Upstream's `main.rs` carries 90.
So the test count in the files we touched is roughly tripled, which is the
strongest argument available that this work is contributable rather than
merely local.

| file | tests |
|---|---|
| `main.rs` | 196 |
| `links.rs` | 29 |
| `rti.rs` | 22 |
| `fusion.rs` | 13 |
| `phase_diag.rs` | 8 |

### Other areas, not yet enumerated by symbol

`ui/` (20 files), `v2/data/walktest/` (7 analysis scripts),
`wifi-densepose-hardware` (2), `wifi-densepose-signal` (1). The UI is the
largest untouched area and needs the same treatment.

### What this changes about sequencing

The four new modules are natural PR boundaries and are already separated by
file, unlike the firmware where fourteen topics shared six files. That should
make server packaging considerably easier than firmware packaging was --
provided `main.rs` changes can be attributed to the right module, which has not
been checked yet.

## Contribution branches: 9 of 14 packaged (2026-09-03)

All nine verified building standalone against `origin/main` in the ESP-IDF v5.4
container. Each is one topic, applied to upstream's code as it stands.

| branch | files | stacked on |
|---|---|---|
| `contrib/disable-unused-154-radio` | 1 | - |
| `contrib/mesh-aligned-rate-gate` | 1 | - |
| `contrib/adaptive-floor` | 2 | - |
| `contrib/subcarrier-grids-256` | 1 | - |
| `contrib/thermal` | 6 | - |
| `contrib/remote-config` | 10 | - |
| `contrib/provisioning-tooling` | 5 | - |
| `contrib/rollback` | 10 | `remote-config` |
| `contrib/wifi-retry-watchdog` | 11 | `thermal` |

Stacked branches must SAY SO in the PR description; they will not apply alone.

### csi-wire-v3 answers a stated blocker in upstream ADR-138

Worth leading the PR with this rather than describing the feature.

Upstream **ADR-138** (WiFi-7 MLO LinkGroup / ArrayCoordinator clock-quality
gating, ruv, 2026-05-28) is *"Accepted -- partial (built + tested building
block; integration glue pending)"*. Its own limitations section states the
blocker:

> "Until they land, the coordinator can be tested with synthetic
> ClockQualityScores but cannot be wired end-to-end. The `mesh_aligned_us`
> plumbing exists today only in the sensing server, not in a shared FrameMeta."

Upstream's `csi_collector.h` contains **zero** references to `rx_seq`. They
built the fusion mathematics; nothing produces the per-frame identity it needs.

That is the half this work supplies: CSI wire v3 carrying the 802.11 `rx_seq`,
pairing on `(tx_mac, rx_seq)`, and the mesh-aligned rate gate.

The gate matters as much as the wire format, and the measurement is the
argument: with the per-node elapsed gate, two boards side by side heard **72%
of frames in common but accepted only 25%** (2026-08-30). Carrying `rx_seq`
without fixing the gate would still have thrown away three quarters of the
pairing.

So the claim is not "we built this first" -- it is "this answers the blocker
ADR-138 names". `contrib/mesh-aligned-rate-gate` is already cut and should be
referenced alongside it.

### The five still to cut

- **`csi-wire-v3`** (commit `857cc179`). Conflicts in `csi_collector.c/h` AND
  `v2/.../main.rs`. The server conflict is inherent, not a mistake -- a wire
  format needs both ends, so this branch legitimately spans firmware and
  server. Biggest of the five. Highest value: rx_seq is what makes cross-node
  frame pairing possible, and upstream is still on v2.
- **`espnow-recovery`** and **`espnow-beacon-scaling`**. Both live in
  `c6_sync_espnow.c`, and commit `3a6018a6` contains BOTH ("stop the ESP-NOW
  queue wedge and size the beacon to the gate"). Needs hand-separation like
  `adaptive-floor` got: stall detection/recovery is one concern, beacon period
  derivation is another.
- **`vitals-slots`** (`EDGE_VITALS_SLOTS_MAGIC`, `edge_processing.c/h`).
- **`diagnostics-census`** (`diag_census_*`, `diag_survey_visible_aps`;
  `csi_collector.c` + `main.c`). Opt-in and off by default, so lowest value of
  the five.

### Method that worked

Every conflict so far was an unrelated topic riding along in a shared file --
`CSI_SEQ_DIAG` into thermal, `thermal.c` into remote-config,
`edge_processing.h` into rollback, `CSI_GATE_MESH_ALIGNED` into the watchdog,
enclosure STLs into provisioning. Not one was a real disagreement about the
same code. Resolve by keeping only the hunk belonging to the branch's topic,
then verify by grepping the branch for the OTHER topic's marker.

Two traps hit while doing this, both worth avoiding on the remaining five:
`git add <directory>` on a branch whose `.gitignore` has been reduced will
evaluate build trees; and `git reset` to undo it silently unstages real work
(it unstaged the provision.py secrets removal, which was only noticed by
checking the tree rather than trusting the commit).

## Nice-to-offer, low priority (Joe, 2026-09-03)

Neither is worth holding up the fourteen contribution topics.

- **16 MB partition layout.** Already written in upstream's own style
  (`partitions_16mb.csv`, `sdkconfig.defaults.16mb`), documenting which boards
  it is for and why the 4 MB and display layouts do not substitute. Offer as an
  additional option alongside their existing ones, never as a default -- a
  smaller table written over a deployed 16 MB node relocates NVS and destroys
  its provisioning.

- **Enclosure.** Offer the two print files for this specific board and nothing
  else -- no parametric generator, no build system. **Attribute the original
  maker**: the shipped case derives from a downloaded STEP model, not from
  `case/make_case.py`, which is superseded and unprintable. Confirm the
  attribution before publishing anything.

## Solver ignores position uncertainty

- **`uncertainty_m` is stored, validated, round-tripped -- and dropped.**
  `approved_emitter_positions()` returns `HashMap<[u8;6], [f32;3]>`: position
  only. So an emitter surveyed to +/-1 ft and one estimated to +/-25 ft carry
  identical weight once approved.

  Found 2026-09-03 while adding the two neighbouring houses as exterior
  illuminators. Their positions are known to roughly a house-width, which is
  useful as *directional* evidence -- something to the west lit up -- and
  useless as a trilateration anchor. With no weighting there is no way to say
  that, so they are recorded as `pending` with positions attached: the data is
  captured, the solver cannot use it.

  Worth fixing, because exterior illuminators are geometry no internal emitter
  can produce (links crossing the outer walls and the full width of the house),
  and 6 of 9 nodes are illuminator-blind to the NW. Until the solver weights by
  uncertainty, that geometry stays unusable.

## Fleet health, unexplained

- **Node 8 sits at -90 dBm**, worst in the fleet, despite notes recording it as
  a strong performer after it came out of the cabinet. Either it moved, or
  something changed around it. (2026-09-02)
- **Fleet-wide fps dropped** from 30-55 earlier on 2026-09-02 to 10-26 that
  night, on every node at once. Could be a quiet house, could be a transmitter
  population shift. Run `v2/data/walktest/population_fingerprint.py` before
  comparing any measurement across that boundary. (2026-09-02)
- **Nodes 3 and 4 both claim mesh leadership** and did not resolve over 60 s.
  Consistent with the "demotion flaps" already on record; not proven to be
  caused by the node 3 reboots. (2026-09-02)
- **`sendto ENOMEM` floods on a weak link.** Node 3 at -84 dBm could not
  complete a 1 MB OTA at all; at -70 it completed in 61 s. Worth knowing that
  OTA is effectively gated on link quality. (2026-09-02)

## Firmware

- **Soak node 3 before rolling remote config to the other eight.** It is the
  only node on the new firmware. (2026-09-02)
- **Put fuzzies back in its room.** It is on a USB cable at the desk for the
  rollback test; its surveyed position feeds the geometry work, so it must go
  back to exactly where it was. (2026-09-02)

## Geometry

- **Family/kitchen wall is disputed by 19 inches**, and node 5's x was placed
  by hand as a stopgap. One tape pull settles it.
- **Nodes 2 and 5 are untested** for the same 16-inch south-wall anchoring
  error that node 0 had.

## Repo hygiene

- **`phase2-rxseq-fusion` is unmerged** — one commit, firmware-side CSI frame
  selection by rx_seq. Real planned work, not abandoned.
- **Stale branches** (`backup/pre-upstream-merge-20260902`,
  `phase3-adaptive-calibration`, `link-sensing-and-nine-node-prep`) are all
  contained in `main` now and could be pruned, but nothing is deleted without
  asking. (2026-09-02)
