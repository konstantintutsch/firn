/** Inline SVG icons on a 24 by 24 grid, drawn with the current text color. */
const ICONS = {
  map: '<path d="M9 4 3 6v14l6-2 6 2 6-2V4l-6 2-6-2Z"/><path d="M9 4v14M15 6v14"/>',
  list: '<path d="M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01"/>',
  chart: '<path d="M4 4v16h16"/><path d="M8 16v-4M12 16V8M16 16v-6"/>',
  route: '<circle cx="6" cy="18" r="2"/><circle cx="18" cy="6" r="2"/><path d="M8 18h7a3 3 0 0 0 0-6H9a3 3 0 0 1 0-6h7"/>',
  settings:
    '<path d="M4 6h9M17 6h3M4 12h3M11 12h9M4 18h11M19 18h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>',
  sync: '<path d="M19.73 14.07A8 8 0 0 1 6 17.3L4 15"/><path d="M4.27 9.93A8 8 0 0 1 18 6.7L20 9"/><path d="M20 4v5h-5M4 20v-5h5"/>',
  close: '<path d="M6 6l12 12M18 6 6 18"/>',
  previous: '<path d="m15 6-6 6 6 6"/>',
  collapse: '<path d="m6 15 6-6 6 6"/>',
  expand: '<path d="m6 9 6 6 6-6"/>',
  download: '<path d="M12 4v11"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>',
  external: '<path d="M14 5h5v5"/><path d="M19 5l-8 8"/><path d="M18 14v5H5V6h5"/>',
  trophy: '<path d="M8 4h8v5a4 4 0 0 1-8 0V4Z"/><path d="M8 6H5a3 3 0 0 0 3 4M16 6h3a3 3 0 0 1-3 4"/><path d="M12 13v4M8 20h8"/>',
  next: '<path d="m9 6 6 6-6 6"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-4-4"/>',
};

export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy({ "src/css": "css", "src/js": "js", "src/icons": "icons", "src/manifest.webmanifest": "manifest.webmanifest", "src/sw.js": "sw.js" });
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
