# @jacobragsdale/parallax

Angular components for the parallax engine: a data grid and pivot table drawn
on a Canvas 2D surface, for desktops without a GPU. The design and its
correctness rules are in [How parallax is designed](../../../docs/design.md).

## Show a view

Provide the connection once, then bind a layout to a grid:

```ts
// app.config.ts
import { provideParallax } from "@jacobragsdale/parallax";

export const appConfig: ApplicationConfig = { providers: [provideParallax({ url: "/ws" })] };
```

```ts
// positions.component.ts
import { GridComponent } from "@jacobragsdale/parallax";
import type { LayoutInput } from "@jacobragsdale/parallax";

@Component({ selector: "app-positions", imports: [GridComponent], template: `<px-grid [(layout)]="layout" theme="dark" />` })
export class PositionsComponent {
  protected readonly layout = signal<LayoutInput>({
    dataset: "positions",
    group_by: ["fund", "strategy"],
    measures: [{ column: "market_value_usd", aggregate: "sum" }],
    formats: { market_value_usd: { abbreviate: "millions", negative: "parentheses" } }
  });
}
```

`url` is the reference server's WebSocket endpoint. A path such as `/ws` is
resolved against the page. The grid fills its container, so give the container
a height.

The two-way binding keeps the layout current as the user sorts, resizes and
expands groups, so saving the layout saves the view. `parseLayout` checks a
layout read from storage or a URL before it is used.

## Show a pivot with its settings

`<px-pivot>` takes the same layout and adds a settings panel beside the grid:
row and column groups, values, filters picked from the column's values, open
levels and top-N. Double-clicking a value opens the rows behind its group.

```html
<px-pivot [(layout)]="layout" />
```

## Share a view

`encodeLayout(layout)` returns text for a URL parameter, and
`decodeLayout(text)` reads it back, refusing anything the schema rejects.

## Inputs

| Input     | Type                         | Default     | Meaning                          |
| --------- | ---------------------------- | ----------- | -------------------------------- |
| `layout`  | `LayoutInput`                | required    | The view. Two-way: `[(layout)]`. |
| `theme`   | `"light" \| "dark"`          | `"light"`   | The token set.                   |
| `density` | `"compact" \| "comfortable"` | `"compact"` | Row height: 24 or 30 pixels.     |

## Keys

| Key                                   | Action                                                       |
| ------------------------------------- | ------------------------------------------------------------ |
| Arrows, Page Up, Page Down, Home, End | Move the selection; Shift extends it, Ctrl jumps to the edge |
| Ctrl+A                                | Select every cell                                            |
| Ctrl+C                                | Copy the selection as tab-separated text with every digit    |
| Enter or Space on a group             | Expand or collapse it                                        |
| Escape                                | Clear the selection                                          |

## Theme tokens

Every colour and the font are CSS custom properties, such as `--px-grid-bg`,
`--px-negative` and `--px-selection-bg`; `TOKENS` lists them all. Set any of
them on `px-grid` to override the theme:

```css
px-grid {
  --px-negative: #c00;
}
```
