// The ledger window: three big dials, a legend, the full conversation list,
// and the calibration/settings dialog.
import { applySavedTheme, bindThemeButton, effectiveTheme } from "./shared/theme";
import {
  broadcastScheme, calibrate, checkLiveReadings, getAutostart, getHome, getSettings, getSnapshot,
  markThreadRead, onOpenSettings, onSchemeChanged, onSnapshot, refreshNow, setAutostart,
  setLiveReadings, setThreadSort, updateSettings,
} from "./shared/bridge";
import { convColor, modelColor, paint, paceNote } from "./shared/color";
import { entryName, esc, hhmm, setHome, short, tidy, tok, until, usd, when } from "./shared/format";
import { grow, numText, ringArcs } from "./shared/ring";
import { bindSetup, diagOf, setupCard, setupPanel } from "./shared/setup";
import {
  applyScheme, CUSTOM_ID, currentSchemeId, customScheme, GROUPS, paletteOf, SCHEMES, saveCustom, setScheme,
} from "./shared/schemes";
import {
  allowedBuckets, BAND, BAND_HOVER, bucketLabel, bucketOf, chart, defaultBucket, guide, hitBucket, subline,
} from "./shared/timeline";
import {
  deviceTag, listRows, localRows, mergeThreads, sortOf, statusMark, statusOf, threadName,
} from "./shared/threads";
import type { ThreadRow } from "./shared/threads";
import type { View } from "./shared/timeline";
import type { BucketId, CalibrationInput, Entry, GroupKey, Settings, Snapshot, ThreadSort, WeeklyReset, WindowOut } from "./shared/types";

applySavedTheme();
applyScheme();

const BR = 58;
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let snap: Snapshot | null = null;
let sel = 0;
let mode: "by_model" | "by_session" = "by_model";
let group: GroupKey = "by_session";
let open: number | null = null;
// Timeline-only state. Kept off the `apply()` path so a refresh doesn't
// fight the pointer: only renderTimeline() re-runs when these change.
let bucket: BucketId = "hour";
let hoverKey: string | null = null;
let hoverBucket: number | null = null;

// ---------- header + notices ----------

/// Re-read Claude Code's cached Usage numbers after the user has run `/usage`.
/// A failed scan still republishes a snapshot (with a fresh diagnosis), so the
/// error goes next to the button rather than into an alert.
async function recheck(btn: HTMLButtonElement): Promise<void> {
  const label = btn.textContent;
  btn.disabled = true;
  btn.textContent = "Checking…";
  try {
    apply(await refreshNow());
  } catch (e) {
    btn.disabled = false;
    btn.textContent = label;
    const box = btn.closest(".setup");
    if (box) {
      const p = document.createElement("p");
      p.className = "serr";
      p.textContent = String(e);
      box.querySelector(".serr")?.remove();
      box.appendChild(p);
    }
  }
}


/// First-run path: turn live readings on straight from the setup card, so the
/// most accurate route doesn't require finding it in settings first. A refusal
/// is reported in place — the card is where the user is looking.
async function turnOnLive(btn: HTMLButtonElement): Promise<void> {
  const label = btn.textContent;
  btn.disabled = true;
  btn.textContent = "Turning on…";
  try {
    apply(await setLiveReadings(true));
  } catch (e) {
    btn.disabled = false;
    btn.textContent = label;
    const box = btn.closest(".setup");
    if (box) {
      const p = document.createElement("p");
      p.className = "serr";
      p.textContent = String(e);
      box.querySelector(".serr")?.remove();
      box.appendChild(p);
    }
  }
}

