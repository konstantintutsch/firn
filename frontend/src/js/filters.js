import { typeName } from "/js/account.js";

const STORAGE_KEY = "filters";
const DAY_MS = 86_400_000;
const FILTER_DELAY_MS = 300;

export function localDay(time) {
  const date = new Date(time);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${String(date.getDate()).padStart(2, "0")}`;
}

/**
 * The filters that the map and the statistics share, kept for the next visit.
 * `days` is a period that ends today, like the last 30 days, so it moves along
 * with the date instead of keeping the day it was chosen on. Types that one
 * page does not list, like those without GPS on the map, stay checked.
 */
export function setUpFilters({ withoutGps = false, onChange }) {
  const form = document.querySelector("#filters");
  const filterCount = document.querySelector("#filter-count");
  const typesField = document.querySelector("#types-field");
  const typesElement = document.querySelector("#types");
  let saved = { values: {}, days: null };
  try {
    saved = { ...saved, ...JSON.parse(localStorage.getItem(STORAGE_KEY)) };
  } catch {}
  let days = saved.days;
  let checkedTypes = saved.values.type ?? [];
  let timer = null;

  for (const element of form.elements) {
    if (element.name && element.type !== "checkbox") element.value = saved.values[element.name] ?? "";
  }
  if (days) setPeriod(days);
  updateCount();

  function setPeriod(next) {
    days = next;
    form.elements.from.value = days ? localDay(Date.now() - (Number(days) - 1) * DAY_MS) : "";
    form.elements.to.value = "";
  }

  function save() {
    const listed = [...typesElement.querySelectorAll("input")].map((input) => input.value);
    const values = { type: checkedTypes.filter((type) => !listed.includes(type)) };
    for (const [name, value] of new FormData(form)) {
      if (name === "type") values.type.push(value);
      else if (value.trim() !== "") values[name] = value;
    }
    checkedTypes = values.type;
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ values, days }));
    } catch {
      // Without storage the filters still apply until the page is reloaded.
    }
  }

  function updateCount() {
    const fields = [...form.elements].filter(
      (element) => element.name && element.type !== "checkbox" && element.value.trim() !== "",
    );
    const count = fields.length + checkedTypes.length;
    filterCount.textContent = count > 0 ? String(count) : "";
  }

  function changed() {
    save();
    updateCount();
    onChange();
  }

  form.addEventListener("input", (event) => {
    if (event.target.name === "from" || event.target.name === "to") days = null;
    clearTimeout(timer);
    timer = setTimeout(changed, FILTER_DELAY_MS);
  });

  // Fires before the form clears its fields, so this has to wait for that to finish.
  form.addEventListener("reset", () => {
    clearTimeout(timer);
    days = null;
    checkedTypes = [];
    setTimeout(changed);
  });

  form.addEventListener("submit", (event) => event.preventDefault());

  return {
    get days() {
      return days;
    },

    /** Shows the last `days` days, or all of them with `null`. */
    setDays(next) {
      clearTimeout(timer);
      setPeriod(next);
      changed();
    },

    /** The active filters as API parameters. */
    query() {
      const values = new FormData(form);
      const query = new URLSearchParams();
      if (checkedTypes.length > 0) query.set("types", checkedTypes.join(","));
      for (const name of ["title", "description", "location", "from", "to"]) {
        const value = values.get(name).trim();
        if (value) query.set(name, value);
      }
      const setScaled = (from, to, scale) => {
        const value = values.get(from);
        if (value !== "") query.set(to, Number(value) * scale);
      };
      setScaled("min_distance_km", "min_distance_m", 1000);
      setScaled("max_distance_km", "max_distance_m", 1000);
      setScaled("min_duration_min", "min_duration_s", 60);
      setScaled("max_duration_min", "max_duration_s", 60);
      return query;
    },

    /** Keeps the checked types, because a sync can reload the list at any time. */
    async loadTypes() {
      let types;
      try {
        const response = await fetch(`/api/activity-types${withoutGps ? "?without_gps=true" : ""}`);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        ({ types } = await response.json());
      } catch {
        return;
      }
      typesElement.replaceChildren(
        ...types.map(({ activity_type: type, activities }) => {
          const label = document.createElement("label");
          const input = document.createElement("input");
          input.type = "checkbox";
          input.name = "type";
          input.value = type;
          input.checked = checkedTypes.includes(type);
          const count = document.createElement("small");
          count.textContent = activities.toLocaleString();
          label.append(input, typeName(type), count);
          return label;
        }),
      );
      typesField.hidden = types.length === 0;
      updateCount();
    },
  };
}
