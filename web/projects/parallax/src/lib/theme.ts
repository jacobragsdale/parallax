/**
 * Design tokens. Every colour and font the grid uses is a CSS custom property,
 * defined once here for the light and dark themes. The values are emitted into a
 * cascade layer, so any rule a host writes for `px-grid` overrides them whatever
 * its specificity. Canvas drawing reads the computed values on every full redraw.
 */

/** The tokens a grid draws with. */
export interface Theme {
  readonly font: string;
  readonly background: string;
  readonly stripe: string;
  readonly foreground: string;
  readonly muted: string;
  readonly negative: string;
  readonly totalForeground: string;
  readonly headerBackground: string;
  readonly headerForeground: string;
  readonly gridLine: string;
  readonly selection: string;
  readonly selectionBorder: string;
  readonly statusBackground: string;
  readonly statusForeground: string;
  readonly warning: string;
  readonly danger: string;
}

/** The themes the library ships. */
export type ThemeName = "light" | "dark";

/** The CSS custom property behind each token. */
export const TOKENS: Readonly<Record<keyof Theme, string>> = {
  font: "--px-font",
  background: "--px-grid-bg",
  stripe: "--px-grid-stripe",
  foreground: "--px-grid-fg",
  muted: "--px-muted-fg",
  negative: "--px-negative",
  totalForeground: "--px-total-fg",
  headerBackground: "--px-header-bg",
  headerForeground: "--px-header-fg",
  gridLine: "--px-grid-line",
  selection: "--px-selection-bg",
  selectionBorder: "--px-selection-border",
  statusBackground: "--px-status-bg",
  statusForeground: "--px-status-fg",
  warning: "--px-warning",
  danger: "--px-danger"
};

const FONT = "13px system-ui, -apple-system, 'Segoe UI', sans-serif";

/** Token values by theme. */
export const THEMES: Readonly<Record<ThemeName, Theme>> = {
  light: {
    font: FONT,
    background: "#ffffff",
    stripe: "#f6f7f9",
    foreground: "#1d2330",
    muted: "#667085",
    negative: "#b42318",
    totalForeground: "#0b4ea2",
    headerBackground: "#e8ebf0",
    headerForeground: "#1d2330",
    gridLine: "#e3e6eb",
    selection: "rgba(11, 78, 162, 0.14)",
    selectionBorder: "#0b4ea2",
    statusBackground: "#f2f4f7",
    statusForeground: "#344054",
    warning: "#b54708",
    danger: "#b42318"
  },
  dark: {
    font: FONT,
    background: "#16181d",
    stripe: "#1c1f26",
    foreground: "#e3e6ec",
    muted: "#8a93a3",
    negative: "#ff7b72",
    totalForeground: "#7cb7ff",
    headerBackground: "#23262e",
    headerForeground: "#e3e6ec",
    gridLine: "#2b2f38",
    selection: "rgba(91, 155, 255, 0.25)",
    selectionBorder: "#5b9bff",
    statusBackground: "#1c1f26",
    statusForeground: "#b8bfcc",
    warning: "#e0a400",
    danger: "#ff7b72"
  }
};

/** Returns `keys`, failing to compile unless it names every token. */
function everyToken<const T extends readonly (keyof Theme)[]>(keys: T & (Exclude<keyof Theme, T[number]> extends never ? unknown : never)): T {
  return keys;
}

const KEYS = everyToken([
  "font",
  "background",
  "stripe",
  "foreground",
  "muted",
  "negative",
  "totalForeground",
  "headerBackground",
  "headerForeground",
  "gridLine",
  "selection",
  "selectionBorder",
  "statusBackground",
  "statusForeground",
  "warning",
  "danger"
]);

function declarations(theme: Theme): string {
  return KEYS.map((key) => `${TOKENS[key]}: ${theme[key]};`).join(" ");
}

/** The stylesheet that defines the tokens for each theme, in the `parallax` layer. */
export const THEME_STYLES = `@layer parallax { px-grid, px-pivot { ${declarations(THEMES.light)} } px-grid[data-px-theme="dark"], px-pivot[data-px-theme="dark"] { ${declarations(THEMES.dark)} } }`;

const STYLE_ID = "px-theme-tokens";

/** Adds the token stylesheet to a document once. Angular needs component styles to be static, so the generated sheet is added here. */
export function installThemeStyles(document: Document): void {
  if (document.getElementById(STYLE_ID) !== null) {
    return;
  }
  const style = document.createElement("style");
  style.id = STYLE_ID;
  style.textContent = THEME_STYLES;
  document.head.append(style);
}

/** Reads the tokens in effect for `element`, falling back to the light theme. */
export function readTheme(element: Element): Theme {
  const style = getComputedStyle(element);
  const read = (key: keyof Theme): string => {
    const value = style.getPropertyValue(TOKENS[key]).trim();
    return value === "" ? THEMES.light[key] : value;
  };
  return {
    font: read("font"),
    background: read("background"),
    stripe: read("stripe"),
    foreground: read("foreground"),
    muted: read("muted"),
    negative: read("negative"),
    totalForeground: read("totalForeground"),
    headerBackground: read("headerBackground"),
    headerForeground: read("headerForeground"),
    gridLine: read("gridLine"),
    selection: read("selection"),
    selectionBorder: read("selectionBorder"),
    statusBackground: read("statusBackground"),
    statusForeground: read("statusForeground"),
    warning: read("warning"),
    danger: read("danger")
  };
}
