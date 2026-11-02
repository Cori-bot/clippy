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
  Search,
  CloseBold as Close,
  Minus,
  ExpandSm as Maximize,
  CollapseSm as Restore,
  X,
} from '@openai/apps-sdk-ui/components/Icon';

import { createElement, type SVGProps } from 'react';

/** A game controller. The SDK ships none, so this one is drawn here in the same 20 px outline style. */
export function Gamepad(props: SVGProps<SVGSVGElement>) {
  return createElement(
    'svg',
    { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', strokeWidth: 1.75, strokeLinecap: 'round', strokeLinejoin: 'round', 'aria-hidden': true, ...props },
    createElement('path', { d: 'M8 5.5h8a5 5 0 0 1 4.9 4l1 5.2a3.2 3.2 0 0 1-5.6 2.7L14.8 15.5H9.2l-1.5 1.9a3.2 3.2 0 0 1-5.6-2.7l1-5.2A5 5 0 0 1 8 5.5Z' }),
    createElement('path', { d: 'M8 9.25v3.5M6.25 11h3.5' }),
    createElement('circle', { cx: 15.75, cy: 10, r: 1, fill: 'currentColor', stroke: 'none' }),
    createElement('circle', { cx: 17.75, cy: 12.25, r: 1, fill: 'currentColor', stroke: 'none' }),
  );
}
