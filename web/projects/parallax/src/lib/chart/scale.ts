/**
 * Scales and value-axis ticks for charts.
 *
 * The engine places and labels the x axis. The y axis spans values the browser
 * already holds, so its round-number ticks are made here. They label the axis,
 * not data: every data value a reader sees comes as the engine's text.
 */

/** Maps values to pixels and back. */
export interface Scale {
  readonly domain: readonly [number, number];
  readonly range: readonly [number, number];
  readonly log: boolean;
  readonly map: (value: number) => number;
  readonly invert: (px: number) => number;
}

/** A linear scale from `domain` onto `range`. */
export function linearScale(domain: readonly [number, number], range: readonly [number, number]): Scale {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  const span = d1 - d0 === 0 ? 1 : d1 - d0;
  return { domain, range, log: false, map: (value) => r0 + ((value - d0) / span) * (r1 - r0), invert: (px) => d0 + ((px - r0) / (r1 - r0 === 0 ? 1 : r1 - r0)) * span };
}

/** A base-10 logarithmic scale from a positive `domain` onto `range`. Values at or below zero map to the range's start. */
export function logScale(domain: readonly [number, number], range: readonly [number, number]): Scale {
  const [l0, l1] = [Math.log10(domain[0]), Math.log10(domain[1])];
  const inner = linearScale([l0, l1], range);
  return { domain, range, log: true, map: (value) => (value > 0 ? inner.map(Math.log10(value)) : range[0]), invert: (px) => 10 ** inner.invert(px) };
}

/** A round step of 1, 2 or 5 times a power of ten near `span / count`. */
function niceStep(span: number, count: number): number {
  const raw = span / Math.max(1, count);
  const power = 10 ** Math.floor(Math.log10(raw));
  return [1, 2, 5, 10].map((m) => m * power).find((s) => s >= raw) ?? 10 * power;
}

/**
 * A domain that covers `min..max` and ends on round steps, about `count` of them. With `zero`, the domain also
 * reaches zero, as bars and areas, which grow from a baseline, need.
 */
export function niceDomain(min: number, max: number, count: number, zero: boolean): readonly [number, number] {
  let [low, high] = [zero ? Math.min(0, min) : min, zero ? Math.max(0, max) : max];
  if (!Number.isFinite(low) || !Number.isFinite(high)) {
    return [0, 1];
  }
  if (low === high) {
    [low, high] = low === 0 ? [0, 1] : [low - Math.abs(low) / 2, high + Math.abs(high) / 2];
  }
  const step = niceStep(high - low, count);
  return [Math.floor(low / step) * step, Math.ceil(high / step) * step];
}

/** A positive domain for a log scale covering `min..max`, widened to whole powers of ten. */
export function logDomain(min: number, max: number): readonly [number, number] {
  if (!(min > 0) || !(max > 0)) {
    return [1, 10];
  }
  const low = 10 ** Math.floor(Math.log10(min));
  const high = 10 ** Math.ceil(Math.log10(max));
  return [low, high === low ? low * 10 : high];
}

/** A tick on a value axis. */
export interface ValueTick {
  readonly value: number;
  readonly px: number;
  readonly label: string;
}

const SUFFIXES: readonly (readonly [number, string])[] = [
  [1e12, "T"],
  [1e9, "B"],
  [1e6, "M"],
  [1e3, "K"]
];

/**
 * Labels for an axis's tick values, one style per axis: in full while the axis stays under 10,000, compact (K, M,
 * B, T) once it reaches it; with as many decimals as the step needs.
 */
export function tickLabels(values: readonly number[], step: number): readonly string[] {
  const largest = Math.max(0, ...values.map((v) => Math.abs(v)));
  const [divisor, suffix] = largest >= 1e4 ? (SUFFIXES.find(([d]) => largest >= d) ?? [1, ""]) : [1, ""];
  // Ticks are multiples of the step, so the step's own decimals show each exactly.
  const scaled = step / divisor;
  let decimals = 0;
  while (decimals < 12 && Math.abs(Math.round(scaled * 10 ** decimals) - scaled * 10 ** decimals) > 1e-9 * 10 ** decimals) {
    decimals += 1;
  }
  const format = new Intl.NumberFormat("en-US", { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
  return values.map((v) => (v === 0 ? "0" : `${format.format(v / divisor)}${suffix}`));
}

/** About `count` round ticks across a scale's domain: steps of 1, 2 or 5 on a linear scale, powers of ten on a log one. */
export function valueTicks(scale: Scale, count: number): readonly ValueTick[] {
  const [low, high] = scale.domain;
  if (scale.log) {
    const values: number[] = [];
    for (let p = Math.ceil(Math.log10(low) - 1e-9); p <= Math.floor(Math.log10(high) + 1e-9); p += 1) {
      values.push(10 ** p);
    }
    // One style for the axis: compact throughout once it reaches 10,000, where every power from 1,000 takes a suffix.
    const compact = (values[values.length - 1] ?? 0) >= 1e4;
    const labels = values.map((v) => (compact && v >= 1e3 ? (tickLabels([v, 1e4], v)[0] ?? "") : (tickLabels([v], v)[0] ?? "")));
    return values.map((value, i) => ({ value, px: scale.map(value), label: labels[i] ?? "" }));
  }
  const step = niceStep(high - low, count);
  const values: number[] = [];
  for (let i = Math.ceil(low / step - 1e-9); i <= Math.floor(high / step + 1e-9); i += 1) {
    values.push(Math.abs(i * step) < step * 1e-9 ? 0 : i * step);
  }
  const labels = tickLabels(values, step);
  return values.map((value, i) => ({ value, px: scale.map(value), label: labels[i] ?? "" }));
}
