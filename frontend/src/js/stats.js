import { bars, chart, forgetCharts } from "/js/charts.js";
import { requireAccount, setUpSync, t, typeName } from "/js/account.js";

const DAY_MS = 86_400_000;
const DEFAULT_DAYS = "90";
const RANGE_STORAGE_KEY = "statistics-range";
/** Activity types that get their own color in the distance chart. The rest are "Other". */
const MAX_TYPES = 3;
const MAX_TYPE_BARS = 8;

const statusElement = document.querySelector("#statistics-status");
const activitySection = document.querySelector("#activity-section");
const healthSection = document.querySelector("#health-section");
const rangeButtons = [...document.querySelectorAll(".range-picker button")];

const number = (digits = 0) => (value) =>
  value.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
const hours = (value) => {
  const minutes = Math.round(value * 60);
  return `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")} h`;
};
const pace = (minutes) => {
  const seconds = Math.round(minutes * 60);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")} /km`;
};
const unit = (digits, suffix) => (value) => `${number(digits)(value)} ${t(suffix)}`;

/** The health charts, each shown only if Garmin had data for one of its metrics. */
const HEALTH_CHARTS = [
  {
    title: "Sleep",
    subtitle: "Per night, by stage",
    kind: "columns",
    stacked: true,
    format: hours,
    metrics: [
      ["deep_sleep_h", "Deep"],
      ["light_sleep_h", "Light"],
      ["rem_sleep_h", "REM"],
      ["awake_h", "Awake"],
    ],
  },
  { title: "Sleep score", kind: "line", format: number(), metrics: [["sleep_score", "Score"]] },
  {
    title: "Heart rate variability",
    subtitle: "Overnight average, in ms",
    kind: "line",
    format: unit(0, "ms"),
    metrics: [
      ["hrv", "Last night"],
      ["hrv_weekly", "7-day average"],
    ],
  },
  {
    title: "Resting heart rate",
    kind: "line",
    format: unit(0, "bpm"),
    metrics: [["resting_heart_rate", "Resting"]],
  },
  {
    title: "Stress",
    subtitle: "Daily average, from 0 to 100",
    kind: "line",
    format: number(),
    metrics: [["stress", "Stress"]],
  },
  {
    title: "Body Battery",
    kind: "line",
    format: number(),
    metrics: [
      ["body_battery_high", "Highest"],
      ["body_battery_low", "Lowest"],
    ],
  },
  { title: "Steps", subtitle: "Per day", kind: "columns", format: number(), metrics: [["steps", "Steps"]] },
  {
    title: "Intensity minutes",
    subtitle: "Total per period",
    kind: "columns",
    stacked: true,
    sum: true,
    format: unit(0, "min"),
    metrics: [
      ["moderate_intensity_minutes", "Moderate"],
      ["vigorous_intensity_minutes", "Vigorous"],
    ],
  },
  { title: "Floors climbed", subtitle: "Per day", kind: "columns", format: number(), metrics: [["floors", "Floors"]] },
  {
    title: "Calories",
    subtitle: "Per day",
    kind: "columns",
    format: unit(0, "kcal"),
    metrics: [["calories", "Total"]],
  },
  { title: "Weight", kind: "line", format: unit(1, "kg"), metrics: [["weight_kg", "Weight"]] },
  { title: "Body fat", kind: "line", format: unit(1, "%"), metrics: [["body_fat_percent", "Body fat"]] },
  {
    title: "Blood oxygen",
    kind: "line",
    format: unit(0, "%"),
    metrics: [
      ["spo2", "Average"],
      ["lowest_spo2", "Lowest"],
    ],
  },
  {
    title: "Respiration",
    subtitle: "Breaths per minute while awake",
    kind: "line",
    format: unit(1, "brpm"),
    metrics: [["respiration", "Awake"]],
  },
  { title: "Hydration", subtitle: "Per day", kind: "columns", format: unit(1, "l"), metrics: [["hydration_l", "Water"]] },
  {
    title: "Fitness age",
    kind: "line",
    format: unit(1, "years"),
    metrics: [
      ["fitness_age", "Fitness age"],
      ["chronological_age", "Age"],
    ],
  },
];

await requireAccount();

