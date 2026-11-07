/**
 * The selected activity: its metrics, a profile to scrub along, and its track
 * on the map, colored by a metric instead of the heatmap if one is chosen.
 */

import { bars } from "/js/charts.js";
import { activityDetails, errorMessage, send, t, typeName } from "/js/account.js";
import { profile } from "/js/profile.js";

const TRACK_SOURCE_ID = "selected-track";
const CURSOR_SOURCE_ID = "selected-cursor";
const TRACK_LAYER_ID = "selected-track-color";
const HIT_LAYER_ID = "selected-track-hit";
const CURSOR_LAYER_ID = "selected-cursor";
const COLOR_STORAGE_KEY = "track-color";
const PROFILE_STORAGE_KEY = "profile-metric";
/** From low to high. Reversed for pace, where low is fast. */
const RAMP = ["#2c7bb6", "#00a6ca", "#90eb9d", "#f9d057", "#f29e2e", "#d7191c"];
/** Samples on each side that are averaged, to calm metrics that jump from second to second. */
const SMOOTHING = 2;
/** Slower than this counts as standing still, which has no pace. */
const MIN_SPEED_M_S = 0.5;

const twoDigits = (value) => String(value).padStart(2, "0");
const number = (digits = 0) => (value) =>
  value.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
const unit = (digits, suffix) => (value) => `${number(digits)(value)} ${t(suffix)}`;

function duration(seconds) {
  const total = Math.round(seconds);
  const [hours, minutes] = [Math.floor(total / 3600), Math.floor((total % 3600) / 60)];
  return hours > 0 ? `${hours}:${twoDigits(minutes)}:${twoDigits(total % 60)}` : `${minutes}:${twoDigits(total % 60)}`;
}

/** Minutes per kilometer, as minutes and seconds. */
function pace(minutes) {
  const seconds = Math.round(minutes * 60);
  return `${Math.floor(seconds / 60)}:${twoDigits(seconds % 60)} /km`;
}

const paceOfSpeed = (speed) => pace(1000 / speed / 60);
const kmh = (speed) => unit(1, "km/h")(speed * 3.6);
const km = (meters) => unit(2, "km")(meters / 1000);

/** Activities on foot show pace and steps instead of speed and revolutions. */
function isOnFoot(type) {
  return /run|walk|hik|treadmill|mountaineering/.test(type);
}

/** Garmin's summary fields, in the order they are shown. Missing and zero values are left out. */
function summaryMetrics(type) {
  const foot = isOnFoot(type);
  return [
    ["distance", "Distance", km],
    ["duration", "Time", duration],
    ["movingDuration", "Moving time", duration],
    ["elapsedDuration", "Elapsed time", duration],
    ["averageSpeed", foot ? "Average pace" : "Average speed", foot ? paceOfSpeed : kmh],
    ["avgGradeAdjustedSpeed", "Grade adjusted pace", paceOfSpeed],
    ["maxSpeed", foot ? "Best pace" : "Max. speed", foot ? paceOfSpeed : kmh],
    ["elevationGain", "Elevation gain", unit(0, "m")],
    ["elevationLoss", "Elevation loss", unit(0, "m")],
    ["minElevation", "Lowest point", unit(0, "m")],
    ["maxElevation", "Highest point", unit(0, "m")],
    ["averageHR", "Average heart rate", unit(0, "bpm")],
    ["maxHR", "Max. heart rate", unit(0, "bpm")],
    ["averageRunningCadenceInStepsPerMinute", "Average cadence", unit(0, "spm")],
    ["maxRunningCadenceInStepsPerMinute", "Max. cadence", unit(0, "spm")],
    ["averageBikingCadenceInRevPerMinute", "Average cadence", unit(0, "rpm")],
    ["maxBikingCadenceInRevPerMinute", "Max. cadence", unit(0, "rpm")],
    ["avgPower", "Average power", unit(0, "W")],
    ["normPower", "Normalized power", unit(0, "W")],
    ["maxPower", "Max. power", unit(0, "W")],
    ["avgStrideLength", "Stride length", (cm) => unit(2, "m")(cm / 100)],
    ["avgVerticalOscillation", "Vertical oscillation", unit(1, "cm")],
    ["avgVerticalRatio", "Vertical ratio", unit(1, "%")],
    ["avgGroundContactTime", "Ground contact time", unit(0, "ms")],
    ["steps", "Steps", number()],
    ["calories", "Calories", unit(0, "kcal")],
    ["aerobicTrainingEffect", "Aerobic training effect", number(1)],
    ["anaerobicTrainingEffect", "Anaerobic training effect", number(1)],
    ["activityTrainingLoad", "Training load", number()],
    ["vO2MaxValue", "VO2 max", number()],
    ["avgRespirationRate", "Average respiration", unit(0, "brpm")],
    ["differenceBodyBattery", "Body Battery", (value) => (value > 0 ? `+${value}` : `${value}`)],
    ["moderateIntensityMinutes", "Moderate intensity minutes", unit(0, "min")],
    ["vigorousIntensityMinutes", "Vigorous intensity minutes", unit(0, "min")],
    ["waterEstimated", "Estimated sweat loss", unit(0, "ml")],
    ["minTemperature", "Lowest temperature", unit(0, "°C")],
    ["maxTemperature", "Highest temperature", unit(0, "°C")],
    ["lapCount", "Laps", number()],
  ];
}

