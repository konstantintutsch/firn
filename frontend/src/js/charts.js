/**
 * Small SVG charts: lines over days and columns over buckets, with a hover
 * tooltip and a table of the values. Colors come from the --series-* tokens.
 */

import { t } from "/js/i18n.js";

const SVG = "http://www.w3.org/2000/svg";
const HEIGHT = 200;
const MARGIN = { top: 10, right: 12, bottom: 24, left: 44 };
const MAX_COLUMN_WIDTH = 24;
const COLUMN_GAP = 2;
const RADIUS = 4;
const DAY_MS = 86_400_000;
/** Points further apart than this are not joined by a line. */
const MAX_LINE_GAP_MS = 2 * DAY_MS;

const charts = new Set();
let resizeTimer = null;
addEventListener("resize", () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => charts.forEach((chart) => chart.draw()), 150);
});

function element(name, attributes = {}, parent = null) {
  const node = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
  parent?.append(node);
  return node;
}

function html(name, className, text) {
  const node = document.createElement(name);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

/** Round tick values that cover `min` to `max`. */
export function niceTicks(min, max, count = 4) {
  if (min === max) {
    const pad = Math.abs(min) * 0.1 || 1;
    min -= pad;
    max += pad;
  }
  const rough = (max - min) / count;
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const step = [1, 2, 2.5, 5, 10].map((factor) => factor * magnitude).find((s) => s >= rough);
  const ticks = [];
  for (let tick = Math.floor(min / step) * step; tick <= max + step * 1e-9; tick += step) {
    ticks.push(Number(tick.toPrecision(12)));
  }
  if (ticks.at(-1) < max) ticks.push(Number((ticks.at(-1) + step).toPrecision(12)));
  return ticks;
}

export function formatDay(time) {
  return new Date(time).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function shortDay(time, spanDays) {
  const options =
    spanDays > 400 ? { month: "short", year: "numeric" } : { day: "numeric", month: "short" };
  return new Date(time).toLocaleDateString(undefined, options);
}

/** A rectangle with 4px rounded corners at the top, square at the baseline. */
function columnPath(x, y, width, height) {
  const r = Math.min(RADIUS, width / 2, height);
  return `M${x},${y + height}V${y + r}Q${x},${y} ${x + r},${y}H${x + width - r}` +
    `Q${x + width},${y} ${x + width},${y + r}V${y + height}Z`;
}

/**
 * Draws a chart into `container` and keeps it drawn at the container's width.
 *
 * - `kind`: "line" (points over time) or "columns" (one column per bucket).
 * - `series`: [{ name, slot, values }], where `slot` picks --series-{slot} and
 *   `values` are [time, value] pairs for lines, or values by bucket for columns.
 * - `buckets`: for columns, [{ time, label }] in order.
 * - `stacked`: stack the columns of all series.
 * - `format`: turns a value into text.
 */
export function chart(container, options) {
  const state = { container, ...options };
  state.draw = () => draw(state);
  container.replaceChildren();
  container.classList.add("chart");

  if (options.series.length > 1) {
    const legend = html("div", "chart-legend");
    for (const { name, slot } of options.series) {
      const item = html("span", "chart-legend-item");
      const swatch = html("span", "chart-swatch");
      swatch.style.background = `var(--series-${slot})`;
      item.append(swatch, name);
      legend.append(item);
    }
    container.append(legend);
  }
  state.plot = html("div", "chart-plot");
  state.tooltip = html("div", "chart-tooltip");
  state.tooltip.hidden = true;
  state.plot.append(state.tooltip);
  container.append(state.plot, dataTable(options));
  charts.add(state);
  state.draw();
}

/** Forgets charts that are no longer on the page. */
export function forgetCharts(root) {
  for (const state of charts) {
    if (root.contains(state.container)) charts.delete(state);
  }
}

function dataTable({ kind, series, buckets, format }) {
  const details = html("details", "chart-data");
  details.append(html("summary", null, t("Show values")));
  details.addEventListener("toggle", () => {
    if (!details.open || details.querySelector("table")) return;
    const table = html("table");
    const head = html("tr");
    head.append(html("th", null, t(kind === "line" ? "Day" : "Period")));
    for (const { name } of series) head.append(html("th", null, name));
    table.append(head);
    const rows = kind === "line" ? lineRows(series) : buckets.map((bucket, index) => [
      bucket.label,
      series.map(({ values }) => values[index]),
    ]);
    for (const [label, values] of rows) {
      const row = html("tr");
      row.append(html("td", null, label));
      for (const value of values) row.append(html("td", null, value == null ? "–" : format(value)));
      table.append(row);
    }
    details.append(table);
  });
  return details;
}

/** One row per day that any series has a value on, newest first. */
function lineRows(series) {
  const days = new Map();
  series.forEach(({ values }, index) => {
    for (const [time, value] of values) {
      if (!days.has(time)) days.set(time, new Array(series.length).fill(null));
      days.get(time)[index] = value;
    }
  });
  return [...days.entries()]
    .sort((a, b) => b[0] - a[0])
    .map(([time, values]) => [formatDay(time), values]);
}

function draw(state) {
  const { plot, kind } = state;
  const width = plot.clientWidth;
  if (width === 0) return;
  plot.querySelector("svg")?.remove();
  const svg = element("svg", {
    width,
    height: HEIGHT,
    viewBox: `0 0 ${width} ${HEIGHT}`,
    role: "img",
    "aria-label": state.label ?? "",
  });
  plot.prepend(svg);
  const inner = {
    left: MARGIN.left,
    right: width - MARGIN.right,
    top: MARGIN.top,
    bottom: HEIGHT - MARGIN.bottom,
  };
  if (kind === "line") drawLines(state, svg, inner);
  else drawColumns(state, svg, inner);
}

function drawYAxis(svg, inner, ticks, y, format) {
  for (const tick of ticks) {
    const ty = y(tick);
    element("line", { x1: inner.left, x2: inner.right, y1: ty, y2: ty, class: "chart-grid" }, svg);
    const label = element("text", { x: inner.left - 6, y: ty, class: "chart-tick", "text-anchor": "end", "dominant-baseline": "middle" }, svg);
    label.textContent = format(tick);
  }
}

/** The smallest and largest number. A loop, since spreading many values into Math.min can overflow the stack. */
function extent(numbers) {
  let low = Infinity;
  let high = -Infinity;
  for (const number of numbers) {
    if (number < low) low = number;
    if (number > high) high = number;
  }
  return [low, high];
}

function drawLines(state, svg, inner) {
  const { series, format } = state;
  const all = series.flatMap(({ values }) => values);
  if (all.length === 0) return;
  const times = all.map(([time]) => time);
  const [minTime, maxTime] = extent(times);
  const ticks = niceTicks(...extent(all.map(([, value]) => value)));
  const [low, high] = [ticks[0], ticks.at(-1)];
  const span = Math.max(maxTime - minTime, DAY_MS);
  const x = (time) => inner.left + ((time - minTime) / span) * (inner.right - inner.left);
  const y = (value) => inner.bottom - ((value - low) / (high - low)) * (inner.bottom - inner.top);

  drawYAxis(svg, inner, ticks, y, format);
  const spanDays = span / DAY_MS;
  const labels = Math.max(2, Math.floor((inner.right - inner.left) / 90));
  for (let index = 0; index < labels; index++) {
    const time = minTime + (span * index) / (labels - 1);
    const label = element("text", {
      x: x(time),
      y: HEIGHT - 6,
      class: "chart-tick",
      "text-anchor": index === 0 ? "start" : index === labels - 1 ? "end" : "middle",
    }, svg);
    label.textContent = shortDay(time, spanDays);
  }

  for (const { slot, values: points } of series) {
    // Breaks the line where days are missing, instead of drawing across gaps.
    const maxGap = state.maxGap ?? MAX_LINE_GAP_MS;
    const joins = (index) => index > 0 && points[index][0] - points[index - 1][0] <= maxGap;
    let path = "";
    points.forEach(([time, value], index) => {
      path += `${joins(index) ? "L" : "M"}${x(time).toFixed(1)},${y(value).toFixed(1)}`;
    });
    element("path", { d: path, class: "chart-line", stroke: `var(--series-${slot})` }, svg);
    points.forEach(([time, value], index) => {
      // A point that no line reaches gets a larger dot, so that it does not get lost.
      const alone = !joins(index) && !(index + 1 < points.length && joins(index + 1));
      if (!alone && points.length > 60) return;
      const attributes = { cx: x(time), cy: y(value), r: alone ? 4 : 2.5, fill: `var(--series-${slot})` };
      element("circle", alone ? { ...attributes, class: "chart-point-alone" } : attributes, svg);
    });
  }

  const crosshair = element("line", { y1: inner.top, y2: inner.bottom, class: "chart-crosshair", visibility: "hidden" }, svg);
  const dots = series.map(({ slot }) =>
    element("circle", { r: 4, fill: `var(--series-${slot})`, class: "chart-dot", visibility: "hidden" }, svg),
  );
  const sortedTimes = [...new Set(times)].sort((a, b) => a - b);
  const lookups = series.map(({ values: points }) => new Map(points));

  hover(state, svg, (px) => {
    const target = minTime + ((px - inner.left) / (inner.right - inner.left)) * span;
    const time = nearest(sortedTimes, target);
    const cx = x(time);
    crosshair.setAttribute("x1", cx);
    crosshair.setAttribute("x2", cx);
    crosshair.setAttribute("visibility", "visible");
    const rows = [];
    series.forEach(({ name, slot }, index) => {
      const value = lookups[index].get(time);
      dots[index].setAttribute("visibility", value == null ? "hidden" : "visible");
      if (value == null) return;
      dots[index].setAttribute("cx", cx);
      dots[index].setAttribute("cy", y(value));
      rows.push({ name, slot, value: format(value) });
    });
    return { x: cx, title: formatDay(time), rows };
  }, () => {
    crosshair.setAttribute("visibility", "hidden");
    dots.forEach((dot) => dot.setAttribute("visibility", "hidden"));
  });
}

function nearest(sorted, target) {
  let low = 0;
  let high = sorted.length - 1;
  while (low < high) {
    const middle = (low + high) >> 1;
    if (sorted[middle] < target) low = middle + 1;
    else high = middle;
  }
  if (low > 0 && target - sorted[low - 1] < sorted[low] - target) return sorted[low - 1];
  return sorted[low];
}

function drawColumns(state, svg, inner) {
  const { series, buckets, format, stacked } = state;
  const totals = buckets.map((_, index) =>
    stacked
      ? series.reduce((sum, { values }) => sum + (values[index] ?? 0), 0)
      : Math.max(0, ...series.map(({ values }) => values[index] ?? 0)),
  );
  const ticks = niceTicks(0, Math.max(...totals, 0));
  const high = ticks.at(-1);
  const y = (value) => inner.bottom - (value / high) * (inner.bottom - inner.top);
  drawYAxis(svg, inner, ticks, y, format);

  const band = (inner.right - inner.left) / buckets.length;
  const groups = stacked ? 1 : series.length;
  const columnWidth = Math.max(1, Math.min(MAX_COLUMN_WIDTH, (band * 0.7) / groups));
  const labelEvery = Math.ceil(buckets.length / Math.max(1, Math.floor((inner.right - inner.left) / 70)));

  buckets.forEach((bucket, index) => {
    const center = inner.left + band * (index + 0.5);
    let base = 0;
    series.forEach(({ slot, values }, seriesIndex) => {
      const value = values[index] ?? 0;
      if (value <= 0) return;
      const left = stacked
        ? center - columnWidth / 2
        : center - (columnWidth * groups) / 2 + columnWidth * seriesIndex;
      const top = y(base + value);
      // The gap between stacked segments is the surface showing through.
      const bottom = y(base) - (stacked && base > 0 ? COLUMN_GAP : 0);
      const height = Math.max(0, bottom - top);
      const isTop = !stacked || seriesIndex === lastSegment(series, index);
      element("path", {
        d: isTop ? columnPath(left, top, columnWidth - (stacked ? 0 : 1), height) : `M${left},${top}h${columnWidth}v${height}h${-columnWidth}Z`,
        fill: `var(--series-${slot})`,
      }, svg);
      if (stacked) base += value;
    });
    if (index % labelEvery === 0) {
      const label = element("text", { x: center, y: HEIGHT - 6, class: "chart-tick", "text-anchor": "middle" }, svg);
      label.textContent = bucket.short ?? bucket.label;
    }
  });
  element("line", { x1: inner.left, x2: inner.right, y1: inner.bottom, y2: inner.bottom, class: "chart-baseline" }, svg);

  const highlight = element("rect", { y: inner.top, height: inner.bottom - inner.top, width: band, class: "chart-highlight", visibility: "hidden" }, svg);
  svg.prepend(highlight);
  hover(state, svg, (px) => {
    const index = Math.min(buckets.length - 1, Math.max(0, Math.floor((px - inner.left) / band)));
    highlight.setAttribute("x", inner.left + band * index);
    highlight.setAttribute("visibility", "visible");
    const rows = series
      .filter(({ values }) => values[index] != null)
      .map(({ name, slot, values }) => ({ name, slot, value: format(values[index]) }));
    if (stacked && series.length > 1) rows.push({ name: t("Total"), value: format(totals[index]) });
    return { x: inner.left + band * (index + 0.5), title: buckets[index].label, rows };
  }, () => highlight.setAttribute("visibility", "hidden"));
}

function lastSegment(series, index) {
  for (let seriesIndex = series.length - 1; seriesIndex >= 0; seriesIndex--) {
    if ((series[seriesIndex].values[index] ?? 0) > 0) return seriesIndex;
  }
  return -1;
}

function hover(state, svg, show, hide) {
  const { tooltip, plot } = state;
  const move = (event) => {
    const box = svg.getBoundingClientRect();
    const px = event.clientX - box.left;
    if (px < MARGIN.left || px > box.width - MARGIN.right) return leave();
    const { x, title, rows } = show(px);
    tooltip.replaceChildren(html("strong", null, title));
    for (const row of rows) {
      const line = html("div", "chart-tooltip-row");
      if (row.slot) {
        const swatch = html("span", "chart-swatch");
        swatch.style.background = `var(--series-${row.slot})`;
        line.append(swatch);
      }
      line.append(html("span", null, row.name), html("span", "chart-tooltip-value", row.value));
      tooltip.append(line);
    }
    tooltip.hidden = false;
    const width = tooltip.offsetWidth;
    const left = x + 12 + width > plot.clientWidth ? x - 12 - width : x + 12;
    tooltip.style.left = `${Math.max(0, left)}px`;
  };
  const leave = () => {
    tooltip.hidden = true;
    hide();
  };
  svg.addEventListener("pointermove", move);
  svg.addEventListener("pointerdown", move);
  svg.addEventListener("pointerleave", leave);
}

/** Horizontal bars with the value at each tip, for a few named amounts. */
export function bars(container, items, format) {
  container.replaceChildren();
  container.classList.add("bars");
  const max = Math.max(...items.map(({ value }) => value), 0) || 1;
  for (const { label, value, slot, color } of items) {
    const row = html("div", "bar-row");
    const track = html("div", "bar-track");
    const bar = html("div", "bar");
    // Leaves room at the tip for the value.
    bar.style.width = `calc((100% - 64px) * ${value / max})`;
    bar.style.background = color ?? `var(--series-${slot})`;
    track.append(bar, html("span", "bar-value", format(value)));
    row.append(html("span", "bar-label", label), track);
    container.append(row);
  }
}
