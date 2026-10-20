/**
 * The icons the app uses, under the names it uses them by. The shapes are
 * OpenAI's (Apps SDK UI); this file is the only place that knows which of
 * their names stands for which job, so a swap is one line.
 */
export {
  Video as Clips,
  Settings,
  Camera,
  Play,
  Pause,
  Star,
  StarFilled,
  Pencil,
  Trash,
  Folder,
  Scissor as Scissors,
  SoundOnReadOutLoudSpeaker as Volume,
  SoundOffSimpleMute as VolumeMute,
  ExpandLarge as Fullscreen,
  CollapseLarge as FullscreenExit,
  ChevronLeft,
  ChevronRight,
  ChevronDown,
  DotsHorizontal as Dots,
  Check,
  ArrowRotateCw as Rearm,
  Warning as Alert,
  Copy,
  CloseBold as Close,
} from '@openai/apps-sdk-ui/components/Icon';

import { createElement, type SVGProps } from 'react';

/** A game controller. The SDK ships none, so this one is drawn here in the same 20 px outline style. */
export function Gamepad(props: SVGProps<SVGSVGElement>) {
  return createElement(
    'svg',
    { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', strokeWidth: 1.75, strokeLinecap: 'round', strokeLinejoin: 'round', 'aria-hidden': true, ...props },
    createElement('path', { d: 'M7 7h10a4 4 0 0 1 3.9 3.1l1 4.5a3 3 0 0 1-5.1 2.6L15 15H9l-1.8 2.2a3 3 0 0 1-5.1-2.6l1-4.5A4 4 0 0 1 7 7Z' }),
    createElement('path', { d: 'M8 10v3M6.5 11.5h3' }),
    createElement('circle', { cx: 15.5, cy: 10.5, r: 0.6, fill: 'currentColor' }),
    createElement('circle', { cx: 17.5, cy: 12.5, r: 0.6, fill: 'currentColor' }),
  );
}
