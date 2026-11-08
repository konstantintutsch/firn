import { errorMessage, plural, requireAccount, setUpSettings, setUpSync, t } from "/js/account.js";
import { LANGUAGE_STORAGE_KEY } from "/js/i18n.js";
import { setUnits, units } from "/js/units.js";

await requireAccount();
setUpSettings();
setUpSync(() => {});
setUpImport();
// Both apply to every page, so a reload shows the change everywhere at once.
setUpChoice("#units-picker", units, (choice) => setUnits(choice));
setUpChoice("#language-picker", chosenLanguage(), (choice) => {
  try {
    if (choice) localStorage.setItem(LANGUAGE_STORAGE_KEY, choice);
    else localStorage.removeItem(LANGUAGE_STORAGE_KEY);
  } catch {}
});

function chosenLanguage() {
  try {
    return localStorage.getItem(LANGUAGE_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

/** Buttons that pick one choice, which `save` keeps before the page reloads. */
function setUpChoice(selector, current, save) {
  const buttons = [...document.querySelectorAll(`${selector} [data-choice]`)];
  for (const button of buttons) {
    button.setAttribute("aria-pressed", String(button.dataset.choice === current));
    button.addEventListener("click", () => {
      if (button.dataset.choice === current) return;
      save(button.dataset.choice);
      location.reload();
    });
  }
}


/** Uploads a ZIP to import. Through XMLHttpRequest, since fetch cannot report upload progress. */
function setUpImport() {
  const input = document.querySelector("#import-file");
  const progress = document.querySelector("#import-progress");
  const message = document.querySelector("#import-message");
  const replace = document.querySelector("#import-replace");

  input.addEventListener("change", () => {
    const [file] = input.files;
    if (!file) return;
    if (replace.checked && !confirm(t("Remove everything in this account that is not in the import? This cannot be undone. A later sync from Garmin saves it again."))) {
      input.value = "";
      return;
    }
    input.disabled = true;
    progress.hidden = false;
    progress.value = 0;
    message.textContent = t("Uploading…");

    const request = new XMLHttpRequest();
    request.open("POST", replace.checked ? "/api/import?replace=true" : "/api/import");
    request.setRequestHeader("X-Requested-With", "runmap");
    request.setRequestHeader("Content-Type", "application/zip");
    request.upload.addEventListener("progress", (event) => {
      if (event.lengthComputable) progress.value = event.loaded / event.total;
      if (event.loaded === event.total) {
        progress.removeAttribute("value");
        message.textContent = t("Importing…");
      }
    });
    request.addEventListener("loadend", async () => {
      input.disabled = false;
      input.value = "";
      progress.hidden = true;
      if (request.status === 200) {
        message.textContent = importResult(JSON.parse(request.responseText));
      } else if (request.status === 0) {
        message.textContent = t("Could not reach the API. Is it running?");
      } else {
        message.textContent = await errorMessage(new Response(request.responseText), t("Could not import the file."));
      }
    });
    request.send(file);
  });
}

function importResult({
  activities,
  skipped,
  failed,
  health_documents: health,
  routes,
  removed_activities: removedActivities,
  removed_routes: removedRoutes,
  removed_health_documents: removedHealth,
  replace_skipped: replaceSkipped,
}) {
  const parts = [t("Imported {count}.", { count: plural(activities, "activity", "activities") })];
  if (routes > 0) parts.push(t("Routes: {count}.", { count: routes }));
  if (health > 0) parts.push(t("Health data of {count}.", { count: plural(health, "document", "documents") }));
  if (skipped > 0) parts.push(t("{count} belong to another account and were skipped.", { count: skipped }));
  if (failed > 0) parts.push(t("{count} could not be read.", { count: plural(failed, "file", "files") }));
  if (replaceSkipped) parts.push(t("Nothing was removed, since some files could not be read."));
  const removed = removedActivities + removedRoutes + removedHealth;
  if (removed > 0) {
    const counts = [
      removedActivities > 0 && plural(removedActivities, "activity", "activities"),
      removedRoutes > 0 && plural(removedRoutes, "route", "routes"),
      removedHealth > 0 && plural(removedHealth, "health document", "health documents"),
    ].filter(Boolean);
    parts.push(t("Removed: {counts}.", { counts: counts.join(", ") }));
  }
  return parts.join(" ");
}
