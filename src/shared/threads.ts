// The conversation list, as far as both windows agree on it: how it is ranked,
// and what a row says about itself. The ranking is one setting held by the
// backend — the snapshot carries it — so the widget and the ledger show the
// same list and the same toggle state without talking to each other.
import { esc } from "./format";
import type { Entry, RemoteThread, Snapshot, ThreadSort, WindowOut } from "./types";

export const SORTS: { id: ThreadSort; label: string; note: string }[] = [
  { id: "usage", label: "Top", note: "heaviest first" },
  { id: "recent", label: "Recent", note: "most recently active first" },
];

/** A fixture written before the toggle existed ranks by usage, as it did then. */
export const sortOf = (s: Snapshot | null): ThreadSort => s?.thread_sort ?? "usage";

/** How many rows each window draws. The backend already truncated the list to
 *  the larger of the two, so this is the second cut, not the only one. */
export const listRows = (s: Snapshot | null): number => s?.list_rows ?? 25;
export const widgetRows = (s: Snapshot | null): number => s?.widget_rows ?? 5;

export const sortMeta = (id: ThreadSort) => SORTS.find((s) => s.id === id) ?? SORTS[0];

export const otherSort = (id: ThreadSort): ThreadSort => (id === "usage" ? "recent" : "usage");

/** A row in a conversation list. Either a conversation this machine wrote a
 *  transcript for — priced, expandable, the only kind there used to be — or one
 *  the account reports from another device, which has state and a name and
 *  nothing else. Both windows draw the same union. */
export type ThreadRow =
  | { remote: false; e: Entry }
  | { remote: true; r: RemoteThread };

export const localRows = (es: Entry[]): ThreadRow[] => es.map((e): ThreadRow => ({ remote: false, e }));

const lastOf = (row: ThreadRow): number => Date.parse(row.remote ? row.r.last : row.e.last);

/** A remote row is priced once its event stream has been walked; before that
 *  there is nothing to rank it by. */
export const priced = (row: ThreadRow): boolean => !row.remote || row.r.models.length > 0;

/** Weighted dollars, or -1 for a row we have no number for yet, which sorts it
 *  to the bottom rather than to the top. */
const weightOf = (row: ThreadRow): number =>
  row.remote ? (row.r.models.length ? row.r.cost : -1) : row.e.cost;

/** The conversation list for one window: what this machine recorded, plus what
 *  the account says is running elsewhere.
 *
 *  Three rules do the merging. A row that ran here is dropped — `by_session`
 *  already holds it, with the cost and the transcript the remote row lacks, so
 *  the local entry is the one that survives and that is the whole of the
 *  de-duplication. An archived row is a conversation its owner closed out.
 *  And the remote list is account-wide rather than windowed, so a thread last
 *  touched before this window opened has no business in it. */
export function mergeThreads(snap: Snapshot | null, w: WindowOut): ThreadRow[] {
  const locals = localRows(w.by_session);
  const start = Date.parse(w.start);
  const remote = (snap?.remote_threads ?? [])
    .filter((r) => !r.this_device && !r.archived && Date.parse(r.last) >= start)
    .map((r): ThreadRow => ({ remote: true, r }))
    .sort((a, b) => lastOf(b) - lastOf(a));
  if (remote.length === 0) return locals;
  const all = [...locals, ...remote];
  // Both rankings now take remote rows: their tokens come from the session's
  // own event stream. A row whose stream has not been walked yet has no weight,
  // so under Top it waits at the bottom until it has one. Its cost is a floor
  // (no output tokens in the stream), so it ranks a little low against a local
  // row — better than the arbitrary place it used to be given.
  return sortOf(snap) === "recent"
    ? all.sort((a, b) => lastOf(b) - lastOf(a))
    : all.sort((a, b) => weightOf(b) - weightOf(a));
}

/** A row's status in the local vocabulary. Anthropic's `requires_action` is a
 *  turn that stopped and wants you — the same call to action as an unread one,
 *  so it wears the same mark rather than inventing a third. */
export const statusOf = (row: ThreadRow): Pick<Entry, "working" | "unread"> =>
  row.remote ? { working: row.r.working, unread: row.r.unread || row.r.requires_action } : row.e;

/** What a row calls itself. A remote row's title is Anthropic's, which is
 *  usually the first prompt; without one the repo says more than the id would. */
export const threadName = (row: ThreadRow): string =>
  row.remote ? row.r.title || row.r.repo || "untitled session" : row.e.title || "untitled session";

/** Marks a row as running somewhere that isn't this machine.
 *
 *  Nothing identifies *which* machine — no field in the session list names one
 *  — so the tag says only that, and local rows carry no tag at all: a label on
 *  every row but the odd one out is noise, and "not here" is the whole signal.
 *  The repo it reported working in is the nearest thing to a place, so it rides
 *  the tooltip rather than the tag, where it used to read as a device name. */
export function remoteTag(row: ThreadRow): string {
  if (!row.remote) return "";
  const repo = row.r.repo;
  const tip = repo ? `Running on another device \u00b7 ${repo}` : "Running on another device";
  return `<span class="dtag" title="${esc(tip)}">Remote</span>`;
}

/** The dot in front of a conversation: still working, or waiting to be read.
 *  Nothing at all when neither — a list where every row wears a badge says
 *  nothing, so only the two states worth interrupting for get one. */
export function statusMark(e: Pick<Entry, "working" | "unread">): string {
  if (e.working) {
    return `<span class="tmark work" title="The agent is still working in this conversation" role="img" aria-label="still working"></span>`;
  }
  if (e.unread) {
    return `<span class="tmark new" title="New messages you haven't opened yet" role="img" aria-label="unread"></span>`;
  }
  return `<span class="tmark" aria-hidden="true"></span>`;
}

/** How a row's status reads in a tooltip that is already prose. */
export function statusNote(e: Pick<Entry, "working" | "unread">): string {
  return e.working ? "\nthe agent is still working" : e.unread ? "\nunread" : "";
}

/** The widget's ranking toggle: both labels side by side, divided by a rule,
 *  with the live one outlined. Showing only the alternative would have been
 *  smaller, but then the caption has to say which ranking you are looking at —
 *  and a control that names the state it is *not* in is read wrong as often as
 *  it is read right. */
export function sortToggle(cur: ThreadSort): string {
  const opts = SORTS.map((s) => `<button type="button" data-s="${s.id}" aria-pressed="${s.id === cur}"
      title="Rank by ${esc(s.note)}">${esc(s.label)}</button>`).join("");
  return `<span class="tsort" id="wsort" role="group" aria-label="Rank conversations by">${opts}</span>`;
}
