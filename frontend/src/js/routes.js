import * as maplibregl from "/vendor/maplibre-gl.mjs";
import {
  currentTheme,
  errorMessage,
  onThemeChange,
  requireAccount,
  send,
  setUpSync,
  t,
} from "/js/account.js";
import { language } from "/js/i18n.js";
import { distanceUnit, elevationUnit, toDistance, toElevation } from "/js/units.js";

const STYLES = {
  light: "https://tiles.openfreemap.org/styles/positron",
  dark: "https://tiles.openfreemap.org/styles/dark",
};
const SOURCE_ID = "route";
const LINE_LAYER_ID = "route-line";
const FIT_PADDING = 40;

const listElement = document.querySelector("#route-list");
const statusElement = document.querySelector("#route-status");
const messageElement = document.querySelector("#route-message");
const importInput = document.querySelector("#route-import");
const detailElement = document.querySelector("#route-detail");
const titleElement = document.querySelector("#route-title");
const metaElement = document.querySelector("#route-meta");
const descriptionElement = document.querySelector("#route-description");
const downloadLink = document.querySelector("#route-download");
const activityLink = document.querySelector("#route-activity");
const editButton = document.querySelector("#route-edit");
const deleteButton = document.querySelector("#route-delete");
const form = document.querySelector("#route-form");

await requireAccount();

let routes = [];
let selected = null;
/** The shown route's points as `[longitude, latitude]`, for the map. */
let line = [];
let map = null;
let pending = null;

const decimals = (value) => value.toLocaleString(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

function describe({ distance_m: distance, elevation_gain_m: gain }) {
  const parts = [`${decimals(toDistance(distance))} ${distanceUnit}`];
  if (gain != null) parts.push(`↑ ${Math.round(toElevation(gain)).toLocaleString(language)} ${elevationUnit}`);
  return parts.join(" · ");
}

function toGeoJson() {
  const geometry = { type: "LineString", coordinates: line };
  return { type: "Feature", properties: {}, geometry };
}

function addLine() {
  map.addSource(SOURCE_ID, { type: "geojson", data: toGeoJson() });
  map.addLayer({
    id: LINE_LAYER_ID,
    type: "line",
    source: SOURCE_ID,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-width": 4, "line-color": currentTheme() === "dark" ? "#fbd25d" : "#d6246e" },
  });
}

function fit() {
  const bounds = line.reduce((box, point) => box.extend(point), new maplibregl.LngLatBounds(line[0], line[0]));
  map.fitBounds(bounds, { padding: FIT_PADDING, animate: false, maxZoom: 17 });
}

function showOnMap() {
  if (map == null) {
    map = new maplibregl.Map({
      container: "route-map",
      style: STYLES[currentTheme()],
      attributionControl: { compact: true },
      renderWorldCopies: false,
    });
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
    // Fires again after each theme change, because a new style drops source and layer.
    map.on("style.load", addLine);
    onThemeChange((theme) => map.setStyle(STYLES[theme]));
  } else {
    map.getSource(SOURCE_ID)?.setData(toGeoJson());
  }
  map.resize();
  fit();
}

function listItem(route) {
  const item = document.createElement("li");
  const entry = document.createElement("a");
  entry.className = "activity";
  entry.href = `/routes/?${new URLSearchParams({ route: route.id })}`;
  entry.addEventListener("click", (event) => {
    event.preventDefault();
    select(route.id, { scroll: true });
  });
  if (route.id === selected) entry.setAttribute("aria-current", "true");
  const title = document.createElement("strong");
  title.textContent = route.name;
  const details = document.createElement("small");
  details.textContent = `${describe(route)} · ${new Date(route.created_at).toLocaleDateString(language)}`;
  entry.append(title, details);
  item.append(entry);
  return item;
}

function renderList() {
  listElement.replaceChildren(...routes.map(listItem));
  statusElement.textContent = routes.length === 0 ? t("No routes saved yet.") : "";
}

async function load() {
  try {
    const response = await fetch("/api/routes");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ routes } = await response.json());
  } catch {
    statusElement.textContent = t("Could not load routes. Is the API running?");
    return;
  }
  renderList();
}

