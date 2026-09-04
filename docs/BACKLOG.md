# RuView backlog

Open work that is not captured by code, tests, or an accepted ADR.

The point of this file is that it survives a conversation ending. Anything
living only in a chat log or an assistant's memory is lost the moment the
session is; anything here is in git and readable by a human. Decisions and
designs belong in `docs/adr/`; this file is for *work items*.

Keep entries short and dated. Delete them when done — git remembers.

## PLANNED: move the sensing server to the NAS (Docker)

**Decided 2026-09-03 (Joe). Not started — filed deliberately rather than built.**

### Why the NAS, and why it is better than the desktop

TerraMaster **F6-424 Max**, `TERRY` at `192.168.1.27`, always on regardless.
Confirmed by `lscpu -e`: **Intel i5-1235U**, a HYBRID part.

| CPUs | max MHz | type |
|---|---|---|
| 0,1,2,3 | 4400 | 2 P-cores, hyperthreaded |
| 4..11 | 3300 | 8 E-cores |

The sensing server is **effectively single-threaded**: measured 2026-09-03, one
thread at **94.3%** of an i7-7700K core with every other thread idle, and only
54 MB resident. So the whole question is which core that thread lands on:

  - **P-core** (Golden Cove, ~1.4x the i7 per clock): ~67% of one core. Fits,
    with headroom for growth past nine nodes.
  - **E-core** (Gracemont at 3.3 GHz, ~0.78x): ~120% of one core. **Would never
    keep up** — and nodes stream UDP without retry, so falling behind loses
    frames rather than delaying them.

**Therefore the container MUST be pinned to the P-cores.** `cpuset: "0-3"` in
compose, or `--cpuset-cpus="0-3"`. Do not rely on the scheduler; a NAS kernel
may lack Intel thread-director support. Verify once running with
`ps -o pid,psr,pcpu,comm -T -p $(pgrep sensing-server)` — `psr` must show 0-3.

### It also fixes a defect rather than only relocating

`docker/docker-compose.yml` records that on **Docker Desktop for Windows**,
multi-source UDP collapses to one source IP at the WSL/Hyper-V boundary, so all
but one node's frames are silently dropped (issues #374, #386). Native Linux
does not have this. Containerising on Windows would have hit it; the NAS does
not.

### What exists already

`docker/Dockerfile.rust` (multi-stage, builds sensing-server with the `mqtt`
feature) and `docker/docker-compose.yml`. No cross-compile config, but building
inside the container makes that unnecessary.

### The pipeline Joe asked for, not yet built

Requirement: automate deployment on every stack build. Intended shape, mirroring
the two staged-swap mechanisms already proven here (firmware OTA rollback, and
the rename-then-build binary swap):

1. build the image on the desktop from `docker/Dockerfile.rust`
2. ship it — `docker save | ssh clone@TERRY 'docker load'` avoids needing a
   registry and keeps the tree off any remote, which matters because this repo
   must not be pushed anywhere public
3. retag the running image `:previous` before swapping
4. `docker compose up -d` with `cpuset: "0-3"`
5. health-check `/api/v1/mesh`; on failure retag `:previous` back and restart

**Prerequisite not yet met:** there are no SSH keys on this workstation
(`~/.ssh` is empty — no `id_ed25519`, no `known_hosts`). Key-based auth to
`clone@TERRY` has to be set up before any of this can be non-interactive.

### Open question that decides how much headroom there is

The 94% single-thread cost has never been profiled. ~450 frames/second at
roughly 9 million cycles each is high enough to be worth understanding before
scaling past nine nodes. On a P-core it fits either way; the profiling decides
whether there is room to grow.

## OPEN: USB pass for the recovery bootloader

**Blocked on physical access, one board at a time. Not urgent, not optional.**

`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` is a BOOTLOADER capability. OTA
replaces the app partition (`0x20000`) only, so this cannot be deployed over
the air on any board. The bootloader sits at `0x0` and the partition table at
`0x8000`.

Until a board has it, `ota_rollback_boot_check()` and the `PENDING_VERIFY`
handling in `ota_update.c` are inert on that board: a bad OTA there does NOT
self-revert, and recovery means a cable.

**Which boards already have it is unknown and cannot be determined remotely.**
No bootloader version is reported on the wire. The sync packet carries app
health, not bootloader capability. Answering this from `/api/v1/mesh` health
data was tried on 2026-09-03 and was wrong.

### The pass

Per board, with `firmware/esp32-csi-node/RUNBOOK.md` open:

    python -m esptool --chip esp32c6 -b 460800       --before default_reset --after hard_reset write_flash       --flash_mode dio --flash_size 16MB --flash_freq 80m       0x0     build/bootloader/bootloader.bin       0x8000  build/partition_table/partition-table.bin       0xf000  build/ota_data_initial.bin       0x20000 build/esp32-csi-node.bin

`0x9000` is untouched, so SSID, password and `node_id` survive.

**Record which boards have been done.** That list is the only place the answer
will exist.

### Do it once, get both

The TX-path counters (proto v3, `send_fail` / `rate_skip` / `early_drop`) are
app-only and CAN ship by OTA. But since every board needs a USB visit for the
bootloader anyway, flashing v0.8.8 in the same pass covers both and avoids a
second round.

If the counters are wanted sooner: OTA them to one node, soak, then the fleet.
Safe on an old bootloader -- `ota_rollback_boot_check()` is guarded on
`esp_ota_get_state_partition()` returning `PENDING_VERIFY`, which simply never
happens there, so it no-ops. The caveat is that there is no rollback net on
those boards, so a bad image needs a cable.

### Ready to flash

    bootloader.bin        19,696 bytes   rollback ENABLED
    esp32-csi-node.bin 1,066,256 bytes   v0.8.8, proto v3 counters
    partition-table.bin    3,072 bytes   16MB, two 4MB OTA slots

All fourteen firmware topics verified present in the image. Backout is intact:
`_revert_backup/ota_1_production.bin` (v0.8.4) and `_revert_backup/otadata.bin`.

## CORRECTION: there IS pending USB work, and a stale sdkconfig

**I said "no pending USB flash". That was wrong.** Joe corrected it: the USB
requirement was for the RECOVERY path, which cannot go over OTA.

OTA replaces the APP partition only (0x20000). The bootloader lives at 0x0 and
the partition table at 0x8000; neither is reachable over the air.
`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` is a BOOTLOADER capability, so a node
whose bootloader predates it cannot roll back a bad image no matter what the
app does -- and `ota_rollback_boot_check()` / the `PENDING_VERIFY` handling in
`ota_update.c` are written expecting it.

What I checked was app-level health reporting, which every node has. That says
nothing about the bootloader. **The right question was never "is the app
current" but "which nodes have a rollback-capable bootloader", and that is not
reported anywhere** -- there is no bootloader version on the wire, so it cannot
be determined remotely at all.

### The stale sdkconfig, found while building

The working `sdkconfig` did not match the fleet:

    active sdkconfig          fleet / defaults
    FLASHSIZE 4MB             16MB
    partitions_4mb.csv        partitions_16mb.csv
    DYNAMIC_TX_BUFFER_NUM=64  128

Anything built from it would have been a 4MB image with the buffer change
absent -- i.e. silently NOT the configuration under test.

**It was backed up, not deleted**, before regenerating. The documented build
command begins `rm -rf build sdkconfig`, which is exactly the pattern that lost
`partitions_16mb.csv`; `CLAUDE.md` warns about it *even when a README documents
it*, and that warning earned its place again here.

