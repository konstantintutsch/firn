import { bars, chart, forgetCharts } from "/js/charts.js";
import { duration, number, pace, raceDistanceName, unit } from "/js/format.js";
import { language } from "/js/i18n.js";
import { requireAccount, setUpSync, t, typeName } from "/js/account.js";
import { localDay, setUpFilters } from "/js/filters.js";
import { distanceUnit, elevationUnit, toDistance, toElevation } from "/js/units.js";

const DAY_MS = 86_400_000;
/** Activity types that get their own color in the distance chart. The rest are "Other". */
const MAX_TYPES = 3;
const MAX_TYPE_BARS = 8;

const statusElement = document.querySelector("#statistics-status");
const activitySection = document.querySelector("#activity-section");
const healthSection = document.querySelector("#health-section");
const healthNote = document.querySelector("#health-note");
const rangeButtons = [...document.querySelectorAll(".range-picker button")];

const hours = (value) => {
  const minutes = Math.round(value * 60);
  return `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")} h`;
};

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
    metrics: [
      ["resting_heart_rate", "Resting"],
      ["resting_heart_rate_7d", "7-day average"],
    ],
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
  {
    title: "Floors",
    subtitle: "Per day",
    kind: "columns",
    format: number(),
    metrics: [
      ["floors", "Climbed"],
      ["floors_descended", "Descended"],
    ],
  },
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

const filters = setUpFilters({ withoutGps: true, onChange: load });
let pending = null;

for (const button of rangeButtons) {
  button.addEventListener("click", () => filters.setDays(button.dataset.days === "all" ? null : button.dataset.days));
}

setUpSync(() => {
  filters.loadTypes();
  load();
});
filters.loadTypes();
load();

function parseDay(text) {
  const [year, month, day] = text.split("-").map(Number);
  return new Date(year, month - 1, day).getTime();
}

async function load() {
  const query = filters.query();
  const from = query.get("from");
  const to = query.get("to");
  const period = filters.days ?? (from || to ? null : "all");
  for (const button of rangeButtons) {
    button.setAttribute("aria-pressed", String(button.dataset.days === period));
  }
  pending?.abort();
  const request = new AbortController();
  pending = request;
  if (from && to && from > to) {
    statusElement.textContent = t("The start date is after the end date.");
    activitySection.hidden = true;
    healthSection.hidden = true;
    return;
  }
  // Health values belong to days, not to activities, so only the dates apply to them.
  const healthQuery = new URLSearchParams([...query].filter(([name]) => name === "from" || name === "to"));
  const filtersActivities = query.size > healthQuery.size;
  healthNote.hidden = !filtersActivities;

  statusElement.textContent = t("Loading…");
  let activities, metrics;
  try {
    const [activityResponse, healthResponse] = await Promise.all([
      fetch(`/api/statistics/activities?${query}`, { signal: request.signal }),
      fetch(`/api/statistics/health?${healthQuery}`, { signal: request.signal }),
    ]);
    if (!activityResponse.ok || !healthResponse.ok) throw new Error("HTTP error");
    ({ activities } = await activityResponse.json());
    ({ metrics } = await healthResponse.json());
  } catch (error) {
    if (error.name !== "AbortError") statusElement.textContent = t("Could not load statistics. Is the API running?");
    return;
  }

  const first = from ? parseDay(from) : earliest(activities, metrics);
  const buckets = makeBuckets(first, parseDay(to ?? localDay(Date.now())));
  const hasHealth = Object.keys(metrics).length > 0;
  statusElement.textContent =
    activities.length === 0 && filtersActivities
      ? t("No activities match these filters.")
      : activities.length === 0 && !hasHealth
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
  // A loop, not Math.min(...days): spreading a long history can overflow the stack.
  let first = Infinity;
  for (const day of days) first = Math.min(first, parseDay(day));
  return days.length === 0 ? Date.now() : first;
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
        label: date.toLocaleDateString(language, { weekday: "short", day: "numeric", month: "short" }),
        short: date.toLocaleDateString(language, { day: "numeric", month: "short" }),
      });
      date.setDate(date.getDate() + 1);
    } else if (size === "week") {
      const short = date.toLocaleDateString(language, { day: "numeric", month: "short" });
      buckets.push({ time, label: t("Week of {day}, {year}", { day: short, year: date.getFullYear() }), short });
      date.setDate(date.getDate() + 7);
    } else {
      buckets.push({
        time,
        label: date.toLocaleDateString(language, { month: "long", year: "numeric" }),
        short: date.toLocaleDateString(language, { month: "short", year: "2-digit" }),
      });
      date.setMonth(date.getMonth() + 1);
    }
  }
  buckets.size = size;
  return buckets;
}

