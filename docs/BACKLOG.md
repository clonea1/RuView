# RuView backlog

Open work that is not captured by code, tests, or an accepted ADR.

The point of this file is that it survives a conversation ending. Anything
living only in a chat log or an assistant's memory is lost the moment the
session is; anything here is in git and readable by a human. Decisions and
designs belong in `docs/adr/`; this file is for *work items*.

Keep entries short and dated. Delete them when done — git remembers.

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
