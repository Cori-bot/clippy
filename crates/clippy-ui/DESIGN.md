---
name: Clippy
description: A quiet dark workspace for finding and editing game clips.
colors:
  page: "#131313"
  rail: "#181818"
  surface: "#212121"
  elevated: "#303030"
  text: "#ffffff"
  text-secondary: "#afafaf"
  logo-blue: "#2b7fff"
  editor-ground: "#151719"
  editor-panel: "#202225"
  editor-rule: "#34373b"
  editor-text: "#eef0f3"
  editor-muted: "#b5b9c0"
  editor-accent: "#4b94ff"
  editor-export: "#287ce4"
typography:
  title:
    fontSize: "1.5rem"
    fontWeight: 600
    lineHeight: "1.75rem"
  body:
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: "1.25rem"
  label:
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: "1.125rem"
rounded:
  editor-control: "5px"
  editor-output: "4px"
  lg: "0.625rem"
  xl: "0.75rem"
  2xl: "1rem"
spacing:
  rail-padding: "12px"
  grid-column-gap: "16px"
  grid-row-gap: "24px"
components:
  navigation:
    backgroundColor: "{colors.rail}"
    width: "192px"
    padding: "{spacing.rail-padding}"
  thumbnail:
    rounded: "{rounded.xl}"
  dialog:
    backgroundColor: "{colors.elevated}"
    rounded: "{rounded.2xl}"
    padding: "20px"
---

# Design System: Clippy

## Overview

Mode: **Operate**. A quiet workspace: scanability and consistency lead, the clip content supplies the visual interest. The user approved a clean OpenAI Apps SDK UI foundation, the second library composition in dark, and the interlocking-loop logo on 26 October 2026. The first proposal was rejected as “IA slop”; do not reintroduce decorative dashboard furniture.

React 19, Tailwind 4 and OpenAI Apps SDK UI run inside Tauri. SDK components, icons and semantic tokens are primary; app-specific composition, Radix dialogs and a small motion stylesheet complete them. The blue loop mark is the identity asset, not a new palette for all controls.

The SDK stylesheet is imported once in `web/src/index.css`; `<html data-theme="dark">` selects its dark theme. Frontmatter snapshots the installed SDK's reused dark tokens and measured component decisions. Runtime CSS remains sourced from the SDK; refresh these extracted values when upgrading it. Surface-specific composition lives in `.impeccable/library-direction.md`, product commitments in root `PRODUCT.md`.

## Colors

Neutral dark surfaces and stepped white text keep thumbnails dominant. SDK primary controls and active navigation use neutral fills. Blue belongs to the approved mark and the SDK's informational roles; do not assume `primary` means blue. Success green signals operational replay, danger red signals destructive actions or failure, warning marks favorites.

Page, rail, normal and elevated surfaces use the corresponding frontmatter roles. Use SDK semantic text and border tokens, rather than matching screenshots with new literal grays. Tertiary text is for subordinate metadata, never the only carrier of an actionable error.

## Typography

Use the SDK system sans stack (including Segoe UI on Windows). Titles use `heading-lg`; compact control and body text use the small text scale; counts and metadata use the extra-small scale. Numeric durations, counts and storage values use tabular numerals. No display face is introduced.

UI copy is French. Prefer typographic apostrophes and non-breaking spaces before French double punctuation. Preserve accessible names on icon-only controls.

## Layout

The desktop shell fills the window and keeps its title bar at 40px. The library rail is 192px wide with replay controls anchored at its bottom. Library criteria wrap rather than overflow. Its virtualized content grid uses `repeat(auto-fill,minmax(min(230px,100%),1fr))`, with the documented column and row gaps: three columns at the reviewed 1180 × 760 viewport and two at the minimum 880 × 560 viewport. These are observed outcomes, not additional breakpoint tokens.

Keep the scroll container in the main region. Preserve ID-based virtualized card keys and the measured row stride; cosmetic layout changes must not break keyboard reveal or row measurement.

The editor uses the approved composition B: a full-height docked workstation with no library rail. A compact toolbar holds library return, clip identity, navigation, history, save state and export. The contained viewer and independently scrolling inspector share the upper area; transport and timeline span the width below. Inspector groups selection, audio and output, with replay and export tracking available there. At compact widths, hide secondary brand/save/count text before sacrificing primary controls. Toolbar height is 56px, reduced to 45px below 650px viewport height; status bar is 29px, reduced to 25px. The editor itself must not acquire a whole-page vertical scrollbar.

## Elevation & Depth

Depth comes primarily from tonal layering and quiet borders. Menus, dialogs and toasts sit on elevated surfaces; dialogs also use the SDK/Tailwind large shadow over a 55% black scrim below the title bar. A card lifts 2px on hover over 200ms; disable that transform while its pointer-anchored context menu is open. Portalled poppers sit above lifted cards (`z-index: 60`).

