// Applies a saved theme before the first paint, to avoid a flash of the wrong one.
// A classic script in <head>, not a module, so that it runs before the page renders.
try {
  const theme = localStorage.getItem("theme");
  if (theme) document.documentElement.dataset.theme = theme;
} catch {}