let days = DEFAULT_DAYS;
try {
  days = localStorage.getItem(RANGE_STORAGE_KEY) ?? DEFAULT_DAYS;
} catch {}
let pending = null;

for (const button of rangeButtons) {
  button.addEventListener("click", () => {
    days = button.dataset.days;
    try {
      localStorage.setItem(RANGE_STORAGE_KEY, days);
    } catch {
      // Without storage the choice still applies until the page is reloaded.
    }
    load();
  });
}

setUpSync(load);
load();

function localDay(time) {
  const date = new Date(time);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${String(date.getDate()).padStart(2, "0")}`;
}

function parseDay(text) {
  const [year, month, day] = text.split("-").map(Number);
  return new Date(year, month - 1, day).getTime();
}

async function load() {
  for (const button of rangeButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.days === days));
  }
  pending?.abort();
  const request = new AbortController();
  pending = request;
  const query = new URLSearchParams();
  if (days !== "all") query.set("from", localDay(Date.now() - (Number(days) - 1) * DAY_MS));

  statusElement.textContent = t("Loading…");
  let activities, metrics;
  try {
    const [activityResponse, healthResponse] = await Promise.all([
      fetch(`/api/statistics/activities?${query}`, { signal: request.signal }),
      fetch(`/api/statistics/health?${query}`, { signal: request.signal }),
    ]);
    if (!activityResponse.ok || !healthResponse.ok) throw new Error("HTTP error");
    ({ activities } = await activityResponse.json());
    ({ metrics } = await healthResponse.json());
  } catch (error) {
    if (error.name !== "AbortError") statusElement.textContent = t("Could not load statistics. Is the API running?");
    return;
  }

  const first = query.has("from") ? parseDay(query.get("from")) : earliest(activities, metrics);
  const buckets = makeBuckets(first, parseDay(localDay(Date.now())));
  const hasHealth = Object.keys(metrics).length > 0;
  statusElement.textContent =
    activities.length === 0 && !hasHealth
      ? t("Nothing saved for this period. Select Sync to save your data from Garmin.")
      : "";
  renderActivities(activities, buckets);
  renderHealth(metrics, buckets);
}

function earliest(activities, metrics) {
  const days = [
    ...activities.map(({ day }) => day).filter(Boolean),
    ...Object.values(metrics).map((values) => values[0][0]),
  ];
  return days.length === 0 ? Date.now() : Math.min(...days.map(parseDay));
}

/** Days for a month, weeks for half a year, and months beyond. */
function makeBuckets(first, last) {
  const span = (last - first) / DAY_MS;
  const size = span <= 45 ? "day" : span <= 200 ? "week" : "month";
  const start = new Date(first);
  if (size === "week") start.setDate(start.getDate() - ((start.getDay() + 6) % 7));
  if (size === "month") start.setDate(1);
  const buckets = [];
  for (const date = start; date.getTime() <= last; ) {
    const time = date.getTime();
    if (size === "day") {
      buckets.push({
        time,
        label: date.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" }),
        short: date.toLocaleDateString(undefined, { day: "numeric", month: "short" }),
      });
      date.setDate(date.getDate() + 1);
    } else if (size === "week") {
      const short = date.toLocaleDateString(undefined, { day: "numeric", month: "short" });
      buckets.push({ time, label: t("Week of {day}, {year}", { day: short, year: date.getFullYear() }), short });
      date.setDate(date.getDate() + 7);
    } else {
      buckets.push({
        time,
        label: date.toLocaleDateString(undefined, { month: "long", year: "numeric" }),
        short: date.toLocaleDateString(undefined, { month: "short", year: "2-digit" }),
      });
      date.setMonth(date.getMonth() + 1);
    }
  }
  buckets.size = size;
  return buckets;
}

function bucketIndex(buckets, time) {
  let index = -1;
  for (let i = 0; i < buckets.length && buckets[i].time <= time; i++) index = i;
  return index;
}

/** Sums or averages `[time, value]` pairs into the buckets. */
function intoBuckets(buckets, points, sum) {
  const totals = new Array(buckets.length).fill(0);
  const counts = new Array(buckets.length).fill(0);
  for (const [time, value] of points) {
    const index = bucketIndex(buckets, time);
    if (index < 0) continue;
    totals[index] += value;
    counts[index] += 1;
  }
  return totals.map((total, index) => (counts[index] === 0 ? null : sum ? total : total / counts[index]));
}

function card(container, title, subtitle) {
  const element = document.createElement("article");
  element.className = "card";
  const heading = document.createElement("h3");
  heading.textContent = t(title);
  element.append(heading);
  if (subtitle) {
    const text = document.createElement("p");
    text.className = "muted";
    text.textContent = t(subtitle);
    element.append(text);
  }
  const body = document.createElement("div");
  element.append(body);
  container.append(element);
  return body;
}

function tiles(container, items) {
  container.replaceChildren(
    ...items.map(([label, value]) => {
      const tile = document.createElement("div");
      tile.className = "tile";
      const name = document.createElement("span");
      name.className = "muted";
      name.textContent = t(label);
      const figure = document.createElement("strong");
      figure.textContent = value;
      tile.append(name, figure);
      return tile;
    }),
  );
}

const sum = (items, key) => items.reduce((total, item) => total + (item[key] ?? 0), 0);

function averagePerDay(points) {
  const days = new Map();
  for (const [time, value] of points) {
    const [total, count] = days.get(time) ?? [0, 0];
    days.set(time, [total + value, count + 1]);
  }
  return [...days.entries()].sort((a, b) => a[0] - b[0]).map(([time, [total, count]]) => [time, total / count]);
}

function renderActivities(activities, buckets) {
  const container = document.querySelector("#activity-charts");
  forgetCharts(container);
  container.replaceChildren();
  activitySection.hidden = activities.length === 0;
  if (activities.length === 0) return;

  tiles(document.querySelector("#activity-tiles"), [
    ["Activities", activities.length.toLocaleString()],
    ["Distance", `${number()(sum(activities, "distance_m") / 1000)} km`],
    ["Time", `${number()(sum(activities, "duration_s") / 3600)} h`],
    ["Elevation gain", `${number()(sum(activities, "elevation_gain_m"))} m`],
    ["Calories", `${number()(sum(activities, "calories"))} kcal`],
  ]);

  const dated = activities.filter(({ day }) => day).map((activity) => ({ ...activity, time: parseDay(activity.day) }));
  const byType = new Map();
  for (const activity of dated) {
    const entry = byType.get(activity.activity_type) ?? { distance: 0, duration: 0, count: 0 };
    entry.distance += activity.distance_m ?? 0;
    entry.duration += activity.duration_s ?? 0;
    entry.count += 1;
    byType.set(activity.activity_type, entry);
  }

  // Colors follow the type: the types with the most distance in the period, in a fixed order.
  const topTypes = [...byType.entries()]
    .filter(([, { distance }]) => distance > 0)
    .sort((a, b) => b[1].distance - a[1].distance)
    .slice(0, MAX_TYPES)
    .map(([type]) => type);
  const distanceSeries = topTypes.map((type, index) => ({
    name: typeName(type),
    slot: index + 1,
    values: intoBuckets(
      buckets,
      dated.filter((activity) => activity.activity_type === type).map((activity) => [activity.time, (activity.distance_m ?? 0) / 1000]),
      true,
    ),
  }));
  const others = dated.filter((activity) => !topTypes.includes(activity.activity_type) && activity.distance_m);
  if (others.length > 0) {
    distanceSeries.push({
      name: t("Other"),
      slot: topTypes.length + 1,
      values: intoBuckets(buckets, others.map((activity) => [activity.time, activity.distance_m / 1000]), true),
    });
  }
  if (distanceSeries.length > 0) {
    chart(card(container, "Distance", t("Per {size}, by activity type", { size: t(buckets.size) })), {
      kind: "columns",
      stacked: true,
      buckets,
      series: distanceSeries,
      format: unit(1, "km"),
      label: t("Distance per period by activity type"),
    });
  }

  chart(card(container, "Training time", t("Per {size}", { size: t(buckets.size) })), {
    kind: "columns",
    buckets,
    series: [{
      name: t("Time"),
      slot: 1,
      values: intoBuckets(buckets, dated.map((activity) => [activity.time, (activity.duration_s ?? 0) / 3600]), true),
    }],
    format: hours,
    label: t("Training time per period"),
  });

  const typeBars = [...byType.entries()]
    .sort((a, b) => b[1].duration - a[1].duration)
    .slice(0, MAX_TYPE_BARS)
    .map(([type, { duration, count }]) => ({ label: `${typeName(type)} (${count})`, value: duration / 3600, slot: 1 }));
  bars(card(container, "Time by activity type", "With the number of activities"), typeBars, hours);

  const runs = dated.filter(
    (activity) => activity.activity_type.includes("running") && activity.distance_m > 500 && activity.duration_s,
  );
  if (runs.length > 1) {
    chart(card(container, "Running pace", "Average per run, lower is faster"), {
      kind: "line",
      series: [{
        name: t("Pace"),
        slot: 1,
        values: averagePerDay(runs.map((run) => [run.time, run.duration_s / 60 / (run.distance_m / 1000)])),
      }],
      format: pace,
      label: t("Running pace over time"),
    });
  }

  lineFromActivities(container, dated, "average_heart_rate", "Heart rate in activities", "Average per activity", unit(0, "bpm"));
  lineFromActivities(container, dated, "vo2_max", "VO2 max", "As Garmin estimated it after activities", number(0));

  const loads = dated.filter((activity) => activity.training_load != null);
  if (loads.length > 0) {
    chart(card(container, "Training load", t("Total per {size}", { size: t(buckets.size) })), {
      kind: "columns",
      buckets,
      series: [{ name: t("Load"), slot: 1, values: intoBuckets(buckets, loads.map((activity) => [activity.time, activity.training_load]), true) }],
      format: number(),
      label: t("Training load per period"),
    });
  }
}

function lineFromActivities(container, activities, key, title, subtitle, format) {
  const points = activities.filter((activity) => activity[key] != null).map((activity) => [activity.time, activity[key]]);
  if (points.length < 2) return;
  chart(card(container, title, subtitle), {
    kind: "line",
    series: [{ name: t(title), slot: 1, values: averagePerDay(points) }],
    format,
    label: t(title),
  });
}

function average(points) {
  return points.length === 0 ? null : points.reduce((total, [, value]) => total + value, 0) / points.length;
}

function renderHealth(metrics, buckets) {
  const container = document.querySelector("#health-charts");
  forgetCharts(container);
  container.replaceChildren();
  const series = Object.fromEntries(
    Object.entries(metrics).map(([name, values]) => [name, values.map(([day, value]) => [parseDay(day), value])]),
  );
  healthSection.hidden = Object.keys(series).length === 0;
  if (healthSection.hidden) return;

  const tileItems = [
    ["Resting heart rate", "resting_heart_rate", unit(0, "bpm")],
    ["HRV", "hrv", unit(0, "ms")],
    ["Sleep", "sleep_h", hours],
    ["Sleep score", "sleep_score", number()],
    ["Steps", "steps", number()],
    ["Stress", "stress", number()],
  ]
    .filter(([, key]) => series[key]?.length)
    .map(([label, key, format]) => [t("{label}, average", { label: t(label) }), format(average(series[key]))]);
  tiles(document.querySelector("#health-tiles"), tileItems);

  for (const definition of HEALTH_CHARTS) {
    const present = definition.metrics.filter(([key]) => series[key]?.length);
    if (present.length === 0) continue;
    // Slots follow the metric's place in the definition, so colors stay put when one is missing.
    const chartSeries = definition.metrics
      .map(([key, name], index) => ({ key, name, slot: index + 1 }))
      .filter(({ key }) => series[key]?.length)
      .map(({ key, name, slot }) => ({
        name: t(name),
        slot,
        values: definition.kind === "line" ? series[key] : intoBuckets(buckets, series[key], definition.sum),
      }));
    const size = t(buckets.size);
    const subtitle =
      definition.kind === "columns" && buckets.size !== "day" && !definition.sum
        ? definition.subtitle
          ? t("{subtitle}, averaged per {size}", { subtitle: t(definition.subtitle), size })
          : t("Averaged per {size}", { size })
        : definition.subtitle;
    chart(card(container, definition.title, subtitle), {
      kind: definition.kind,
      stacked: definition.stacked,
      buckets,
      series: chartSeries,
      format: definition.format,
      label: t(definition.title),
    });
  }
  if (container.childElementCount === 0) healthSection.hidden = true;
}

