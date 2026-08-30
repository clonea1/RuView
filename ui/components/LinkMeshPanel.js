/**
 * LinkMeshPanel — live per-link CSI perturbation (ADR-345).
 *
 * Every link the fleet can measure, one row each, sorted strongest first. The
 * data existed before this panel but only reached a 30-second server log line,
 * which is the wrong medium for something whose whole value is watching it move
 * while you walk across the room.
 *
 * What each row is: `rx_node <- tx` is one radio path. `motion` is the
 * baseline-subtracted perturbation on that path — the quantity a localiser
 * would consume. `raw` is shown next to it on purpose: a link whose baseline
 * has not settled yet shows a healthy raw value and a near-zero motion, and
 * with only the one number that is indistinguishable from a dead link.
 *
 * Node-to-node links are marked because they are the point of the exercise —
 * they cross the room at ~109 degrees of angular spread against ~36 for the AP
 * links, with both endpoints fixed and known. Their identity is *inferred*
 * server-side (a node cannot hear itself) and is drawn as an inference.
 *
 * Nothing here is a position estimate. Per-link motion is the measurement layer
 * a localiser would be built on; no localiser consumes it yet.
 */

import { apiService } from '../services/api.service.js';

const POLL_INTERVAL_MS = 1000;

/** Scale bars against this until a larger value is seen, so an idle room
 *  doesn't render noise as full-width bars. */
const MIN_FULL_SCALE = 0.5;

/* This panel lives in a narrow sidebar card, so every width here is fluid.
   An earlier version used fixed pixel columns whose total exceeded the card:
   the bar column collapsed to nothing and the row pushed a horizontal
   scrollbar onto the whole tab. Nothing in here may have a fixed width, and
   the bar gets its own full-width line rather than competing for space with
   the numbers. */
const CSS = `
.lmp-wrap { font: 11px/1.35 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
            min-width: 0; overflow-x: hidden; }
.lmp-summary { opacity: 0.75; margin-bottom: 8px; }
.lmp-empty { opacity: 0.6; padding: 10px 0; }
.lmp-row { display: grid; grid-template-columns: minmax(0, 1fr) auto auto auto;
           gap: 2px 7px; align-items: center; padding: 3px 0; }
.lmp-label { min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.lmp-kind-node { color: #6ee7a8; }
.lmp-kind-infra { color: #9ab4e8; }
/* Full width on its own line, under the numbers it belongs to. */
.lmp-bar { grid-column: 1 / -1; position: relative; height: 7px; min-width: 0;
           background: rgba(127,127,127,0.16); border-radius: 2px; overflow: hidden; }
.lmp-bar > i { position: absolute; inset: 0 auto 0 0; display: block; border-radius: 2px;
               transition: width 180ms linear; }
.lmp-bar > i.node { background: #34d399; }
.lmp-bar > i.infra { background: #60a5fa; }
/* Raw sits behind motion: the gap between them is the baseline still being
   subtracted, which is exactly what you want to see while a link warms up. */
.lmp-bar > u { position: absolute; inset: 0 auto 0 0; display: block; border-radius: 2px;
               background: rgba(255,255,255,0.16); transition: width 180ms linear; }
.lmp-num { text-align: right; opacity: 0.85; font-variant-numeric: tabular-nums;
           white-space: nowrap; }
.lmp-warm { opacity: 0.5; }
.lmp-note { margin-top: 9px; opacity: 0.5; font-size: 10px; line-height: 1.45; }
.lmp-rti { margin: 0 0 8px; padding: 5px 7px; border-radius: 3px;
           background: rgba(96,165,250,0.12); border-left: 2px solid #60a5fa; }
.lmp-rti-off { background: rgba(127,127,127,0.10); border-left-color: #7f7f7f; opacity: 0.65; }
.lmp-rti-wide { color: #fbbf24; }
`;

export class LinkMeshPanel {
  /** @param {HTMLElement} container */
  constructor(container) {
    this.container = container;
    this._timer = null;
    this._fullScale = MIN_FULL_SCALE;
    this._lastError = null;
  }

  init() {
    if (!document.getElementById('lmp-style')) {
      const style = document.createElement('style');
      style.id = 'lmp-style';
      style.textContent = CSS;
      document.head.appendChild(style);
    }
    this.container.innerHTML = '<div class="lmp-wrap"><div class="lmp-empty">Loading links…</div></div>';
    void this._poll();
    this._timer = setInterval(() => void this._poll(), POLL_INTERVAL_MS);
  }

  async _poll() {
    try {
      const resp = await fetch('/api/v1/links', { headers: apiService.getHeaders() });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      this._render(await resp.json());
      this._lastError = null;
    } catch (err) {
      // Don't blank a populated table on one failed poll — a stale reading with
      // the failure named beats an empty panel that looks like "no links".
      if (this._lastError !== err.message) {
        this._lastError = err.message;
        console.warn('[LinkMesh] poll failed:', err.message);
      }
    }
  }