function renderHeader(): void {
  if (!snap) return;
  $("tier").textContent = snap.plan;
  $("stamp").textContent = "snapshot " + when(snap.generated_at);
  const notices: string[] = [];
  const cache = snap.usage_cache;
  const cacheAge = cache ? (Date.now() - new Date(cache.fetched_at).getTime()) / 3.6e6 : null;
  if (!snap.calibrated) {
    notices.push(setupCard(snap));
  } else if (diagOf(snap).live && cache) {
    notices.push(`<div class="notice"><span><b>Live readings.</b> Percentages come straight from Anthropic, last fetched ${when(cache.fetched_at)}; only usage since then is estimated from transcripts.</span></div>`);
  } else if (snap.calibration_source === "auto" && cache) {
    const d = diagOf(snap);
    const fell = d.connected && d.live_error
      ? ` Live readings are on but ${d.live_auth_failed ? "Claude Code's login couldn't be used" : "the last fetch failed"} — ${esc(d.live_error)}`
      : "";
    // Someone calibrated from /usage never meets the setup card, so this is
    // the only place they learn the exact route exists. Offer it, once, next
    // to the number it would improve — not as a banner they must dismiss.
    const offer = !d.connected
      ? ` <button type="button" class="btn tiny" data-live>Turn on live readings</button> for exact percentages that never go stale.`
      : "";
    notices.push(`<div class="notice${fell ? " warn" : ""}"><span><b>Auto-calibrated</b> from Claude Code's own Usage reading, fetched ${when(cache.fetched_at)}${cacheAge! > 24 ? ` — ${cacheAge!.toFixed(0)} h old; run /usage in a Claude Code terminal session to refresh it` : ""}.${offer}${fell}</span></div>`);
  }
  if (snap.boost) {
    notices.push(`<div class="notice"><span><b>Boost active.</b> ${esc(snap.boost)}. These meters read against the boosted ceiling.</span></div>`);
  }
  const rolling = snap.windows.find((w) => w.id === "session" && !w.boundary_known);
  if (rolling) {
    notices.push(`<div class="notice warn"><span><b>Session boundary unknown.</b> No transcripts found yet, so the session dial is a rolling 5-hour window.</span></div>`);
  }
  $("notices").innerHTML = notices.join("");
  bindSetup($("notices"), { recheck, settings: openSettingsDialog, live: turnOnLive });
  const s = snap.scan;
  $("scan").textContent = `index: ${snap.index_records.toLocaleString()} unique turns · last scan ${s.files_in_window} of ${s.files_total} files in range, ${s.files_read} read, ${(s.bytes_read / 1e6).toFixed(1)} MB, +${s.records_added} new, ${s.duplicates_skipped} duplicates skipped, ${s.duration_ms} ms`;
}

// ---------- big dials ----------

const segColor = (e: Entry, i: number) => (mode === "by_model" ? modelColor(e.key as string) : convColor(i));
const segName = (e: Entry) => (mode === "by_model" ? short(e.key as string) : e.title || "untitled session");

function bigDial(w: WindowOut, i: number): string {
  const items = w[mode].slice(0, 8);
  const segs = items.map((e, j) => ({
    color: segColor(e, j),
    frac: e.share,
    label: `${segName(e)} — ${e.pct === null ? e.share.toFixed(0) + "% of usage" : e.pct.toFixed(1) + "% of this limit"}`,
  }));
  const shown = items.reduce((a, e) => a + e.share, 0);
  const fill = w.pct === null ? 1 : Math.min(1, w.pct / 100);
  // Segments cover `shown`% of the usage; the remainder of the fill is "everything else".
  const arcs = ringArcs(BR, segs, fill * (shown / 100), null, w.pct === null)
    + (shown < 99.5 ? ringArcs(BR, [{ color: "var(--m-rest)", frac: 1, label: "everything else" }], fill, null, w.pct === null).replace(/stroke-dashoffset="[^"]*"/, `stroke-dashoffset="${(-(fill * (shown / 100)) * 2 * Math.PI * BR).toFixed(2)}"`).replace(/data-len="[^"]*"/, `data-len="${(fill * (1 - shown / 100) * 2 * Math.PI * BR).toFixed(2)}"`) : "");
  const meta = w.idle
    ? `<span>no active block</span><span>the next message opens a <b>5 h</b> block</span><span>last block's usage no longer counts</span>`
    : w.pct === null
    ? `<span>opened <b>${when(w.start)}</b></span><span>resets <b>${when(w.reset)}</b></span><span><b>${w.messages}</b> messages · <b>${tok(w.total_raw)}</b> tokens</span><span>weighted <b>${usd(w.total_cost)}</b></span>`
    : `<span>opened <b>${when(w.start)}</b></span><span>resets <b>${when(w.reset)}</b> · ${until(w.reset)}</span><span><b>${w.messages}</b> messages · <b>${tok(w.total_raw)}</b> tokens</span><span>burn rate <b>${paceNote(w.pace)}</b></span>`;
  return `<button class="bigdial" data-i="${i}" aria-selected="${i === sel}" style="${paint(w.ramp)}">
    <svg width="150" height="150" viewBox="-75 -75 150 150" aria-hidden="true">
      <g transform="rotate(-90)"><circle class="ring-bg" r="${BR}" cx="0" cy="0"></circle>${arcs}</g>
      <g class="gauge-fill">${numText(w.pct, 6)}</g>
      <text class="cap2" x="0" y="26" text-anchor="middle">${w.pct === null ? "uncalibrated" : "used"}</text>
    </svg>
    <span class="bname">${esc(w.label)}</span>
    <span class="bmeta">${meta}</span>
  </button>`;
}

function renderBig(): void {
  if (!snap) return;
  $("bigdials").innerHTML = snap.windows.map(bigDial).join("");
  document.querySelectorAll<HTMLButtonElement>(".bigdial").forEach((b) => b.addEventListener("click", () => setWindow(Number(b.dataset.i))));
  grow($("bigdials"));
}

