/** The signed-in account, the settings page and the theme, shared by all pages. */

import { language, plural, t, typeName } from "/js/i18n.js";

export { plural, t, typeName };

const THEME_STORAGE_KEY = "theme";
const systemDark = matchMedia("(prefers-color-scheme: dark)");
const themeListeners = [];

export function currentTheme() {
  return document.documentElement.dataset.theme ?? (systemDark.matches ? "dark" : "light");
}

/** Calls `listener` with the new theme whenever it changes. */
export function onThemeChange(listener) {
  themeListeners.push(listener);
}

let theme = currentTheme();

/** Tells the listeners if the theme changed, by a choice or with the system's. */
function notifyTheme() {
  const next = currentTheme();
  if (next === theme) return;
  theme = next;
  for (const listener of themeListeners) listener(theme);
}

systemDark.addEventListener("change", notifyTheme);

/** Wires the choice between the system's, the light and the dark theme on the settings page. */
function setUpThemePicker() {
  const buttons = [...document.querySelectorAll("#theme-picker [data-theme-choice]")];
  const show = () => {
    const chosen = document.documentElement.dataset.theme ?? "system";
    for (const button of buttons) {
      button.setAttribute("aria-pressed", String(button.dataset.themeChoice === chosen));
    }
  };
  for (const button of buttons) {
    button.addEventListener("click", () => {
      const choice = button.dataset.themeChoice;
      try {
        if (choice === "system") localStorage.removeItem(THEME_STORAGE_KEY);
        else localStorage.setItem(THEME_STORAGE_KEY, choice);
      } catch {
        // Without storage the choice still applies until the page is reloaded.
      }
      if (choice === "system") delete document.documentElement.dataset.theme;
      else document.documentElement.dataset.theme = choice;
      // From /js/theme.js, which every page loads first.
      applyThemeColor();
      show();
      notifyTheme();
    });
  }
  show();
}

/** Sends a request that changes something, with the header the API requires. */
export function send(url, options = {}) {
  return fetch(url, {
    ...options,
    headers: { "X-Requested-With": "runmap", ...options.headers },
  });
}

/** The API's error message, or a fallback if the answer has none. */
export async function errorMessage(response, fallback) {
  try {
    const { error } = await response.json();
    return error ? t(error) : fallback;
  } catch {
    return fallback;
  }
}

function showSignIn(message) {
  const screen = document.querySelector("#sign-in");
  document.querySelector("#sign-in-message").textContent = message;
  screen.hidden = false;
}

let account = null;

/**
 * Resolves with the account once signed in. Without a session, it shows the
 * sign-in screen and never resolves, so the page waits there.
 */
export async function requireAccount() {
  const failed = new URLSearchParams(location.search).get("login") === "failed";
  if (failed) history.replaceState(null, "", location.pathname + location.hash);

  let response;
  try {
    response = await fetch("/api/me");
  } catch {
    showSignIn(t("Could not reach the API. Is it running?"));
    return new Promise(() => {});
  }
  if (response.status === 401) {
    showSignIn(failed ? t("Signing in did not work. Try again.") : "");
    return new Promise(() => {});
  }
  if (!response.ok) {
    showSignIn(await errorMessage(response, t("Could not load your account.")));
    return new Promise(() => {});
  }
  account = await response.json();
  document.querySelector("#account-button").hidden = false;
  return account;
}

export function hasGarminKey() {
  return account?.has_garmin_key ?? false;
}

/** Shows the account and whether it has a Garmin key on the settings page, with an optional `message`. */
function showSettings(message = "") {
  document.querySelector("#settings-account").textContent =
    account?.name || account?.email || t("Signed in");
  document.querySelector("#key-status").textContent = hasGarminKey()
    ? t("A Garmin key is saved.")
    : t("No Garmin key is saved yet. Add one to sync your data.");
  document.querySelector("#key-remove").hidden = !hasGarminKey();
  document.querySelector("#settings-message").textContent = message;
}

/** Wires the settings page. Other pages send people here with `?key=missing` to add a key before syncing. */
export function setUpSettings() {
  setUpThemePicker();
  const form = document.querySelector("#key-form");
  const input = form.elements.api_key;
  const message = document.querySelector("#settings-message");

  const keyMissing = new URLSearchParams(location.search).get("key") === "missing";
  showSettings(keyMissing ? t("Add your Garmin key first.") : "");

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const submit = form.querySelector("button[type=submit]");
    submit.disabled = true;
    message.textContent = t("Checking the key with Garmin…");
    try {
      const response = await send("/api/me/garmin-key", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ api_key: input.value }),
      });
      if (response.ok) {
        account.has_garmin_key = true;
        input.value = "";
        showSettings(t("Saved. Select Sync to save your data."));
      } else {
        message.textContent = await errorMessage(response, t("Could not save the key."));
      }
    } catch {
      message.textContent = t("Could not reach the API. Is it running?");
    } finally {
      submit.disabled = false;
    }
  });

  document.querySelector("#key-remove").addEventListener("click", async () => {
    if (!confirm(t("Remove the Garmin key? Your saved data stays, but you cannot sync until you add a key again."))) {
      return;
    }
    try {
      const response = await send("/api/me/garmin-key", { method: "DELETE" });
      if (response.ok) {
        account.has_garmin_key = false;
        showSettings(t("Removed. Your saved data stays."));
      } else {
        message.textContent = await errorMessage(response, t("Could not remove the key."));
      }
    } catch {
      message.textContent = t("Could not reach the API. Is it running?");
    }
  });

  document.querySelector("#sign-out").addEventListener("click", async () => {
    try {
      const response = await send("/api/auth/logout", { method: "POST" });
      if (!response.ok) {
        message.textContent = await errorMessage(response, t("Could not sign out."));
        return;
      }
    } catch {
      message.textContent = t("Could not reach the API. Is it running?");
      return;
    }
    location.reload();
  });
}