  _render(data) {
    const links = Array.isArray(data.links) ? data.links : [];
    const wrap = this.container.querySelector('.lmp-wrap') || this.container;

    if (links.length === 0) {
      wrap.innerHTML =
        '<div class="lmp-empty">No links yet. Per-link data needs wire-v2 firmware ' +
        '(ADR-345); a v1 node sends no transmitter address, so its frames cannot be ' +
        'attributed to a link.</div>';
      return;
    }

    // Track the largest motion seen so bars stay comparable between polls
    // instead of rescaling every frame and making a still room look busy.
    const peak = Math.max(...links.map((l) => l.raw_motion || 0));
    this._fullScale = Math.max(MIN_FULL_SCALE, peak, this._fullScale * 0.97);

    const nodeLinks = links.filter((l) => l.kind === 'node').length;
    const pct = (v) => Math.max(0, Math.min(100, (v / this._fullScale) * 100));

    const rows = links
      .map((l) => {
        const isNode = l.kind === 'node';
        const peer = l.tx_node_inferred != null ? `node ${l.tx_node_inferred}?` : l.tx_mac.slice(9);
        // A link below the frame minimum has no trustworthy metric yet.
        const warming = (l.frames || 0) < 64;
        return `
          <div class="lmp-row${warming ? ' lmp-warm' : ''}">
            <span class="lmp-label ${isNode ? 'lmp-kind-node' : 'lmp-kind-infra'}"
                  title="receiver ${l.rx_node} &larr; transmitter ${l.tx_mac}">
              ${l.rx_node} &larr; ${peer}
            </span>
            <span class="lmp-num">${(l.motion ?? 0).toFixed(2)}</span>
            <span class="lmp-num">${(l.raw_motion ?? 0).toFixed(2)}</span>
            <span class="lmp-num">${Math.round(l.rssi_dbm ?? 0)}</span>
            <span class="lmp-bar">
              <u style="width:${pct(l.raw_motion)}%"></u>
              <i class="${isNode ? 'node' : 'infra'}" style="width:${pct(l.motion)}%"></i>
            </span>
          </div>`;
      })
      .join('');

    wrap.innerHTML = `
      <div class="lmp-summary">
        ${links.length} link${links.length === 1 ? '' : 's'} &middot;
        ${nodeLinks} node&harr;node &middot; ${links.length - nodeLinks} infra
      </div>
      ${this._renderRti(data.rti)}
      <div class="lmp-row" style="opacity:0.55">
        <span>rx &larr; tx</span>
        <span class="lmp-num">motion</span>
        <span class="lmp-num">raw</span>
        <span class="lmp-num">dBm</span>
      </div>
      ${rows}
      <div class="lmp-note">
        Green = node&harr;node. The peer id carries "?" because it is inferred, not
        measured. Pale bar = <em>raw</em>; the gap to the solid bar is the resting
        baseline being subtracted. Dimmed rows are still warming up. This is
        per-link motion, not a position.
      </div>`;
  }

  /**
   * The link-line position candidate.
   *
   * Drawn as a candidate, never as the position. It has not been scored
   * against ground truth, and `spread_m` is shown next to it because with six
   * sparse links the score surface can be genuinely multi-modal — a tight
   * co-ordinate with a wide spread means "one of several places", and hiding
   * that would make an ambiguous answer look like a confident one.
   */
  _renderRti(rti) {
    if (!rti) {
      return '<div class="lmp-rti lmp-rti-off">position candidate: needs AP + room bounds configured</div>';
    }
    if (rti.x == null) {
      const why = rti.skipped_cold_baseline > 0
        ? `${rti.skipped_cold_baseline} link(s) still learning their baseline`
        : `${rti.links_used} usable link(s)`;
      return `<div class="lmp-rti lmp-rti-off">position candidate: none &mdash; ${why}</div>`;
    }
    const wide = rti.spread_m > 1.5;
    return `
      <div class="lmp-rti">
        <b>candidate</b> ${rti.x.toFixed(2)}, ${rti.y.toFixed(2)} m
        &middot; fit ${rti.confidence.toFixed(2)}
        &middot; <span class="${wide ? 'lmp-rti-wide' : ''}">spread ${rti.spread_m.toFixed(2)} m</span>
        &middot; ${rti.links_used} links
        ${wide ? '<br><span class="lmp-rti-wide">wide spread &mdash; several positions explain this equally well</span>' : ''}
      </div>`;
  }

  dispose() {
    if (this._timer) {
      clearInterval(this._timer);
      this._timer = null;
    }
  }
}
