/**
 * Small pictures of tracks for lists: the map under the track. One hidden map
 * draws them one after the other, and each picture is a still image of it, so
 * a long list needs no map per entry. Until its picture is ready, an entry
 * shows the shape of its track.
 */

import * as maplibregl from "/vendor/maplibre-gl.mjs";
import { currentTheme, onThemeChange } from "/js/account.js";
import { svgElement } from "/js/format.js";

const WIDTH = 96;
const HEIGHT = 72;
const PADDING = 10;
const MAX_ZOOM = 16;
/** A picture that takes longer than this is skipped, so that one failure does not stall the rest. */
const RENDER_TIMEOUT_MS = 8000;
/** Starts pictures a little before their entries come into view. */
const ROOT_MARGIN = "300px";
const STYLES = {
  light: "https://tiles.openfreemap.org/styles/positron",
  dark: "https://tiles.openfreemap.org/styles/dark",
};
const LINE_COLORS = { light: "#d6246e", dark: "#fbd25d" };
const SOURCE_ID = "preview";

/** Finished pictures by theme and track. */
const pictures = new Map();
/** Entries that wait for their picture, or have one that the next theme replaces. */
const entries = new Set();
const queue = [];
let rendering = false;
let map = null;
let current = null;
let theme = currentTheme();

const observer = new IntersectionObserver(
  (changes) => {
    for (const { isIntersecting, target } of changes) {
      if (!isIntersecting) continue;
      observer.unobserve(target);
      enqueue(target);
    }
  },
  { rootMargin: ROOT_MARGIN },
);

const keyOf = (points) => `${theme}:${points.map(([lat, lon]) => `${lat},${lon}`).join(";")}`;

/** The shape of the track as an SVG, without a map. North is up. */
function shape(points) {
  const svg = svgElement("svg", { viewBox: `0 0 ${WIDTH} ${HEIGHT}`, "aria-hidden": "true" });
  // Degrees of longitude get shorter towards the poles.
  const squeeze = Math.cos((points[0][0] * Math.PI) / 180);
  const xs = points.map(([, lon]) => lon * squeeze);
  const ys = points.map(([lat]) => -lat);
  const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const scale = Math.min(
    (WIDTH - 2 * PADDING) / Math.max(maxX - minX, 1e-9),
    (HEIGHT - 2 * PADDING) / Math.max(maxY - minY, 1e-9),
  );
  const offsetX = (WIDTH - (maxX - minX) * scale) / 2;
  const offsetY = (HEIGHT - (maxY - minY) * scale) / 2;
  const path = xs.map((x, index) => `${(offsetX + (x - minX) * scale).toFixed(1)},${(offsetY + (ys[index] - minY) * scale).toFixed(1)}`);
  svgElement("polyline", { points: path.join(" ") }, svg);
  return svg;
}

/** A picture of a track, from `[latitude, longitude]` points. It fills in when it is near the screen. */
export function trackPreview(points) {
  const element = document.createElement("div");
  element.className = "track-preview";
  if (points.length < 2) return element;
  element.append(shape(points));
  element.points = points;
  entries.add(element);
  const picture = pictures.get(keyOf(points));
  if (picture) show(element, picture);
  else observer.observe(element);
  return element;
}

function show(element, picture) {
  element.style.backgroundImage = `url(${picture})`;
  element.classList.add("has-picture");
}

function enqueue(element) {
  const picture = pictures.get(keyOf(element.points));
  if (picture) {
    show(element, picture);
    return;
  }
  queue.push(element);
  if (!rendering) drain();
}

async function drain() {
  rendering = true;
  while (queue.length > 0) {
    const element = queue.shift();
    // Entries that left the page, like after a new search, need no picture.
    if (!element.isConnected) continue;
    const key = keyOf(element.points);
    if (!pictures.has(key)) {
      try {
        pictures.set(key, await render(element.points));
      } catch {
        continue;
      }
    }
    show(element, pictures.get(key));
  }
  rendering = false;
}

function ready() {
  if (map != null) return current;
  const container = document.createElement("div");
  container.style.cssText = `position:fixed;left:-${WIDTH * 2}px;top:0;width:${WIDTH}px;height:${HEIGHT}px;pointer-events:none;`;
  document.body.append(container);
  map = new maplibregl.Map({
    container,
    style: STYLES[theme],
    interactive: false,
    attributionControl: false,
    fadeDuration: 0,
    renderWorldCopies: false,
    canvasContextAttributes: { preserveDrawingBuffer: true },
  });
  map.on("style.load", () => {
    map.addSource(SOURCE_ID, { type: "geojson", data: { type: "FeatureCollection", features: [] } });
    map.addLayer({
      id: SOURCE_ID,
      type: "line",
      source: SOURCE_ID,
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-width": 3, "line-color": LINE_COLORS[theme] },
    });
  });
  current = new Promise((resolve) => map.once("load", resolve));
  return current;
}

/** Waits for the map to draw what it was just told to, which can be a view it shows already. */
function drawn() {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout")), RENDER_TIMEOUT_MS);
    map.once("idle", () => {
      clearTimeout(timer);
      resolve();
    });
    map.triggerRepaint();
  });
}

async function render(points) {
  await ready();
  const line = points.map(([lat, lon]) => [lon, lat]);
  map.getSource(SOURCE_ID).setData({ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: line } });
  const bounds = line.reduce((box, point) => box.extend(point), new maplibregl.LngLatBounds(line[0], line[0]));
  map.fitBounds(bounds, { padding: PADDING, animate: false, maxZoom: MAX_ZOOM });
  await drawn();
  return map.getCanvas().toDataURL("image/jpeg", 0.85);
}

// A new theme needs new pictures, in the style that fits.
onThemeChange((next) => {
  theme = next;
  queue.length = 0;
  if (map != null) {
    current = new Promise((resolve) => map.once("style.load", resolve));
    map.setStyle(STYLES[theme]);
  }
  for (const element of entries) {
    if (!element.isConnected) {
      entries.delete(element);
      continue;
    }
    element.classList.remove("has-picture");
    element.style.backgroundImage = "";
    observer.unobserve(element);
    observer.observe(element);
  }
});
