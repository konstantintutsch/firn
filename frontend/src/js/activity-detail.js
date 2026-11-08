/**
 * The selected activity: its metrics, a profile to scrub along, and its track
 * on the map, in the accent color or colored by a metric if one is chosen.
 */

import { bars } from "/js/charts.js";
import { accentColor, activityDetails, errorMessage, send, t, typeName } from "/js/account.js";
import { RACE_DISTANCES, duration, html, number, pace, raceDistanceName, unit } from "/js/format.js";
import { profile } from "/js/profile.js";
import { DISTANCE_M, distanceUnit, elevationUnit, speedUnit, toDistance, toElevation, toPace, toSpeed } from "/js/units.js";

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



const paceOfSpeed = (speed) => pace(toPace(speed));
const kmh = (speed) => unit(1, speedUnit)(toSpeed(speed));
const km = (meters) => unit(2, distanceUnit)(toDistance(meters));
const elevation = (meters) => unit(0, elevationUnit)(toElevation(meters));

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
    ["elevationGain", "Elevation gain", elevation],
    ["elevationLoss", "Elevation loss", elevation],
    ["minElevation", "Lowest point", elevation],
    ["maxElevation", "Highest point", elevation],
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
    const paces = speed?.map((value) => (value != null && value >= MIN_SPEED_M_S ? toPace(value) : null));
    add("pace", "Pace", paces, pace, true);
  } else {
    add("speed", "Speed", speed?.map((value) => (value == null ? null : toSpeed(value))), unit(1, speedUnit));
  }
  add("elevation", "Elevation", samples.elevation_m?.map((value) => (value == null ? null : toElevation(value))), unit(0, elevationUnit));
  add("heart_rate", "Heart rate", samples.heart_rate, unit(0, "bpm"));
  // Devices count the strides of one foot, while steps per minute count both.
  const cadence = samples.cadence?.map((value) => (value ? value * (foot ? 2 : 1) : null));
  add("cadence", "Cadence", cadence && smooth(cadence), unit(0, foot ? "spm" : "rpm"));
  add("power", "Power", samples.power_w && smooth(samples.power_w), unit(0, "W"));
  add("temperature", "Temperature", samples.temperature_c, unit(0, "°C"));
  return metrics;
}

/** Distance along the samples, from the device or else from the positions. */
function distances(samples) {
  if (samples.distance_m?.some((value) => value != null)) return samples.distance_m;
  if (!samples.position) return null;
  let total = 0;
  let last = null;
  return samples.position.map((position) => {
    if (position && last) total += meters(last, position);
    if (position) last = position;
    return position ? total : null;
  });
}

function meters([lon1, lat1], [lon2, lat2]) {
  const radians = Math.PI / 180;
  const h =
    Math.sin(((lat2 - lat1) * radians) / 2) ** 2 +
    Math.cos(lat1 * radians) * Math.cos(lat2 * radians) * Math.sin(((lon2 - lon1) * radians) / 2) ** 2;
  return 2 * 6_371_000 * Math.asin(Math.sqrt(h));
}

/**
 * One row per kilometer or mile, and one for the rest if it is longer than 100 m: its
 * distance, time, average heart rate and the change in elevation.
 */
