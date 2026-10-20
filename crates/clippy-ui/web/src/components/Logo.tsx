/**
 * The Clippy mark, redrawn as vectors from `assets/logo-transparent.png` so it
 * stays sharp at title-bar size. The outer chevron takes `currentColor` (it is
 * dark navy in the PNG and would vanish on a dark surface); the cyan and blue
 * are the PNG's own. Decorative: the name is always written beside it.
 */
const VIEW = { x: 264, y: 252, w: 562, h: 494 };

export function Logo({ height = 20 }: { height?: number }) {
  return (
    <svg
      className="block shrink-0 text-default"
      width={(height * VIEW.w) / VIEW.h}
      height={height}
      viewBox={`${VIEW.x} ${VIEW.y} ${VIEW.w} ${VIEW.h}`}
      aria-hidden="true"
      focusable="false">
      <path
        fill="currentColor"
        d="M397.5 343.5L397.5 297.2A40.7 40.7 0 0 1 458.4 261.9L798.4 456.2A45 45 0 0 1 799.9 533.4L476.1 735A36.4 36.4 0 0 1 420.5 704.1L420.5 699.9L738.9 495.3L482.3 343.5z"
      />
      <path fill="#095ded" d="M495 404.5L585.4 404.5L738.9 495.3L495 652.1z" />
      <path
        fill="#0ae7fb"
        d="M269 343.5L482.3 343.5L585.4 404.5L495 404.5L495 652.1L420.5 699.9L420.5 404.5L328 404.5z"
      />
    </svg>
  );
}
