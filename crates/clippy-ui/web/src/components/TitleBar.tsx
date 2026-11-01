import { Logo } from './Logo';
import { WindowControls } from './WindowControls';

export function TitleBar() {
  return (
    <header className="flex h-10 shrink-0 items-center gap-2 border-b border-subtle bg-surface-secondary pl-3">
      <Logo height={16} />
      <span className="text-xs text-secondary">Clippy</span>
      <div className="flex-1 self-stretch" data-tauri-drag-region />
      <WindowControls />
    </header>
  );
}
