# Clippy — design system

Mode: **Operate** (a tool people open to find a clip, trim it, change a setting). Scanability and consistency outrank expression; the brand lives in precise details.

## Where each decision lives (one seam each)

| Concern | The one place | Callers see |
| --- | --- | --- |
| Palette | `web/src/vendor/openai/tokens.css` (generated), mapped to Clippy's names at the top of `web/src/app.css` | `var(--bg)`, `--surface`, `--text`, `--accent`… never a hex |
| Icons | `web/src/lib/icons.ts` + `components/ui/Icon.svelte` | `<Icon name="play" />`; solid or stroke is hidden |
| Motion | `--duration-*`, `--ease`, `--scale-*`, `--blur-*`, `--distance-*` in `app.css` | `var(--t)`, `var(--t-slow)` |
| Settings rows | `web/src/lib/settings.ts` descriptors (`help`, `more`, `advanced`, `onlyWhen`) | `Field.svelte` renders them |

## Source of the visuals

- **Colour and icons:** OpenAI's Apps SDK UI (`@openai/apps-sdk-ui` 0.2.2, MIT). It is React + Tailwind, so its components are not used; its neutral dark gray scale, status hues and SVG icons are copied by `web/scripts/sync-openai-design.mjs` (`node scripts/sync-openai-design.mjs`; bump `VERSION` there to update). Notice: `THIRD-PARTY-NOTICES.txt`.
- **Motion:** the transitions.dev scale (durations 80/150/250 ms, strong ease-out `cubic-bezier(.22,1,.36,1)`, scale .96/.97, 2 px blur, 8 px rise). No bounce/overshoot easing (the app is a tool; the detector bans it).

## Colour contract (each token means one thing)

| Token | Means |
| --- | --- |
| `--accent` | you selected / this is primary |
| `--live` | Clippy is recording or ready to |
| `--danger` | destructive or broken |
| `--fav` | kept as a favourite |
| `--text` / `--dim` / `--faint` | primary / secondary / tertiary text — all ≥ 4.5:1 on `--surface` |

Surfaces step one gray at a time: `--bg` (gray-0) < `--surface` (50) < `--raised` (100) < `--overlay` (200). Elevation is lightness, not shadow; only floating surfaces (menu, modal, toast) cast one.

## Motion rules

- Press: `scale(.97)` on buttons, `.95` on icon buttons. Hover washes ≤ 150 ms.
- Things that appear from a trigger grow from it (menu: `transform-origin` at the trigger, scale .97). A modal hangs from nothing: scale .96 from centre. Toasts rise 8 px with a 2 px blur.
- Nothing animates longer than 250 ms. Keyboard-driven actions do not animate. `prefers-reduced-motion` collapses every duration (`app.css`).
- Animate `transform` and `opacity` only (the buffer meter is `scaleX`).