/** The last bucket that starts at or before `time`, or -1. A binary search, since it runs once per value. */
function bucketIndex(buckets, time) {
  let low = 0;
  let high = buckets.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (buckets[middle].time <= time) low = middle + 1;
    else high = middle;
  }
  return low - 1;
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

  const dated = activities.filter(({ day }) => day).map((activity) => ({ ...activity, time: parseDay(activity.day) }));
  tiles(document.querySelector("#activity-tiles"), [
    ["Activities", activities.length.toLocaleString(language)],
    ["Distance", `${number()(toDistance(sum(activities, "distance_m")))} ${distanceUnit}`],
    ["Time", `${number()(sum(activities, "duration_s") / 3600)} h`],
    ["Elevation gain", `${number()(toElevation(sum(activities, "elevation_gain_m")))} ${elevationUnit}`],
    ["Calories", `${number()(sum(activities, "calories"))} kcal`],
  ]);

  const calendarCard = card(container, "Calendar", "Distance per day, darker is more");
  calendarCard.parentElement.classList.add("wide");
  calendar(calendarCard, dated, buckets);
  records(container, dated);
  yearOverYear(container, dated);
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
      dated.filter((activity) => activity.activity_type === type).map((activity) => [activity.time, toDistance(activity.distance_m ?? 0)]),
      true,
    ),
  }));
  const others = dated.filter((activity) => !topTypes.includes(activity.activity_type) && activity.distance_m);
  if (others.length > 0) {
    distanceSeries.push({
      name: t("Other"),
      slot: topTypes.length + 1,
      values: intoBuckets(buckets, others.map((activity) => [activity.time, toDistance(activity.distance_m)]), true),
    });
  }
  if (distanceSeries.length > 0) {
    chart(card(container, "Distance", t("Per {size}, by activity type", { size: t(buckets.size) })), {
      kind: "columns",
      stacked: true,
      buckets,
      series: distanceSeries,
      format: unit(1, distanceUnit),
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
        values: averagePerDay(runs.map((run) => [run.time, run.duration_s / 60 / toDistance(run.distance_m)])),
      }],
      format: pace,
      label: t("Running pace over time"),
    });
  }

  lineFromActivities(container, dated, "average_heart_rate", "Heart rate in activities", "Average per activity", unit(0, "bpm"));

  const isCycling = ({ activity_type }) => /cycling|biking/.test(activity_type);
  const vo2Series = [
    ["Running", 1, (activity) => !isCycling(activity)],
    ["Cycling", 2, isCycling],
  ]
    .map(([name, slot, matches]) => ({
      name: t(name),
      slot,
      points: dated.filter((activity) => activity.vo2_max != null && matches(activity)).map((activity) => [activity.time, activity.vo2_max]),
    }))
    .filter(({ points }) => points.length > 1);
  if (vo2Series.length > 0) {
    chart(card(container, "VO2 max", "As Garmin estimated it for each activity"), {
      kind: "line",
      series: vo2Series.map(({ name, slot, points }) => ({ name, slot, values: averagePerDay(points) })),
      format: number(1),
      label: t("VO2 max"),
    });
  }

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