/** Shows one route with its map. `null` closes the details. */
async function select(id, { scroll = false } = {}) {
  pending?.abort();
  selected = id;
  form.hidden = true;
  const url = new URL(location);
  if (id == null) url.searchParams.delete("route");
  else url.searchParams.set("route", id);
  history.replaceState(null, "", url);
  renderList();
  if (id == null) {
    detailElement.hidden = true;
    return;
  }
  const request = new AbortController();
  pending = request;
  let route;
  try {
    const response = await fetch(`/api/routes/${id}`, { signal: request.signal });
    if (response.status === 404) return select(null);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    route = await response.json();
  } catch (error) {
    if (error.name !== "AbortError") statusElement.textContent = t("Could not load routes. Is the API running?");
    return;
  }
  titleElement.textContent = route.name;
  metaElement.textContent = describe(route);
  descriptionElement.textContent = route.description ?? "";
  descriptionElement.hidden = !route.description;
  downloadLink.href = `/api/routes/${id}/file`;
  activityLink.hidden = route.activity_id == null;
  activityLink.href = `/?${new URLSearchParams({ activity: route.activity_id })}`;
  line = route.points.map(([lat, lon]) => [lon, lat]);
  detailElement.hidden = false;
  showOnMap();
  if (scroll) window.scrollTo({ top: 0, behavior: "smooth" });
}

editButton.addEventListener("click", () => {
  const route = routes.find(({ id }) => id === selected);
  form.elements.name.value = route.name;
  form.elements.description.value = route.description ?? "";
  form.hidden = false;
  form.elements.name.focus();
});

document.querySelector("#route-cancel").addEventListener("click", () => {
  form.hidden = true;
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const body = JSON.stringify({ name: form.elements.name.value, description: form.elements.description.value });
  try {
    const response = await send(`/api/routes/${selected}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body,
    });
    if (!response.ok) {
      messageElement.textContent = await errorMessage(response, t("Could not save the route."));
      return;
    }
  } catch {
    messageElement.textContent = t("Could not reach the API. Is it running?");
    return;
  }
  messageElement.textContent = "";
  const id = selected;
  await load();
  select(id);
});

deleteButton.addEventListener("click", async () => {
  const route = routes.find(({ id }) => id === selected);
  if (!confirm(t("Delete the route “{name}”? The activity it was made from stays.", { name: route.name }))) return;
  try {
    const response = await send(`/api/routes/${selected}`, { method: "DELETE" });
    if (!response.ok) {
      messageElement.textContent = await errorMessage(response, t("Could not delete the route."));
      return;
    }
  } catch {
    messageElement.textContent = t("Could not reach the API. Is it running?");
    return;
  }
  messageElement.textContent = "";
  await select(null);
  await load();
});

importInput.addEventListener("change", async () => {
  const [file] = importInput.files;
  if (!file) return;
  importInput.disabled = true;
  messageElement.textContent = t("Importing…");
  const name = file.name.replace(/\.gpx$/i, "");
  try {
    const response = await send(`/api/routes/import?${new URLSearchParams({ name })}`, {
      method: "POST",
      headers: { "Content-Type": "application/gpx+xml" },
      body: file,
    });
    if (response.ok) {
      const route = await response.json();
      messageElement.textContent = t("Imported the route “{name}”.", { name: route.name });
      await load();
      select(route.id);
    } else {
      messageElement.textContent = await errorMessage(response, t("Could not import the file."));
    }
  } catch {
    messageElement.textContent = t("Could not reach the API. Is it running?");
  } finally {
    importInput.disabled = false;
    importInput.value = "";
  }
});

setUpSync(() => {});
await load();
const linked = Number(new URLSearchParams(location.search).get("route"));
if (linked) select(linked);
