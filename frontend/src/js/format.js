/** Number formats and element helpers that the pages share. */

import { language, t } from "/js/i18n.js";
import { paceUnit } from "/js/units.js";

const SVG = "http://www.w3.org/2000/svg";

const twoDigits = (value) => String(value).padStart(2, "0");

/** A number with exactly `digits` decimals, in the page's language. */
export const number = (digits = 0) => (value) =>
  value.toLocaleString(language, { maximumFractionDigits: digits, minimumFractionDigits: digits });

/** A number with a translated unit after it. */
export const unit = (digits, suffix) => (value) => `${number(digits)(value)} ${t(suffix)}`;

/** Seconds as hours, minutes and seconds, or minutes and seconds under an hour. */
export function duration(seconds) {
  const total = Math.round(seconds);
  const [hours, minutes] = [Math.floor(total / 3600), Math.floor((total % 3600) / 60)];
  return hours > 0 ? `${hours}:${twoDigits(minutes)}:${twoDigits(total % 60)}` : `${minutes}:${twoDigits(total % 60)}`;
}

/** Minutes per kilometer or mile, as minutes and seconds. */
export function pace(minutes) {
  const seconds = Math.round(minutes * 60);
  return `${Math.floor(seconds / 60)}:${twoDigits(seconds % 60)} ${paceUnit}`;
}

export function html(name, className, text) {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/** An SVG element with `attributes`, appended to `parent` if there is one. */
export function svgElement(name, attributes = {}, parent = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  parent?.append(node);
  return node;
}
