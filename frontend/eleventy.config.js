/** Inline SVG icons on a 24 by 24 grid, drawn with the current text color. */
const ICONS = {
  map: '<path d="M9 4 3 6v14l6-2 6 2 6-2V4l-6 2-6-2Z"/><path d="M9 4v14M15 6v14"/>',
  list: '<path d="M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01"/>',
  chart: '<path d="M4 4v16h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/>',
  settings:
    '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>',
  sync: '<path d="M20 12a8 8 0 0 1-14 5.3L4 15"/><path d="M4 12a8 8 0 0 1 14-5.3L20 9"/><path d="M20 4v5h-5M4 20v-5h5"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  close: '<path d="M6 6l12 12M18 6 6 18"/>',
  moon: '<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5Z"/>',
};

export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy({ "src/css": "css", "src/js": "js", "src/icons": "icons", "src/manifest.webmanifest": "manifest.webmanifest" });
  eleventyConfig.addPassthroughCopy({
    "node_modules/maplibre-gl/dist/maplibre-gl.mjs": "vendor/maplibre-gl.mjs",
    "node_modules/maplibre-gl/dist/maplibre-gl-worker.mjs": "vendor/maplibre-gl-worker.mjs",
    "node_modules/maplibre-gl/dist/maplibre-gl.css": "vendor/maplibre-gl.css",
  });
  eleventyConfig.addShortcode(
    "icon",
    (name) => `<svg class="icon icon-${name}" viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`,
  );

  return { dir: { input: "src", output: "_site" } };
}