function renderLegend(): void {
  if (!snap) return;
  const w = snap.windows[sel], items = w[mode].slice(0, 8);
  const pctOf = (e: Entry) => (e.pct === null ? e.share.toFixed(0) + "%" : e.pct.toFixed(1) + "%");
  const head = w.pct === null ? "Remaining (unknown)" : "remaining headroom";
  $("legend").innerHTML =
    `<span class="lt">${mode === "by_model" ? "Models" : "Conversations"} in ${esc(w.label)}${w.pct === null ? " · share of usage" : " · % of limit"}</span>` +
    items.map((e, j) => `<span class="e"><i style="background:${segColor(e, j)}"></i><em title="${esc(segName(e))}">${esc(segName(e))}</em><b>${pctOf(e)}</b></span>`).join("") +
    (w.pct === null ? "" : `<span class="e"><i style="background:var(--m-rest)"></i><em>${head}</em><b>${Math.max(0, 100 - w.pct).toFixed(0)}%</b></span>`);
}

// ---------- usage over time ----------

/// The chart is drawn at its container's pixel size, one viewBox unit per CSS
/// pixel, so widening or heightening the window grows the plot instead of
/// scaling the type up with it. CSS gives the box its height.
function chartSize(): [number, number] {
  const el = $("tlChart");
  const w = el.clientWidth || 900;
  const h = el.clientHeight || 300;
  return [w, h];
}

const view = (): View => ({
  mode, bucket, hoverKey, hoverBucket,
  now: Date.parse(snap!.generated_at), size: chartSize(), sort: sortOf(snap),
});

/// The panel needs `series`, which a snapshot fixture written before the
/// timeline existed doesn't carry. Hide it rather than draw an empty box.
function renderTimeline(): void {
  if (!snap) return;
  const panel = $("timeline");
  const w = snap.windows[sel];
  if (!w.series) { panel.hidden = true; return; }
  panel.hidden = false;
  bucket = bucketOf(w, bucket);
  const v = view();

  $("tlTitle").textContent = `Usage over time — ${w.label}`;
  $("tlSub").textContent = subline(w, v);
  $("tlChart").innerHTML = chart(w, v);
  ($("tlBucketBtn") as HTMLButtonElement).textContent = `${bucketLabel(w, bucket)} ▾`;

  const svg = $("tlChart").querySelector<SVGSVGElement>(".tlsvg");
  svg?.addEventListener("mousemove", (e) => {
    const r = svg.getBoundingClientRect();
    // The viewBox is 1:1 with the chart's pixels, so the pointer maps through
    // the chart's own width — a fixed 900 here silently mis-hit every bucket
    // as soon as the window was any other size.
    const v2 = view();
    const hit = hitBucket(w, v2, ((e.clientX - r.left) / r.width) * v2.size[0]);
    if (hit !== hoverBucket) { hoverBucket = hit; drawGuide(); }
  });
  svg?.addEventListener("mouseleave", () => {
    if (hoverBucket !== null) { hoverBucket = null; drawGuide(); }
  });
  drawGuide();
}

/// Hover repaints in place rather than through renderTimeline(): rebuilding the
/// SVG under the cursor loses the pointer's own hover target. The bands running
/// through the hovered bucket also thicken, so the tooltip's rows and the
/// ribbon name the same conversations.
function drawGuide(): void {
  const g = document.getElementById("tlGuide");
  if (g && snap) g.innerHTML = guide(snap.windows[sel], view());
  document.querySelectorAll<SVGPathElement>("#tlChart .band").forEach((b) => {
    const on = hoverBucket !== null
      && hoverBucket >= Number(b.dataset.b0) && hoverBucket <= Number(b.dataset.b1);
    b.setAttribute("stroke-width", String(on ? BAND_HOVER : BAND));
  });
}

function setHoverKey(k: string | null): void {
  if (k === hoverKey) return;
  hoverKey = k;
  document.querySelectorAll<SVGPathElement>("#tlChart .band").forEach((b) =>
    b.setAttribute("opacity", !hoverKey || b.dataset.k === hoverKey ? "1" : "0.14"));
}

/// The bucket menu. A native `<select>` would do, but the list changes with the
/// window and this keeps the trigger looking like every other `.btn`.
function renderBucketMenu(): void {
  if (!snap) return;
  const w = snap.windows[sel];
  const menu = $("tlMenu");
  menu.innerHTML = allowedBuckets(w).map((b) =>
    `<button type="button" role="menuitemradio" data-b="${b.id}" aria-checked="${b.id === bucket}">${b.label}</button>`).join("");
  menu.querySelectorAll<HTMLButtonElement>("button").forEach((b) =>
    b.addEventListener("click", () => {
      bucket = b.dataset.b as BucketId;
      hoverBucket = null;
      closeBucketMenu();
      renderTimeline();
    }));
}

