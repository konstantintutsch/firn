/** The signed-in account, the settings dialog and the theme, shared by all pages. */

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

function setUpThemeToggle() {
  const toggle = document.querySelector("#theme-toggle");
  let theme = currentTheme();
  const update = () => {
    toggle.textContent = theme === "dark" ? "Light" : "Dark";
    toggle.setAttribute("aria-label", `Switch to ${theme === "dark" ? "light" : "dark"} mode`);
  };
  const apply = () => {
    const next = currentTheme();
    if (next === theme) return;
    theme = next;
    update();
    for (const listener of themeListeners) listener(theme);
  };
  toggle.addEventListener("click", () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // Without storage the choice still applies until the page is reloaded.
    }
    apply();
  });
  systemDark.addEventListener("change", apply);
  update();
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
    return (await response.json()).error ?? fallback;
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
  setUpThemeToggle();
  setUpSettings();
  const failed = new URLSearchParams(location.search).get("login") === "failed";
  if (failed) history.replaceState(null, "", location.pathname + location.hash);

  let response;
  try {
    response = await fetch("/api/me");
  } catch {
    showSignIn("Could not reach the API. Is it running?");
    return new Promise(() => {});
  }
  if (response.status === 401) {
    showSignIn(failed ? "Signing in did not work. Try again." : "");
    return new Promise(() => {});
  }
  if (!response.ok) {
    showSignIn(await errorMessage(response, "Could not load your account."));
    return new Promise(() => {});
  }
  account = await response.json();
  document.querySelector("#account-button").hidden = false;
  return account;
}

export function hasGarminKey() {
  return account?.has_garmin_key ?? false;
}

export function openSettings(message = "") {
  const dialog = document.querySelector("#settings");
  document.querySelector("#settings-account").textContent =
    account?.name || account?.email || "Signed in";
  document.querySelector("#key-status").textContent = hasGarminKey()
    ? "A Garmin key is saved."
    : "No Garmin key is saved yet. Add one to sync your data.";
  document.querySelector("#key-remove").hidden = !hasGarminKey();
  document.querySelector("#settings-message").textContent = message;
  dialog.showModal();
}

function setUpSettings() {
  const dialog = document.querySelector("#settings");
  const form = document.querySelector("#key-form");
  const input = form.elements.api_key;
  const message = document.querySelector("#settings-message");

  document.querySelector("#account-button").addEventListener("click", () => openSettings());
  document.querySelector("#settings-close").addEventListener("click", () => dialog.close());

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const submit = form.querySelector("button[type=submit]");
    submit.disabled = true;
    message.textContent = "Checking the key with Garmin…";
    try {
      const response = await send("/api/me/garmin-key", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ api_key: input.value }),
      });
      if (response.ok) {
        account.has_garmin_key = true;
        input.value = "";
        openSettings("Saved. Select Sync to save your data.");
      } else {
        message.textContent = await errorMessage(response, "Could not save the key.");
      }
    } catch {
      message.textContent = "Could not reach the API. Is it running?";
    } finally {
      submit.disabled = false;
    }
  });

  document.querySelector("#key-remove").addEventListener("click", async () => {
    const response = await send("/api/me/garmin-key", { method: "DELETE" });
    if (response.ok) {
      account.has_garmin_key = false;
      openSettings("Removed. Your saved data stays.");
    } else {
      message.textContent = await errorMessage(response, "Could not remove the key.");
    }
  });

  document.querySelector("#sign-out").addEventListener("click", async () => {
    await send("/api/auth/logout", { method: "POST" });
    location.reload();
  });
}

export function plural(count, noun) {
  const word = count === 1 ? noun : noun.replace(/y$/, "ie") + "s";
  return `${count.toLocaleString()} ${word}`;
}

export function typeName(type) {
  const words = type.replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Describes the progress of a sync, for the sync status line. */
export function syncProgress(sync) {
  const { to_download: total, downloaded, failed } = sync;
  if (sync.health_to_fetch > 0) {
    const done = sync.health_fetched + sync.health_failed;
    return `Saving health data ${Math.min(done + 1, sync.health_to_fetch)} of ${sync.health_to_fetch}…`;
  }
  if (total === 0) return "Looking for new activities…";
  return `Saving activity ${Math.min(downloaded + failed + 1, total)} of ${total}…`;
}

export function syncResult({ state, error, downloaded, failed, health_failed: healthFailed }) {
  if (state === "failed") return error;
  const saved =
    downloaded === 0 ? "No new activities." : `Saved ${plural(downloaded, "new activity")}.`;
  const problems = [];
  if (failed > 0) problems.push(plural(failed, "activity"));
  if (healthFailed > 0) problems.push(plural(healthFailed, "health data request"));
  if (problems.length === 0) return `${saved} Health data is up to date.`;
  return `${saved} ${problems.join(" and ")} failed. Sync again to retry.`;
}

const SYNC_POLL_MS = 1000;

/**
 * Wires the sync button and status line. Calls `onFinished` after a sync ends,
 * so that the page can show the new data.
 */
export function setUpSync(onFinished) {
  const button = document.querySelector("#sync-button");
  const status = document.querySelector("#sync-status");
  let isRunning = false;

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
      setTimeout(poll, SYNC_POLL_MS);
      return;
    }
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
      openSettings("Add your Garmin key first.");
      return;
    }
    button.disabled = true;
    status.textContent = "Starting…";
    try {
      const response = await send("/api/sync", { method: "POST" });
      if (response.ok) {
        isRunning = true;
      } else if (response.status !== 409 || !(await isActive())) {
        button.disabled = false;
        status.textContent = await errorMessage(response, "Could not start the sync.");
        return;
      }
      poll();
    } catch {
      button.disabled = false;
      status.textContent = "Could not start the sync. Is the API running?";
    }
  });

  poll();
}
