/**
 * Units for distance, speed, pace and elevation, metric or imperial as chosen
 * in the settings. Values from the API are always metric, and only turn into
 * the chosen units here, when they are shown.
 */

const STORAGE_KEY = "units";

function chosen() {
  try {
    return localStorage.getItem(STORAGE_KEY) === "imperial" ? "imperial" : "metric";
  } catch {
    return "metric";
  }
}

export const units = chosen();
const imperial = units === "imperial";

export function setUnits(next) {
  try {
    if (next === "imperial") localStorage.setItem(STORAGE_KEY, next);
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Without storage the choice is lost, and metric stays.
  }
}

/** Meters in one kilometer or mile, the unit of distances. */
export const DISTANCE_M = imperial ? 1609.344 : 1000;
/** "km" or "mi". */
export const distanceUnit = imperial ? "mi" : "km";
export const elevationUnit = imperial ? "ft" : "m";
export const speedUnit = imperial ? "mph" : "km/h";
/** "/km" or "/mi". */
export const paceUnit = `/${distanceUnit}`;

/** Kilometers or miles. */
export const toDistance = (meters) => meters / DISTANCE_M;
/** Meters or feet. */
export const toElevation = (meters) => (imperial ? meters / 0.3048 : meters);
/** Kilometers or miles per hour. */
export const toSpeed = (metersPerSecond) => (metersPerSecond * 3600) / DISTANCE_M;
/** Minutes per kilometer or mile. */
export const toPace = (metersPerSecond) => DISTANCE_M / metersPerSecond / 60;
