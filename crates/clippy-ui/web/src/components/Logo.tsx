/** Approved interlocking-loop mark, kept as crisp native geometry. */
export function Logo({ height = 20 }: { height?: number }) {
  return (
    <svg width={height} height={height} viewBox="0 0 24 24" fill="none" aria-hidden="true" focusable="false">
      <path d="M13.7 5.2a5.2 5.2 0 0 0-7.4 0l-3.1 3.1a5.2 5.2 0 0 0 7.4 7.4l2.1-2.1" stroke="#2b7fff" strokeWidth="3.1" strokeLinecap="round" />
      <path d="M10.3 18.8a5.2 5.2 0 0 0 7.4 0l3.1-3.1a5.2 5.2 0 0 0-7.4-7.4l-2.1 2.1" stroke="#2b7fff" strokeWidth="3.1" strokeLinecap="round" />
    </svg>
  );
}