export function activityTitle({ name, activity_type: type }) {
  return name || typeName(type);
}

export function activityDetails({ started_at: started, distance_m: distance, duration_s: duration, location }) {
  const parts = [new Date(started).toLocaleDateString()];
  if (distance != null) {
    const km = (distance / 1000).toLocaleString(language, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
    parts.push(`${km} km`);
  }
  if (duration != null) parts.push(`${Math.round(duration / 60)} min`);
  if (location) parts.push(location);
  return parts.join(" · ");
}

/** Describes the progress of a sync, for the sync status line. */
export function syncProgress(sync) {
  const { to_download: total, downloaded, failed } = sync;
  if (sync.health_to_fetch > 0) {
    const done = sync.health_fetched + sync.health_failed;
    return t("Saving health data {current} of {total}…", {
      current: Math.min(done + 1, sync.health_to_fetch),
      total: sync.health_to_fetch,
    });
  }
  if (total === 0) return t("Looking for new activities…");
  return t("Saving activity {current} of {total}…", { current: Math.min(downloaded + failed + 1, total), total });
}

export function syncResult({ state, error, downloaded, failed, health_failed: healthFailed }) {
  if (state === "failed") return t(error);
  if (state === "cancelled") return t("Sync stopped. The next sync continues where it stopped.");
  const saved =
    downloaded === 0
      ? t("No new activities.")
      : t("Saved {count}.", { count: plural(downloaded, "new activity", "new activities") });
  const problems = [];
  if (failed > 0) problems.push(plural(failed, "activity", "activities"));
  if (healthFailed > 0) problems.push(plural(healthFailed, "health data request", "health data requests"));
  if (problems.length === 0) return t("{saved} Health data is up to date.", { saved });
  return t("{saved} {problems} failed. Sync again to retry.", { saved, problems: problems.join(` ${t("and")} `) });
}

const SYNC_POLL_MS = 1000;
/** Polling waits longer after each failed request, up to this. */
const SYNC_POLL_MAX_MS = 30_000;
/** After this many failed requests in a row, polling gives up until the next sync. */
const SYNC_POLL_ATTEMPTS = 10;

/**
 * Wires the sync button and status line. Calls `onFinished` after a sync ends,
 * so that the page can show the new data.
 */
export function setUpSync(onFinished) {
  const button = document.querySelector("#sync-button");
  const status = document.querySelector("#sync-status");
  let isRunning = false;
  let failures = 0;

  async function isActive() {
    const response = await fetch("/api/sync");
    return response.ok && (await response.json()).state === "running";
  }

  /** Also runs on page load, to pick up a sync that is already running. */
  async function poll() {
    let sync;
    try {
      const response = await fetch("/api/sync");
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      sync = await response.json();
    } catch {
      // On page load no sync is known to run, so there is nothing to wait for.
      if (!isRunning) return;
      failures += 1;
      if (failures >= SYNC_POLL_ATTEMPTS) {
        isRunning = false;
        failures = 0;
        button.disabled = false;
        status.textContent = t("Lost contact with the API. Reload the page to see the sync.");
        return;
      }
      setTimeout(poll, Math.min(SYNC_POLL_MS * 2 ** failures, SYNC_POLL_MAX_MS));
      return;
    }
    failures = 0;
    if (sync.state === "running") {
      isRunning = true;
      button.disabled = true;
      status.textContent = syncProgress(sync);
      setTimeout(poll, SYNC_POLL_MS);
    } else if (isRunning) {
      isRunning = false;
      button.disabled = false;
      status.textContent = syncResult(sync);
      onFinished();
    }
  }

  button.addEventListener("click", async () => {
    if (!hasGarminKey()) {
      location.href = "/settings/?key=missing";
      return;
    }
    button.disabled = true;
    status.textContent = t("Starting…");
    try {
      const response = await send("/api/sync", { method: "POST" });
      if (response.ok) {
        isRunning = true;
      } else if (response.status !== 409 || !(await isActive())) {
        button.disabled = false;
        status.textContent = await errorMessage(response, t("Could not start the sync."));
        return;
      }
      poll();
    } catch {
      button.disabled = false;
      status.textContent = t("Could not start the sync. Is the API running?");
    }
  });

  poll();
}
