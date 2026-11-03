/**
 * Design tokens. Every colour and font the grids use is a CSS custom property, so a
 * host restyles the components without touching them. Canvas drawing reads the
 * computed values.
 */

/** The tokens a grid draws with. */
export interface Theme {
  readonly font: string;
  readonly background: string;
  readonly stripe: string;
  readonly foreground: string;
  readonly negative: string;
  readonly totalForeground: string;
  readonly headerBackground: string;
  readonly headerForeground: string;
}

const DEFAULTS: Theme = {
  font: "13px system-ui, sans-serif",
  background: "#ffffff",
  stripe: "#f6f7f9",
  foreground: "#1d2330",
  negative: "#b42318",
  totalForeground: "#0b4ea2",
  headerBackground: "#e8ebf0",
  headerForeground: "#1d2330"
};

const TOKENS: Readonly<Record<keyof Theme, string>> = {
  font: "--px-font",
  background: "--px-grid-bg",
  stripe: "--px-grid-stripe",
  foreground: "--px-grid-fg",
  negative: "--px-negative",
  totalForeground: "--px-total-fg",
  headerBackground: "--px-header-bg",
  headerForeground: "--px-header-fg"
};

/** Reads the tokens in effect for `element`, falling back to the defaults. */
export function readTheme(element: Element): Theme {
  const style = getComputedStyle(element);
  const read = (key: keyof Theme): string => {
    const value = style.getPropertyValue(TOKENS[key]).trim();
    return value === "" ? DEFAULTS[key] : value;
  };
  return {
    font: read("font"),
    background: read("background"),
    stripe: read("stripe"),
    foreground: read("foreground"),
    negative: read("negative"),
    totalForeground: read("totalForeground"),
    headerBackground: read("headerBackground"),
    headerForeground: read("headerForeground")
  };
}