function kilometerSplits(samples) {
  const distance = distances(samples);
  const time = samples.time_s;
  if (!distance || !time) return [];
  const points = distance
    .map((meters, index) => ({ meters, seconds: time[index], index }))
    .filter(({ meters, seconds }) => meters != null && seconds != null);
  if (points.length < 2) return [];

  const splits = [];
  let start = points[0];
  let next = DISTANCE_M;
  for (let i = 1; i < points.length; i++) {
    const point = points[i];
    const isLast = i === points.length - 1;
    if (point.meters < next && !(isLast && point.meters - start.meters > 100)) continue;
    const end = point.meters >= next ? { ...point, meters: next } : point;
    // Interpolates the time at the exact kilometer, between this sample and the one before.
    if (point.meters >= next && i > 0) {
      const before = points[i - 1];
      const share = (next - before.meters) / (point.meters - before.meters || 1);
      end.seconds = before.seconds + share * (point.seconds - before.seconds);
    }
    const within = (values) =>
      values?.slice(start.index, point.index + 1).filter((value) => value != null) ?? [];
    const rates = within(samples.heart_rate);
    const elevations = within(samples.elevation_m);
    splits.push({
      meters: end.meters - start.meters,
      seconds: end.seconds - start.seconds,
      heartRate: rates.length ? rates.reduce((sum, value) => sum + value, 0) / rates.length : null,
      climb: elevations.length > 1 ? elevations.at(-1) - elevations[0] : null,
    });
    start = end;
    next += DISTANCE_M;
    // A long gap can pass several kilometers at once.
    while (point.meters >= next && !isLast) next += DISTANCE_M;
  }
  return splits;
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


/**
 * Wires the detail panel to the map. `heatLayers` are hidden while the track is
 * colored by a metric, and `legend` shows that metric's colors.
 */
export function setUpActivityDetail(map, { heatLayers, beforeLayer, legend, onClose, onReload, onNavigate }) {
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
  const downloadElement = document.querySelector("#detail-download");
  setUpMenu(document.querySelector("#detail-download-more"), document.querySelector("#detail-download-menu"));
  const race = setUpRace(statusElement, (id) => onReload(id));
  const lapsElement = document.querySelector("#detail-laps");
  const previousButton = document.querySelector("#detail-previous");
  const nextButton = document.querySelector("#detail-next");
  previousButton.addEventListener("click", () => detail?.previous_id != null && onNavigate(detail.previous_id));
  nextButton.addEventListener("click", () => detail?.next_id != null && onNavigate(detail.next_id));
  const photos = setUpPhotoDialog();

  let detail = null;
  let metrics = [];
  let x = null;
  /** Whether the profile runs along the distance, or else along the time. */
  let byDistance = false;
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

  const routeButton = document.querySelector("#detail-route");
  routeButton.addEventListener("click", async () => {
    const id = detail?.activity.id;
    if (id == null) return;
    routeButton.disabled = true;
    statusElement.textContent = t("Saving the route…");
    try {
      const response = await send("/api/routes", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ activity_id: id }),
      });
      if (response.ok) {
        const route = await response.json();
        location.href = `/?${new URLSearchParams({ route: route.id })}`;
        return;
      }
      statusElement.textContent = await errorMessage(response, t("Could not save the route."));
    } catch {
      statusElement.textContent = t("Could not reach the API. Is it running?");
    }
    routeButton.disabled = false;
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
      paint: { "line-width": ["interpolate", ["linear"], ["zoom"], 8, 2.5, 16, 5], "line-color": accentColor() },
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
    // The track hides the heat, which means nothing for one activity.
    const hasTrack = detail?.samples.position != null;
    const colored = metric != null && hasTrack;
    for (const id of heatLayers) {
      if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", hasTrack ? "none" : "visible");
    }
    map.setLayoutProperty(TRACK_LAYER_ID, "visibility", hasTrack ? "visible" : "none");
    if (!colored) {
      map.setPaintProperty(TRACK_LAYER_ID, "line-color", accentColor());
      if (hasTrack) legend.hide();
      else legend.reset();
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
    chart = profile(profileChart, {
      x,
      values: metric.values,
      xFormat: byDistance ? (meters) => unit(meters < 10 * DISTANCE_M ? 1 : 0, distanceUnit)(toDistance(meters)) : duration,
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

  }

  /** Time per heart rate zone, each in its color, with its range if Garmin has one. */
  function renderZones(summary, floors) {
    // The hardest zone on top, like on the watch.
    const zones = [5, 4, 3, 2, 1].map((zone) => {
      const low = floors?.[zone - 1];
      const high = floors?.[zone];
      const range = low != null && high != null ? ` · ${Math.round(low)}–${Math.round(high) - (zone < 5 ? 1 : 0)}` : "";
      return {
        label: `${t("Zone {zone}", { zone })}${range}`,
        value: summary[`hrTimeInZone_${zone}`] ?? 0,
        color: `var(--zone-${zone})`,
      };
    });
    zonesElement.hidden = zones.every(({ value }) => value === 0);
    if (!zonesElement.hidden) bars(zonesElement.querySelector("div"), zones, duration);
  }

  /** A bar per kilometer or mile, longer for faster, with the pace or speed and heart rate. */
  function renderLaps(type, samples) {
    const splits = kilometerSplits(samples);
    lapsElement.hidden = splits.length < 2;
    if (lapsElement.hidden) return;
    const foot = isOnFoot(type);
    const speeds = splits.map(({ meters, seconds }) => meters / seconds);
    const full = splits.map(({ meters }) => meters >= DISTANCE_M - 1);
    const fastest = Math.max(...speeds.filter((_, index) => full[index]));
    const items = splits.map(({ meters, heartRate }, index) => {
      const speed = speeds[index];
      const parts = [foot ? paceOfSpeed(speed) : kmh(speed)];
      if (heartRate != null) parts.push(`${Math.round(heartRate)} ${t("bpm")}`);
      return {
        label: full[index] ? String(index + 1) : number(2)(toDistance(index * DISTANCE_M + meters)),
        value: speed,
        text: parts.join(" · "),
        color: speed === fastest ? "var(--accent)" : "var(--series-1)",
      };
    });
    // Starts the bars below the slowest split, so that the differences show.
    const slowest = Math.min(...speeds);
    bars(lapsElement.querySelector("div"), items, String, { baseline: slowest * 0.8, valueWidth: 120 });
  }

  /** The photos from Garmin Connect, each opening at full size. */
  function renderImages(images) {
    imagesElement.hidden = images.length === 0;
    imagesElement.replaceChildren(
      ...images.map((url, index) => {
        const link = html("a");
        link.href = url;
        link.addEventListener("click", (event) => {
          event.preventDefault();
          photos.open(images, index);
        });
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
  function renderActions(activity, samples, originalFormat) {
    const file = `/api/activities/${activity.id}/file`;
    const format = (originalFormat ?? "fit").toUpperCase();
    downloadElement.hidden = originalFormat == null;
    document.querySelector("#detail-original").href = file;
    document.querySelector("#detail-original-label").textContent = format;
    document.querySelector("#detail-menu-original").href = file;
    document.querySelector("#detail-menu-original").textContent = format;
    document.querySelector("#detail-gpx").href = `${file}?format=gpx`;
    // A GPX file needs positions, and is the original already for GPX uploads.
    const gpx = samples.position != null && originalFormat !== "gpx";
    document.querySelector("#detail-gpx").hidden = !gpx;
    document.querySelector("#detail-download-more").hidden = !gpx;
    // Imported files have negative Garmin IDs, since Garmin does not know them.
    const fromGarmin = activity.garmin_id > 0;
    document.querySelector("#detail-garmin").href = `https://connect.garmin.com/modern/activity/${activity.garmin_id}`;
    document.querySelector("#detail-garmin").hidden = !fromGarmin;
    document.querySelector("#detail-resync").hidden = !fromGarmin;
    // A route needs positions.
    document.querySelector("#detail-route").hidden = samples.position == null;
    actionsElement.hidden = false;
  }

  /** Shows a loaded activity from `/api/activities/{id}`. */
  function show(loaded) {
    detail = loaded;
    const { activity, summary, samples, images } = detail;
    metrics = trackMetrics(activity.activity_type, samples);
    // Without a track, the time is what the activity is about, like on a treadmill.
    byDistance = samples.position != null && samples.distance_m != null;
    x = byDistance ? samples.distance_m : (samples.time_s ?? null);

    titleElement.textContent = activity.name || typeName(activity.activity_type);
    const event = EVENT_TYPES[summary.eventType?.typeKey];
    metaElement.textContent = [typeName(activity.activity_type), event && t(event), activityDetails(activity)]
      .filter(Boolean)
      .join(" · ");
    descriptionElement.textContent = activity.description ?? "";
    statusElement.textContent = "";
    renderMetrics(summary, activity.activity_type);
    renderImages(images ?? []);
    renderActions(activity, samples, detail.original_format);
    race.show(activity, detail.result);
    renderLaps(activity.activity_type, samples);
    renderZones(summary, detail.heart_rate_zones);
    previousButton.disabled = detail.previous_id == null;
    nextButton.disabled = detail.next_id == null;

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
      new Option(t("Plain"), "heat"),
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
    document.querySelector("#detail-race").hidden = true;
    lapsElement.hidden = true;
    zonesElement.hidden = true;
    previousButton.disabled = true;
    nextButton.disabled = true;
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

/** A dialog that shows the photos of an activity large, one at a time. */
function setUpPhotoDialog() {
  const dialog = document.querySelector("#photo-dialog");
  const image = document.querySelector("#photo-image");
  const previous = document.querySelector("#photo-previous");
  const next = document.querySelector("#photo-next");
  let urls = [];
  let index = 0;

  const show = (at) => {
    index = (at + urls.length) % urls.length;
    image.src = urls[index];
    image.alt = t("Photo {number} of {count}", { number: index + 1, count: urls.length });
    previous.hidden = next.hidden = urls.length < 2;
  };
  previous.addEventListener("click", () => show(index - 1));
  next.addEventListener("click", () => show(index + 1));
  document.querySelector("#photo-close").addEventListener("click", () => dialog.close());
  // A click beside the photo closes it, like on most sites.
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
  dialog.addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft") show(index - 1);
    if (event.key === "ArrowRight") show(index + 1);
  });

  return {
    open(all, at) {
      urls = all;
      show(at);
      dialog.showModal();
    },
  };
}

/** A button that opens a menu below it, which closes on a choice, outside clicks and Escape. */
function setUpMenu(button, menu) {
  const toggle = (open) => {
    menu.hidden = !open;
    button.setAttribute("aria-expanded", String(open));
  };
  button.addEventListener("click", () => toggle(menu.hidden));
  menu.addEventListener("click", () => toggle(false));
  document.addEventListener("pointerdown", (event) => {
    if (!menu.hidden && !button.parentElement.contains(event.target)) toggle(false);
  });
  button.parentElement.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !menu.hidden) {
      event.stopPropagation();
      toggle(false);
      button.focus();
    }
  });
}

/** Reads "1:29:59" or "29:59" as seconds, or `null`. */
function parseClock(text) {
  const parts = text.trim().split(":").map(Number);
  if (parts.length < 2 || parts.length > 3 || parts.some((part) => !Number.isFinite(part) || part < 0)) return null;
  return parts.reduce((total, part) => total * 60 + part, 0) || null;
}

/** The official result of a race: shown, entered and changed by hand. */
function setUpRace(statusElement, reload) {
  const view = document.querySelector("#race-view");
  const summary = document.querySelector("#race-summary");
  const editButton = document.querySelector("#race-edit");
  const editLabel = document.querySelector("#race-edit-label");
  const form = document.querySelector("#race-form");
  const other = document.querySelector("#race-other");
  let activity = null;
  let result = null;

  const editing = (on) => {
    form.hidden = !on;
    editButton.hidden = on;
    view.hidden = on || result == null;
  };

  form.elements.distance.addEventListener("change", () => {
    other.hidden = form.elements.distance.value !== "other";
  });
  editButton.addEventListener("click", () => {
    const named = result && Object.keys(RACE_DISTANCES).find((distance) => Math.abs(distance - result.distance_m) < 1);
    form.elements.distance.value = result ? (named ?? "other") : "5000";
    form.elements.other_km.value = result && !named ? result.distance_m / 1000 : "";
    other.hidden = form.elements.distance.value !== "other";
    form.elements.time.value = result ? duration(result.time_s) : "";
    form.elements.target.value = result?.target_time_s ? duration(result.target_time_s) : "";
    form.elements.event.value = result?.event ?? activity.name ?? "";
    form.elements.results_url.value = result?.results_url ?? "";
    document.querySelector("#race-remove").hidden = result == null;
    editing(true);
    form.elements.time.focus();
  });
  document.querySelector("#race-cancel").addEventListener("click", () => editing(false));

  async function save(method, body) {
    try {
      const response = await send(`/api/activities/${activity.id}/result`, {
        method,
        headers: { "Content-Type": "application/json" },
        body: body && JSON.stringify(body),
      });
      if (!response.ok) {
        statusElement.textContent = await errorMessage(response, t("Could not save the race result."));
        return;
      }
      statusElement.textContent = "";
      reload(activity.id);
    } catch {
      statusElement.textContent = t("Could not reach the API. Is it running?");
    }
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const choice = form.elements.distance.value;
    const meters = choice === "other" ? Number(form.elements.other_km.value) * 1000 : Number(choice);
    const seconds = parseClock(form.elements.time.value);
    const target = form.elements.target.value.trim() ? parseClock(form.elements.target.value) : null;
    if (!(meters > 0) || seconds == null || (target == null && form.elements.target.value.trim())) {
      statusElement.textContent = t("Enter a distance and times like 1:29:59.");
      return;
    }
    save("PUT", {
      distance_m: meters,
      time_s: seconds,
      target_time_s: target,
      event: form.elements.event.value,
      results_url: form.elements.results_url.value,
    });
  });
  document.querySelector("#race-remove").addEventListener("click", () => {
    if (confirm(t("Remove the race result?"))) save("DELETE");
  });

  return {
    show(shown, entered) {
      document.querySelector("#detail-race").hidden = false;
      activity = shown;
      result = entered ?? null;
      editLabel.textContent = t(result ? "Change race result" : "Enter race result");
      summary.replaceChildren();
      if (result) {
        const parts = [duration(result.time_s), raceDistanceName(result.distance_m)];
        if (result.event) parts.push(result.event);
        if (result.target_time_s) parts.push(`${t("Target")} ${duration(result.target_time_s)}`);
        summary.append(parts.join(" · "));
        if (result.results_url) {
          const link = document.createElement("a");
          link.href = result.results_url;
          link.target = "_blank";
          link.rel = "noopener noreferrer";
          link.textContent = t("Official results");
          summary.append(" · ", link);
        }
      }
      editing(false);
    },
  };
}
