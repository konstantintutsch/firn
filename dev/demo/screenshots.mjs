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
  await context.addCookies([{ name: "firn_session", value: "demo-session-token", url: BASE }]);
  await context.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.removeItem("filters");
  });
  return context.newPage();
}

async function shoot(name, url, { theme, viewport = { width: 1440, height: 900 }, wait = 4000, before, section, height } = {}) {
  const tab = await page(theme, viewport);
  await tab.goto(`${BASE}${url}`);
  await tab.waitForTimeout(wait);
  await before?.(tab);
  // JPEG keeps the map photos small enough for the repository.
  const path = `${OUT}/${name}-${theme}.jpg`;
  if (section) {
    // The top of one section, cut after a row of cards, without the floating nav, so the charts fill the image and stay readable.
    await tab.evaluate(() => document.querySelector(".nav").style.display = "none");
    const box = await tab.locator(section).boundingBox();
    await tab.screenshot({
      path,
      type: "jpeg",
      quality: 85,
      fullPage: true,
      clip: { x: box.x, y: box.y, width: box.width, height: Math.min(box.height, height) },
    });
  } else {
    await tab.screenshot({ path, type: "jpeg", quality: 85 });
  }
  await tab.context().close();
  console.log(`${name}-${theme}.jpg`);
}

/** The Köln Marathon, which has photos and shows the most in its details. */
async function showcase() {
  const tab = await page("light", { width: 800, height: 600 });
  await tab.goto(`${BASE}/settings/`);
  const id = await tab.evaluate(async () => {
    const { activities } = await (await fetch("/api/activities?q=K%C3%B6ln%20Marathon&limit=1")).json();
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
  for (const [name, section, height] of [["training", "#activity-section", 835], ["health", "#health-section", 820]]) {
    await shoot(name, "/stats/?days=365", { theme, viewport: { width: 1280, height: 900 }, section, height });
  }
  await shoot("activities", "/activities/", { theme, viewport: { width: 1280, height: 900 } });
}
await shoot("mobile", "/#11/52.5/13.39", {
  theme: "light",
  viewport: { width: 390, height: 844 },
  wait: 7000,
});

await browser.close();
