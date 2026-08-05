# DESIGN.md

Design system for the Portreaper app UI (`src/App.css` + `src/App.tsx` + `src/components/`).
Row / detail-panel markup that consumes these class names lives in
`src/components/ProcessRow.tsx` and `src/components/ProcessDetail.tsx`.
Register: product. Color strategy: **Restrained** (tinted dark neutrals, one
green accent ≤10%, red strictly semantic).

## Theme

Dark only. Scene: a developer at night, dark IDE and terminal filling the
screen, opens the tray window for under 15 seconds to find and kill a port
squatter. The window must feel native to that ambient light and surface its
single answer instantly.

## Color (OKLCH, defined in `:root` of App.css)

Neutrals are tinted toward the brand green hue (h≈165, chroma 0.004–0.008).
**Every token is declared twice: sRGB hex/rgb first, oklch second** — macOS
10.15–12.2 WKWebView (Safari < 15.4) cannot parse oklch and would otherwise
drop every color. Never add a color as a bare oklch literal; add a token with
both declarations.

| Token | Value | Role |
|---|---|---|
| `--bg` | `oklch(0.16 0.006 165)` | window background |
| `--bg-elev` | `oklch(0.19 0.007 165)` | header, footer, detail panels |
| `--surface` | `oklch(0.22 0.008 165)` | inputs, hover rows, segmented control |
| `--border` | `oklch(0.28 0.008 165)` | hairlines |
| `--text` | `oklch(0.93 0.005 165)` | primary text |
| `--text-muted` | `oklch(0.68 0.01 165)` | secondary text |
| `--text-dim` | `oklch(0.62 0.01 165)` | tertiary/labels (≥4.5:1 on `--bg`; do not darken below WCAG AA) |
| `--accent` | `oklch(0.72 0.15 165)` | star, all-clear, focus, port hover |
| `--danger` | `oklch(0.66 0.19 25)` | zombie verdict text tints/borders |
| `--danger-btn` | `oklch(0.58 0.2 25)` | destructive button fills (white text needs ≥4.5:1) |
| `--on-danger` | `oklch(0.98 0.005 25)` | text on `--danger-btn` |
| `--warn` | `oklch(0.76 0.13 80)` | likely tier |

Confidence tiers: confirmed = danger, likely = warn, possible = muted. No
other hues. Category colors do not exist; categories are plain text in the
detail panel.

## Typography

System stack (`-apple-system, "Segoe UI", "PingFang SC", …`), one family.
Base 13px. Mono (`ui-monospace, SF Mono, Consolas`) for ports, PIDs, paths,
commands. Scale: 11 / 12 / 13 / 15. Weight contrast over size contrast:
600 for names and verdicts, 400 elsewhere. Tabular numerals on all numbers.

## Layout

- Single-row header (brand · search · segmented filter with counts · sweep ·
  language). No pill rows.
- Verdict-grouped list, not a uniform table: "suspects" section first (red
  section label), then "healthy". With zero suspects, a one-line all-clear
  note replaces the section.
- Rows are two-line flex rows (~44px): disclosure button · main column
  (line 1: name + sub + port links; line 2: plain-language description /
  verdict story) · coarse uptime · actions. Ellipsis truncation relies on the
  `min-width: 0` chain through `.row-main` and `.row-title`.
- The disclosure caret is a real `<button>` (keyboard path, `aria-expanded` +
  `aria-controls`); the row itself is a mouse-only click enhancement with no
  ARIA role — never nest buttons inside a `role="button"` container.
- Click a row to expand an inline detail panel: full command, exe path, all
  ports, PID/parent, launcher chain, resources, and every classification
  reason with its full explanation as readable text (not tooltips).
- Footer status bar: process/port counts + auto-refresh note.

## Components

- **Story cell**: tier dot + verdict + one plain-language primary reason
  ("确认僵尸 · 启动它的终端已关闭"). Healthy rows show provenance ("Terminal
  启动" / "launchd 托管").
- **Actions**: hover-revealed on healthy rows, persistent on suspects.
  macOS = Kill (SIGTERM) + 强杀 (SIGKILL); Windows = single 终止. Star toggles
  whitelist everywhere.
- **Modals**: only for kill confirmation (destructive). 150–200ms ease-out,
  gated by `prefers-reduced-motion`. `role="dialog"` + focus trapped inside;
  initial focus always lands on Cancel (Enter must never kill by default).
- **Empty states** teach: "no listeners" vs "no matches" vs all-clear.

## Bans (enforced)

No side-stripe row accents, no gradient buttons/text, no category badge
rainbow, no glow/drop-shadow decoration, no hero metrics, no spinners inside
content (the 2s poll re-renders silently).
