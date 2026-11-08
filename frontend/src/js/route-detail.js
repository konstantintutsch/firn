/** The panel and the line on the map for one saved route, shown like an activity. */

import * as maplibregl from "/vendor/maplibre-gl.mjs";
import { accentColor, errorMessage, send, t } from "/js/account.js";
import { language } from "/js/i18n.js";
import { distanceUnit, elevationUnit, toDistance, toElevation } from "/js/units.js";

const SOURCE_ID = "route";
const LAYER_ID = "route-line";

const decimals = (value) => value.toLocaleString(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/**
 * Wires the route panel to the map. `heatLayers` are hidden while a route is
 * shown. `onClose` runs when the route is closed or deleted, and `fitPadding`
 * tells how much of the map the panel covers.
 */
export function setUpRouteDetail(map, { heatLayers, beforeLayer, fitPadding, onClose }) {
  const panel = document.querySelector("#route-detail");
  const titleElement = document.querySelector("#route-title");
  const metaElement = document.querySelector("#route-meta");
  const descriptionElement = document.querySelector("#route-description");
  const downloadLink = document.querySelector("#route-download");
  const activityLink = document.querySelector("#route-activity");
  const statusElement = document.querySelector("#route-status");
  const form = document.querySelector("#route-form");

  let route = null;
  /** `[longitude, latitude]` of the shown route. */
  let line = [];
  let pending = null;

  function geoJson() {
    return { type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: line } };
  }

  function showHeat(visible) {
    for (const id of heatLayers) {
      if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", visible ? "visible" : "none");
    }
  }

  /** Adds the source and layer again, because a new map style drops them. */
  function addLayers() {
    map.addSource(SOURCE_ID, { type: "geojson", data: geoJson() });
    map.addLayer(
      {
        id: LAYER_ID,
        type: "line",
        source: SOURCE_ID,
        layout: { "line-cap": "round", "line-join": "round" },
        paint: {
          "line-width": ["interpolate", ["linear"], ["zoom"], 8, 2.5, 16, 5],
          "line-color": accentColor(),
        },
      },
      beforeLayer(),
    );
    showHeat(route == null);
  }

  function describe({ distance_m: distance, elevation_gain_m: gain, created_at: created }) {
    const parts = [`${decimals(toDistance(distance))} ${distanceUnit}`];
    if (gain != null) parts.push(`↑ ${Math.round(toElevation(gain)).toLocaleString(language)} ${elevationUnit}`);
    parts.push(new Date(created).toLocaleDateString(language));
    return parts.join(" · ");
  }

  function render() {
    titleElement.textContent = route.name;
    metaElement.textContent = describe(route);
    descriptionElement.textContent = route.description ?? "";
    descriptionElement.hidden = !route.description;
    downloadLink.href = `/api/routes/${route.id}/file`;
    activityLink.hidden = route.activity_id == null;
    if (route.activity_id != null) activityLink.href = `/?${new URLSearchParams({ activity: route.activity_id })}`;
  }

  /** Shows the route, or with `null` hides it. With `fit` the map moves to it. */
  async function open(id, { fit = false } = {}) {
    pending?.abort();
    route = null;
    line = [];
    form.hidden = true;
    statusElement.textContent = "";
    panel.hidden = id == null;
    map.getSource(SOURCE_ID)?.setData(geoJson());
    showHeat(id == null);
    if (id == null) return;
    titleElement.textContent = "";
    metaElement.textContent = "";
    descriptionElement.hidden = true;
    activityLink.hidden = true;
    statusElement.textContent = t("Loading…");
    const request = new AbortController();
    pending = request;
    try {
      const response = await fetch(`/api/routes/${id}`, { signal: request.signal });
      if (!response.ok) {
        statusElement.textContent =
          response.status === 404 || response.status === 400
            ? t("Could not find this route.")
            : t("Could not load routes. Is the API running?");
        return;
      }
      route = await response.json();
    } catch (error) {
      if (error.name !== "AbortError") statusElement.textContent = t("Could not load routes. Is the API running?");
      return;
    }
    statusElement.textContent = "";
    line = route.points.map(([lat, lon]) => [lon, lat]);
    render();
    map.getSource(SOURCE_ID)?.setData(geoJson());
    showHeat(false);
    if (fit) {
      const bounds = line.reduce((box, point) => box.extend(point), new maplibregl.LngLatBounds(line[0], line[0]));
      map.fitBounds(bounds, { padding: fitPadding(), maxZoom: 17 });
    }
  }

  document.querySelector("#route-close").addEventListener("click", () => onClose());

  document.querySelector("#route-edit").addEventListener("click", () => {
    if (route == null) return;
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
    if (route == null) return;
    const change = { name: form.elements.name.value, description: form.elements.description.value };
    try {
      const response = await send(`/api/routes/${route.id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(change),
      });
      if (!response.ok) {
        statusElement.textContent = await errorMessage(response, t("Could not save the route."));
        return;
      }
    } catch {
      statusElement.textContent = t("Could not reach the API. Is it running?");
      return;
    }
    route = { ...route, name: change.name.trim(), description: change.description.trim() || null };
    statusElement.textContent = "";
    form.hidden = true;
    render();
  });

  document.querySelector("#route-delete").addEventListener("click", async () => {
    if (route == null) return;
    if (!confirm(t("Delete the route “{name}”? The activity it was made from stays.", { name: route.name }))) return;
    try {
      const response = await send(`/api/routes/${route.id}`, { method: "DELETE" });
      if (!response.ok) {
        statusElement.textContent = await errorMessage(response, t("Could not delete the route."));
        return;
      }
    } catch {
      statusElement.textContent = t("Could not reach the API. Is it running?");
      return;
    }
    onClose();
  });

  return { addLayers, open };
}
