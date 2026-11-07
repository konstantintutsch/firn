import * as maplibregl from "/vendor/maplibre-gl.mjs";

const STYLE_URL = "https://tiles.openfreemap.org/styles/dark";
const SOURCE_ID = "runs";
const REFRESH_DELAY_MS = 150;
/** Fetches beyond the viewport so that small pans do not show empty edges. */
const VIEWPORT_PADDING = 0.25;

const statsElement = document.querySelector("#stats");
const statusElement = document.querySelector("#status");
const legendElement = document.querySelector("#legend");

const map = new maplibregl.Map({
  container: "map",
  style: STYLE_URL,
  center: [0, 25],
  zoom: 1.5,
  maxZoom: 18,
  renderWorldCopies: false,
  hash: true,
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");

let pendingRequest = null;
let refreshTimer = null;
let hasRuns = false;

map.on("load", () => {
  map.addSource(SOURCE_ID, { type: "geojson", data: toGeoJson([]), tolerance: 0 });
  map.addLayer(
    {
      id: "runs-heat",
      type: "heatmap",
      source: SOURCE_ID,
      paint: {
        "heatmap-weight": ["get", "weight"],
        "heatmap-radius": 10,
        // A lone cell of maximum weight peaks at 0.4 times this, a line at about 0.6.
        "heatmap-intensity": 2,
        "heatmap-color": [
          "interpolate",
          ["linear"],
          ["heatmap-density"],
          0, "rgba(91, 42, 134, 0)",
          0.1, "rgba(91, 42, 134, 0.75)",
          0.35, "#c2366b",
          0.6, "#f4743b",
          0.85, "#fbd25d",
          1, "#fff7d6",
        ],
      },
    },
    firstLabelLayer(),
  );
  loadStats();
  refresh();
});

map.on("moveend", () => {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, REFRESH_DELAY_MS);
});

/** Keeps place names readable by drawing the heat below them. */
function firstLabelLayer() {
  return map.getStyle().layers.find((layer) => layer.type === "symbol")?.id;
}

async function refresh() {
  pendingRequest?.abort();
  const request = new AbortController();
  pendingRequest = request;

  try {
    const response = await fetch(`/api/heatmap?${viewportQuery()}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const { cells } = await response.json();
    map.getSource(SOURCE_ID).setData(toGeoJson(cells));
    legendElement.hidden = cells.length === 0;
    if (hasRuns) showStatus("");
  } catch (error) {
    if (error.name !== "AbortError") showStatus("Could not load runs. Is the API running?");
  }
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

/** Log scale, so a route run once stays visible next to one run a thousand times. */
function toGeoJson(cells) {
  const maxRuns = Math.max(1, ...cells.map((cell) => cell.runs));
  return {
    type: "FeatureCollection",
    features: cells.map((cell) => ({
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
      statusElement.innerHTML = "No runs saved yet. Run <code>just login</code>, then <code>just sync</code>.";
    }
  } catch {
    showStatus("Could not load runs. Is the API running?");
  }
}

function showStatus(message) {
  statusElement.textContent = message;
}