/** A grid of the last weeks of the period, one square per day, darker for more distance. */
function calendar(container, dated, buckets) {
  const SVG = "http://www.w3.org/2000/svg";
  const SIZE = 11;
  const GAP = 3;
  const WEEKS = 53;
  const perDay = new Map();
  for (const activity of dated) {
    const entry = perDay.get(activity.time) ?? { meters: 0, activities: [] };
    entry.meters += activity.distance_m ?? 0;
    entry.activities.push(activity);
    perDay.set(activity.time, entry);
  }
  const last = Math.min(parseDay(localDay(Date.now())), buckets.at(-1).time + 31 * DAY_MS);
  const end = new Date(last);
  // Weeks start on Monday, and the last column is the week of `last`.
  const start = new Date(end);
  start.setDate(start.getDate() - ((start.getDay() + 6) % 7) - (WEEKS - 1) * 7);
  const max = Math.max(1, ...[...perDay.values()].map(({ meters }) => meters));

  const wrapper = document.createElement("div");
  wrapper.className = "calendar-wrapper";
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", `0 0 ${WEEKS * (SIZE + GAP)} ${7 * (SIZE + GAP)}`);
  svg.setAttribute("class", "calendar");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", t("Distance per day over the last year. Use the arrow keys to go through the days."));
  // One tab stop for the whole calendar: the arrow keys move from day to day.
  svg.setAttribute("tabindex", "0");
  const tooltip = document.createElement("div");
  tooltip.className = "chart-tooltip calendar-tooltip";
  tooltip.hidden = true;
  tooltip.setAttribute("aria-live", "polite");
  tooltip.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      svg.focus();
      show(null);
    }
  });

  const days = [];
  for (let week = 0, date = new Date(start); week < WEEKS; week++) {
    for (let weekday = 0; weekday < 7; weekday++, date.setDate(date.getDate() + 1)) {
      if (date.getTime() > last) break;
      const entry = perDay.get(date.getTime());
      const square = document.createElementNS(SVG, "rect");
      square.setAttribute("x", week * (SIZE + GAP));
      square.setAttribute("y", weekday * (SIZE + GAP));
      square.setAttribute("width", SIZE);
      square.setAttribute("height", SIZE);
      square.setAttribute("rx", 2);
      const level = entry ? Math.min(4, 1 + Math.floor((Math.sqrt(entry.meters / max) || 0) * 3.99)) : 0;
      square.setAttribute("class", `calendar-day level-${level}`);
      square.dataset.index = days.length;
      days.push({ time: date.getTime(), entry, square });
      svg.append(square);
    }
  }

  // Made once, since a new one on every day would slow the pop-up down.
  const dateFormat = new Intl.DateTimeFormat(language, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
  let active = null;
  function show(index) {
    active?.square.classList.remove("active");
    active = index == null ? null : days[index];
    tooltip.hidden = active == null;
    if (!active) return;
    active.square.classList.add("active");
    const { time, entry } = active;
    const heading = document.createElement("strong");
    heading.textContent = dateFormat.format(time);
    tooltip.replaceChildren(heading);
    for (const activity of entry?.activities ?? []) {
      const row = document.createElement("a");
      row.className = "chart-tooltip-row";
      row.href = `/?${new URLSearchParams({ activity: activity.id })}`;
      const name = document.createElement("span");
      name.textContent = activity.name || typeName(activity.activity_type);
      const value = document.createElement("span");
      value.className = "chart-tooltip-value";
      const parts = [];
      if (activity.distance_m) parts.push(`${number(1)(toDistance(activity.distance_m))} ${distanceUnit}`);
      if (activity.duration_s) parts.push(duration(activity.duration_s));
      value.textContent = parts.join(" · ");
      row.append(name, value);
      tooltip.append(row);
    }
    if (!entry) {
      const none = document.createElement("div");
      none.className = "muted";
      none.textContent = t("No activities");
      tooltip.append(none);
    }
    // Above the day, or below it in the top rows, and kept inside the card.
    const box = active.square.getBoundingClientRect();
    const outer = wrapper.getBoundingClientRect();
    const width = tooltip.offsetWidth;
    const left = box.left - outer.left + box.width / 2 - width / 2;
    tooltip.style.left = `${Math.min(Math.max(0, left), outer.width - width)}px`;
    const above = box.top - outer.top - tooltip.offsetHeight - 6;
    const roomAbove = box.top - tooltip.offsetHeight - 6 > 0;
    tooltip.style.top = `${roomAbove ? above : box.bottom - outer.top + 6}px`;
  }

  // The pop-up stays a moment after the pointer leaves the day, so that it can
  // reach the activities in it, and as long as the pointer is on it.
  let hideTimer = null;
  let switchTimer = null;
  const hideSoon = () => {
    clearTimeout(switchTimer);
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (!wrapper.contains(document.activeElement)) show(null);
    }, 300);
  };
  // Whether the pointer comes closer to the pop-up's links, on its way to them.
  let pointer = null;
  function towardPopUp(event) {
    const from = pointer;
    pointer = { x: event.clientX, y: event.clientY };
    if (!from || tooltip.hidden || !tooltip.querySelector("a")) return false;
    const box = tooltip.getBoundingClientRect();
    const distance = ({ x, y }) =>
      Math.hypot(Math.max(box.left - x, 0, x - box.right), Math.max(box.top - y, 0, y - box.bottom));
    return distance(pointer) < distance(from);
  }
  svg.addEventListener("pointermove", (event) => {
    clearTimeout(hideTimer);
    const toward = towardPopUp(event);
    const index = event.target.dataset?.index;
    if (index == null || days[index] === active) {
      clearTimeout(switchTimer);
      return;
    }
    // Another day takes over at once, unless the pointer heads for the pop-up:
    // then it waits a moment, so that the way there can cross other days.
    clearTimeout(switchTimer);
    if (toward) switchTimer = setTimeout(() => show(Number(index)), 150);
    else show(Number(index));
  });
  svg.addEventListener("pointerleave", hideSoon);
  tooltip.addEventListener("pointerenter", () => {
    clearTimeout(hideTimer);
    clearTimeout(switchTimer);
  });
  tooltip.addEventListener("pointerleave", hideSoon);
  svg.addEventListener("focus", () => show(days.indexOf(active ?? days.at(-1))));
  // Tab goes on from the day to the activities in the pop-up, which keeps it open.
  wrapper.addEventListener("focusout", (event) => {
    if (!wrapper.contains(event.relatedTarget)) show(null);
  });
  svg.addEventListener("keydown", (event) => {
    const step = { ArrowUp: -1, ArrowDown: 1, ArrowLeft: -7, ArrowRight: 7 }[event.key];
    if (step == null) {
      if (event.key === "Escape") show(null);
      // Enter opens the day's first activity.
      if (event.key === "Enter") tooltip.querySelector("a")?.click();
      return;
    }
    event.preventDefault();
    const index = active ? days.indexOf(active) : days.length - 1;
    show(Math.min(days.length - 1, Math.max(0, index + step)));
  });

  wrapper.append(svg, tooltip);
  container.append(wrapper);
}

