// Three states: explicit light, explicit dark, or follow the system (nothing
// stamped on the root). An explicit choice is remembered per window.

const KEY = "tl-theme";

export function applySavedTheme(): void {
  let saved: string | null = null;
  try { saved = localStorage.getItem(KEY); } catch { /* storage may be unavailable */ }
  if (saved === "light" || saved === "dark") document.documentElement.dataset.theme = saved;
}

export const effectiveTheme = (): "light" | "dark" =>
  (document.documentElement.dataset.theme as "light" | "dark" | undefined) ??
  (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");

export function toggleTheme(): "light" | "dark" {
  const next = effectiveTheme() === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  try { localStorage.setItem(KEY, next); } catch { /* ignore */ }
  return next;
}

export function bindThemeButton(btn: HTMLElement): void {
  const sync = () => {
    const next = effectiveTheme() === "dark" ? "light" : "dark";
    btn.setAttribute("aria-label", `Switch to ${next} theme`);
    btn.setAttribute("title", `Switch to ${next} theme`);
  };
  btn.addEventListener("click", () => { toggleTheme(); sync(); });
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", sync);
  sync();
}