function closeBucketMenu(): void {
  $("tlMenu").hidden = true;
  $("tlBucketBtn").setAttribute("aria-expanded", "false");
}

$("tlBucketBtn").addEventListener("click", (e) => {
  e.stopPropagation();
  const menu = $("tlMenu");
  if (menu.hidden) {
    renderBucketMenu();
    menu.hidden = false;
    $("tlBucketBtn").setAttribute("aria-expanded", "true");
  } else {
    closeBucketMenu();
  }
});
document.addEventListener("click", (e) => {
  if (!$("tlMenu").hidden && !$("tlMenu").contains(e.target as Node)) closeBucketMenu();
});
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeBucketMenu(); });
// The chart is drawn at its box's pixel size, so it has to be redrawn whenever
// that box changes — on the first layout as much as on a window resize. The
// observer watches the box itself rather than the window, which also covers
// the 700 px breakpoint where the sub-line and tooltip shed detail.
let lastBox = "";
new ResizeObserver((entries) => {
  const r = entries[0].contentRect;
  const box = `${Math.round(r.width)}x${Math.round(r.height)}`;
  if (box === lastBox || r.width === 0) return;
  lastBox = box;
  renderTimeline();
}).observe($("tlChart"));

// ---------- list ----------

function sublabel(e: Entry, g: GroupKey): string {
  if (g === "by_model") return `${e.n} messages · ${usd(e.cost)} weighted`;
  if (g === "by_project") return `${e.n} messages · ${hhmm(e.first)}–${hhmm(e.last)} · ${usd(e.cost)}`;
  const [sid, cwd] = e.key as [string, string];
  return `${tidy(cwd)}  ·  ${sid.slice(0, 8)} · ${hhmm(e.first)}–${hhmm(e.last)} · ${usd(e.cost)}`;
}

/// The list header's count line. Which ranking produced it matters as much as
/// how many rows there are — "25 conversations" means something different when
/// the cut was by recency.
function listCount(w: WindowOut, rows: ThreadRow[]): string {
  if (group !== "by_session") {
    return `${rows.length} ${group === "by_project" ? "projects" : "models"} · ${w.messages} messages, ranked by weighted cost`;
  }
  const marks = rows.map(statusOf);
  const live = marks.filter((m) => m.working).length;
  const unread = marks.filter((m) => m.unread).length;
  const away = rows.filter((r) => r.remote).length;
  const flags = [
    live ? `${live} working` : "",
    unread ? `${unread} unread` : "",
    away ? `${away} on other devices` : "",
  ].filter(Boolean);
  const by = sortOf(snap) === "recent" ? "most recently active first" : "ranked by weighted cost";
  return `${rows.length} conversations · ${w.messages} messages, ${by}${flags.length ? " · " + flags.join(", ") : ""}`;
}

function detail(e: Entry): string {
  const rows = e.models.map((m) => `<tr>
      <td class="k">${short(m.model)}</td>
      <td>${tok(m.input)}</td><td>${tok(m.output)}</td><td>${tok(m.cache_write)}</td><td>${tok(m.cache_read)}</td>
      <td>${usd(m.cost)}</td>
      <td class="c">${(m.cost / (e.cost || 1) * 100).toFixed(1)}%</td>
      <td class="c">${e.pct === null ? "—" : (e.pct * m.cost / (e.cost || 1)).toFixed(2) + "%"}</td>
    </tr>`).join("");
  return `<div class="detail">
    <span class="eyebrow">Token kinds by model</span>
    <div class="dwrap"><table>
      <thead><tr><th>Model</th><th>Input</th><th>Output</th><th>Cache write</th><th>Cache read</th><th>Weighted</th><th>Of row</th><th>Of limit</th></tr></thead>
      <tbody>${rows}</tbody></table></div>
    <div class="dmeta">
      <span>first message <b>${when(e.first)}</b></span><span>last message <b>${when(e.last)}</b></span>
      <span>messages <b>${e.n}</b></span><span>raw tokens <b>${e.raw.toLocaleString()}</b></span>
      ${e.title_kind === "prompt" ? `<span>title from <b>first prompt</b></span>` : ""}
    </div></div>`;
}