A second trap: `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` lives in
`sdkconfig.defaults.16mb`, NOT in `sdkconfig.defaults.esp32c6`. Building from
`defaults + esp32c6` alone produces a rollback-DISABLED image that looks
correct in every other respect. The correct combination is
`sdkconfig.defaults + sdkconfig.defaults.16mb + sdkconfig.defaults.esp32c6`.

### Built and verified

    bootloader.bin        19,696 bytes   ROLLBACK ENABLED
    esp32-csi-node.bin 1,066,256 bytes   proto v3 counters
    partition-table.bin    3,072 bytes   16MB, two 4MB OTA slots

## ALL 37 TOPICS CUT. Final privacy sweep, including commit messages

Topic 27 was the last: `server-classification-stability`, two genuine upstream
fixes.

  - Upstream's `DEBOUNCE_FRAMES: u32 = 4` carries the doc comment
    "at ~10 FPS = ~0.4s". ESP32-C6 boards run at 48-50 FPS, so the real window
    is ~0.08 s and a ~1 s motion EMA becomes ~0.2 s. Per-frame noise then flips
    the reported level almost directly. Fixed by deriving the thresholds from
    each node's own measured `csi_fps_ema` rather than hardcoding a different
    fixed rate, which would only move the bug to the next chip that diverges.
  - `fuse_room` recomputed its plurality vote with no memory of its own, so
    with per-node confidences near the boundary (44%, 83%, 53% observed
    simultaneously) whichever two of three agreed could flip the room result
    while every node's own reading was stable. Everything gating on presence
    then flapped, and each of those looked like its own separate bug.

### The sweep found four branches carrying identifiers

Run across every branch's DIFF, its FILENAMES, and -- newly -- its COMMIT
MESSAGES:

| branch | what | fix |
|---|---|---|
| `remote-config` | a real node IP in four README and four `config_push.py` examples | `<node-ip>` |
| `rollback` | a real node IP **inside a commit message**, in a quoted boot log | genericised, provenance line dropped |
| `server-rti` | a doc comment describing this deployment as the motivation | generalised to any building |
| `provisioning-tooling` | reviewed, no change: `192.168.1.10` sits beside `"thisismyssid"` and `D:\path	o\...` and is plainly illustrative |

**A commit message ships with the branch.** Screening diffs is not enough, and
this was found only because the sweep was widened to messages.

One accepted false positive: `rti.rs` asserts `"west wing"` and `"east wing"`
as labels for two synthetic disjoint polygons at x 0-4 and 8-12. Ordinary
geometry vocabulary, no relation to any real building. Left as-is -- degrading
readable test code to satisfy a grep is the wrong trade -- and recorded here so
the next sweep does not re-investigate it.

### Durability check while in the area

`CLAUDE.md` names `board_index.json`, `partitions_16mb.csv` and `sdkconfig` as
irreplaceable. Verified on `main`: the first two are **tracked**, so the hole
that lost `partitions_16mb.csv` is closed. `sdkconfig` remains untracked and
ignored; it is regenerable through menuconfig, so this is noted rather than
raised.

The `.gitignore` entries on `provisioning-tooling` are correct FOR UPSTREAM --
`board_index.json` and `provision_conf*.json` are per-fleet and should not land
in a shared repository. They do not weaken the local tree, because a
`.gitignore` rule has no effect on an already-tracked file.

**37 branches, all build-verified, all full-suite green, all screened.**

## Round 6: fusion completed, world3d, node-MAC registry

**36 branches.** Only topic 27 (`classification-smoothing`) remains uncut.

### `server-fusion` finished -- it was never fed

Marked PARTIAL for good reason. `/api/v1/fusion` READ `fusion_index` while
nothing ever wrote to it: no `observe` call existed anywhere. The branch also
sat directly on `origin/main`, so it had no `rx_seq` to pair on in the first
place.

Rebased onto `server-wire-v3` and the ingestion wired, running BEFORE the
per-node grid gate for the same reason the link path does -- pairing is keyed on
transmission identity, not time, so it must see frames the per-node feature
path discards.

**Fourth instance of declare-check-never-populate.** The pattern is now
unmistakable enough to be a checklist item rather than a series of lucky
catches.

### `world3d` (topic 31) found a real robustness bug

It fetched five endpoints through `Promise.all`, so **any single failure blanked
the entire scene** -- a server built without the fusion endpoint, or one
transient 500 on any route, rendered nothing at all rather than the four sources
that answered.

Rewritten to fetch independently, with failures yielding null and the panel
naming which sources did not respond. That also removed the need to force a
messy five-way merge of the links and fusion stacks: the page now degrades
honestly instead of requiring everything at once.

Guarded every `cfg` dereference while doing it, since a null room config would
otherwise throw where it previously could not.

### Node-MAC registry cut, and a reminder that commit MESSAGES travel

`66330ff9`'s body contains a real MAC from this deployment. **A commit message
ships with the branch**, so the message was rewritten from scratch rather than
reused -- the privacy screen has to cover messages, not only diffs.

The commit was entangled with RTI and room-builder code across five conflict
blocks, so the topic was rebuilt by hand instead: registry field, populate from
the sync packet, and attribute by address with the hearing-set heuristic kept
only for a fleet where nothing has reported a MAC yet.

Its evidence is worth keeping: the heuristic attributed **0 of 32 links** with
nine nodes in a house, and reported every peer link as infrastructure. A
misattributed link is still a link, so nothing downstream reported that the
geometry it received was wrong.

## Round 5: three branches, and real MACs found in test fixtures

**34 branches.** `ui-room-builder-footprint`, `server-emitter-triage`, plus the
collapsed geometry work.

### `e2a7a9e4` split three ways, cleanly by file

| file | lines | topic |
|---|---|---|
| `RoomBuilderTab.js` | 388 | footprint editor -- storeys become polygons |
| `world3d.html` | 649 | **topic 31**, which was hiding in here |
| `illuminators.html` | 291 | emitter triage, topic 24 |

Topic 31 was listed as a separate future item; it was inside this commit all
along.

### PRIVACY: real MACs and a neighbour's name in Rust test fixtures

The emitter tests carried, in `main.rs`:

  - three real MACs from this deployment, including the utility gateway
  - a variable literally named `sue`, and a label reading `"Sue's house"` --
    a NEIGHBOUR, not even a member of the household
  - a label `"TEG-394 (basement)"`, naming a real device and its location

All replaced with locally-administered example MACs (`02:00:00:00:00:0x`) and
generic labels (`neighbour AP`, `utility gateway`). Also removed a set of
`ap_mac` tests that belonged to a different topic AND carried another real MAC.

**The lesson is where these were.** Every screen so far had been aimed at UI
files, HTML and docs -- the places house data was expected to live. These were
in Rust test fixtures, and they would have passed every check run to date.

**Screen `.rs` fixtures too, not just UI and docs.** The pattern
`([0-9a-f]{2}:){5}[0-9a-f]{2}` finds MACs in any language, and variable names
deserve a look as much as string literals: `let mut sue = ...` is a person's
name in code, and no MAC-shaped regex would ever have found it.

### Emitter triage depends on two stacks

`illuminators.html` calls `/api/v1/config/room` (room-builder) AND
`/api/v1/links/inventory` (server-links) -- two independent stacks. They merge
cleanly, so the branch carries an explicit merge commit rather than pretending
one depends on the other.

### Dead-wiring caught twice more, proactively

`excluded_emitters` was declared and checked at ingestion while nothing
populated it. Populated on config save -- and then found still missing at
STARTUP, which would have meant a restart silently re-admitting every emitter
already judged as moving. Both wired, with the reasoning in comments.