/** Garmin's event types that say something. Others, like "uncategorized", are left out. */
const EVENT_TYPES = {
  race: "Race",
  training: "Training",
  specialEvent: "Special event",
  transportation: "Transportation",
  geocaching: "Geocaching",
  fitness: "Fitness",
  recreation: "Recreation",
  touring: "Touring",
};

const FASTEST_SPLITS = [
  ["fastestSplit_1000", "1 km"],
  ["fastestSplit_1609", "1 mile"],
  ["fastestSplit_5000", "5 km"],
  ["fastestSplit_10000", "10 km"],
  ["fastestSplit_21098", "Half marathon"],
  ["fastestSplit_42195", "Marathon"],
];

function smooth(values) {
  return values.map((value, index) => {
    if (value == null) return null;
    let [total, count] = [0, 0];
    for (let other = index - SMOOTHING; other <= index + SMOOTHING; other++) {
      if (values[other] != null) {
        total += values[other];
        count += 1;
      }
    }
    return total / count;
  });
}

/** The metrics recorded along the activity, each with a value per sample or `null`. */
function trackMetrics(type, samples) {
  const foot = isOnFoot(type);
  const metrics = [];
  const add = (key, label, values, format, invert = false) => {
    if (values?.some((value) => value != null)) metrics.push({ key, label, values, format, invert });
  };
  const speed = samples.speed_m_s && smooth(samples.speed_m_s);
  if (foot) {
    const paces = speed?.map((value) => (value != null && value >= MIN_SPEED_M_S ? 1000 / value / 60 : null));
    add("pace", "Pace", paces, pace, true);
  } else {
    add("speed", "Speed", speed?.map((value) => (value == null ? null : value * 3.6)), unit(1, "km/h"));
  }
  add("elevation", "Elevation", samples.elevation_m, unit(0, "m"));
  add("heart_rate", "Heart rate", samples.heart_rate, unit(0, "bpm"));
  // Devices count the strides of one foot, while steps per minute count both.
  const cadence = samples.cadence?.map((value) => (value ? value * (foot ? 2 : 1) : null));
  add("cadence", "Cadence", cadence && smooth(cadence), unit(0, foot ? "spm" : "rpm"));
  add("power", "Power", samples.power_w && smooth(samples.power_w), unit(0, "W"));
  add("temperature", "Temperature", samples.temperature_c, unit(0, "°C"));
  return metrics;
}

function percentile(sorted, fraction) {
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
}

function readStorage(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Without storage the choice still applies until the page is reloaded.
  }
}

