"""Collect link time series to disk INCREMENTALLY, analysing nothing.

Twice now a run has been lost: once to a power cut with everything buffered in
memory, once to an edit that landed after the process had already started. Both
had the same root cause -- collection and analysis in one process, with the data
existing only inside it. So this does collection only, appends every poll to
disk as it happens, and flushes. Any analysis reads the file afterwards, as many
times and as many ways as needed, without re-polling.

The fleet is at steady state now (median window 224 s, max 4340 s), so no
warm-up is needed -- unlike a cold start where every link briefly looks fast
because window_span_s is bounded by uptime.

One line of JSON per poll: timestamp, then {rx|tx: [raw_motion, window_span_s]}.
"""
import io
import json
import sys
import time
import urllib.request

B = "http://192.168.1.66:3000"
POLL_S = 2.0
WINDOW_S = float(sys.argv[1]) if len(sys.argv) > 1 else 2400
OUT = sys.argv[2] if len(sys.argv) > 2 else "links_series.jsonl"


def get(p):
    return json.load(urllib.request.urlopen(B + p, timeout=20))


# Geometry first, as its own line, so the analyser needs no server.
cfg = get("/api/v1/config/room")
meta = {
    "kind": "meta",
    "nodes": {str(n["id"]): [n["x"], n["y"], n["z"], n.get("label") or ""]
              for n in cfg["nodes"]},
    "ap_mac": (cfg.get("ap_mac") or "").lower(),
    "ap_position": cfg.get("ap_position") or cfg.get("ap"),
    "emitters": [{"mac": e["mac"].lower(), "label": e.get("label") or "",
                  "status": e.get("status"), "position": e.get("position")}
                 for e in cfg.get("emitters", [])],
    "poll_s": POLL_S,
    "start_unix": time.time(),
}
f = io.open(OUT, "w", encoding="utf-8")
f.write(json.dumps(meta) + "\n")
f.flush()

t0 = time.time()
n = 0
while time.time() - t0 < WINDOW_S:
    try:
        d = get("/api/v1/links")
    except Exception:
        time.sleep(POLL_S)
        continue
    row = {"t": round(time.time() - t0, 2), "u": round(time.time(), 2), "l": {}}
    for l in d.get("links", []):
        k = "%d|%s" % (l["rx_node"], l["tx_mac"].lower())
        row["l"][k] = [l.get("raw_motion"), l.get("window_span_s"),
                       l.get("rssi_dbm")]
    f.write(json.dumps(row) + "\n")
    f.flush()          # every poll is on disk before the next one starts
    n += 1
    if n % 60 == 0:
        print("polls: %d  (%.0f s elapsed)" % (n, time.time() - t0), flush=True)
    time.sleep(POLL_S)

f.close()
print("done: %d polls -> %s" % (n, OUT), flush=True)
