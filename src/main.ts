// The ledger window: three big dials, a legend, the full conversation list,
// and the calibration/settings dialog.
import { applySavedTheme, bindThemeButton } from "./shared/theme";
import {
  calibrate, getAutostart, getHome, getSettings, getSnapshot, onOpenSettings, onSnapshot,
  refreshNow, setAutostart, updateSettings,
} from "./shared/bridge";
import { convColor, modelColor, paint, paceNote } from "./shared/color";
import { entryName, esc, hhmm, setHome, short, tidy, tok, until, usd, when } from "./shared/format";
import { grow, numText, ringArcs } from "./shared/ring";
import type { CalibrationInput, Entry, GroupKey, Settings, Snapshot, WeeklyReset, WindowOut } from "./shared/types";

applySavedTheme();

const BR = 58;
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let snap: Snapshot | null = null;
let sel = 0;
let mode: "by_model" | "by_session" = "by_model";
let group: GroupKey = "by_session";
let open: number | null = null;

// ---------- header + notices ----------

function renderHeader(): void {
  if (!snap) return;
  $("tier").textContent = snap.plan;
  $("stamp").textContent = "snapshot " + when(snap.generated_at);
  const notices: string[] = [];
  const cache = snap.usage_cache;
  const cacheAge = cache ? (Date.now() - new Date(cache.fetched_at).getTime()) / 3.6e6 : null;
  if (!snap.calibrated) {
    const why = cache
      ? `Claude Code's cached Usage reading is from ${when(cache.fetched_at)} (${cacheAge!.toFixed(0)} h ago) and too low to anchor on. Run /usage in a Claude Code terminal session to refresh it (the desktop app's Usage tab does not), or enter the values by hand.`
      : `No cached Usage reading was found in Claude Code's config. Read the three percentages off the Usage tab and enter them.`;
    notices.push(`<div class="notice warn"><span><b>Not calibrated.</b> Rings show relative breakdown only. ${esc(why)}</span><button class="btn primary" data-open-settings>Calibrate</button></div>`);
  } else if (snap.calibration_source === "auto" && cache) {
    notices.push(`<div class="notice"><span><b>Auto-calibrated</b> from Claude Code's own Usage reading, fetched ${when(cache.fetched_at)}${cacheAge! > 24 ? ` — ${cacheAge!.toFixed(0)} h old; run /usage in a Claude Code terminal session to refresh it` : ""}.</span></div>`);
  }
  if (snap.boost) {
    notices.push(`<div class="notice"><span><b>Boost active.</b> ${esc(snap.boost)}. These meters read against the boosted ceiling.</span></div>`);
  }
  const rolling = snap.windows.find((w) => w.id === "session" && !w.boundary_known);
  if (rolling) {
    notices.push(`<div class="notice warn"><span><b>Session boundary unknown.</b> No transcripts found yet, so the session dial is a rolling 5-hour window.</span></div>`);
  }
  $("notices").innerHTML = notices.join("");
  $("notices").querySelectorAll<HTMLButtonElement>("[data-open-settings]").forEach((b) => b.addEventListener("click", openSettingsDialog));
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

// ---------- list ----------

function sublabel(e: Entry, g: GroupKey): string {
  if (g === "by_model") return `${e.n} messages · ${usd(e.cost)} weighted`;
  if (g === "by_project") return `${e.n} messages · ${hhmm(e.first)}–${hhmm(e.last)} · ${usd(e.cost)}`;
  const [sid, cwd] = e.key as [string, string];
  return `${tidy(cwd)}  ·  ${sid.slice(0, 8)} · ${hhmm(e.first)}–${hhmm(e.last)} · ${usd(e.cost)}`;
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

function renderList(): void {
  if (!snap) return;
  const w = snap.windows[sel], rows = w[group];
  $("listTitle").innerHTML = `${esc(w.label)} <span>${rows.length} ${group === "by_session" ? "conversations" : group === "by_project" ? "projects" : "models"} · ${w.messages} messages, ranked by weighted cost</span>`;
  if (rows.length === 0) {
    $("rows").innerHTML = `<div class="empty">Nothing in this window yet.</div>`;
    return;
  }
  $("rows").innerHTML = rows.map((e, i) => {
    const mini = e.models.map((m) => `<i style="width:${(m.cost / (e.cost || 1) * 100).toFixed(2)}%;background:${modelColor(m.model)}"></i>`).join("");
    const pct = e.pct === null ? `${e.share.toFixed(1)}%<small>of usage</small>` : `${e.pct.toFixed(1)}%<small>${e.share.toFixed(0)}% of window</small>`;
    return `<button class="row" data-i="${i}" aria-expanded="${open === i}">
        <span class="chip" style="background:${modelColor(e.models[0].model)}"></span>
        <span class="name"><b>${esc(entryName(e, group))}</b><em>${esc(sublabel(e, group))}</em></span>
        <span class="raw r">${tok(e.raw)}</span>
        <span class="r mix"><span class="minibar">${mini}</span></span>
        <span class="pct r">${pct}</span>
      </button>` + (open === i ? detail(e) : "");
  }).join("");
  document.querySelectorAll<HTMLButtonElement>(".row").forEach((b) =>
    b.addEventListener("click", () => { const i = Number(b.dataset.i); open = open === i ? null : i; renderList(); }));
}

function setWindow(i: number): void {
  sel = i; open = null;
  document.querySelectorAll<HTMLElement>(".bigdial").forEach((d) => d.setAttribute("aria-selected", String(Number(d.dataset.i) === i)));
  renderLegend(); renderList();
}

function apply(s: Snapshot): void {
  snap = s;
  renderHeader(); renderBig(); renderLegend(); renderList();
}

// ---------- controls ----------

document.querySelectorAll<HTMLButtonElement>("#modeBtns button").forEach((b) =>
  b.addEventListener("click", () => {
    mode = b.dataset.m as typeof mode;
    document.querySelectorAll<HTMLButtonElement>("#modeBtns button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    renderBig(); renderLegend();
  }));
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
  ($("f_widget") as HTMLInputElement).checked = s.show_widget;
  try { ($("f_auto") as HTMLInputElement).checked = await getAutostart(); } catch { /* plugin unavailable in dev */ }
  const hint = (a: Settings["calibration"]["session"]) => a ? `anchored ${a.pct}% at ${when(a.captured_at)}` : "not set";
  const src = s.calibration.auto_fetched_at && (!s.calibration.manual_at || s.calibration.manual_at < s.calibration.auto_fetched_at)
    ? `Currently anchored from Claude Code's cached Usage reading (${when(s.calibration.auto_fetched_at)}). Values you enter here take precedence until a newer reading appears.`
    : s.calibration.manual_at ? `Currently anchored from your manual entry (${when(s.calibration.manual_at)}). A newer reading from Claude Code will replace it.`
    : `Nothing anchored yet. Claude Code's cached Usage reading is applied automatically when it's fresh; enter values here to anchor now.`;
  $("f_src").textContent = src;
  ($("f_session") as HTMLInputElement).placeholder = hint(s.calibration.session);
  ($("f_weekly") as HTMLInputElement).placeholder = hint(s.calibration.weekly);
  ($("f_fable") as HTMLInputElement).placeholder = hint(s.calibration.fable);
  $("f_err").textContent = "";
  dlg.showModal();
}

$("f_cancel").addEventListener("click", () => dlg.close());
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
      refresh_secs: Number(val("f_refresh")) || 60,
      show_widget: ($("f_widget") as HTMLInputElement).checked,
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
})();