/// A conversation running on another machine: a name, a state and a time, and
/// dashes where this machine's numbers would be. Not a button — there is no
/// transcript here to expand, and nothing to mark read that this app owns.
function remoteRow(row: ThreadRow & { remote: true }, snap: Snapshot | null): string {
  const r = row.r;
  const state = r.working ? "still working" : r.requires_action ? "waiting on you" : r.unread ? "unread" : "idle";
  const tip = `${threadName(row)}\non another device — no token counts reach this machine\n${state} · last active ${when(r.last)}`;
  return `<div class="row remote" title="${esc(tip)}">
      <span class="chip ghost"></span>
      ${statusMark(statusOf(row))}
      <span class="name"><b>${esc(threadName(row))}</b><em>${deviceTag(row, snap)}${esc(`${state} · last active ${when(r.last)}`)}</em></span>
      <span class="raw r">—</span>
      <span class="r mix"></span>
      <span class="pct r">—<small>not counted here</small></span>
    </div>`;
}

function renderList(): void {
  if (!snap) return;
  const w = snap.windows[sel];
  // Conversations are cut to the size you chose — after the account's other
  // devices are merged in, so the cut is of the whole list and not of this
  // machine's half. The other groupings have ceilings of their own that nobody
  // has ever wanted to change.
  const rows = group === "by_session"
    ? mergeThreads(snap, w).slice(0, listRows(snap))
    : localRows(w[group]);
  $("listTitle").innerHTML = `${esc(w.label)} <span>${esc(listCount(w, rows))}</span>`;
  $("sortBtns").hidden = group !== "by_session";
  if (rows.length === 0) {
    $("rows").innerHTML = `<div class="empty">Nothing in this window yet.</div>`;
    return;
  }
  $("rows").innerHTML = rows.map((row, i) => {
    if (row.remote) return remoteRow(row, snap);
    const e = row.e;
    const mini = e.models.map((m) => `<i style="width:${(m.cost / (e.cost || 1) * 100).toFixed(2)}%;background:${modelColor(m.model)}"></i>`).join("");
    const pct = e.pct === null ? `${e.share.toFixed(1)}%<small>of usage</small>` : `${e.pct.toFixed(1)}%<small>${e.share.toFixed(0)}% of window</small>`;
    return `<button class="row${e.working ? " working" : e.unread ? " unread" : ""}" data-i="${i}" aria-expanded="${open === i}">
        <span class="chip" style="background:${modelColor(e.models[0].model)}"></span>
        ${statusMark(e)}
        <span class="name"><b>${esc(entryName(e, group))}</b><em>${group === "by_session" ? deviceTag(row, snap) : ""}${esc(sublabel(e, group))}</em></span>
        <span class="raw r">${tok(e.raw)}</span>
        <span class="r mix"><span class="minibar">${mini}</span></span>
        <span class="pct r">${pct}</span>
      </button>` + (open === i ? detail(e) : "");
  }).join("");
  // Remote rows carry no `data-i`, which is what keeps expanding, read-marking
  // and the ribbon's hover keyed to rows this machine actually recorded.
  document.querySelectorAll<HTMLButtonElement>(".row[data-i]").forEach((b) => {
    const row = rows[Number(b.dataset.i)];
    if (row.remote) return;
    const e = row.e;
    b.addEventListener("click", () => {
      const i = Number(b.dataset.i);
      open = open === i ? null : i;
      renderList();
      // Opening a conversation is the only "you have read this" signal the app
      // ever gets — Claude Code keeps no read receipts of its own. The badge
      // clears in the widget too, because the backend republishes.
      if (open === i && group === "by_session" && e.unread) void markRead(e);
    });
    // Only when the list is grouped the same way the ribbon is keyed.
    if (group !== mode) return;
    const key = mode === "by_model" ? (e.key as string) : (e.key as [string, string])[0];
    b.addEventListener("mouseenter", () => setHoverKey(key));
    b.addEventListener("mouseleave", () => setHoverKey(null));
  });
}

/// Clear one row's badge without waiting for the round trip to come back:
/// the click already told us, and a badge that lingers for a moment after you
/// open the row reads as a bug.
async function markRead(e: Entry): Promise<void> {
  e.unread = false;
  renderList();
  const [sid] = e.key as [string, string];
  try { apply(await markThreadRead(sid)); } catch { /* the next refresh will */ }
}

/// Ranking is one backend setting, so this returns the list both windows get.
async function chooseSort(sort: ThreadSort): Promise<void> {
  open = null;
  try { apply(await setThreadSort(sort)); } catch { /* no backend in dev */ }
}

function setWindow(i: number): void {
  sel = i; open = null;
  hoverKey = null; hoverBucket = null;
  if (snap) bucket = defaultBucket(snap.windows[i]);
  closeBucketMenu();
  document.querySelectorAll<HTMLElement>(".bigdial").forEach((d) => d.setAttribute("aria-selected", String(Number(d.dataset.i) === i)));
  renderLegend(); renderTimeline(); renderList();
}