/** The best official time over each distance, from the race results entered by hand. */
function records(container, dated) {
  const races = dated.filter(({ result }) => result).sort((a, b) => b.time - a.time);
  if (races.length === 0) return;
  // Results over the same distance, within a meter, count against each other.
  const best = new Map();
  for (const race of races) {
    const key = Math.round(race.result.distance_m);
    const current = best.get(key);
    if (!current || race.result.time_s < current.result.time_s) best.set(key, race);
  }
  const list = document.createElement("dl");
  list.className = "records";
  for (const race of [...best.values()].sort((a, b) => a.result.distance_m - b.result.distance_m)) {
    list.append(raceRow(race, raceDistanceName(race.result.distance_m)));
  }
  card(container, "Personal records", "The best official time over each distance, from the race results").append(list);

  const all = document.createElement("dl");
  all.className = "records";
  for (const race of races) {
    all.append(raceRow(race, `${race.result.event || activityName(race)} · ${raceDistanceName(race.result.distance_m)}`));
  }
  card(container, "Races", "Every race with an official result, newest first").append(all);
}

function activityName({ name, activity_type: type }) {
  return name || typeName(type);
}

/** A row with the time, linked to the activity, the date and the official results. */
function raceRow(race, label) {
  const row = document.createElement("div");
  const name = document.createElement("dt");
  name.textContent = label;
  const value = document.createElement("dd");
  const link = document.createElement("a");
  link.href = `/?${new URLSearchParams({ activity: race.id })}`;
  link.textContent = duration(race.result.time_s);
  const when = document.createElement("small");
  when.textContent = new Date(race.time).toLocaleDateString(language);
  value.append(link, when);
  if (race.result.results_url) {
    const results = document.createElement("a");
    results.href = race.result.results_url;
    results.target = "_blank";
    results.rel = "noopener noreferrer";
    results.className = "records-results";
    results.textContent = t("Results");
    value.append(results);
  }
  row.append(name, value);
  return row;
}

/** The distance so far in each year, on the same calendar, to compare the years. */
function yearOverYear(container, dated) {
  const years = [...new Set(dated.map(({ time }) => new Date(time).getFullYear()))].sort().slice(-4);
  if (years.length < 2) return;
  const base = new Date().getFullYear();
  const series = years.map((year, index) => {
    let total = 0;
    const values = dated
      .filter(({ time }) => new Date(time).getFullYear() === year)
      .sort((a, b) => a.time - b.time)
      .map((activity) => {
        total += toDistance(activity.distance_m ?? 0);
        const date = new Date(activity.time);
        // Moved to this year, so that all years share one axis. Leap days land on March 1.
        return [new Date(base, date.getMonth(), date.getDate()).getTime(), total];
      });
    return { name: String(year), slot: index + 1, values };
  });
  chart(card(container, "Year over year", "Distance so far in each year"), {
    kind: "line",
    series,
    format: unit(0, distanceUnit),
    label: t("Distance so far in each year"),
    // A total goes on between activities, so its line never breaks.
    maxGap: Infinity,
  });
}
