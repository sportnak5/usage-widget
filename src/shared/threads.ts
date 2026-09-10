// The conversation list, as far as both windows agree on it: how it is ranked,
// and what a row says about itself. The ranking is one setting held by the
// backend — the snapshot carries it — so the widget and the ledger show the
// same list and the same toggle state without talking to each other.
import { esc } from "./format";
import type { Entry, Snapshot, ThreadSort } from "./types";

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

/** The dot in front of a conversation: still working, or waiting to be read.
 *  Nothing at all when neither — a list where every row wears a badge says
 *  nothing, so only the two states worth interrupting for get one. */
export function statusMark(e: Entry): string {
  if (e.working) {
    return `<span class="tmark work" title="The agent is still working in this conversation" role="img" aria-label="still working"></span>`;
  }
  if (e.unread) {
    return `<span class="tmark new" title="New messages you haven't opened yet" role="img" aria-label="unread"></span>`;
  }
  return `<span class="tmark" aria-hidden="true"></span>`;
}

/** How a row's status reads in a tooltip that is already prose. */
export function statusNote(e: Entry): string {
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