This is the third instance of declare-check-never-populate. It is worth
treating as a standard question rather than a lucky catch: **what writes to
this, and when?**

## Do not introduce a thing and then fix it (Joe, 2026-09-03)

I wrote the rule for the deadband -- *a fix to code that does not exist
upstream is not a separate PR, it is the initial implementation being correct*
-- and then immediately broke it building the Room Builder stack, which
introduced the feature and then fixed it. Joe caught it.

Upstream has never seen the Room Builder, so a commit reading "the round trip
loses storey data" describes a bug that, from their side, never existed. It
also reads badly: it tells a reviewer the thing being handed to them was
broken.

### Audit of all 32 branches

Six had a `fix` commit in a stack. **Three were legitimate** -- verified by
checking that upstream actually has the bug:

| branch | fixes | upstream evidence |
|---|---|---|
| `wifi-retry-watchdog` | WiFi gives up permanently | `main.c:62` `#define MAX_RETRY 10` |
| `rollback` | httpd stack too small for OTA validation | `HTTPD_DEFAULT_CONFIG()` in their `ota_update.c` |
| `ui-room-builder` | hardcoded node position | `[2.0, 0.0, 1.5]` in three places |

Each of those sits at the BOTTOM of its stack, which is the right order: fix
the upstream bug, then build on it.

**Three were self-fixes and were collapsed:**

  - `ui-room-builder-geometry` -- 4 commits into 1. Storeys, walls and AP
    placement introduced already serving every persisted field.
  - `remote-config` -- 3 commits into 1, folding in both a self-fix and a chore
    correcting its own default. `rollback` rebased onto it.
  - `server-doppler-deadband` -- retitled from `fix` to `feat`. The commit
    introduces `node_doppler_sample`, so it cannot be fixing it. The bin-vs-m/s
    arithmetic is kept as design rationale rather than bug history, since it is
    the part a reviewer should check.

### Squashes verified to preserve content

History rewriting must not change what ships:

  - `remote-config`: tree hash IDENTICAL before and after (`fcba125d`).
  - `ui-room-builder-geometry`: 136 files differ, ALL of them under `v2/data/`
    -- exactly the CSI captures removed by the earlier correction, and nothing
    else.

### The tell

A `fix` commit whose subject names something introduced in the same stack. Ask
of every one: **does upstream have this bug?** If not, it is not a fix, and the
introduction should simply be correct.

## HAZARD: contribution branches inherit a WEAKER .gitignore

**Near miss, 2026-09-03.** A `git add -A ui v2` on a contribution branch staged
**136 CSI diagnostic files** from `v2/data/` into a commit. Caught before it
left the machine, commit rewritten, all 32 branches re-audited and clean.

### Why it happened, and why it will happen again