function apply(s: Snapshot): void {
  const first = snap === null;
  snap = s;
  if (first) bucket = defaultBucket(s.windows[sel]);
  // The ranking can change under us — the widget's toggle republishes to both
  // windows — so the buttons follow the snapshot rather than a local variable.
  const cur = sortOf(s);
  document.querySelectorAll<HTMLButtonElement>("#sortBtns button")
    .forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.s === cur)));
  renderHeader(); renderBig(); renderLegend(); renderTimeline(); renderList();
}

// ---------- controls ----------

/// One `mode` drives the ring and the ribbon, so both toggles have to move
/// together whichever one was clicked.
function setMode(m: typeof mode): void {
  mode = m;
  hoverKey = null;
  document.querySelectorAll<HTMLButtonElement>("#modeBtns button,#tlModeBtns button")
    .forEach((x) => x.setAttribute("aria-pressed", String(x.dataset.m === m)));
  renderBig(); renderLegend(); renderTimeline(); renderList();
}
document.querySelectorAll<HTMLButtonElement>("#modeBtns button,#tlModeBtns button").forEach((b) =>
  b.addEventListener("click", () => setMode(b.dataset.m as typeof mode)));
document.querySelectorAll<HTMLButtonElement>("#sortBtns button").forEach((b) =>
  b.addEventListener("click", () => void chooseSort(b.dataset.s as ThreadSort)));
