import { errorMessage, requireAccount, send, setUpSync, t } from "/js/account.js";
import { svgElement } from "/js/format.js";
import { language } from "/js/i18n.js";
import { distanceUnit, elevationUnit, toDistance, toElevation } from "/js/units.js";

const PREVIEW_WIDTH = 72;
const PREVIEW_HEIGHT = 54;
const PREVIEW_PADDING = 5;

const listElement = document.querySelector("#route-list");
const statusElement = document.querySelector("#route-status");
const messageElement = document.querySelector("#route-message");
const importInput = document.querySelector("#route-import");

await requireAccount();

const decimals = (value) => value.toLocaleString(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

function describe({ distance_m: distance, elevation_gain_m: gain }) {
  const parts = [`${decimals(toDistance(distance))} ${distanceUnit}`];
  if (gain != null) parts.push(`↑ ${Math.round(toElevation(gain)).toLocaleString(language)} ${elevationUnit}`);
  return parts.join(" · ");
}

/** A small drawing of the route's shape, without a map. North is up. */
function preview(points) {
  const svg = svgElement("svg", {
    class: "route-preview",
    viewBox: `0 0 ${PREVIEW_WIDTH} ${PREVIEW_HEIGHT}`,
    "aria-hidden": "true",
  });
  if (points.length < 2) return svg;
  // Degrees of longitude get shorter towards the poles.
  const squeeze = Math.cos((points[0][0] * Math.PI) / 180);
  const xs = points.map(([, lon]) => lon * squeeze);
  const ys = points.map(([lat]) => -lat);
  const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const scale = Math.min(
    (PREVIEW_WIDTH - 2 * PREVIEW_PADDING) / Math.max(maxX - minX, 1e-9),
    (PREVIEW_HEIGHT - 2 * PREVIEW_PADDING) / Math.max(maxY - minY, 1e-9),
  );
  const offsetX = (PREVIEW_WIDTH - (maxX - minX) * scale) / 2;
  const offsetY = (PREVIEW_HEIGHT - (maxY - minY) * scale) / 2;
  const shape = xs.map((x, index) => `${(offsetX + (x - minX) * scale).toFixed(1)},${(offsetY + (ys[index] - minY) * scale).toFixed(1)}`);
  svgElement("polyline", { points: shape.join(" ") }, svg);
  return svg;
}

/** Each route links to the map, which shows it like an activity. */
function listItem(route) {
  const item = document.createElement("li");
  const entry = document.createElement("a");
  entry.className = "activity route-entry";
  entry.href = `/?${new URLSearchParams({ route: route.id })}`;
  const text = document.createElement("span");
  const title = document.createElement("strong");
  title.textContent = route.name;
  const details = document.createElement("small");
  details.textContent = `${describe(route)} · ${new Date(route.created_at).toLocaleDateString(language)}`;
  text.append(title, details);
  entry.append(preview(route.preview), text);
  item.append(entry);
  return item;
}

async function load() {
  let routes;
  try {
    const response = await fetch("/api/routes");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ routes } = await response.json());
  } catch {
    statusElement.textContent = t("Could not load routes. Is the API running?");
    return;
  }
  listElement.replaceChildren(...routes.map(listItem));
  statusElement.textContent = routes.length === 0 ? t("No routes saved yet.") : "";
}

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
load();
