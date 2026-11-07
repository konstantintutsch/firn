export default function (eleventyConfig) {
  eleventyConfig.addPassthroughCopy({ "src/css": "css", "src/js": "js" });
  eleventyConfig.addPassthroughCopy({
    "node_modules/maplibre-gl/dist/maplibre-gl.mjs": "vendor/maplibre-gl.mjs",
    "node_modules/maplibre-gl/dist/maplibre-gl-worker.mjs": "vendor/maplibre-gl-worker.mjs",
    "node_modules/maplibre-gl/dist/maplibre-gl.css": "vendor/maplibre-gl.css",
  });

  return { dir: { input: "src", output: "_site" } };
}