document.querySelectorAll<HTMLButtonElement>("#groupBtns button").forEach((b) =>
  b.addEventListener("click", () => {
    group = b.dataset.g as GroupKey; open = null;
    document.querySelectorAll<HTMLButtonElement>("#groupBtns button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    renderList();
  }));
$("refresh").addEventListener("click", async () => {
  const b = $("refresh") as HTMLButtonElement;
  b.disabled = true; b.textContent = "Scanning…";
  try { apply(await refreshNow()); } catch (e) { alert(String(e)); }
  b.disabled = false; b.textContent = "Refresh";
});
$("calibrate").addEventListener("click", openSettingsDialog);
bindThemeButton($("themebtn"));
$("themebtn").addEventListener("click", () => { if (dlg.open) renderSchemes(); });

// ---------- settings dialog ----------

const dlg = $("settings") as HTMLDialogElement;
const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
const num = (id: string): number | undefined => { const v = val(id); return v === "" ? undefined : Number(v); };

async function openSettingsDialog(): Promise<void> {
  const s: Settings = await getSettings();
  ($("f_session") as HTMLInputElement).value = "";
  ($("f_weekly") as HTMLInputElement).value = "";
  ($("f_fable") as HTMLInputElement).value = "";
  ($("f_rh") as HTMLInputElement).value = "";
  ($("f_rm") as HTMLInputElement).value = "";
  const wr = s.calibration.weekly_reset ?? { weekday: "Tue", hour: 1, minute: 0 };
  ($("f_wd") as HTMLSelectElement).value = wr.weekday;
  ($("f_wh") as HTMLInputElement).value = String(wr.hour);
  ($("f_wm") as HTMLInputElement).value = String(wr.minute);
  ($("f_plan") as HTMLInputElement).value = s.plan;
  ($("f_boost") as HTMLInputElement).value = s.boost ?? "";
  ($("f_dir") as HTMLInputElement).value = s.claude_dir ?? "";
  ($("f_refresh") as HTMLInputElement).value = String(s.refresh_secs);
  ($("f_rows") as HTMLInputElement).value = String(s.list_rows);
  ($("f_wrows") as HTMLInputElement).value = String(s.widget_rows);
  ($("f_widget") as HTMLInputElement).checked = s.show_widget;
  ($("f_ontop") as HTMLInputElement).checked = s.widget_on_top;
  try { ($("f_auto") as HTMLInputElement).checked = await getAutostart(); } catch { /* plugin unavailable in dev */ }
  const hint = (a: Settings["calibration"]["session"]) => a ? `anchored ${a.pct}% at ${when(a.captured_at)}` : "not set";
  const src = s.calibration.auto_fetched_at && (!s.calibration.manual_at || s.calibration.manual_at < s.calibration.auto_fetched_at)
    ? `Anchored from Claude Code's cached Usage reading (${when(s.calibration.auto_fetched_at)}). Values you enter below take precedence until a newer reading appears.`
    : s.calibration.manual_at ? `Anchored from your manual entry (${when(s.calibration.manual_at)}). A newer reading from Claude Code will replace it.`
    : `Nothing anchored yet.`;
  $("f_setup").innerHTML = setupPanel(snap, src);
  bindSetup($("f_setup"), { recheck: async (b) => { await recheck(b); if (dlg.open) { dlg.close(); void openSettingsDialog(); } } });
  // Manual entry is the fallback: closed unless it is already what's in use.
  ($("f_manual") as HTMLDetailsElement).open = snap?.calibration_source === "manual";
  ($("f_session") as HTMLInputElement).placeholder = hint(s.calibration.session);
  ($("f_weekly") as HTMLInputElement).placeholder = hint(s.calibration.weekly);
  ($("f_fable") as HTMLInputElement).placeholder = hint(s.calibration.fable);
  ($("f_live") as HTMLInputElement).checked = s.live_readings;
  renderConnection();
  $("f_err").textContent = "";
  renderSchemes();
  dlg.showModal();
}

/// State comes from the last snapshot's diagnosis, not from a fresh probe:
/// opening settings shouldn't cost a network round trip, or raise the OS
/// permission prompt before the user has asked for anything.
function renderConnection(): void {
  const d = snap ? diagOf(snap) : null;
  const box = $("f_connstate");
  if (!d?.connected) {
    box.className = "";
    box.textContent = "Off — using Claude Code's cached reading, or the percentages you typed in.";
    return;
  }
  if (d.live_error) {
    box.className = "bad";
    box.textContent = `On, but the last fetch failed — ${d.live_error}`;
  } else {
    box.className = "good";
    box.textContent = `On. Percentages come from Anthropic on every refresh${d.fetched_at ? `, last ${when(d.fetched_at)}` : ""}.`;
  }
}

/// Turning it on is refused by the backend unless a fetch actually works, so
/// a failure here leaves the checkbox where it was rather than lying about it.
async function toggleLive(): Promise<void> {
  const box = $("f_live") as HTMLInputElement;
  const want = box.checked;
  box.disabled = true;
  $("f_err").textContent = "";
  try {
    apply(await setLiveReadings(want));
  } catch (e) {
    box.checked = !want;
    $("f_err").textContent = String(e);
  } finally {
    box.disabled = false;
    renderConnection();
  }
}

/// Say whether it would work, without turning anything on.
async function checkLive(): Promise<void> {
  const btn = $("f_live_check") as HTMLButtonElement;
  const label = btn.textContent;
  btn.disabled = true;
  btn.textContent = "Checking…";
  $("f_err").textContent = "";
  const box = $("f_connstate");
  try {
    box.textContent = await checkLiveReadings();
    box.className = "good";
  } catch (e) {
    box.textContent = String(e);
    box.className = "bad";
  } finally {
    btn.disabled = false;
    btn.textContent = label;
  }
}

$("f_live").addEventListener("change", toggleLive);
$("f_live_check").addEventListener("click", checkLive);
// The settings dialog has its own copy buttons, outside any setup card.
bindSetup($("f_conn"), { recheck });

$("f_cancel").addEventListener("click", () => dlg.close());
// Click the backdrop to dismiss. The press has to have started on the backdrop
// too: dragging a selection out of a field and releasing over the dark area
// otherwise closes the dialog and loses what was typed.
let pressedBackdrop = false;
dlg.addEventListener("mousedown", (e) => { pressedBackdrop = e.target === dlg; });
dlg.addEventListener("click", (e) => {
  if (pressedBackdrop && e.target === dlg) dlg.close();
  pressedBackdrop = false;
});
$("settingsForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const save = $("f_save") as HTMLButtonElement;
  save.disabled = true; $("f_err").textContent = "";
  try {
    const rh = num("f_rh"), rm = num("f_rm");
    const input: CalibrationInput = {
      session_pct: num("f_session"),
      weekly_pct: num("f_weekly"),
      fable_pct: num("f_fable"),
      session_resets_in_minutes: rh === undefined && rm === undefined ? undefined : (rh ?? 0) * 60 + (rm ?? 0),
      weekly_reset: { weekday: val("f_wd") as WeeklyReset["weekday"], hour: Number(val("f_wh")), minute: Number(val("f_wm")) },
      plan: val("f_plan") || undefined,
      boost: val("f_boost"),
    };
    await updateSettings({
      claude_dir: val("f_dir"),
      refresh_secs: Number(val("f_refresh")) || 15,
      show_widget: ($("f_widget") as HTMLInputElement).checked,
      widget_on_top: ($("f_ontop") as HTMLInputElement).checked,
      list_rows: Number(val("f_rows")) || 25,
      widget_rows: Number(val("f_wrows")) || 5,
    });
    try { await setAutostart(($("f_auto") as HTMLInputElement).checked); } catch { /* ignore in dev */ }
    apply(await calibrate(input));
    dlg.close();
  } catch (err) {
    $("f_err").textContent = String(err);
  } finally {
    save.disabled = false;
  }
});


// ---------- appearance ----------

// Which palette the custom editor is editing — not necessarily the one showing,
// since a scheme carries both and the theme button decides which is on screen.
let cmode: "light" | "dark" = "light";