function html(name, className, text) {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/**
 * Wires the detail panel to the map. `heatLayers` are hidden while the track is
 * colored by a metric, and `legend` shows that metric's colors.
 */
export function setUpActivityDetail(map, { heatLayers, beforeLayer, legend, onClose, onReload }) {
  const panel = document.querySelector("#detail");
  const titleElement = document.querySelector("#detail-title");
  const metaElement = document.querySelector("#detail-meta");
  const descriptionElement = document.querySelector("#detail-description");
  const metricsElement = document.querySelector("#detail-metrics");
  const splitsElement = document.querySelector("#detail-splits");
  const zonesElement = document.querySelector("#detail-zones");
  const profileSection = document.querySelector("#detail-profile");
  const profileTabs = document.querySelector("#profile-tabs");
  const profileChart = document.querySelector("#profile-chart");
  const readout = document.querySelector("#profile-readout");
  const colorField = document.querySelector("#track-color-field");
  const colorSelect = document.querySelector("#track-color");
  const statusElement = document.querySelector("#detail-status");
  const imagesElement = document.querySelector("#detail-images");
  const actionsElement = document.querySelector("#detail-actions");

  let detail = null;
  let metrics = [];
  let x = null;
  let chart = null;
  let colorKey = readStorage(COLOR_STORAGE_KEY) ?? "heat";
  let profileKey = readStorage(PROFILE_STORAGE_KEY) ?? "elevation";

  document.querySelector("#detail-close").addEventListener("click", onClose);

  const resyncButton = document.querySelector("#detail-resync");
  resyncButton.addEventListener("click", async () => {
    const id = detail?.activity.id;
    if (id == null) return;
    resyncButton.disabled = true;
    statusElement.textContent = t("Downloading the activity from Garmin again…");
    try {
      const response = await send(`/api/activities/${id}/sync`, { method: "POST" });
      if (response.ok) {
        statusElement.textContent = "";
        onReload(id);
      } else {
        statusElement.textContent = await errorMessage(response, t("Could not download the activity again."));
      }
    } catch {
      statusElement.textContent = t("Could not reach the API. Is it running?");
    } finally {
      resyncButton.disabled = false;
    }
  });

  colorSelect.addEventListener("change", () => {
    colorKey = colorSelect.value;
    writeStorage(COLOR_STORAGE_KEY, colorKey);
    applyColor();
  });

  map.on("mousemove", HIT_LAYER_ID, (event) => {
    const index = event.features[0]?.properties.index;
    if (index == null) return;
    map.getCanvas().style.cursor = "crosshair";
    chart?.setCursor(index);
    scrub(index);
  });
  map.on("mouseleave", HIT_LAYER_ID, () => {
    map.getCanvas().style.cursor = "";
    chart?.setCursor(null);
    scrub(null);
  });

  /** Adds the sources and layers again, because a new map style drops them. */
  function addLayers() {
    map.addSource(TRACK_SOURCE_ID, { type: "geojson", data: segments(), tolerance: 0 });
    map.addSource(CURSOR_SOURCE_ID, { type: "geojson", data: cursorPoint(null) });
    const before = beforeLayer();
    map.addLayer({
      id: TRACK_LAYER_ID,
      type: "line",
      source: TRACK_SOURCE_ID,
      layout: { "line-cap": "round", "line-join": "round", visibility: "none" },
      paint: { "line-width": ["interpolate", ["linear"], ["zoom"], 8, 2.5, 16, 5] },
    }, before);
    // Wide and invisible, so that the track is easy to point at.
    map.addLayer({
      id: HIT_LAYER_ID,
      type: "line",
      source: TRACK_SOURCE_ID,
      paint: { "line-width": 16, "line-opacity": 0 },
    }, before);
    map.addLayer({
      id: CURSOR_LAYER_ID,
      type: "circle",
      source: CURSOR_SOURCE_ID,
      paint: {
        "circle-radius": 6,
        "circle-color": "#ffffff",
        "circle-stroke-color": "#1d2027",
        "circle-stroke-width": 2.5,
      },
    }, before);
    applyColor();
  }

  /** Each piece between two samples with a position, with every metric at its start. */
  function segments() {
    const positions = detail?.samples.position ?? [];
    const features = [];
    for (let index = 0; index + 1 < positions.length; index++) {
      const [from, to] = [positions[index], positions[index + 1]];
      if (!from || !to) continue;
      const properties = { index };
      for (const { key, values } of metrics) {
        if (values[index] != null) properties[key] = values[index];
      }
      features.push({ type: "Feature", geometry: { type: "LineString", coordinates: [from, to] }, properties });
    }
    return { type: "FeatureCollection", features };
  }

  function cursorPoint(index) {
    const position = index == null ? null : detail?.samples.position?.[index];
    return {
      type: "FeatureCollection",
      features: position ? [{ type: "Feature", geometry: { type: "Point", coordinates: position }, properties: {} }] : [],
    };
  }

  function applyColor() {
    if (!map.getLayer(TRACK_LAYER_ID)) return;
    const metric = detail ? metrics.find(({ key }) => key === colorKey) : null;
    const colored = metric != null && detail.samples.position != null;
    for (const id of heatLayers) {
      if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", colored ? "none" : "visible");
    }
    map.setLayoutProperty(TRACK_LAYER_ID, "visibility", colored ? "visible" : "none");
    if (!colored) {
      legend.reset();
      return;
    }
    // The middle 90% spreads over the colors, so that a few outliers do not flatten them.
    const sorted = metric.values.filter((value) => value != null).sort((a, b) => a - b);
    const low = percentile(sorted, 0.05);
    const high = Math.max(percentile(sorted, 0.95), low + 1e-6);
    const ramp = metric.invert ? [...RAMP].reverse() : RAMP;
    const stops = ramp.flatMap((color, index) => [low + ((high - low) * index) / (ramp.length - 1), color]);
    map.setPaintProperty(TRACK_LAYER_ID, "line-color", [
      "case",
      ["has", metric.key],
      ["interpolate", ["linear"], ["get", metric.key], ...stops],
      "#9a9a9a",
    ]);
    const [left, right] = metric.invert ? [high, low] : [low, high];
    legend.show(`linear-gradient(90deg, ${RAMP.join(", ")})`, metric.format(left), metric.format(right));
  }

  function scrub(index) {
    map.getSource(CURSOR_SOURCE_ID)?.setData(cursorPoint(index));
    if (index == null) {
      readout.replaceChildren(html("span", "muted", t("Move along the profile or the track to see the values there.")));
      return;
    }
    const { samples } = detail;
    const items = [];
    if (samples.distance_m?.[index] != null) items.push([t("Distance"), km(samples.distance_m[index])]);
    if (samples.time_s?.[index] != null) items.push([t("Time"), duration(samples.time_s[index])]);
    for (const { label, values, format } of metrics) {
      if (values[index] != null) items.push([t(label), format(values[index])]);
    }
    readout.replaceChildren(
      ...items.map(([label, value]) => {
        const item = html("span", "readout-item");
        item.append(html("small", "muted", label), html("strong", null, value));
        return item;
      }),
    );
  }

  function renderProfile() {
    chart?.destroy();
    chart = null;
    profileChart.replaceChildren();
    const metric = metrics.find(({ key }) => key === profileKey) ?? metrics[0];
    profileSection.hidden = metric == null || x == null;
    if (profileSection.hidden) return;
    for (const button of profileTabs.children) {
      button.setAttribute("aria-pressed", String(button.dataset.key === metric.key));
    }
    const byDistance = detail.samples.distance_m != null;
    chart = profile(profileChart, {
      x,
      values: metric.values,
      xFormat: byDistance ? (meters) => unit(meters < 10_000 ? 1 : 0, "km")(meters / 1000) : duration,
      format: metric.format,
      invert: metric.invert,
      label: t(metric.label),
      onScrub: scrub,
    });
    scrub(null);
  }

  function renderMetrics(summary, type) {
    const rows = summaryMetrics(type)
      .filter(([key]) => summary[key] != null && summary[key] !== 0)
      .map(([key, label, format]) => [t(label), format(summary[key])]);
    metricsElement.replaceChildren(
      ...rows.map(([label, value]) => {
        const row = html("div");
        row.append(html("dt", null, label), html("dd", null, value));
        return row;
      }),
    );

    const splits = FASTEST_SPLITS.filter(([key]) => summary[key] > 0);
    splitsElement.hidden = splits.length === 0;
    splitsElement.querySelector("dl").replaceChildren(
      ...splits.map(([key, label]) => {
        const row = html("div");
        row.append(html("dt", null, t(label)), html("dd", null, duration(summary[key])));
        return row;
      }),
    );

    const zones = [1, 2, 3, 4, 5].map((zone) => ({
      label: t("Zone {zone}", { zone }),
      value: summary[`hrTimeInZone_${zone}`] ?? 0,
      slot: 2,
    }));
    zonesElement.hidden = zones.every(({ value }) => value === 0);
    if (!zonesElement.hidden) bars(zonesElement.querySelector("div"), zones, duration);
  }

  /** The photos from Garmin Connect, each opening at full size. */
  function renderImages(images) {
    imagesElement.hidden = images.length === 0;
    imagesElement.replaceChildren(
      ...images.map((url) => {
        const link = html("a");
        link.href = url;
        link.target = "_blank";
        link.rel = "noreferrer";
        const image = html("img");
        image.src = url;
        image.alt = t("Photo of the activity");
        image.loading = "lazy";
        link.append(image);
        return link;
      }),
    );
  }

  /** Links to download the files and to open the activity on Garmin Connect. */
  function renderActions(activity, samples) {
    const file = `/api/activities/${activity.id}/file`;
    const hasFile = Object.keys(samples).length > 0;
    document.querySelector("#detail-original").href = file;
    document.querySelector("#detail-gpx").href = `${file}?format=gpx`;
    document.querySelector("#detail-original").hidden = !hasFile;
    document.querySelector("#detail-gpx").hidden = samples.position == null;
    document.querySelector("#detail-garmin").href = `https://connect.garmin.com/modern/activity/${activity.id}`;
    actionsElement.hidden = false;
  }

  /** Shows a loaded activity from `/api/activities/{id}`. */
  function show(loaded) {
    detail = loaded;
    const { activity, summary, samples, images } = detail;
    metrics = trackMetrics(activity.activity_type, samples);
    x = samples.distance_m ?? samples.time_s ?? null;

    titleElement.textContent = activity.name || typeName(activity.activity_type);
    const event = EVENT_TYPES[summary.eventType?.typeKey];
    metaElement.textContent = [typeName(activity.activity_type), event && t(event), activityDetails(activity)]
      .filter(Boolean)
      .join(" · ");
    descriptionElement.textContent = activity.description ?? "";
    statusElement.textContent = "";
    renderMetrics(summary, activity.activity_type);
    renderImages(images ?? []);
    renderActions(activity, samples);

    profileTabs.replaceChildren(
      ...metrics.map(({ key, label }) => {
        const button = html("button", "panel-button", t(label));
        button.type = "button";
        button.dataset.key = key;
        button.addEventListener("click", () => {
          profileKey = key;
          writeStorage(PROFILE_STORAGE_KEY, key);
          renderProfile();
        });
        return button;
      }),
    );
    renderProfile();

    colorField.hidden = metrics.length === 0 || samples.position == null;
    colorSelect.replaceChildren(
      new Option(t("Heatmap"), "heat"),
      ...metrics.map(({ key, label }) => new Option(t(label), key)),
    );
    if (!metrics.some(({ key }) => key === colorKey)) colorKey = "heat";
    colorSelect.value = colorKey;

    map.getSource(TRACK_SOURCE_ID)?.setData(segments());
    applyColor();
  }

  /** Shows the panel for an activity that is still loading, or hides it with `null`. */
  function open(activity) {
    detail = null;
    metrics = [];
    chart?.destroy();
    chart = null;
    panel.hidden = activity == null;
    map.getSource(TRACK_SOURCE_ID)?.setData(segments());
    map.getSource(CURSOR_SOURCE_ID)?.setData(cursorPoint(null));
    applyColor();
    if (activity == null) return;
    titleElement.textContent = activity.name || (activity.activity_type ? typeName(activity.activity_type) : "");
    metaElement.textContent = "";
    descriptionElement.textContent = "";
    actionsElement.hidden = true;
    renderImages([]);
    metricsElement.replaceChildren();
    splitsElement.hidden = true;
    zonesElement.hidden = true;
    profileSection.hidden = true;
    colorField.hidden = true;
    statusElement.textContent = t("Loading…");
  }

  function fail(message) {
    statusElement.textContent = message;
  }

  return { addLayers, open, show, fail };
}
