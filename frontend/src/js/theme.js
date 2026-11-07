// Applies a saved theme before the first paint, to avoid a flash of the wrong one.
// A classic script in <head>, not a module, so that it runs before the page renders.

/** Colors the browser's bar for the chosen theme, or for the system's without a choice. */
function applyThemeColor() {
  const theme = document.documentElement.dataset.theme;
  for (const meta of document.querySelectorAll('meta[name="theme-color"]')) {
    const scheme = meta.dataset.scheme;
    meta.media = !theme ? `(prefers-color-scheme: ${scheme})` : scheme === theme ? "all" : "not all";
  }
}

try {
  const theme = localStorage.getItem("theme");
  if (theme) document.documentElement.dataset.theme = theme;
} catch {}
applyThemeColor();
