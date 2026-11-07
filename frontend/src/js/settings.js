import { errorMessage, plural, requireAccount, setUpSettings, setUpSync, t } from "/js/account.js";

await requireAccount();
setUpSettings();
setUpSync(() => {});
setUpImport();

/** Uploads a ZIP to import. Through XMLHttpRequest, since fetch cannot report upload progress. */
function setUpImport() {
  const input = document.querySelector("#import-file");
  const progress = document.querySelector("#import-progress");
  const message = document.querySelector("#import-message");

  input.addEventListener("change", () => {
    const [file] = input.files;
    if (!file) return;
    input.disabled = true;
    progress.hidden = false;
    progress.value = 0;
    message.textContent = t("Uploading…");

    const request = new XMLHttpRequest();
    request.open("POST", "/api/import");
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

function importResult({ activities, skipped, failed, health_documents: health }) {
  const parts = [t("Imported {count}.", { count: plural(activities, "activity", "activities") })];
  if (health > 0) parts.push(t("Health data of {count}.", { count: plural(health, "document", "documents") }));
  if (skipped > 0) parts.push(t("{count} belong to another account and were skipped.", { count: skipped }));
  if (failed > 0) parts.push(t("{count} could not be read.", { count: plural(failed, "file", "files") }));
  return parts.join(" ");
}