The active card has an inset ring; checked cards use a stronger ring plus an explicit checkbox. Those states have different meanings even when they overlap.

## Shapes

Thumbnails use gently rounded corners (`xl`), a 16:10 crop and object-cover media. Dialogs use the larger `2xl` radius. Navigation uses `lg`; SDK controls keep their own size/variant radius. The loop mark stays crisp SVG geometry in the title bar and rail; do not replace it with an unrelated icon or animate it continuously.

## Components

**SDK primitives.** Import Button, Input, Select, Checkbox, Menu, Switch, Slider, Tooltip and Alert from the installed SDK where applicable. Use its variant APIs and semantic tokens. Centralize icon aliases in `web/src/components/icons.ts`; the Lucide Animated reference informs behavior, not a second icon set.

**Library identity and selection.** The active clip is the current preview/editor target, preserved by clip ID across criteria changes when still visible (its visible index is derived). Bulk selection is a separate set of checked IDs. Changing search, game, sort or favorites criteria clears bulk selection and preview; this avoids deleting hidden checked items. “Tout sélectionner” targets visible results. Active highlight alone is not proof that a clip belongs to the bulk action.

**Replay.** The persistent rail section exposes operational state, buffered seconds, shortcut and capture action. Disable unavailable/pending actions. Show save confirmation only after a successful clip response with an ID. Text carries the status alongside its colored dot.

**Motion.** Interaction references are transitions.dev, amicro.dev and lucide-animated.com. Page entrance travels 8px with opacity over 250ms using `cubic-bezier(0.22, 1, 0.36, 1)`. Hover/focus icon nudges and turns, search focus, favorite state and save confirmation each run once for 250ms. Navigation color feedback is 150ms; card feedback is 200ms. Dialog entrance scales from .96. Toast entrance rises 8px with a 2px blur. These are short responses, with no ornamental idle loop or bounce. Keyboard focus may trigger the same icon feedback as hover; arrow navigation must remain immediate. Reduced-motion CSS collapses animation and transition durations to 0.01ms. The 250ms ceiling applies to app-authored feedback; do not claim that all SDK internals or operational indicators have been audited against it.

**Editor.** Keep SDK primitives within the scoped charcoal editor palette above; these overrides do not change the library. Joined panels use thin dividers rather than detached cards. Body and section labels are 12px; clip title is 13px semibold, inspector title 16px semibold (14px in short windows). Rectangular controls use the editor radii; output controls use the smaller radius. Blue marks export, timeline selection and visible keyboard focus (2px outline with 2px offset). Keep the video contained, without cropping. Timeline thumbnails and waveforms must come from media data; label unavailable data instead of fabricating it. Gain values are percentages (0–200), not mock dB values. Preserve precise/fast cuts and share/master outputs. Editing behavior and export correctness require their own tests; static captures verify composition only.

**Precise editing.** Default to precise cuts, with a separate explicit fast mode. Frame buttons have accessible names and comma/period shortcuts. Indexed timestamps drive stepping and trim handles; sparse keyframe marks remain independent. Display milliseconds, pause when stepping and keep requested positions during rapid seeks. I/O reads the presented frame rather than a stale timeupdate. Frame/audio fetches belong to a directory and clip identity. Keep saved drafts independent of the output mode; show the effective cut boundaries for that mode. Export tracking remains available across navigation: in the library rail and editor inspector. It reports daemon progress and confirms cancellation from the daemon.

**Ownership seams.** `web/src/lib/state.ts` plus `lib/reactive.ts` own observable app state; components use `useApp()` and app methods. Settings rows come from `lib/settings.ts` descriptors and `Field.tsx`; `components/ui/Dialog.tsx` owns the Radix dialog wrapper and focus behavior.

## Do's and Don'ts

- Do keep hit areas at least 24px, visible keyboard focus and named icon controls.
- Do preserve the distinction between active clip identity and checked bulk selection.
- Do test pure logic with Vitest and run the production build after behavioral UI changes. Existing tests do not constitute a DOM accessibility audit.
- Do inspect `.impeccable/review/desktop.png` and `minimum.png` for layout at the two supported review sizes. Both use fictitious clips and neutral thumbnails labeled “Miniature de vérification”; they establish composition and density only. They do not establish real-video fidelity, GPU performance, native capture, editor behavior or animation timing.
- Don't replace SDK primitives or semantic tokens with a parallel custom component system.
- Don't add decorative metrics banners or perpetual icon animations.
- Don't infer color, keyboard, motion or media correctness from a static screenshot.

OpenAI Apps SDK UI attribution remains in `THIRD-PARTY-NOTICES.txt`.
