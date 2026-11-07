import { activityDetails, activityTitle, requireAccount, setUpSync, t, typeName } from "/js/account.js";

const PAGE_SIZE = 50;
const SEARCH_DELAY_MS = 300;

const searchInput = document.querySelector("#activity-search");
const statusElement = document.querySelector("#activity-status");
const listElement = document.querySelector("#activity-list");
const moreButton = document.querySelector("#activity-more");

await requireAccount();

let pending = null;
let searchTimer = null;
let loaded = 0;

/** Loads the first page, or with `append` the next one. */
async function load(append = false) {
  pending?.abort();
  const request = new AbortController();
  pending = request;
  if (!append) loaded = 0;

  const query = new URLSearchParams({ limit: PAGE_SIZE, offset: loaded });
  const text = searchInput.value.trim();
  if (text) query.set("q", text);
  moreButton.disabled = true;
  if (!append) statusElement.textContent = t("Loading…");

  let activities;
  try {
    const response = await fetch(`/api/activities?${query}`, { signal: request.signal });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    ({ activities } = await response.json());
  } catch (error) {
    if (error.name !== "AbortError") statusElement.textContent = t("Could not load activities. Is the API running?");
    moreButton.disabled = false;
    return;
  }

  const items = activities.map(listItem);
  if (append) listElement.append(...items);
  else listElement.replaceChildren(...items);
  loaded += activities.length;
  moreButton.hidden = activities.length < PAGE_SIZE;
  moreButton.disabled = false;
  if (loaded > 0) statusElement.textContent = "";
  else if (text) statusElement.textContent = t("Nothing found.");
  else statusElement.textContent = t("No activities saved yet. Select Sync to save them from Garmin.");
}

/** Activities with a track link to the map, which then shows only them. */
function listItem(activity) {
  const item = document.createElement("li");
  const entry = document.createElement(activity.bounds ? "a" : "div");
  entry.className = "activity";
  if (activity.bounds) entry.href = `/?${new URLSearchParams({ activity: activity.id })}`;
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

moreButton.addEventListener("click", () => load(true));

setUpSync(() => load());
load();