const SWATCH = ["--ground", "--accent", "--m-fable51", "--m-sonnet5", "--m-opus5", "--ink"];

function schemeCard(id: string, name: string, note: string, cur: string, mode: "light" | "dark"): string {
  const p = paletteOf(id, mode);
  const sw = SWATCH.map((v) => `<i style="background:${p[v]}"></i>`).join("");
  return `<button type="button" class="scard" role="radio" data-scheme="${id}" aria-checked="${id === cur}">
    <span class="sw">${sw}</span><b>${esc(name)}</b><span>${esc(note)}</span></button>`;
}

function renderSchemes(): void {
  const cur = currentSchemeId();
  const mode = effectiveTheme();
  const custom = customScheme();
  const n = Object.keys(custom.light).length + Object.keys(custom.dark).length;
  $("f_schemes").innerHTML =
    SCHEMES.map((s) => schemeCard(s.id, s.name, s.note, cur, mode)).join("") +
    schemeCard(CUSTOM_ID, "Custom", n ? `${n} color${n === 1 ? "" : "s"} of your own` : "pick every color yourself", cur, mode);
  $("f_schemes").querySelectorAll<HTMLButtonElement>(".scard").forEach((b) =>
    b.addEventListener("click", () => chooseScheme(b.dataset.scheme!)));
  $("f_custom").hidden = cur !== CUSTOM_ID;
  if (cur === CUSTOM_ID) renderCustom();
}

function chooseScheme(id: string): void {
  const from = currentSchemeId();
  if (id === CUSTOM_ID) {
    // Switching in with nothing of your own yet: start from what you were
    // looking at, so the pickers open on the palette you just had.
    const c = customScheme();
    if (!Object.keys(c.light).length && !Object.keys(c.dark).length) {
      saveCustom({ light: paletteOf(from, "light"), dark: paletteOf(from, "dark") });
    }
    cmode = effectiveTheme();
  }
  setScheme(id);
  void broadcastScheme();
  renderSchemes();
}

function renderCustom(): void {
  const showing = effectiveTheme();
  $("f_cnote").textContent = cmode === showing
    ? `Editing the ${cmode} palette — the one on screen now.`
    : `Editing the ${cmode} palette. The app is showing ${showing}, so these changes won't appear until you switch themes.`;
  $("f_cmode").querySelectorAll<HTMLButtonElement>("button").forEach((b) =>
    b.setAttribute("aria-pressed", String(b.dataset.mode === cmode)));
  const p = paletteOf(CUSTOM_ID, cmode);
  $("f_colors").innerHTML = GROUPS.map((g) => `<div class="cgroup"><h4>${esc(g.name)}</h4><div class="cswatches">${
    g.tokens.map((t) => `<label class="citem">
      <input type="color" value="${p[t.v]}" data-var="${t.v}" aria-label="${esc(g.name)} — ${esc(t.label)}">
      <span class="cl"><b>${esc(t.label)}</b><code>${p[t.v]}</code></span></label>`).join("")
  }</div></div>`).join("");
  $("f_colors").querySelectorAll<HTMLInputElement>("input[type=color]").forEach((i) =>
    i.addEventListener("input", () => {
      const c = customScheme();
      c[cmode][i.dataset.var!] = i.value;
      saveCustom(c);
      applyScheme();
      void broadcastScheme();
      const code = i.parentElement!.querySelector("code");
      if (code) code.textContent = i.value;
    }));
}

$("f_cmode").querySelectorAll<HTMLButtonElement>("button").forEach((b) =>
  b.addEventListener("click", () => { cmode = b.dataset.mode as "light" | "dark"; renderCustom(); }));

$("f_cseed").addEventListener("change", () => {
  const sel = $("f_cseed") as HTMLSelectElement;
  if (!sel.value) return;
  saveCustom({ light: paletteOf(sel.value, "light"), dark: paletteOf(sel.value, "dark") });
  sel.value = "";
  applyScheme();
  void broadcastScheme();
  renderSchemes();
});

$("f_creset").addEventListener("click", () => {
  saveCustom({ light: {}, dark: {} });
  applyScheme();
  void broadcastScheme();
  renderSchemes();
});

($("f_cseed") as HTMLSelectElement).append(...SCHEMES.map((s) => new Option(s.name, s.id)));

// ---------- boot ----------

(async () => {
  try { setHome(await getHome()); } catch { /* fine */ }
  const s = await getSnapshot();
  if (s) apply(s);
  else {
    $("bigdials").innerHTML = `<div class="empty">Reading transcripts for the first time — this takes a few seconds.</div>`;
  }
  await onSnapshot(apply);
  await onOpenSettings(openSettingsDialog);
  await onSchemeChanged(() => { applyScheme(); if (dlg.open) renderSchemes(); });
})();
