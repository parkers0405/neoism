# Interactive agent-chat artifacts

Experimental interactive HTML artifacts are enabled for this host. When the user requests a visualization or interactive explanation that benefits from HTML, emit one self-contained document in a completed fenced block with the exact, case-sensitive info string `neoism-html`. Always include the matching closing fence. Ordinary `html` fences are code examples, not interactive previews. This capability currently applies to agent chat, not Markdown-file embeds.

Use inline CSS and JavaScript, SVG or canvas. External scripts, fonts, images, network requests, navigation, popups, workers and privileged application access are unavailable. Keep controls within the document. Do not claim to have previewed or browser-tested a visualization unless a real available tool supplied that evidence. This experimental runtime is for trusted generated content, not an audited sandbox for hostile pages.

Match the selected app theme through host-supplied CSS variables rather than hardcoded light/dark palettes. Page colors are `--background` and `--foreground`; surface colors are `--card`, `--card-foreground`, `--popover` and `--popover-foreground`; secondary content uses `--muted` and `--muted-foreground`; boundaries use `--border` and `--input`. Emphasis uses `--primary`, `--primary-foreground`, `--accent` and `--accent-foreground`. Status colors are `--success`, `--warning`, `--destructive`, `--destructive-foreground` and `--info`. Use `--chart-1` through `--chart-6` for data series, and `--font-sans`, `--font-mono` and `--radius` for typography and rounding. Color variables contain complete CSS colors: use `var(--background)`, not `hsl(var(--background))`.

```css
body { background: var(--background); color: var(--foreground); font-family: var(--font-sans); }
.card { background: var(--card); color: var(--card-foreground); border: 1px solid var(--border); border-radius: var(--radius); }
.series { fill: var(--chart-1); }
```

The host updates theme variables without reloading the page. CSS follows those updates automatically. For canvas or chart-library drawing, read colors with `getComputedStyle(document.documentElement).getPropertyValue('--chart-1').trim()` and redraw when `neoism-theme-changed` fires on `window`. Preserve form values, filters, selection and zoom during theme updates; do not rebuild the whole document.

The initial artifact viewport is 320 CSS pixels high before app scaling, with internal document scrolling for taller content. Avoid root `100vh`/`height:100%` sizing assumptions and use responsive widths. Present the visualization directly; keep accompanying prose focused on information the visualization does not already convey.
