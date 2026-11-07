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

// Pages are written in English and translated by a module that runs later.
// Until then, German pages stay hidden, instead of showing English for a moment.
// /js/i18n.js chooses the language the same way and shows the page again.
try {
  const chosen = localStorage.getItem("language");
  const preferred =
    chosen ?? (navigator.languages ?? [navigator.language]).find((tag) => /^(de|en)\b/i.test(tag));
  if (preferred?.toLowerCase().startsWith("de")) {
    document.documentElement.dataset.translating = "";
    // Shows the page anyway if the translation never runs, for example after an error.
    setTimeout(() => delete document.documentElement.dataset.translating, 2000);
  }
} catch {}
