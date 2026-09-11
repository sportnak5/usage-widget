// The conversation list, as far as both windows agree on it: how it is ranked,
// and what a row says about itself. The ranking is one setting held by the
// backend — the snapshot carries it — so the widget and the ledger show the
// same list and the same toggle state without talking to each other.
import { basename, esc } from "./format";
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
  // Under the weighted ranking there is nothing to weigh a remote row by, so
  // they follow the locals rather than landing somewhere arbitrary inside a
  // ranking they can't take part in. Recency they can be ranked by, so they are.
  return sortOf(snap) === "recent"
    ? [...locals, ...remote].sort((a, b) => lastOf(b) - lastOf(a))
    : [...locals, ...remote];
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

/** Which machine a row is on, as far as anything can tell.
 *
 *  Local rows get this machine's name. Remote ones can't be named at all — no
 *  field in the session list identifies a device — so the tag says "elsewhere",
 *  or names the repo it reported working in, which is the only hint there is. */
export function deviceTag(row: ThreadRow, snap: Snapshot | null): string {
  if (!row.remote) {
    const label = snap?.device_label ?? "";
    return label ? `<span class="dtag" title="Running on this machine">${esc(label)}</span>` : "";
  }
  const repo = row.r.repo;
  const tip = repo ? `Running on another device · ${repo}` : "Running on another device";
  return `<span class="dtag away" title="${esc(tip)}">${esc(repo ? basename(repo) : "elsewhere")}</span>`;
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
