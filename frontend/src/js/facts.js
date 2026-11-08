/** Short texts about an activity for the list: race, training effect, weather and devices. */

import { duration, number, raceDistanceName } from "/js/format.js";
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

/** The primary training effect and what it trained, like "Training effect 3.4 · Tempo". */
export function trainingEffectText({ aerobic_training_effect: effect, training_effect_label: label }) {
  const parts = [];
  if (effect != null) parts.push(`${t("Training effect")} ${number(1)(effect)}`);
  if (label) parts.push(spaced(label.toLowerCase()).replace(/^./, (first) => first.toUpperCase()));
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** Temperature, conditions, wind and humidity, for the parts that Garmin listed. */
export function weatherText(weather) {
  if (!weather) return null;
  const parts = [];
  if (Number.isFinite(weather.temp)) parts.push(`${number(0)(fromFahrenheit(weather.temp))} ${temperatureUnit}`);
  const conditions = weather.weatherTypeDTO?.desc ?? weather.weatherType;
  if (typeof conditions === "string" && conditions) parts.push(conditions);
  if (Number.isFinite(weather.windSpeed) && weather.windSpeed > 0) {
    parts.push(`${t("Wind")} ${number(0)(fromMilesPerHour(weather.windSpeed))} ${windSpeedUnit}`);
  }
  if (Number.isFinite(weather.relativeHumidity)) parts.push(`${number(0)(weather.relativeHumidity)} % ${t("humidity")}`);
  return parts.length > 0 ? parts.join(" · ") : null;
}

function deviceName({ manufacturer, product }) {
  const brand = manufacturer ? spaced(manufacturer).replace(/^./, (first) => first.toUpperCase()) : "";
  return [brand, product && spaced(product)].filter(Boolean).join(" ");
}

/** The watch and the sensors, like "Garmin fenix7, Heart rate sensor". */
export function devicesText(devices) {
  if (!devices || devices.length === 0) return null;
  return devices
    .map((device) => {
      const name = deviceName(device);
      if (device.creator || !device.kind) return name;
      const kind = SENSOR_NAMES[device.kind] ? t(SENSOR_NAMES[device.kind]) : spaced(device.kind);
      return name ? `${kind} (${name})` : kind;
    })
    .filter(Boolean)
    .join(", ");
}
