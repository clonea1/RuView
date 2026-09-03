# RuView backlog

Open work that is not captured by code, tests, or an accepted ADR.

The point of this file is that it survives a conversation ending. Anything
living only in a chat log or an assistant's memory is lost the moment the
session is; anything here is in git and readable by a human. Decisions and
designs belong in `docs/adr/`; this file is for *work items*.

Keep entries short and dated. Delete them when done — git remembers.

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