Our `main` ignores the capture directories:

    v2/data/recordings/     v2/data/diagnostics/     v2/data/walktest/*

`origin/main` ignores **none** of them. Every contribution branch is cut from
`origin/main`, so it inherits the weaker rules -- while the working tree still
holds every capture those directories accumulated. On `main` those files are
invisible to `git add`. On a contrib branch they are fair game.

This is the same hazard that nearly swept build trees earlier in this
programme, in a worse form: build trees are noise, CSI captures are the
category `CLAUDE.md` names explicitly as never-commit.

### The rule

**Never use `git add -A`, `git add .` or `git add <directory>` on a
contribution branch. Stage explicit file paths, always.**

Two cheap guards, both used to catch this one:

    git show <commit> --name-only --format='' | grep -c '^v2/data/'
    git diff --name-only origin/main..<branch>       | grep -cE '^v2/data/|\.csv$|\.jsonl$|room_config|board_index|provision_conf|\.env'

Run the second on every branch before any push. It is the check that proved all
32 clean.

### Worth fixing at the source

The pre-push hook is the durable answer, since it does not depend on
remembering. Whether the hook currently blocks a `v2/data` path on a branch cut
from `origin/main` has NOT been verified -- assume it does not until tested.

## Both pending decisions resolved (Joe, 2026-09-03)

### Deadband: shipped, but not in the shape the option described

Decision was "ship the deadband fix". Option 2 as I wrote it -- the deadband
fix *alone* -- turned out not to exist as a diff, and the difference matters:

  - upstream's signal crate DOES have `BvpConfig` with `max_velocity: 2.0` and
    `n_velocity_bins: 64`, so the 0.0625 m/s bin spacing that makes the bug
    real is **upstream's own default**
  - but upstream's `bvp.rs` has no zero-velocity deadband, because that belongs
    to whatever CONSUMES a BVP and decides how much of it counts as motion
  - and upstream's server never calls `extract_bvp` at all

So the fix only exists inside the consumer it corrects. Shipped as
`contrib/server-doppler-deadband`, stacked on `contrib/signal-signed-bvp`:
sanitized per-node phase retention, the signed Doppler sample with a
**bin-based** deadband, and the two figures surfaced on `/api/v1/nodes` as
DIAGNOSTICS. **No position tier is included** -- our own evidence says the
centroid does not spatially differentiate, so none of that claim ships.

### The test took four attempts, and three of them were worthless

First version asserted a static node reported less moving energy than a moving
one. It passed with the bug deliberately reintroduced -- because a perfectly
constant signal leaves nothing after DC removal, so it cannot detect a broken
deadband either way. Two further fixture designs also passed under the bug.

Rather than guess a fourth time, the values were **measured**:

    modulation        deadband = 0 (bug)   deadband = 2 (fixed)
    0.05 Hz (static)        19.13                 0.56
    2.00 Hz (moving)       310.89               122.59
    static as % of moving    6.2%                0.45%

Both sat under the 50% threshold, which is why it kept passing. The fixtures
were rebuilt to differ ONLY in rate -- identical modulation depth, so the
deadband is the only thing that can separate them -- and the bound set at 2%,
which the correct implementation clears by 4x and the broken one misses by 3x.
The measured table is in the test's doc comment so the bound is not a magic
number.

**A regression test that passes in both configurations is not a weak test, it
is not a test.** Three of these would have shipped as verified.

### Tapper: de-housed to Joe's design, now generic

Rebuilt to Joe's specification: one button per node the server actually
reports, from `/api/v1/nodes`, labelled "Node 1", "Node 2", sorted by id, built
dynamically so it fits any fleet size with no editing. Stand near a node, tap
its button.

Removed: the entire hardcoded floor plan (`EXTRA_PLACES`, room names,
coordinates, "the mud room sits at x=-12 ft in the west wing"), the dependency
on `/api/v1/config/room`, and a hardcoded LAN IP in a comment. Two generic
places remain -- "Elsewhere" and "Outside/away" -- because a fleet never covers
every space and the gaps need ground truth most.

An unreachable server or empty roster is reported as such rather than guessed
at: invented node ids would attach marks to nodes that do not exist.

Screened the **whole branch diff**, not the file: zero name, IP, room or
coordinate hits. Person labels remain role-based. Inline JS parses under
`node --check`.

**30 branches cut.** Remaining: 21 (room-builder, its own onion pass), 24, 27,
31, and `server-fusion` still incomplete.

## PRIVACY: the tapper cannot ship, and history is the reason

Topic 22 was cut as `ui-ground-truth-capture` carrying `mark.html`,
`survey.html` and `walk.html` only. **`tapper.html` was deliberately excluded**,
for two independent reasons.

### 1. A PR carries its history, not just its final state

`c3232c95` introduced the tapper with real names: family first names, "the
girls", a pet's name, and a description reading "the girls in the mud room at
19:43" -- a timestamped account of household activity. `9c5759cd` genericised
all of it to `adult1` / `adult2` / `child1` / `children-together` / `everyone`,
which is correct and is what the file contains today.

But cherry-picking both commits onto a branch would publish the names anyway,
because the diff that introduced them travels with the branch. **Checking the
working tree is not sufficient; the commit range must be checked.** Any branch
carrying this file must be squashed to a single commit built from the final
state, never assembled from that history.

### 2. The file hardcodes the house

Independently of names, `tapper.html` embeds the floor plan: room names with
coordinates, `EXTRA_PLACES`, "the mud room sits at x=-12 ft in the west wing".
That is household-identifying data regardless of what the occupants are called.

**Before the tapper can be offered upstream it needs its room list driven from
configuration rather than hardcoded**, which is real work, not a screening pass.
Pended.

The three pages that did ship were screened for the same things and are clean:
zero name hits, zero coordinate or room-name hits.

### Screening standard going forward

For any branch touching UI or data files, screen **the commit range**, not the
files:

    git log -p origin/main..<branch> | grep -inE "<names>|<pet names>|x=-?[0-9]+ ?ft|EXTRA_PLACES"

The final state being clean says nothing about what the branch publishes.

## Full-suite sweep of every server branch: 2 of 13 were broken

Two branches failed integration tests in a row, so every branch touching the
sensing server, hardware or signal crates was re-verified with the **full**
`cargo test`, not `cargo check` and not unit tests alone.

**Result: 11 pass, 2 fail.** Both failures are now fixed.

### `server-phase-diag` -- `--http-port` stopped existing

All 7 `auth_wiring` integration tests failed. The extraction inserted the
`phase_diagnostics` fields BETWEEN `#[arg(long, default_value = "8080")]` and
the `http_port` field it belonged to, so clap applied that attribute to the new
field and `http_port` became a **positional** argument. The server then rejected
`--http-port` and never started.

Same failure mode as the missing `CSI_MAGIC_V1`: a slice landing on the wrong
side of an attribute/doc-comment boundary. Unit tests all passed; only an
integration test that actually launches the binary caught it.

Verified against pristine `origin/main` first (7 passed) so the failure was
attributed to our branch rather than assumed pre-existing.

### `server-csi-clock` -- a test pinning the behaviour the commit removes

`mesh_timestamp_replaces_skewed_host_arrival_time` asserted the mesh clock
REPLACES host arrival. The commit's whole purpose is to stop trusting the mesh
clock, so the test was asserting the opposite of the change and upstream CI
would have failed on the first run.

Reading the code rather than just fixing the test confirmed the design is
coherent and only the test was stale:

  - **single frame** -- nothing to validate a lone mesh timestamp against, and
    an unconverged node returns one that is briefly WRONG rather than absent,
    so host arrival is the only defensible basis
  - **cohort** -- the spread across nodes IS checkable, so mesh time is kept
    and used only when it passes the guard interval

`mesh_time_allows_fusion_despite_udp_arrival_skew` and
`incoherent_mesh_timestamps_fall_back_to_host_arrival_for_the_cycle` both still
pass and pin the cohort half. The stale test was rewritten to pin the new
single-frame intent, with the reasoning in its doc comment.

### `server-fusion` remains INCOMPLETE

Does not compile: `missing field fusion_index in initializer of AppStateInner`.
Already known and recorded; not a new finding.

### Method

`cargo check` gates the binary. Unit tests gate library logic. **Only an
integration test that launches the built binary catches a CLI regression**, and
that is precisely what a mis-sliced `#[arg]` produces. All three are now the
standard for every branch.

## Round 3: two cut, one struck, and the UI onion

### Topic 26 `baseline-tuning` is OBSOLETE -- struck

`BASELINE_SUBTRACTION_FRACTION` was introduced by `7b63ee1e` (0.7 -> 0.95) and
**removed again** by `e5636cd9`. It exists neither upstream nor in our current
tree. The surviving baseline constants are byte-identical to upstream:

    BASELINE_EMA_ALPHA = 0.003        BASELINE_WARMUP = 50

There is nothing to contribute. This resolves the "three baseline formulations,
none reconciled" note: the server's third formulation no longer exists, leaving
the firmware's `mean + 3*sigma` and the server's EMA, which are the two the
earlier entry should have named.

### `server-node-management` cut, as two ordered commits

`85ba2092` needed a prerequisite nobody had listed: `bb776437`, which learns
each node's address from the source address on packets it already sends.
Nothing else in the server records where a node is on the network -- reaching
one meant a port scan. That is independently useful and goes first.

`bb776437` itself bundled a `node_macs` registry that needs
`hardware-sync-packet`; only the address half was taken.

**Correction to an earlier judgement.** This was nearly held back over ADR-351,
on the grounds that it ships an unauthenticated fleet-reflash. Reading the code
rather than the ADR: the mutating path **already fails closed**. `node_psk` is
`None` unless the operator passes `--ota-psk-file`, and without it only
read-only fleet views are served. The residual risk is narrower and is real --
once enabled, the server's UI becomes the trust boundary because it holds the
credential -- and it is stated in the commit message for reviewers. No gate
needed to be added; one already existed.

Internal docs (`BACKLOG.md`, ADR-351) were excluded: they name the operator,
and the rationale belongs in the PR description written neutrally.

### `ui-no-fabricated-data` cut -- the highest-value UI fix

`e0237ec0` is a **five-topic bucket**: the fabrication fix, a reconnect race,
`LinkMeshPanel` (belongs with server-links), a column layout fix, and three
ground-truth pages (topic 22). Only the first two were taken.

On a dropped socket the client fell back to built-in simulation and rendered
synthetic values indistinguishable from live ones. Everything downstream --
which nodes to move, whether a change helped -- was then judged against
fiction. Simulation is now opt-in, an unreachable server retries rather than
inventing, and the labels say what they mean: "INVENTED DATA - NOT MEASURED"
rather than "OFFLINE - CLIENT SIMULATION", which read as a status, not a
warning.

### Topic 21 `room-builder` is 13 commits and needs its own onion pass

Titles alone announce further splits: "footprint editor, 3D house view,
illuminator triage" is three; "attribute transmitters by MAC; add illuminator
roster UI" is two. Two of the 13 (`195f49ea`, `bef4f2ae`) depend on the
**pended** centroid tier, so they cannot ship before that decision.

None of the 13 touches a `.json` -- the house data lives in separate
`data(room):` commits which must never travel. Verified, not assumed.

### Verification gap CLOSED -- node installed

Node was absent from this machine entirely, which blocked syntax-checking the
UI JavaScript and was also the cause of the `SessionEnd` hook's
`node: command not found`.

Installed **Node v24.20.0 LTS, user scope, no elevation**:

  - official portable zip from `nodejs.org/dist`, SHA256 verified against that
    release's `SHASUMS256.txt` before extraction
  - extracted to `%LOCALAPPDATA%\Programs\nodejs`
  - added to the **user** PATH only; machine PATH untouched
  - reversible by deleting that one directory and the PATH entry

Note: already-running shells inherit the pre-change environment, so a session
started before the install needs `export PATH=...` per invocation. New terminals
and new sessions pick it up automatically.

All 8 JavaScript files across `ui-no-fabricated-data` and
`server-node-management` now **parse as ES modules** (`node --check`). The
earlier balance-and-imports check has been replaced by a real parse.

This also makes the harness validation matrix in CLAUDE.md runnable for the
first time (`npm ci --ignore-scripts`, `npm test`, `brain:verify`,
`manifest:verify`), which had been silently un-runnable.

**Now 27 branches cut. Remaining: 21 (needs its own pass), 22, 24, 27, 31, and
25 pended.**

## `cargo check` is not enough, and onion #3 was a phantom

### `contrib/hardware-sync-packet` was broken in isolation

It adds `node_mac` and `health` to `SyncPacket` but never updates the two
fixtures that construct one, in `multistatic_bridge.rs:326` and `main.rs:9286`.
`cargo check` passes because the BINARY compiles; `cargo test` compiles the
test cfg as well and fails with E0063. The branch had been recorded as
verified.

**`cargo check -p <crate>` is not a sufficient gate for an extracted branch.
Run `cargo test`.** Both fixtures are `proto_ver: 1` packets, which legitimately
carry neither field, so the fix is `node_mac: None` and
`health: Default::default()` with a comment saying why. Folded into the
hardware commit; `server-node-health` rebased onto it.

### Onion #3 `mesh-sync-surfacing` does not exist

Derived from `e5636cd9`'s title -- "per-link CSI metrics, mesh sync surfacing,
nine-node prep" -- without checking the diff. Verified now: our `mesh_endpoint`
and upstream's are **byte-identical**, and every `sync` match in that commit is
`std::sync::`, the Rust namespace. The mesh surfacing was already upstream.

The real difference in `NodeSyncSnapshot` is twelve lines of **node health**,
which is topic 30, not a mesh topic. Struck from the list.

Lesson symmetric to the earlier one: filing work by its title rather than its
diff invents topics as readily as it mislabels them.

### Topic 30 `node-health-telemetry` cut

`474733d1` spans four areas -- firmware `csi_collector.c`, the hardware crate,
the server, and `multistatic_bridge.rs`. Only the server half belongs here; the
hardware half is already `hardware-sync-packet`, and the firmware half is a
separate topic. Receiver ships before sender, as with wire v3: a node on older
firmware leaves the reserved bytes zeroed and reports no health.

### An error worth recording

Resolving that rebase, `git checkout --ours <file>` discarded the file's OTHER
changes, not just the conflicting hunk. The commit became empty and was dropped
silently -- the 22 lines of actual work vanished while the rebase reported
success. Recovered from the reflog and redone by resolving only the conflict
BLOCKS, keeping the rest of the incoming commit.

`--ours`/`--theirs` operate on whole files. When a commit has real content in a
file that also has a trivial conflict, resolve the block, never the file.

**Now 25 branches cut of 37 topics** (38 less the phantom).

## PEND FOR DECISION: topic 25 `centroid-position`

**Not cut. Needs Joe's call before it is offered upstream.**

Three topics collapse into one here. None of `node_doppler_weight`,
`motion_weighted_centroid`, `attach_positions`, `BVP_HOP_SIZE` or `extract_bvp`
exists upstream, so topic 25, onion #4 (`ap-position`) and onion #5
(`bvp-deadband-fix`) are all parts of a single new subsystem, not a feature
plus two fixes. **General rule: a fix to code that does not exist upstream is
not a separate PR -- it is the initial implementation being correct.**

### Why it is pended rather than cut

The deadband bug is real and well-diagnosed: `BVP_ZERO_VELOCITY_DEADBAND_MPS`
was 0.05 m/s while the default `BvpConfig` bin spacing is 0.0625 m/s
(2.0 * 2 / 64), so the deadband was narrower than one bin. It excluded only the
exact-zero bin, and the Hann-windowed STFT's DC leakage into adjacent bins
counted as motion by construction -- the metric sat at ~2.5-2.6 of 3.0
constantly, including while a sensor was covered by hand.

But `b441d70a`'s own body records that after the fix `total_weight` "stayed
elevated (still saturated, just less so) while the live dot rarely showed."
So the fix improved a metric that still does not discriminate.

### What is NOT in question

The falsified work is correctly contained. The **bistatic phase-Doppler** tier
is default-off, enforced by `bistatic_tier_is_disabled_by_default` and
announced in the logs. `doppler_weighted_centroid` is amplitude-BVP based, so
the phase falsification does not apply to it, and both centroids are already
documented in-tree as "heuristics, not calibrated".

### The decision

Offering upstream a position tier our own logs describe as still saturated is
the kind of claim this repository forbids without a `MEASURED` reproducer.
Options:

1. **Hold it.** Keep it local until the saturation is understood.
2. **Submit the deadband fix only**, framed as a metric-correctness fix with
   the bin-spacing arithmetic as the evidence, and say plainly that the metric
   remains saturated afterwards.
3. **Submit the whole tier** labelled CLAIMED, not MEASURED, with the
   saturation stated as a known limitation.

Option 2 is the honest minimum and the arithmetic stands on its own. It needs
Joe's decision either way.

## Correction, and a silent bug the extraction introduced

**Correction to the entry below.** It presents a five-way split as pending
work. Three of the five were already cut: `contrib/server-rti`,
`contrib/server-phase-diag` and `contrib/signal-signed-bvp` exist and hold
their modules. Only `server-links` itself needed building. The split finding
stands; the remaining-effort framing did not.

`contrib/server-links` was a literal duplicate of `contrib/server-wire-v3` --
the same single commit, no `links.rs` -- which is why it misrepresented itself.

**The dependency is real and one-directional.** `noise_floor` exists upstream
(wire v1, byte 17) but `source_mac` does not; it arrives with wire v2. There is
no such thing as a link until a transmitter MAC is parsed, so `server-links`
stacks on `server-wire-v3`. The "empty shell" was the foundation.

### A dead match arm, found by compiling the branch in isolation

`contrib/server-wire-v3` did not carry `const CSI_MAGIC_V1`. The extraction
sliced mid-doc-comment and took the V2 and V3 constants but not V1, leaving an
orphaned comment line.

The parser dispatches `match magic { CSI_MAGIC_V1 => .., CSI_MAGIC_V2 => .. }`.
Those are constant patterns **only while the constants are in scope**. With V1
absent the arm degrades into an irrefutable BINDING that matches every value:
the v2 and v3 arms become dead code, `source_mac` is `None` for every frame,
and the whole links feature is inert. rustc reports `unreachable_pattern` -- a
warning, not an error.

The branch compiled, all 531 tests passed, and the feature would have done
nothing. The tests passed because **the wire parser had no test at all**; that
absence is why the bug was silent.

Fixed at the root (V1 restored, wire commit amended, `server-links` rebased)
and pinned with `csi_wire_v2_v3_tests`: v1 carries no identity, v2 yields the
MAC, v3 yields MAC and rx_seq, an unknown magic is still rejected.

**Negative control run, because a regression test that does not fail is
worthless.** Re-breaking the arm fails three of the four with the diagnostic
naming the cause; deleting the constant now fails the *build*, since the test
module references it by name. Either way the failure is loud.

**Method note.** File-level coverage lied twice before. This is a third mode:
the content was present, compiled and tested green, and was still functionally
dead. Only building each branch **in isolation** exposed it -- on `main` the
constant exists a few thousand lines away and everything works. Every extracted
branch needs its own compile, not a diff review.

## e5636cd9 is the largest bucket in the repository -- it splits four ways

Sizing the links topic for packaging found that its base commit,
"per-link CSI metrics, mesh sync surfacing, nine-node prep", is 4,300 lines
across seven files and introduces **five independent subsystems**:

| module | lines | topic |
|---|---|---|
| `links.rs` | 840 | per-link CSI metrics -- the nominal subject |
| `main.rs` | 2171 | wiring for all of the below, plus mesh surfacing and nine-node prep |
| `phase_diag.rs` | 600 | phase diagnostics |
| `rti.rs` | 473 | radio tomographic imaging |
| `bvp.rs` | 214 | body velocity profile |

**They are genuinely separable.** Checked at module level: `links.rs`,
`rti.rs` and `phase_diag.rs` contain **no** `crate::` reference to one another.
Each is self-contained and meets the others only in `main.rs`, where the wiring
is per-module. The split is along existing file boundaries, not a refactor.

The title's comma tell was right again, and understated -- it named three
topics and the commit holds five.

**Revised plan for the links family** (was one branch, now five):

    server-links        links.rs + its wiring + the grid-gate fix 00fb117e
                        + the four follow-ups 21036fd8 7fb85444 80c62207 2a035fde
    server-rti          rti.rs
    server-phase-diag   phase_diag.rs
    server-bvp          bvp.rs
    server-mesh-sync    mesh surfacing / nine-node prep from main.rs

`server-links` carries the measured numbers (links 31 -> 137) and should be
submitted first; the other four are independent and can follow in any order.

## server-links is the highest-value server PR, and I undersold it

`00fb117e` was filed as a bullet ("subcarrier-grid gate per link, distinct bug
bundled into links"). It is the single largest measured improvement in the
whole body of work:

    links        31 -> 137        transmitters   10 -> 32
    renderable   29 ->  88        illuminators with 2+ receivers: 1 -> 24
    grids in use: 256 only  ->  64:97, 256:33, 128:7

"No firmware, hardware or network change -- the frames were always arriving."

**The mechanism.** The grid gate locks each NODE to the densest grid it has
seen. A node associates with one AP and receives HE-SU data from it at 256
bins, while everything else it merely overhears sends beacons and management at
64. So the lock always settles on the associated AP's format, and every
non-associated transmitter is sparser BY CONSTRUCTION -- dropped by a bare
`continue` before it can become a link. 97 of the 137 links recovered are
64-bin: precisely the population being discarded.

**Upstream has the identical gate** (main.rs ~1038-1056, ~6754). For them it is
defensible: it protects the per-node feature path, which genuinely needs one
grid for smoothing and vitals buffers. The bug only bites once you want
per-link data from several transmitters, which requires `links.rs`.

**Therefore it does not separate.** It ships as part of `server-links`, and
that PR should lead with these numbers rather than with "per-link CSI state".
Grid consistency is a property of the TRANSMITTER, so it belongs on the link,
not the node -- that one sentence is the whole argument.

**Consequence for sequencing:** the highest-value server contribution sits
behind the hardest branch to cut (the `links_endpoint` interdependency). Worth
spending the effort there rather than on easier, lesser branches.

## Two more from Joe's memory (2026-09-03)

**39. `ui-navigation` -- a topic on no list.** Our nav has grown to 18 flat
entries (10 tabs plus 8 links), and three pages are reachable only by typing
the URL: `world3d.html`, `illuminators.html`, `tapper.html`. Wants a Tools
grouping plus the orphans added. Upstream's nav is 10 flat entries with no
Tools menu, so this is additive.

**The flicker fix has a much better justification than "reduces traffic".**
`23a54920` (already listed as #29 broadcast-rate-limit) is the fix Joe
remembers as "the epilepsy issue from the C6's higher clock". Its own commit
body confirms it:

- `udp_receiver_task` broadcast a full sensing_update on EVERY incoming CSI
  frame -- ~48-50 FPS per node -- ignoring `--tick-ms`, unlike every other
  source which was already tick-gated. Clients repainted at raw hardware rate.
- Once throttled, a pre-existing keepalive task on its own independent
  `tick_ms` timer produced **"a visible, regular beat pattern"** -- two
  unsynchronised timers at the same nominal rate. Fixed by moving the
  timestamp to shared state so both paths see it.

That is a photosensitivity concern, not a performance one, and the PR should
lead with it. A UI repainting at 50 Hz with an irregular beat is a genuine
accessibility problem, and it appears specifically on the C6 because it
produces frames far faster than the S3 the code was written against.

**Running total: 39 topics.**

## Onion pass: seven further subdivisions found (2026-09-03)

Applying the mesh.html exercise to all 31 proposals. The tell is a commit title
containing "and" or a comma, and a bundle spanning more than one crate.

### Topics in OTHER CRATES that appeared on no list

1. **`signal-signed-bvp`** -- `wifi-densepose-signal/src/bvp.rs`, 214 lines.
   Extracts a SIGNED Body Velocity Profile from complex CSI, preserving Doppler
   direction. The existing `extract_bvp` takes amplitude-only input, and a real
   signal's FFT magnitude is mathematically symmetric about zero
   (`|X(-f)| == |X(f)|`), so **no post-processing on that path can ever recover
   direction** -- it is not a tunable shortcut, it is information that was
   already destroyed. Self-contained, in a crate nobody else touched, and it
   fixes a limitation rather than adding a feature. Strong candidate.

2. **`hardware-sync-packet`** -- `wifi-densepose-hardware/src/sync_packet.rs`,
   192 lines. Parses the ESP-NOW sync packet including the thermal/health
   fields. Pairs with the firmware side.

### Topics hidden inside bundles

3. **`mesh-sync-surfacing`** -- `/api/v1/mesh` and `NodeSyncSnapshot`, buried in
   `e5636cd9` whose title says "per-link CSI metrics, mesh sync surfacing,
   nine-node prep" -- three topics announced in one title.
4. **`ap-position`** -- from `c811a6b9` ("Doppler-weighted centroid position
   tier + AP position"). Treating the AP as a positioned emitter is separate
   from centroid estimation.
5. **`bvp-deadband-fix`** -- from `b441d70a` ("BVP zero-velocity deadband +
   attach_positions diagnostic"). Belongs with (1), not with centroid work.
6. **`subcarrier-grid-gate-per-link`** -- `00fb117e`. Keying the grid gate per
   link rather than per node is a distinct bug, bundled into the links work.
7. **`noise-floor`** -- `80c62207` + `2a035fde`. Carries the reported noise
   floor and settles the AGC question; a data addition, not a links refinement.

### The reason the server resists cherry-picking

`e5636cd9` is **4,300 lines across 7 files** and creates `links.rs`,
`phase_diag.rs` AND `rti.rs` in one commit, plus 2,171 lines of `main.rs`
wiring and 214 lines in a different crate. Three of the proposed PRs were born
in a single commit, which is why none of them can be lifted by cherry-pick and
all need hand-separation.

**Revised estimate: ~37 branches, not 31.**


### Two onion topics folded into `server-links`, deliberately

Onion #6 (`subcarrier-grid-gate-per-link`, `00fb117e`) and #7 (`noise-floor`,
`80c62207` + `2a035fde`) were listed as separate proposals. Both modify
`links.rs`, which does not exist upstream, so neither can precede
`server-links` and neither stands alone as a PR.

#6 additionally has no meaning without per-link data -- upstream's per-node
gate is defensible for the per-node feature path it protects. #7 could in
principle follow as a second PR, but `links.rs` is new code in this PR, so
shipping it with the noise floor already present is simpler than adding a
field to a file the reviewer has only just seen.

Recorded here rather than left silent, because a folded topic that is not
written down is a topic that gets lost.

Remaining to cut: 21, 22, 23, 24, 25, 26, 27, 30, 31, and onion 3, 4, 5.
**12 of 38.**

## THE COMPLETE PR TABLE (2026-09-03)

Status: DONE = cut and build-verified. PARTIAL = cut, known incomplete.
TODO = identified, not cut.

### Firmware -- 14 of 14 DONE, every one built standalone in ESP-IDF v5.4

| # | branch | files | stacked on | what it fixes |
|---|---|---|---|---|
| 1 | `disable-unused-154-radio` | 1 | - | 802.15.4 radio started but never received |
| 2 | `mesh-aligned-rate-gate` | 1 | - | nodes accept disjoint frames; 72% heard vs 25% paired |
| 3 | `subcarrier-grids-256` | 1 | - | edge pipeline dies silently on C6/C5 (HE20 = 256 bins) |
| 4 | `adaptive-floor` | 2 | - | boot calibration latches a contaminated floor forever |
| 5 | `espnow-recovery` | 2 | - | ESP-NOW wedges permanently on a lost send callback |
| 6 | `vitals-slots` | 2 | - | per-slot vitals discarded at the wire |
| 7 | `diagnostics-census` | 2 | - | opt-in CSI census + AP survey |
| 8 | `espnow-beacon-scaling` | 4 | - | fixed beacon period floods a nine-node fleet |
| 9 | `provisioning-tooling` | 5 | - | WiFi passphrase written to disk in cleartext |
| 10 | `thermal` | 6 | - | no thermal monitoring or radio throttling at all |
| 11 | `wifi-retry-watchdog` | 8 | thermal | WiFi gives up permanently; no uplink watchdog |
| 12 | `csi-wire-v3` | 8 | thermal | no transmitter or transmission identity on the wire |
| 13 | `remote-config` | 10 | - | config changes require USB |
| 14 | `rollback` | 10 | remote-config | a bad OTA means a boot loop and a cable |

### Server -- 2 DONE, 1 PARTIAL, ~13 TODO

| # | branch | state | stacked on | what it is |
|---|---|---|---|---|
| 15 | `server-wire-v3` | DONE | - | parse v2/v3 headers. Receiver half; ships before firmware emits |
| 16 | `server-node-positions` | DONE | - | **best candidate**: hardcoded `[2.0,0.0,1.5]` for every node. Upstream issues #228/#249/#301 |
| 17 | `server-fusion` | PARTIAL | wire-v3 | cross-node pairing on (tx, rx_seq). Ingestion absent |
| 18 | `server-links` | DONE | wire-v3 | per-link CSI state + `mesh.html`, `LinkMeshPanel.js` |
| 19 | `server-rti` | DONE | - | radio-tomographic estimation. No dependencies at all |
| 20 | `server-phase-diag` | DONE | - | phase-channel diagnostics |
| 21 | `room-builder` | TODO | - | **live/persisted node positions**, storeys, walls + `RoomBuilderTab`, `mesh3d`, `illuminators`. Replaces `--node-positions` |
| 22 | `ground-truth-marking` | TODO | - | `diag/mark` + `mark`/`survey`/`tapper`/`walk.html` |
| 23 | `node-management` | TODO | - | server proxies config/firmware to nodes + `NodesTab.js` |
| 24 | `emitter-triage` | TODO | - | four-state emitter approval, enforced exclusion |
| 25 | `centroid-position` | TODO | - | motion- and Doppler-weighted position tiers |
| 26 | `baseline-tuning` | TODO | - | baseline-subtraction fraction 0.7 -> 0.95 -> 0.85 |
| 27 | `classification-smoothing` | TODO | - | real per-node confidence, rate-scaled smoothing, debounce |
| 28 | `csi-timestamp-clock` | TODO | - | stop trusting the mesh-synced clock for CSI timestamps |
| 29 | `broadcast-rate-limit` | TODO | - | rate-limit ESP32 broadcasts to tick_ms |
| 30 | `node-health-telemetry` | TODO | - | node health over the network, not to a console |
| 31 | `world3d` | TODO | everything | integrated 3D view. Consumes every server topic; ships last |

### Adopted FROM upstream (not ours to submit)

| PR | what | status |
|---|---|---|
| 1594 | httpd stack too small for OTA validation | adopted into firmware |
| 1142 | C6 dynamic TX buffers 64 -> 128 | adopted, UNVALIDATED |
| 1683 | expose `ota_check_auth` by dropping `static` | technique adopted |
| 1510 | axum 0.7 `:id` path syntax | adopted, merged to main |
| 1734 | MAC filter before rate gate | already had it independently |
| 1159 | ESP-NOW backoff | already had it |

### Awaiting your decision

1726, 1774 (merge clean), 1529, 1717, 1568, 1728 (conflict). 1568 most likely
to matter -- fail closed on weak adaptive models, bearing on the 48.6% model.

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

## Server branches complete so far

- **`contrib/server-wire-v3`** -- parses v2/v3 headers. Foundation for links
  and fusion. `cargo check` clean.
- **`contrib/server-node-positions`** -- 1 file, 19 insertions, cherry-picked
  cleanly with no hand-separation, `cargo check` clean. **The strongest PR
  candidate in the entire set**: it fixes a live upstream bug that emits a
  hardcoded `[2.0, 0.0, 1.5]` for every node regardless of `--node-positions`,
  and the commit already cites upstream issues #228, #249 and the #301 "same
  output regardless of position" symptom class. Small, obviously correct, and
  someone over there has already reported the symptom.

  Verified the fix is complete: four literals remain in `main.rs`, and the two
  that are NOT wired to configured positions are `simulated_data_task` and
  `sample_node` -- synthetic data and a test fixture, where a fallback is
  correct.

### UI ships with the API it consumes

Established by mapping all 20 changed UI files to their endpoints:

| API | UI |
|---|---|
| `links` | `mesh.html`, `LinkMeshPanel.js` |
| `nodes` | `NodesTab.js` |
| `config/room` | `RoomBuilderTab.js`, `illuminators.html`, `mesh3d.html` |
| `diag/mark` | `mark.html`, `survey.html`, `tapper.html`, `walk.html` |
| all of them | `world3d.html` -- ships last, consumes every server topic |

**`diag/mark` is a topic that was missing from the list**: the ground-truth
marking feature with four UI pages. Count is ~30 branches total, not 29.

`app.js`, `index.html`, `router.js` and `style.css` are shared glue -- no API
calls, but every UI feature touches them for tab registration and routing.
They will conflict between stacked PRs; whoever merges second rebases.

### Room Builder is ONE PR, not several

Its five commits are increments of one feature (config API, then storeys and
walls, then elevation, then the round-trip fix). Split, they ship a half-built
model; and the API without the UI is unusable while the UI without the API does
not work.

**Trap:** commit `ba454775` ("model storeys and walls") also carries firmware
`edge_processing.c/h` -- the adaptive-floor change riding on a room-geometry
commit. Already extracted to `contrib/adaptive-floor`; must not travel twice.

## Server packaging: where it stands, and the wall hit (2026-09-03)

### Done

- **`contrib/server-wire-v3`** -- COMPLETE, `cargo check` clean, one file.
  Named constants for v1/v2/v3, magic dispatch, `source_mac` and `rx_seq` as
  `Option` on `Esp32Frame`. This is the receiver half and the prerequisite for
  everything else. Ships first, before any firmware emits v2/v3.

- **`contrib/server-fusion`** -- INCOMPLETE by design, says so in its own commit
  message. `fusion.rs` plus six of seven wiring points; the ingestion is absent
  because it needs the parser above.

### The wall: the endpoint layer is interdependent

`links.rs` itself is self-contained, but `links_endpoint` in `main.rs` has grown
to call `rti_from_links()` (RTI topic), read `node_macs` (node-identity topic)
and call `attribute_transmitter()` (emitter-attribution topic). Lifting the
endpoint pulls in three other topics; lifting the module without the endpoint
produces something that compiles and is invisible.

**This differs from the firmware**, where every conflict was an unrelated topic
merely sharing a file and could be separated by dropping hunks. Here the
topics genuinely call each other. Splitting them means either:

  a. shipping them as ONE larger PR (per-link CSI + RTI + node identity +
     emitter attribution), which is honest but big; or
  b. writing reduced endpoints per branch that do not call across topics,
     which means writing code that exists in no commit and was never run.

**(a) is probably right** -- these features genuinely arrived together and use
each other. Forcing four PRs out of them would produce four things nobody ran.

### Remaining server topics: 12

Of these, `rti`, `node identity`, `emitter triage` and `per-link CSI` are the
interdependent cluster above. The rest -- baseline tuning, classification and
smoothing, CSI timestamp clock, broadcast rate limiting, node health telemetry,
room-builder config API, centroid position, node management UI -- are
`main.rs`-only and have no new file to anchor on, so each needs the same
hand-lift treatment with no guarantee they separate any better.

## Server packaging: method proven, one structural finding

**Cherry-picking does not work here.** `main.rs` is 15k lines with changes from
most of the fourteen topics interleaved, so every pick conflicts there -- even
when only the server half of a commit is wanted.

**The method that does work** (as used for `contrib/adaptive-floor`): a new
module is a new file, so `git checkout main -- <module>.rs` is clean. Then
hand-wire its `main.rs` touchpoints, which are few and greppable. For
`fusion.rs` there were seven: the `mod` declaration, an `AppStateInner` field,
two constructors, two endpoint fns, two routes, and the ingestion call.

**Structural finding: `fusion.rs` cannot ship alone.** Its ingestion is gated on
`frame.rx_seq`, which exists only once the wire-v3 parser is present. The wire
work is a THREE-part set:

    contrib/csi-wire-v3     firmware: emit rx_seq on the wire
    (server) v3 parser      server: parse it into frame.rx_seq   <- not yet cut
    contrib/server-fusion   server: pair frames on (tx, rx_seq)

Sequenced receiver-before-sender: parser, then fusion, then firmware emits.

**`contrib/server-fusion` exists but is INCOMPLETE and marked so in its own
commit message** -- six of seven wiring points, ingestion deliberately absent.
Merged alone its endpoints compile and always return empty. Do not submit until
the parser topic is cut.

**Remaining server topics to package: 13.** The four new modules should each go
the way fusion did. `main.rs`-only topics (baseline tuning, classification,
clock, broadcast rate) have no new file to anchor on and will be harder.

## PENDING YOUR DECISION -- server PRs that overlap our work

None of these block packaging; they change what we adopt, not what we cut.

| PR | what it does | merges | the question |
|---|---|---|---|
| **1726** | fuse only coherent frame cohorts | CLEAN | We wrote `fusion.rs` (549 lines) for cross-node pairing. Theirs works on `multistatic_bridge.rs`. Do these compose, or are they two answers to one problem? |
| **1774** | expose multistatic cohort quality | CLEAN | Adds a quality metric to the same bridge. Adopt alongside ours, or is our per-link quality enough? |
| **1529** | ESP32 node lifecycle + calibration persistence | conflicts `main.rs`, `multistatic.rs` | Overlaps our calibration work. Theirs persists calibration; ours tracks a leaky floor continuously. Possibly complementary, possibly redundant. |
| **1717** | live-CSI presence/motion tuning | conflicts `main.rs` | Directly overlaps our presence path and the baseline-subtraction tuning (0.7 -> 0.95 -> 0.85). |
| **1568** | fail closed on weak adaptive models | conflicts `main.rs`, `engine_bridge.rs` | Bears on the 48.6%-accuracy model trained on a house that no longer exists. Might be exactly the guard that situation needs. |
| **1728** | wire the ADR-302 OOD gate into the live path | conflicts `main.rs` | Out-of-distribution gating. Unknown whether it duplicates our confidence work. |

Two are free to take (1726, 1774 merge clean). Four need reading before a
decision, and 1568 is the one most likely to matter given the model staleness
already on record.

## Server topics, for packaging (36 commits -> ~14 topics)

| topic | commits |
|---|---|
| room-builder config API | `f5373a30` `a3b99077` `ba454775` `d758d9ae` `3ff89106` |
| per-link CSI (`links.rs`) | `e5636cd9` `21036fd8` `7fb85444` `80c62207` `2a035fde` `00fb117e` |
| cross-node fusion (`fusion.rs`) | `38b889ee` `bce00eb4` |
| RTI (`rti.rs`) | `9660c094` `66330ff9` `6c5ed284` |
| emitter triage | `cf50ae1a` |
| centroid position estimate | `9526f363` `c811a6b9` `7037050b` `b441d70a` |
| baseline-subtraction tuning | `7b63ee1e` `664541f1` |
| classification + smoothing | `27c6309d` `d29d9fb0` `bfa78710` |
| CSI timestamp clock | `d4d4615e` |
| node identity + address | `d34d76c5` `952e9465` `bb776437` |
| broadcast rate limiting | `23a54920` |
| node health telemetry | `474733d1` |
| node management UI + proxy | `85ba2092` |
| phase diagnostics (`phase_diag.rs`) | (within `e5636cd9`) |

The four new modules are one-topic-per-file, so they should package far more
easily than firmware did. `main.rs` is the hard part: 15k lines with changes
from most of these topics interleaved.

## Server PR sweep (2026-09-03)

Same method as firmware: fetch every `refs/pull/*/head`, drop anything already
an ancestor of `origin/main`, diff the rest against our server files.
**24 unmerged PRs touch the sensing server.**

### Adopted

- **PR 1510 (fallen-pc, open since 2026-08-03) -- ADOPTED, merged to main.**
  `axum 0.7` uses `:id`, not `{id}`; the latter compiles and registers a
  literal path segment, so the route silently 404s. Upstream had the bug in
  four routes (`/api/v1/models/{id}` get and delete, `/api/v1/recording/{id}`,
  the recording download path) and **we inherited all of them** -- no commit of
  ours touched those lines. Fixed in our tree now.

  Worth noting: this is the identical mistake made independently while writing
  the node-management proxy tonight, caught there only by calling the endpoint.
  A compiler cannot see it.

### Already resolved, no action

- **PR 1443 / 1447** (two independent fixes for issue #1442, presence
  contradicting motion_level). `classify_vitals` is already present on
  `origin/main` AND on ours, carrying 1447's comment verbatim. Nothing to do.

### Still to examine

`1774` multistatic cohort quality, `1728` ADR-302 OOD gate, `1726` fuse only
coherent frame cohorts, `1720` rufield honesty, `1717` live-CSI presence tuning,
`1696` BLE, `1683` security boundaries, `1669` mqtt, `1647` centroid
localisation (tested, no benefit at n=11), `1629` cross-platform build, `1593`
BSSID telemetry, `1568` fail closed on weak adaptive models, `1531`, `1529` node
lifecycle + calibration persistence, `1511` pose_stats confidence, `1447`,
`1445`, `1443`, `1439`.

`1726` and `1774` are the most likely to overlap `fusion.rs`; `1529` overlaps
the calibration work; `1568` overlaps the adaptive-model concerns.

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
