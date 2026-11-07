import * as maplibregl from "/vendor/maplibre-gl.mjs";
import {
  activityDetails,
  activityTitle,
  currentTheme,
  onThemeChange,
  plural,
  requireAccount,
  setUpSync,
  t,
  typeName,
} from "/js/account.js";
import { setUpActivityDetail } from "/js/activity-detail.js";
import { setUpFilters } from "/js/filters.js";
import { language } from "/js/i18n.js";

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
const SOURCE_ID = "activities";
const HEAT_LAYER_ID = "activities-heat";
const DOT_LAYER_ID = "activities-dots";
/** Below this zoom an activity can be too short to draw as a line, so it also gets a faint dot. */
const DOT_MAX_ZOOM = 9;
const REFRESH_DELAY_MS = 150;
/** While zooming, the heat refreshes at most this often, and only once the last refresh has loaded. */
const ZOOM_REFRESH_MS = 400;
/** Fetches beyond the viewport so that small pans do not show empty edges. */
const VIEWPORT_PADDING = 0.25;
const PLACE_SEARCH_URL = "https://photon.komoot.io/api/";
const PLACE_LIMIT = 5;
/** Place search is a shared service, so it only hears from us after typing pauses. */
const SEARCH_DELAY_MS = 350;
const MIN_PLACE_QUERY_LENGTH = 3;
const PLACE_ZOOM = 14;
const FIT_PADDING = 60;
const NOT_LOADED = "Could not load activities. Is the API running?";
/** The controls' texts that MapLibre lets us replace. */
const MAP_TEXTS = {
  "GeolocateControl.FindMyLocation": "Find my location",
  "GeolocateControl.LocationNotAvailable": "Location not available",
  "NavigationControl.ZoomIn": "Zoom in",
  "NavigationControl.ZoomOut": "Zoom out",
  "AttributionControl.ToggleAttribution": "Toggle attribution",
};

const statsElement = document.querySelector("#stats");
const statusElement = document.querySelector("#status");
const legendElement = document.querySelector("#legend");
const searchInput = document.querySelector("#search");
const searchResults = document.querySelector("#search-results");
const panelElement = document.querySelector(".panel");
const browseElement = document.querySelector("#browse");

await requireAccount();

const filters = setUpFilters({ onChange: applyFilters });

let theme = currentTheme();

