/**
 * A profile of one metric along an activity, like elevation over distance,
 * with a cursor that can be moved along it to scrub through the activity.
 */

import { niceTicks } from "/js/charts.js";

const SVG = "http://www.w3.org/2000/svg";
const HEIGHT = 140;
const MARGIN = { top: 8, right: 8, bottom: 20, left: 48 };
/** The share of samples at each end that may fall outside the axis, like a pause in a run. */
const OUTLIERS = 0.02;

function element(name, attributes = {}, parent = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  parent?.append(node);
  return node;
}

/**
 * Draws the profile into `container` and redraws it when the container resizes.
 *
 * - `x` and `values`: arrays of the same length, with `null` where a sample is missing.
 *   `x` must not decrease.
 * - `xFormat` and `format`: turn an x value and a value into text.
 * - `invert`: puts low values at the top, as for pace, where lower is faster.
 * - `onScrub`: called with the sample index under the pointer, or `null` when it leaves.
 *
 * Returns `setCursor(index)`, which shows the cursor at a sample without calling `onScrub`,
 * and `destroy()`, which stops the redrawing before the container gets another profile.
 */
export function profile(container, { x, values, xFormat, format, invert = false, label, onScrub }) {
  const indices = values.map((_, index) => index).filter((index) => x[index] != null && values[index] != null);
  let cursor = null;
  let scale = null;
  let parts = null;

  function draw() {
    container.replaceChildren();
    const width = container.clientWidth;
    if (width === 0 || indices.length < 2) return;
    const svg = element("svg", { width, height: HEIGHT, viewBox: `0 0 ${width} ${HEIGHT}`, role: "img", "aria-label": label });
    container.append(svg);
    const inner = { left: MARGIN.left, right: width - MARGIN.right, top: MARGIN.top, bottom: HEIGHT - MARGIN.bottom };

    const sorted = indices.map((index) => values[index]).sort((a, b) => a - b);
    const ticks = niceTicks(
      sorted[Math.floor((sorted.length - 1) * OUTLIERS)],
      sorted[Math.ceil((sorted.length - 1) * (1 - OUTLIERS))],
      3,
    );
    const [low, high] = [ticks[0], ticks.at(-1)];
    const [first, last] = [x[indices[0]], x[indices.at(-1)]];
    const span = last - first || 1;
    const px = (value) => inner.left + ((value - first) / span) * (inner.right - inner.left);
    const fraction = (value) => Math.min(1, Math.max(0, (value - low) / (high - low)));
    const py = (value) =>
      invert
        ? inner.top + fraction(value) * (inner.bottom - inner.top)
        : inner.bottom - fraction(value) * (inner.bottom - inner.top);
    scale = { px, py, inner, first, span };

    for (const tick of ticks) {
      const y = py(tick);
      element("line", { x1: inner.left, x2: inner.right, y1: y, y2: y, class: "chart-grid" }, svg);
      const text = element("text", { x: inner.left - 6, y, class: "chart-tick", "text-anchor": "end", "dominant-baseline": "middle" }, svg);
      text.textContent = format(tick);
    }
    const labels = Math.max(2, Math.floor((inner.right - inner.left) / 80));
    for (let index = 0; index < labels; index++) {
      const value = first + (span * index) / (labels - 1);
      const text = element("text", {
        x: px(value),
        y: HEIGHT - 4,
        class: "chart-tick",
        "text-anchor": index === 0 ? "start" : index === labels - 1 ? "end" : "middle",
      }, svg);
      text.textContent = xFormat(value);
    }

    const line = indices.map((index, n) => `${n === 0 ? "M" : "L"}${px(x[index]).toFixed(1)},${py(values[index]).toFixed(1)}`).join("");
    const base = invert ? inner.top : inner.bottom;
    element("path", {
      d: `${line}L${px(last).toFixed(1)},${base}L${px(first).toFixed(1)},${base}Z`,
      class: "profile-area",
    }, svg);
    element("path", { d: line, class: "chart-line profile-line" }, svg);

    parts = {
      line: element("line", { y1: inner.top, y2: inner.bottom, class: "chart-crosshair", visibility: "hidden" }, svg),
      dot: element("circle", { r: 4, class: "chart-dot profile-dot", visibility: "hidden" }, svg),
    };
    svg.addEventListener("pointermove", move);
    svg.addEventListener("pointerdown", move);
    svg.addEventListener("pointerleave", leave);
    // The arrow keys scrub too, a step at a time, or further with Shift.
    svg.setAttribute("tabindex", "0");
    svg.addEventListener("keydown", (event) => {
      const step = { ArrowLeft: -1, ArrowRight: 1 }[event.key];
      if (!step) {
        if (event.key === "Escape") leave();
        return;
      }
      event.preventDefault();
      const at = Math.max(0, indices.indexOf(cursor));
      const next = Math.min(indices.length - 1, Math.max(0, at + step * (event.shiftKey ? 20 : 1)));
      setCursor(indices[next]);
      onScrub?.(indices[next]);
    });
    svg.addEventListener("blur", leave);
    setCursor(cursor);
  }

  /** The shown sample nearest to the pointer. */
  function nearest(clientX) {
    const box = container.getBoundingClientRect();
    const { inner, first, span } = scale;
    const value = first + ((clientX - box.left - inner.left) / (inner.right - inner.left)) * span;
    let [low, high] = [0, indices.length - 1];
    while (low < high) {
      const middle = (low + high) >> 1;
      if (x[indices[middle]] < value) low = middle + 1;
      else high = middle;
    }
    const before = indices[Math.max(0, low - 1)];
    const after = indices[low];
    return Math.abs(x[before] - value) < Math.abs(x[after] - value) ? before : after;
  }

  function move(event) {
    const index = nearest(event.clientX);
    setCursor(index);
    onScrub?.(index);
  }

  function leave() {
    setCursor(null);
    onScrub?.(null);
  }

  function setCursor(index) {
    cursor = index;
    if (!parts) return;
    const visible = index != null && x[index] != null && values[index] != null;
    parts.line.setAttribute("visibility", visible ? "visible" : "hidden");
    parts.dot.setAttribute("visibility", visible ? "visible" : "hidden");
    if (!visible) return;
    const cx = scale.px(x[index]);
    parts.line.setAttribute("x1", cx);
    parts.line.setAttribute("x2", cx);
    parts.dot.setAttribute("cx", cx);
    parts.dot.setAttribute("cy", scale.py(values[index]));
  }

  const observer = new ResizeObserver(draw);
  observer.observe(container);
  return { setCursor, destroy: () => observer.disconnect() };
}
