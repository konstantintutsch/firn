// Takes the screenshots for the README from the demo, in English, in light and
// dark. Runs in the Playwright image on the demo's network, see `just screenshots`.

import { chromium } from "playwright";

const BASE = "http://frontend:8080";
const OUT = "/out";

const browser = await chromium.launch({
  // Software WebGL, since the container has no GPU.
  args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
});

async function page(theme, viewport, deviceScaleFactor = 2) {
  const context = await browser.newContext({ viewport, deviceScaleFactor, colorScheme: theme });
  await context.addCookies([{ name: "runmap_session", value: "demo-session-token", url: BASE }]);
  await context.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.removeItem("filters");
  });
  return context.newPage();
}

async function shoot(name, url, { theme, viewport = { width: 1440, height: 900 }, wait = 4000, before } = {}) {
  const tab = await page(theme, viewport);
  await tab.goto(`${BASE}${url}`);
  await tab.waitForTimeout(wait);
  await before?.(tab);
  // JPEG keeps the map photos small enough for the repository.
  await tab.screenshot({ path: `${OUT}/${name}-${theme}.jpg`, type: "jpeg", quality: 85 });
  await tab.context().close();
  console.log(`${name}-${theme}.jpg`);
}

/** A recent run with photos, which shows the most in its details. */
async function showcase() {
  const tab = await page("light", { width: 800, height: 600 });
  await tab.goto(`${BASE}/settings/`);
  const id = await tab.evaluate(async () => {
    const { activities } = await (await fetch("/api/activities?limit=100")).json();
    for (const activity of activities.filter(({ activity_type: type }) => type === "running")) {
      const detail = await (await fetch(`/api/activities/${activity.id}`)).json();
      if (detail.images.length > 0) return activity.id;
    }
    return activities[0].id;
  });
  await tab.context().close();
  return id;
}

const ACTIVITY = await showcase();

for (const theme of ["light", "dark"]) {
  await shoot("map", "/#11.3/52.497/13.38", { theme, wait: 7000 });
  await shoot("activity", `/?activity=${ACTIVITY}`, {
    theme,
    wait: 7000,
    // The track colored by heart rate shows more than the heat.
    before: async (tab) => {
      await tab.selectOption("#track-color", "heart_rate");
      await tab.waitForTimeout(1500);
    },
  });
  await shoot("statistics", "/stats/?days=365", {
    theme,
    viewport: { width: 1280, height: 1100 },
  });
  await shoot("activities", "/activities/", { theme, viewport: { width: 1280, height: 900 } });
}
await shoot("mobile", "/#11/52.5/13.39", {
  theme: "light",
  viewport: { width: 390, height: 844 },
  wait: 7000,
});

await browser.close();
