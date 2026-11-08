/** Short texts about an activity: its race, weather and devices. */

import { duration, number, raceDistanceName, svgElement } from "/js/format.js";
import { t } from "/js/i18n.js";
import { fromFahrenheit, fromMilesPerHour, temperatureUnit, windSpeedUnit } from "/js/units.js";

const SENSOR_NAMES = {
  heart_rate: "Heart rate sensor",
  bike_power: "Power meter",
  bike_speed_cadence: "Speed and cadence sensor",
  bike_speed: "Speed sensor",
  bike_cadence: "Cadence sensor",
  stride_speed_distance: "Foot pod",
  muscle_oxygen: "Muscle oxygen sensor",
  temperature: "Temperature sensor",
};

/** `heart_rate` as "heart rate", for names that are not translated. */
const spaced = (text) => text.replaceAll("_", " ");

/** The race's time, with the target if there was one, like "Race 1:29:59 (target 1:30:00)". */
export function raceText(result) {
  if (!result) return null;
  const parts = [`${t("Race")} ${duration(result.time_s)} · ${raceDistanceName(result.distance_m)}`];
  if (result.target_time_s) parts.push(`${t("Target")} ${duration(result.target_time_s)}`);
  return parts.join(" · ");
}

/** Garmin's name of what an activity trained, like `AEROBIC_BASE`, as "Aerobic base". */
export const trainingLabel = (label) => spaced(label.toLowerCase()).replace(/^./, (first) => first.toUpperCase());

/** Icons on a 24 by 24 grid for the kinds of weather, drawn like the other icons. */
const WEATHER_ICONS = {
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6 7 7M17 17l1.4 1.4M5.6 18.4 7 17M17 7l1.4-1.4"/>',
  partly: '<circle cx="8" cy="8" r="3"/><path d="M8 2v1M2 8h1M3.8 3.8l.7.7M12.2 3.8l-.7.7"/><path d="M8 19a4 4 0 0 1 .5-8 5 5 0 0 1 9.4 1.5A3.3 3.3 0 0 1 17.5 19Z"/>',
  cloud: '<path d="M7 18a4.5 4.5 0 0 1 .6-9 5.5 5.5 0 0 1 10.5 1.7A3.7 3.7 0 0 1 17.5 18Z"/>',
  rain: '<path d="M7 15a4.5 4.5 0 0 1 .6-9 5.5 5.5 0 0 1 10.5 1.7A3.7 3.7 0 0 1 17.5 15Z"/><path d="M8 18l-1 3M12 18l-1 3M16 18l-1 3"/>',
  snow: '<path d="M7 15a4.5 4.5 0 0 1 .6-9 5.5 5.5 0 0 1 10.5 1.7A3.7 3.7 0 0 1 17.5 15Z"/><path d="M8 18v.01M12 19v.01M16 18v.01M10 21v.01M14 21v.01"/>',
  storm: '<path d="M7 15a4.5 4.5 0 0 1 .6-9 5.5 5.5 0 0 1 10.5 1.7A3.7 3.7 0 0 1 17.5 15Z"/><path d="M12 14l-2 4h3l-2 4"/>',
  fog: '<path d="M7 14a4.5 4.5 0 0 1 .6-8 5.5 5.5 0 0 1 10.5 1.7A3.7 3.7 0 0 1 17.5 14Z"/><path d="M5 17h14M7 20h10"/>',
};

/** The icon for conditions like "Light rain" or "Partly cloudy", or a plain cloud for others. */
function weatherIcon(conditions = "") {
  const text = conditions.toLowerCase();
  const kind =
    [
      ["storm", /thunder|storm/],
      ["snow", /snow|sleet|hail|ice/],
      ["rain", /rain|drizzle|shower/],
      ["fog", /fog|mist|haze|smoke/],
      ["partly", /partly|mostly sunny|few clouds|scattered/],
      ["cloud", /cloud|overcast/],
      ["sun", /sun|clear|fair/],
    ].find(([, pattern]) => pattern.test(text))?.[0] ?? "cloud";
  const icon = svgElement("svg", { class: "icon icon-weather", viewBox: "0 0 24 24", "aria-hidden": "true" });
  icon.innerHTML = WEATHER_ICONS[kind];
  return icon;
}

/**
 * What Garmin listed about the weather: an icon with the temperature for the
 * top of the activity, and the conditions, wind and humidity as `[label, value]`
 * rows for its other metrics. `null` without any.
 */
export function weatherInfo(weather) {
  if (!weather) return null;
  const conditions = weather.weatherTypeDTO?.desc ?? weather.weatherType;
  const named = typeof conditions === "string" && conditions ? conditions : null;
  const temperature = Number.isFinite(weather.temp)
    ? `${number(0)(fromFahrenheit(weather.temp))} ${temperatureUnit}`
    : null;
  const rows = [];
  if (named) rows.push([t("Weather"), named]);
  if (Number.isFinite(weather.windSpeed) && weather.windSpeed > 0) {
    rows.push([t("Wind"), `${number(0)(fromMilesPerHour(weather.windSpeed))} ${windSpeedUnit}`]);
  }
  if (Number.isFinite(weather.relativeHumidity)) {
    rows.push([t("Humidity"), `${number(0)(weather.relativeHumidity)} %`]);
  }
  if (!temperature && rows.length === 0) return null;
  return { temperature, icon: weatherIcon(named ?? ""), title: named, rows };
}

function deviceName({ manufacturer, product }) {
  const brand = manufacturer ? spaced(manufacturer).replace(/^./, (first) => first.toUpperCase()) : "";
  return [brand, product && spaced(product)].filter(Boolean).join(" ");
}

/** The watch and its sensors, like "Garmin fenix7" and "Heart rate sensor (Garmin hrm_pro)". */
export function devicesList(devices) {
  return (devices ?? [])
    .map((device) => {
      const name = deviceName(device);
      if (device.creator || !device.kind) return name;
      const kind = SENSOR_NAMES[device.kind] ? t(SENSOR_NAMES[device.kind]) : spaced(device.kind);
      return name ? `${kind} (${name})` : kind;
    })
    .filter(Boolean);
}
