import { activityDetails, activityTitle, plural, requireAccount, setUpSync, t, typeName } from "/js/account.js";
import { setUpFilters } from "/js/filters.js";

const PAGE_SIZE = 50;
const SEARCH_DELAY_MS = 300;
const SORT_STORAGE_KEY = "activity-sort";

const searchInput = document.querySelector("#activity-search");
const sortSelect = document.querySelector("#activity-sort");
const statusElement = document.querySelector("#activity-status");
const listElement = document.querySelector("#activity-list");
const moreButton = document.querySelector("#activity-more");

await requireAccount();

// The same filters as the map and the statistics, which they share.
const filters = setUpFilters({ withoutGps: true, onChange: () => load() });

try {
  sortSelect.value = localStorage.getItem(SORT_STORAGE_KEY) ?? "newest";
} catch {}

let pending = null;
let searchTimer = null;
let loaded = 0;
let lastMonth = null;
/** Count and distance per month of all matching activities, for the month headings. */
let months = new Map();

function baseQuery() {
  const query = filters.query();
  const text = searchInput.value.trim();
  if (text) query.set("q", text);
  query.set("without_gps", "true");
  return query;
}

const byDate = () => sortSelect.value === "newest" || sortSelect.value === "oldest";

/** Loads the first page, or with `append` the next one. */
async function load(append = false) {
  pending?.abort();
  const request = new AbortController();
  pending = request;
  if (!append) {
    loaded = 0;
    lastMonth = null;
  }

  const query = baseQuery();
  query.set("limit", PAGE_SIZE);
  query.set("offset", loaded);
  query.set("sort", sortSelect.value);
  moreButton.disabled = true;
  if (!append) statusElement.textContent = t("Loading…");

  let activities;
  try {
    const [response] = await Promise.all([
      fetch(`/api/activities?${query}`, { signal: request.signal }),
      append || !byDate() ? null : loadMonths(request.signal),
    ]);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ activities } = await response.json());
  } catch (error) {
    if (error.name !== "AbortError") statusElement.textContent = t("Could not load activities. Is the API running?");
    moreButton.disabled = false;
    return;
  }

  const items = activities.flatMap((activity) => {
    // In local time, like the days that the month totals count.
    const started = new Date(activity.started_at);
    const month = `${started.getFullYear()}-${String(started.getMonth() + 1).padStart(2, "0")}`;
    const heading = byDate() && month !== lastMonth ? [monthHeading(month)] : [];
    lastMonth = month;
    return [...heading, listItem(activity)];
  });
  if (append) listElement.append(...items);
  else listElement.replaceChildren(...items);
  loaded += activities.length;
  moreButton.hidden = activities.length < PAGE_SIZE;
  moreButton.disabled = false;
  if (loaded > 0) statusElement.textContent = "";
  else if (baseQuery().size > 1) statusElement.textContent = t("Nothing found.");
  else statusElement.textContent = t("No activities saved yet. Select Sync to save them from Garmin.");
}

/** Totals per month of everything that matches, not only the loaded pages. */
async function loadMonths(signal) {
  const query = baseQuery();
  query.delete("without_gps");
  const response = await fetch(`/api/statistics/activities?${query}`, { signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const { activities } = await response.json();
  months = new Map();
  for (const { day, distance_m: meters } of activities) {
    if (!day) continue;
    const month = day.slice(0, 7);
    const entry = months.get(month) ?? { count: 0, meters: 0 };
    entry.count += 1;
    entry.meters += meters ?? 0;
    months.set(month, entry);
  }
}

function monthHeading(month) {
  const item = document.createElement("li");
  item.className = "month-heading";
  const [year, number] = month.split("-").map(Number);
  const name = document.createElement("strong");
  name.textContent = new Date(year, number - 1).toLocaleDateString(undefined, { month: "long", year: "numeric" });
  item.append(name);
  const totals = months.get(month);
  if (totals) {
    const summary = document.createElement("span");
    const km = Math.round(totals.meters / 1000).toLocaleString();
    summary.textContent = `${plural(totals.count, "activity", "activities")} · ${km} km`;
    item.append(summary);
  }
  return item;
}

/** Each activity links to its details on the map, which then shows only its track. */
function listItem(activity) {
  const item = document.createElement("li");
  const entry = document.createElement("a");
  entry.className = "activity";
  entry.href = `/?${new URLSearchParams({ activity: activity.id })}`;
  const title = document.createElement("strong");
  title.textContent = activityTitle(activity);
  const type = document.createElement("span");
  type.className = "activity-type";
  type.textContent = activity.bounds ? typeName(activity.activity_type) : `${typeName(activity.activity_type)} · ${t("No GPS track")}`;
  const details = document.createElement("small");
  details.textContent = activityDetails(activity);
  entry.append(title, type, details);
  item.append(entry);
  return item;
}

searchInput.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(load, SEARCH_DELAY_MS);
});

sortSelect.addEventListener("change", () => {
  try {
    localStorage.setItem(SORT_STORAGE_KEY, sortSelect.value);
  } catch {}
  load();
});

moreButton.addEventListener("click", () => load(true));
// Loads the next page before the end of the list comes into view.
new IntersectionObserver(
  ([entry]) => {
    if (entry.isIntersecting && !moreButton.hidden && !moreButton.disabled) load(true);
  },
  { rootMargin: "600px" },
).observe(moreButton);

setUpSync(() => load());
load();
