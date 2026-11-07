/**
 * Bends the backdrop along the rim of each glass surface, like the edge of a lens. Each
 * surface gets a displacement map drawn for its own size, corner radius and corner shape.
 * Only Chromium applies SVG filters in backdrop-filter, so other browsers keep the plain blur.
 */

const GLASS = ".panel, .nav, .settings, .tile, .card, .activity-list, .chart-tooltip, .maplibregl-ctrl-group";
// The widest band along the rim that refracts, in pixels.
const BAND_PX = 22;
// The largest shift at the very edge, as a share of the band.
const BEND = 0.55;
// Each color bends a little less than the one before, which fringes bright edges like real glass.
const CHANNELS = [
  ["1 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0", 1],
  ["0 0 0 0 0  0 1 0 0 0  0 0 0 0 0  0 0 0 1 0", 0.94],
  ["0 0 0 0 0  0 0 0 0 0  0 0 1 0 0  0 0 0 1 0", 0.88],
];
const SVG_NS = "http://www.w3.org/2000/svg";

const filters = document.createElementNS(SVG_NS, "svg");
filters.setAttribute("class", "glass-filter");
filters.setAttribute("aria-hidden", "true");

let nextId = 0;
const lenses = new Map();
const resizes = new ResizeObserver((entries) => {
  for (const { target } of entries) update(target);
});

if (navigator.userAgentData) {
  document.body.append(filters);
  watch(document.body);
  new MutationObserver((records) => {
    for (const record of records) record.addedNodes.forEach(watch);
  }).observe(document.body, { childList: true, subtree: true });
}

function watch(node) {
  if (!(node instanceof Element)) return;
  const found = [...node.querySelectorAll(GLASS)];
  if (node.matches(GLASS)) found.push(node);
  for (const element of found) {
    if (lenses.has(element)) continue;
    lenses.set(element, null);
    resizes.observe(element);
  }
}

/** Redraws the lens of a surface after it changed size, and drops it once it is gone. */
function update(element) {
  const width = element.offsetWidth;
  const height = element.offsetHeight;
  let lens = lenses.get(element);
  if (width === 0 || height === 0) {
    lens?.filter.remove();
    lenses.set(element, null);
    element.style.removeProperty("--glass-lens");
    return;
  }

  const style = getComputedStyle(element);
  const radius = Math.min(parseFloat(style.borderTopLeftRadius) || 0, width / 2, height / 2);
  // A corner of shape superellipse(k) follows |x|ⁿ + |y|ⁿ = rⁿ with n = 2ᵏ: round is k = 1, squircle k = 2.
  const shape = style.getPropertyValue("corner-top-left-shape");
  const k = shape === "squircle" ? 2 : Number(shape.match(/superellipse\(([\d.]+)\)/)?.[1] ?? 1);
  const exponent = 2 ** Math.max(1, k);
  const key = `${width} ${height} ${radius} ${exponent}`;
  if (lens?.key === key) return;

  if (!lens) {
    const id = `glass-lens-${nextId++}`;
    const filter = document.createElementNS(SVG_NS, "filter");
    filter.id = id;
    filter.setAttribute("filterUnits", "userSpaceOnUse");
    filter.setAttribute("primitiveUnits", "userSpaceOnUse");
    filter.setAttribute("color-interpolation-filters", "sRGB");
    const map = document.createElementNS(SVG_NS, "feImage");
    map.setAttribute("result", "map");
    map.setAttribute("preserveAspectRatio", "none");
    filter.append(map);
    CHANNELS.forEach(([matrix], index) => {
      const shift = document.createElementNS(SVG_NS, "feDisplacementMap");
      shift.setAttribute("in", "SourceGraphic");
      shift.setAttribute("in2", "map");
      shift.setAttribute("xChannelSelector", "R");
      shift.setAttribute("yChannelSelector", "G");
      const color = document.createElementNS(SVG_NS, "feColorMatrix");
      color.setAttribute("values", matrix);
      color.setAttribute("result", `color${index}`);
      filter.append(shift, color);
    });
    for (const index of [1, 2]) {
      const sum = document.createElementNS(SVG_NS, "feComposite");
      sum.setAttribute("in", index === 1 ? "color0" : "sum");
      sum.setAttribute("in2", `color${index}`);
      sum.setAttribute("operator", "arithmetic");
      sum.setAttribute("k2", 1);
      sum.setAttribute("k3", 1);
      sum.setAttribute("result", "sum");
      filter.append(sum);
    }
    filters.append(filter);
    lens = { filter };
    element.style.setProperty("--glass-lens", `url(#${id})`);
  }
  lens.key = key;
  lenses.set(element, lens);

  const band = Math.min(BAND_PX, width / 2, height / 2);
  const map = lens.filter.querySelector("feImage");
  for (const node of [lens.filter, map]) {
    node.setAttribute("x", 0);
    node.setAttribute("y", 0);
    node.setAttribute("width", width);
    node.setAttribute("height", height);
  }
  map.setAttribute("href", displacementMap(width, height, radius, exponent, band));
  lens.filter.querySelectorAll("feDisplacementMap").forEach((shift, index) => {
    shift.setAttribute("scale", 2 * band * BEND * CHANNELS[index][1]);
  });
}

/**
 * Draws where each pixel samples the backdrop from: red shifts it horizontally and green
 * vertically, with 128 for no shift. Near the rim, pixels sample from further inside, as
 * light does through a rounded bevel, more steeply the closer they are to the edge.
 */
function displacementMap(width, height, radius, exponent, band) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  const image = context.createImageData(width, height);
  const data = image.data;

  for (let y = 0; y < height; y++) {
    const py = y + 0.5;
    // Offset from the inner rectangle that the corners curve around.
    const cy = py < radius ? py - radius : py > height - radius ? py - (height - radius) : 0;
    for (let x = 0; x < width; x++) {
      const px = x + 0.5;
      const cx = px < radius ? px - radius : px > width - radius ? px - (width - radius) : 0;
      let distance, nx, ny;
      if (cx !== 0 && cy !== 0) {
        const ax = Math.abs(cx);
        const ay = Math.abs(cy);
        distance = radius - (ax ** exponent + ay ** exponent) ** (1 / exponent);
        nx = Math.sign(cx) * ax ** (exponent - 1);
        ny = Math.sign(cy) * ay ** (exponent - 1);
      } else {
        const edges = [
          [px, -1, 0],
          [width - px, 1, 0],
          [py, 0, -1],
          [height - py, 0, 1],
        ];
        [distance, nx, ny] = edges.reduce((nearest, edge) => (edge[0] < nearest[0] ? edge : nearest));
      }

      let dx = 0;
      let dy = 0;
      if (distance < band) {
        // The bevel's profile is a quarter circle: flat inside, steep at the edge.
        const t = Math.min(1, 1 - distance / band);
        const strength = 1 - Math.sqrt(1 - t * t);
        const length = Math.hypot(nx, ny) || 1;
        dx = (-nx / length) * strength;
        dy = (-ny / length) * strength;
      }
      const i = (y * width + x) * 4;
      data[i] = 128 + dx * 127;
      data[i + 1] = 128 + dy * 127;
      data[i + 2] = 128;
      data[i + 3] = 255;
    }
  }
  context.putImageData(image, 0, 0);
  return canvas.toDataURL();
}
