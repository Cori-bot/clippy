# Clippy — design system

Mode: **Operate** (a tool people open to find a clip, trim it, change a setting). Scanability and consistency outrank expression; the brand lives in precise details. The look is OpenAI's: neutral dark grays, one blue, generous space, quiet borders, no decoration.

## Stack

React 19 + Tailwind 4 + **OpenAI Apps SDK UI** (`@openai/apps-sdk-ui`, MIT) on Vite, inside Tauri. The SDK supplies the components (`Button`, `Select`, `Switch`, `Slider`, `Menu`, `Tooltip`, `Alert`, `Input`, …), the icons and the token scale; the app writes no component CSS of its own.

## Where each decision lives (one seam each)

| Concern | The one place | Callers see |
| --- | --- | --- |
| Tokens and theme | `@openai/apps-sdk-ui/css`, imported once in `web/src/index.css` (`data-theme="dark"` on `<html>`) | Tailwind utilities (`bg-surface`, `text-secondary`, `border-subtle`, `heading-lg`) and `var(--color-…)` |
| Icons | `web/src/components/icons.ts` | `import { Play } from '../components/icons'` — the SDK's shapes under the app's names |
| App state | `web/src/lib/state.ts`, made observable by `lib/reactive.ts` | `const a = useApp()` in a component; mutations go through `app.method()` |
| Settings rows | `web/src/lib/settings.ts` descriptors (`help`, `more`, `advanced`, `onlyWhen`) | `Field.tsx` renders them |
| Dialogs | `web/src/components/ui/Dialog.tsx` (Radix + SDK tokens) | `<Dialog open title actions>` |
| Motion | Tailwind `duration-*`/`ease-*` + the keyframes at the end of `index.css` | ≤ 250 ms, strong ease-out |

## Rules

- Colour: `success` green means Clippy is recording or ready, `danger` red means destructive or broken, blue (`info`/primary) means selected. Text steps: `text-default` / `text-secondary` / `text-tertiary`. Surfaces step one gray at a time: `bg-surface-tertiary` (page) < `bg-surface-secondary` (rail, panels) < `bg-surface` < `bg-surface-elevated` (menus, dialogs, toasts).
- Motion (transitions.dev scale): press `active:scale-[0.97]`; things that open from a trigger grow from it (scale .97); a dialog hangs from nothing (scale .96 from centre); toasts rise 8 px with a 2 px blur. Nothing animates longer than 250 ms, no bounce, only `transform` and `opacity`. `prefers-reduced-motion` collapses all of it (`index.css`).
- Hit areas ≥ 24 px; every icon-only control has an `aria-label`; keyboard-driven actions do not animate.
- French UI, French typography (’ apostrophe, no-break space before `: ? ! ;`).
- Tests cover pure logic (`lib/*.test.ts`) in plain Vitest; components are not unit-tested (no DOM environment).

## Credits

`THIRD-PARTY-NOTICES.txt` carries the OpenAI Apps SDK UI licence; it is a normal npm dependency (`npm ci` in `web/`).
