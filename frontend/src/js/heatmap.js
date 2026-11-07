import * as maplibregl from "/vendor/maplibre-gl.mjs";
import { currentTheme, onThemeChange, plural, requireAccount, setUpSync, typeName } from "/js/account.js";

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
/** Fetches beyond the viewport so that small pans do not show empty edges. */
const VIEWPORT_PADDING = 0.25;
const PLACE_SEARCH_URL = "https://photon.komoot.io/api/";
const PLACE_LIMIT = 5;
/** Place search is a shared service, so it only hears from us after typing pauses. */
const SEARCH_DELAY_MS = 350;
const FILTER_DELAY_MS = 300;
const MIN_PLACE_QUERY_LENGTH = 3;
const PLACE_ZOOM = 14;
const FIT_PADDING = 60;
const NOT_LOADED = "Could not load activities. Is the API running?";

const statsElement = document.querySelector("#stats");
const statusElement = document.querySelector("#status");
const legendElement = document.querySelector("#legend");
const searchInput = document.querySelector("#search");
const searchResults = document.querySelector("#search-results");
const selectionElement = document.querySelector("#selection");
const selectionName = document.querySelector("#selection-name");
const selectionClear = document.querySelector("#selection-clear");
const filtersForm = document.querySelector("#filters");
const filterCount = document.querySelector("#filter-count");
const typesField = document.querySelector("#types-field");
const typesElement = document.querySelector("#types");

await requireAccount();

let theme = currentTheme();

const map = new maplibregl.Map({
  container: "map",
  style: THEMES[theme].styleUrl,
  center: [0, 25],
  zoom: 1.5,
  maxZoom: 18,
  renderWorldCopies: false,
  hash: true,
});
map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");

let pendingRequest = null;
let refreshTimer = null;
let pendingStats = null;
let pendingSearch = null;
let searchTimer = null;
let filterTimer = null;
let selectedId = null;
let lines = [];
let isFirstStyle = true;

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
  if (isFirstStyle) {
    isFirstStyle = false;
    loadStats();
    loadTypes();
    refresh();
    setUpSync(() => {
      loadStats();
      loadTypes();
      refresh();
    });
  }
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

  try {
    const query = new URLSearchParams([...viewportQuery(), ...filterQuery()]);
    const response = await fetch(`/api/heatmap?${query}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ lines } = await response.json());
    map.getSource(SOURCE_ID)?.setData(toGeoJson(lines));
    legendElement.hidden = lines.length === 0;
  } catch (error) {
    if (error.name !== "AbortError") showStatus(NOT_LOADED);
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
  const form = new FormData(filtersForm);
  const query = new URLSearchParams();
  const types = form.getAll("type");
  if (types.length > 0) query.set("types", types.join(","));
  for (const name of ["title", "description", "location", "from", "to"]) {
    const value = form.get(name).trim();
    if (value) query.set(name, value);
  }
  const setScaled = (from, to, scale) => {
    const value = form.get(from);
    if (value !== "") query.set(to, Number(value) * scale);
  };
  setScaled("min_distance_km", "min_distance_m", 1000);
  setScaled("max_distance_km", "max_distance_m", 1000);
  setScaled("min_duration_min", "min_duration_s", 60);
  setScaled("max_duration_min", "max_duration_s", 60);
  return query;
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
    statsElement.textContent = `${plural(activities, "activity")} · ${km} km`;
    if (activities > 0) {
      showStatus("");
    } else if (query.size > 0) {
      showStatus("No activities match these filters.");
    } else {
      statsElement.textContent = "";
      showStatus("No activities saved yet. Select Sync to save them from Garmin.");
    }
  } catch (error) {
    if (error.name !== "AbortError") showStatus(NOT_LOADED);
  }
}

/** Keeps the checked types, because a sync can reload the list at any time. */
async function loadTypes() {
  let types;
  try {
    const response = await fetch("/api/activity-types");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ types } = await response.json());
  } catch {
    return;
  }
  const checked = new FormData(filtersForm).getAll("type");
  typesElement.replaceChildren(
    ...types.map(({ activity_type: type, activities }) => {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "checkbox";
      input.name = "type";
      input.value = type;
      input.checked = checked.includes(type);
      const count = document.createElement("small");
      count.textContent = activities.toLocaleString();
      label.append(input, typeName(type), count);
      return label;
    }),
  );
  typesField.hidden = types.length === 0;
}

function applyFilters() {
  const form = new FormData(filtersForm);
  const count = [...form.values()].filter((value) => value.trim() !== "").length;
  filterCount.textContent = count > 0 ? `(${count})` : "";
  select(null);
  searchNow();
}

function select(activity) {
  selectedId = activity?.id ?? null;
  selectionElement.hidden = activity == null;
  selectionName.textContent = activity ? activityTitle(activity) : "";
  loadStats();
  refresh();
}

function activityTitle({ name, activity_type: type }) {
  return name || typeName(type);
}

function activityDetails({ started_at: started, distance_m: distance, duration_s: duration, location }) {
  const parts = [new Date(started).toLocaleDateString()];
  if (distance != null) parts.push(`${(distance / 1000).toFixed(1)} km`);
  if (duration != null) parts.push(`${Math.round(duration / 60)} min`);
  if (location) parts.push(location);
  return parts.join(" · ");
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
      "Places",
      (places.value ?? []).map((place) =>
        resultButton(place.properties.name ?? text, placeDetails(place), () => goToPlace(place)),
      ),
    ),
    ...resultSection(
      "Activities",
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
        ? "Could not search. Is the API running?"
        : "Nothing found.";
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
  const query = new URLSearchParams({ q: text, limit: PLACE_LIMIT, lang: "en" });
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

selectionClear.addEventListener("click", () => select(null));

filtersForm.addEventListener("input", () => {
  clearTimeout(filterTimer);
  filterTimer = setTimeout(applyFilters, FILTER_DELAY_MS);
});

// Fires before the form clears its fields, so this has to wait for that to finish.
filtersForm.addEventListener("reset", () => setTimeout(applyFilters));

filtersForm.addEventListener("submit", (event) => event.preventDefault());

function showStatus(message) {
  statusElement.textContent = message;
}
