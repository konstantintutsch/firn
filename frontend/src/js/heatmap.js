import * as maplibregl from "/vendor/maplibre-gl.mjs";

const THEMES = {
  light: {
    styleUrl: "https://tiles.openfreemap.org/styles/positron",
    heatStops: [
      0, "#f06414",
      0.3, "#e03c14",
      0.55, "#d6246e",
      0.8, "#7b1fa2",
      1, "#2e1065",
    ],
  },
  dark: {
    styleUrl: "https://tiles.openfreemap.org/styles/dark",
    heatStops: [
      0, "#8e3fc4",
      0.3, "#d13a78",
      0.55, "#f4743b",
      0.8, "#fbd25d",
      1, "#fff7d6",
    ],
  },
};
const THEME_STORAGE_KEY = "theme";
const SOURCE_ID = "runs";
const HEAT_LAYER_ID = "runs-heat";
const REFRESH_DELAY_MS = 150;
/** Radius at the zoom the grid was built for, where cells are 1 px apart. */
const HEAT_RADIUS_PX = 1;
/** Extra radius at zoom 0, which fades out by about zoom 8. Keeps lone cells visible. */
const LOW_ZOOM_RADIUS_PX = 3;
const LOW_ZOOM_RADIUS_FALLOFF = 0.35;
/** Fetches beyond the viewport so that small pans do not show empty edges. */
const VIEWPORT_PADDING = 0.25;

const statsElement = document.querySelector("#stats");
const statusElement = document.querySelector("#status");
const legendElement = document.querySelector("#legend");
const themeToggle = document.querySelector("#theme-toggle");
const systemDark = matchMedia("(prefers-color-scheme: dark)");

let theme = currentTheme();

const map = new maplibregl.Map({
  container: "map",
  style: THEMES[theme].styleUrl,
  center: [0, 25],
  zoom: 1.5,
  maxZoom: 18,
  renderWorldCopies: false,
  hash: true,
  // Anti-aliasing fades circles this small at 1x, so those screens get extra pixels too.
  pixelRatio: Math.max(devicePixelRatio, 2),
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");

let pendingRequest = null;
let refreshTimer = null;
let hasRuns = false;
let cells = [];
let gridZoom = Math.floor(map.getZoom());
let isFirstStyle = true;

// Fires again after each theme change, because a new style drops source and layer.
map.on("style.load", () => {
  map.addSource(SOURCE_ID, { type: "geojson", data: toGeoJson(cells), tolerance: 0 });
  map.addLayer(
    {
      id: HEAT_LAYER_ID,
      type: "circle",
      source: SOURCE_ID,
      paint: {
        "circle-radius": heatRadius(),
        // A transition would briefly redraw new cells at the radius of the old grid.
        "circle-radius-transition": { duration: 0 },
        "circle-color": ["interpolate", ["linear"], ["get", "weight"], ...THEMES[theme].heatStops],
      },
    },
    layerAboveHeat(),
  );
  if (isFirstStyle) {
    isFirstStyle = false;
    loadStats();
    refresh();
  }
});

map.on("moveend", () => {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, REFRESH_DELAY_MS);
});

themeToggle.addEventListener("click", () => {
  const next = theme === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  try {
    localStorage.setItem(THEME_STORAGE_KEY, next);
  } catch {
    // Without storage the choice still applies until the page is reloaded.
  }
  applyTheme();
});

systemDark.addEventListener("change", applyTheme);
updateThemeToggle();

function currentTheme() {
  return document.documentElement.dataset.theme ?? (systemDark.matches ? "dark" : "light");
}

function applyTheme() {
  const next = currentTheme();
  if (next === theme) return;
  theme = next;
  updateThemeToggle();
  map.setStyle(THEMES[theme].styleUrl, { diff: false });
}

function updateThemeToggle() {
  themeToggle.textContent = theme === "dark" ? "Light" : "Dark";
  themeToggle.setAttribute("aria-label", `Switch to ${theme === "dark" ? "light" : "dark"} mode`);
}

/** Draws the heat above roads, which would cut through it, but below the labels on top. */
function layerAboveHeat() {
  const layers = map.getStyle().layers;
  return layers[layers.findLastIndex((layer) => layer.type !== "symbol") + 1]?.id;
}

async function refresh() {
  pendingRequest?.abort();
  const request = new AbortController();
  pendingRequest = request;

  try {
    const response = await fetch(`/api/heatmap?${viewportQuery()}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ cells, grid_zoom: gridZoom } = await response.json());
    map.getSource(SOURCE_ID)?.setData(toGeoJson(cells));
    if (map.getLayer(HEAT_LAYER_ID)) map.setPaintProperty(HEAT_LAYER_ID, "circle-radius", heatRadius());
    legendElement.hidden = cells.length === 0;
    if (hasRuns) showStatus("");
  } catch (error) {
    if (error.name !== "AbortError") showStatus("Could not load runs. Is the API running?");
  }
}

/** Grows with the map, so the heat keeps its place and size while zooming. */
function heatRadius() {
  const lowZoomRadius = Math.max(0, LOW_ZOOM_RADIUS_PX - LOW_ZOOM_RADIUS_FALLOFF * gridZoom);
  const radiusAt = (zoom) => (HEAT_RADIUS_PX + lowZoomRadius) * 2 ** (zoom - gridZoom);
  return ["interpolate", ["exponential", 2], ["zoom"], 0, radiusAt(0), 24, radiusAt(24)];
}

function viewportQuery() {
  const bounds = map.getBounds();
  const lonPadding = (bounds.getEast() - bounds.getWest()) * VIEWPORT_PADDING;
  const latPadding = (bounds.getNorth() - bounds.getSouth()) * VIEWPORT_PADDING;
  return new URLSearchParams({
    zoom: Math.floor(map.getZoom()),
    west: Math.max(bounds.getWest() - lonPadding, -180),
    south: Math.max(bounds.getSouth() - latPadding, -90),
    east: Math.min(bounds.getEast() + lonPadding, 180),
    north: Math.min(bounds.getNorth() + latPadding, 90),
  });
}

/**
 * Log scale, so a route run once stays visible next to one run a thousand times.
 * Sorted so that busy cells are drawn last and are never covered by quiet ones.
 * This replaces `circle-sort-key`, which costs one draw call per distinct key.
 */
function toGeoJson(cells) {
  const maxRuns = cells.reduce((max, cell) => Math.max(max, cell.runs), 1);
  return {
    type: "FeatureCollection",
    features: cells.toSorted((a, b) => a.runs - b.runs).map((cell) => ({
      type: "Feature",
      geometry: { type: "Point", coordinates: [cell.lon, cell.lat] },
      properties: { weight: Math.log1p(cell.runs) / Math.log1p(maxRuns) },
    })),
  };
}

async function loadStats() {
  try {
    const response = await fetch("/api/stats");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const { runs, distance_m: distance } = await response.json();
    hasRuns = runs > 0;
    if (hasRuns) {
      const km = Math.round(distance / 1000).toLocaleString();
      statsElement.textContent = `${runs.toLocaleString()} ${runs === 1 ? "run" : "runs"} · ${km} km`;
    } else {
      statusElement.innerHTML = "No runs saved yet. Run <code>just sync</code>.";
    }
  } catch {
    showStatus("Could not load runs. Is the API running?");
  }
}

function showStatus(message) {
  statusElement.textContent = message;
}
