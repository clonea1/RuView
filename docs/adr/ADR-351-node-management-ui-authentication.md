# ADR-351: Authenticating the node management UI

- **Status**: Accepted risk — mutating UI shipped unauthenticated by explicit
  decision (Joe, 2026-09-02); PIN to follow
- **Date**: 2026-09-02
- **Deciders**: Joe
- **Owners**: RuView sensing-server and firmware maintainers
- **Tags**: security, firmware, web-ui, authentication, node-management
- **Extends**: ADR-283 (default-deny mutation policy)
- **Supersedes**: None

## Context

Firmware gained remote configuration and OTA rollback on 2026-09-02. Every
mutating node endpoint (`POST /config`, `POST /ota`, `POST /calibrate`) is
gated by a pre-shared key checked in constant time, and **fails closed** when
no PSK is provisioned. On the wire that is sound: an attacker on the LAN
cannot reconfigure or reflash a node without the key.

The management UI now being added to the sensing server changes who holds that
key. The browser must never see it — a PSK delivered to a web page is a PSK
published to anything that can read the page or its traffic. So the server
holds it and proxies, which means:

**Anyone who can reach the server's web UI can reconfigure or reflash the
entire fleet.** The firmware's authentication is intact and simply no longer
the control that matters; the server's front door becomes the real boundary,
and today that front door has no lock at all.

The realistic exposure on a home LAN is low, and this is deliberately not
being treated as urgent. It is recorded because the failure mode is quiet: the
system will look as secure as it did before, because the firmware check is
still there and still passing.

## Options

1. **No auth.** Current state. Acceptable only while the UI stays read-only.
2. **A PIN in a server-side file** (Joe's proposal). The admin writes a PIN to
   a flat file the server reads at startup and never serves. Mutating actions
   in the UI require the PIN typed into a box before the button does anything;
   the server compares and refuses on mismatch. Read-only views stay open.
3. **Full user accounts / OIDC.** Correct for a multi-user product and
   disproportionate for a nine-node house.

Option 2 is the recommendation. It is small, it needs no dependency, it maps
onto the PSK handling that already works (secret by file path, never printed,
never sent to a browser), and it protects exactly the actions worth protecting
while leaving diagnostics visible.

## Decision

Joe elected to ship the management UI **fully enabled**, including mutating
controls, and to secure it afterwards (2026-09-02). The read-only mitigation
below was offered and declined; this is recorded so the exposure is a known
accepted risk rather than an oversight.

**The window of risk is therefore open now.** Anyone who can reach the
sensing server's web port can reconfigure, reflash or reboot any node in the
fleet. On a home LAN with no port forwarding that is a narrow exposure, and it
is the operator's call to make.

The mitigation that was available, for the record: keeping the UI read-only
costs little, since versions, RSSI, fps and current settings are the useful
part and carry no such exposure.

## Notes for whoever implements it

- Compare in constant time and fail closed on a missing or empty PIN file,
  mirroring `ota_check_auth()` in `firmware/esp32-csi-node/main/ota_update.c`.
- Never log the PIN, never include it in a response, never accept it as a
  query parameter (it would land in access logs and browser history).
- Rate-limit attempts. A short numeric PIN is brute-forceable in seconds
  otherwise, which would make it decorative.
- The PIN authorises an *action*, not a session, unless a session is explicitly
  designed — a persistent cookie quietly turns "type a PIN to reflash" into
  "this browser can reflash forever".