const map = new maplibregl.Map({
  container: "map",
  style: THEMES[theme].styleUrl,
  center: [0, 25],
  zoom: 1.5,
  maxZoom: 18,
  renderWorldCopies: false,
  hash: true,
  locale: Object.fromEntries(Object.entries(MAP_TEXTS).map(([key, text]) => [key, t(text)])),
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
map.addControl(
  new maplibregl.GeolocateControl({ positionOptions: { enableHighAccuracy: true }, trackUserLocation: true }),
  "top-right",
);

let pendingRequest = null;
let refreshTimer = null;
let lastRefresh = 0;
let pendingStats = null;
let pendingSearch = null;
let searchTimer = null;
let selectedId = null;
let lines = [];
let isFirstStyle = true;
let pendingDetail = null;

/** The legend shows the heat, or the colors of the metric that the selected track has. */
const legend = {
  isColored: false,
  show(gradient, low, high) {
    this.isColored = true;
    document.querySelector("#legend-ramp").style.background = gradient;
    document.querySelector("#legend-low").textContent = low;
    document.querySelector("#legend-high").textContent = high;
    legendElement.hidden = false;
  },
  reset() {
    this.isColored = false;
    document.querySelector("#legend-ramp").style.background = "";
    document.querySelector("#legend-low").textContent = t("Fewer activities");
    document.querySelector("#legend-high").textContent = t("More activities");
    legendElement.hidden = lines.length === 0;
  },
};

const detail = setUpActivityDetail(map, {
  heatLayers: [HEAT_LAYER_ID, DOT_LAYER_ID],
  beforeLayer: layerAboveHeat,
  legend,
  onClose: () => select(null),
});

// Fires again after each theme change, because a new style drops source and layer.
map.on("style.load", () => {
  map.addSource(SOURCE_ID, { type: "geojson", data: toGeoJson(lines), tolerance: 0 });
  const color = ["interpolate", ["linear"], ["get", "weight"], ...THEMES[theme].heatStops];
  const beforeLayer = layerAboveHeat();
  map.addLayer(
    {
      id: DOT_LAYER_ID,
      type: "circle",
      source: SOURCE_ID,
      filter: ["==", ["geometry-type"], "Point"],
      paint: {
        "circle-radius": 2.5,
        "circle-blur": 0.7,
        "circle-color": color,
        "circle-opacity": ["interpolate", ["linear"], ["zoom"], 3, 0.45, DOT_MAX_ZOOM, 0],
      },
    },
    beforeLayer,
  );
  map.addLayer(
    {
      id: HEAT_LAYER_ID,
      type: "line",
      source: SOURCE_ID,
      filter: ["==", ["geometry-type"], "LineString"],
      // Flat caps: a busy route is many short pieces, and round ends would look like beads.
      layout: { "line-cap": "butt", "line-join": "round" },
      paint: {
        "line-width": ["interpolate", ["linear"], ["zoom"], 3, 1, 12, 1.25, 18, 1.75],
        "line-color": color,
      },
    },
    beforeLayer,
  );
  detail.addLayers();
  if (isFirstStyle) {
    isFirstStyle = false;
    loadStats();
    filters.loadTypes();
    refresh();
    setUpSync(() => {
      loadStats();
      filters.loadTypes();
      refresh();
    });
    showLinkedActivity();
  }
});

map.on("zoom", () => {
  if (pendingRequest || performance.now() - lastRefresh < ZOOM_REFRESH_MS) return;
  refresh();
});

map.on("moveend", () => {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, REFRESH_DELAY_MS);
});

onThemeChange((next) => {
  theme = next;
  map.setStyle(THEMES[theme].styleUrl, { diff: false });
});

/** Draws the heat above roads, which would cut through it, but below the labels on top. */
function layerAboveHeat() {
  const layers = map.getStyle().layers;
  return layers[layers.findLastIndex((layer) => layer.type !== "symbol") + 1]?.id;
}

async function refresh() {
  pendingRequest?.abort();
  const request = new AbortController();
  pendingRequest = request;
  lastRefresh = performance.now();

  try {
    const query = new URLSearchParams([...viewportQuery(), ...filterQuery()]);
    const response = await fetch(`/api/heatmap?${query}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ lines } = await response.json());
    map.getSource(SOURCE_ID)?.setData(toGeoJson(lines));
    if (!legend.isColored) legendElement.hidden = lines.length === 0;
  } catch (error) {
    if (error.name !== "AbortError") showStatus(t(NOT_LOADED));
  } finally {
    if (pendingRequest === request) pendingRequest = null;
  }
}

/** Goes to the activity that the activity list linked to, as in `/?activity=1`. */
async function showLinkedActivity() {
  const id = new URLSearchParams(location.search).get("activity");
  if (!id) return;
  history.replaceState(null, "", location.pathname + location.hash);
  if (/^\d+$/.test(id)) select({ id: Number(id) }, { fit: true });
  else showStatus(t("Could not find this activity."));
}

/** Loads all of an activity, and with `fit` then moves the map to its track. */
async function loadDetail(id, fit) {
  pendingDetail?.abort();
  const request = new AbortController();
  pendingDetail = request;
  try {
    const response = await fetch(`/api/activities/${id}`, { signal: request.signal });
    if (response.status === 404) {
      detail.fail(t("Could not find this activity."));
      return;
    }
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const loaded = await response.json();
    detail.show(loaded);
    if (fit && loaded.activity.bounds) {
      const [west, south, east, north] = loaded.activity.bounds;
      map.fitBounds([[west, south], [east, north]], { padding: FIT_PADDING });
    }
  } catch (error) {
    if (error.name !== "AbortError") detail.fail(t(NOT_LOADED));
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

/** The API sorts lines by weight, so that the busiest are drawn last and end up on top. */
function toGeoJson(lines) {
  const showDots = map.getZoom() < DOT_MAX_ZOOM;
  return {
    type: "FeatureCollection",
    features: lines.flatMap(({ weight, path }) => {
      const line = {
        type: "Feature",
        geometry: { type: "LineString", coordinates: path },
        properties: { weight },
      };
      if (!showDots) return [line];
      return [line, { ...line, geometry: { type: "Point", coordinates: path[0] } }];
    }),
  };
}

/** The active filters as API parameters, or only the selected activity if there is one. */
function filterQuery() {
  if (selectedId !== null) return new URLSearchParams({ id: selectedId });
  return filters.query();
}

async function loadStats() {
  pendingStats?.abort();
  const request = new AbortController();
  pendingStats = request;

  const query = filterQuery();
  try {
    const response = await fetch(`/api/stats?${query}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const { activities, distance_m: distance } = await response.json();
    const km = Math.round(distance / 1000).toLocaleString();
    statsElement.textContent = `${plural(activities, "activity", "activities")} · ${km} km`;
    if (activities > 0) {
      showStatus("");
    } else if (query.size > 0) {
      showStatus(t("No activities match these filters."));
    } else {
      statsElement.textContent = "";
      showStatus(t("No activities saved yet. Select Sync to save them from Garmin."));
    }
  } catch (error) {
    if (error.name !== "AbortError") showStatus(t(NOT_LOADED));
  }
}

function applyFilters() {
  select(null);
  searchNow();
}

/** Shows one activity with its details, or all of them again with `null`. */
function select(activity, { fit = false } = {}) {
  selectedId = activity?.id ?? null;
  pendingDetail?.abort();
  panelElement.classList.toggle("has-detail", activity != null);
  browseElement.hidden = activity != null;
  detail.open(activity);
  loadStats();
  refresh();
  if (activity) loadDetail(activity.id, fit);
}

function placeDetails({ properties: { city, state, country, name } }) {
  return [city, state, country].filter((part) => part && part !== name).join(", ");
}

function goToPlace({ geometry, properties: { extent } }) {
  if (extent) {
    // Photon lists the extent as west, north, east, south.
    const [west, north, east, south] = extent;
    map.fitBounds([[west, south], [east, north]], { padding: FIT_PADDING, maxZoom: PLACE_ZOOM });
  } else {
    map.flyTo({ center: geometry.coordinates, zoom: PLACE_ZOOM });
  }
}

function goToActivity(activity) {
  select(activity);
  if (activity.bounds) {
    const [west, south, east, north] = activity.bounds;
    map.fitBounds([[west, south], [east, north]], { padding: FIT_PADDING });
  }
}

function closeSearch() {
  searchResults.hidden = true;
  searchResults.replaceChildren();
}

function resultButton(title, details, onSelect) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "search-result";
  const small = document.createElement("small");
  small.textContent = details;
  button.append(title, small);
  button.addEventListener("click", () => {
    closeSearch();
    onSelect();
  });
  return button;
}

function resultSection(heading, buttons) {
  if (buttons.length === 0) return [];
  const title = document.createElement("div");
  title.className = "search-heading";
  title.textContent = heading;
  return [title, ...buttons];
}

/** A failure in one source should not hide the results of the other. */
async function searchNow() {
  pendingSearch?.abort();
  const text = searchInput.value.trim();
  if (!text) {
    closeSearch();
    return;
  }
  const request = new AbortController();
  pendingSearch = request;
  const { signal } = request;

  const [activities, places] = await Promise.allSettled([
    searchActivities(text, signal),
    searchPlaces(text, signal),
  ]);
  if (signal.aborted) return;

  const sections = [
    ...resultSection(
      t("Places"),
      (places.value ?? []).map((place) =>
        resultButton(place.properties.name ?? text, placeDetails(place), () => goToPlace(place)),
      ),
    ),
    ...resultSection(
      t("Activities"),
      (activities.value ?? []).map((activity) =>
        resultButton(activityTitle(activity), activityDetails(activity), () => goToActivity(activity)),
      ),
    ),
  ];
  if (sections.length === 0) {
    const empty = document.createElement("div");
    empty.className = "search-heading";
    empty.textContent =
      activities.status === "rejected" && places.status === "rejected"
        ? t("Could not search. Is the API running?")
        : t("Nothing found.");
    sections.push(empty);
  }
  searchResults.replaceChildren(...sections);
  searchResults.hidden = false;
}

async function searchActivities(text, signal) {
  const query = filterQuery();
  // A selected activity limits the map only, not what can be found next.
  query.delete("id");
  query.set("q", text);
  const response = await fetch(`/api/activities?${query}`, { signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return (await response.json()).activities;
}

async function searchPlaces(text, signal) {
  if (text.length < MIN_PLACE_QUERY_LENGTH) return [];
  const query = new URLSearchParams({ q: text, limit: PLACE_LIMIT, lang: language });
  const response = await fetch(`${PLACE_SEARCH_URL}?${query}`, { signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return (await response.json()).features;
}

searchInput.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(searchNow, SEARCH_DELAY_MS);
});

searchInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    searchResults.querySelector(".search-result")?.click();
  } else if (event.key === "Escape") {
    clearTimeout(searchTimer);
    pendingSearch?.abort();
    searchInput.value = "";
    closeSearch();
  } else if (event.key === "ArrowDown") {
    event.preventDefault();
    searchResults.querySelector(".search-result")?.focus();
  }
});

searchResults.addEventListener("keydown", (event) => {
  const buttons = [...searchResults.querySelectorAll(".search-result")];
  const index = buttons.indexOf(document.activeElement);
  if (event.key === "ArrowDown") {
    event.preventDefault();
    buttons[index + 1]?.focus();
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    (buttons[index - 1] ?? searchInput).focus();
  } else if (event.key === "Escape") {
    searchInput.focus();
    closeSearch();
  }
});


function showStatus(message) {
  statusElement.textContent = message;
}
