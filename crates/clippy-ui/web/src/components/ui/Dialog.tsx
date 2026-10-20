import { Dialog as RadixDialog } from 'radix-ui';
import type { ReactNode } from 'react';

/**
 * A confirmation or short form in a centred panel over a scrim. Radix owns the
 * focus trap, Escape, and focus restoration; this only dresses it in the
 * Apps SDK UI tokens. The scrim starts below the 40 px title bar so the
 * window stays movable and closable while a dialog is open.
 *
 *   <Dialog open={ask} onOpenChange={setAsk} title="Supprimer ce clip ?" actions={<>...buttons...</>}>
 *     texte
 *   </Dialog>
 */
export function Dialog({
  open,
  onOpenChange,
  title,
  children,
  actions,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  children?: ReactNode;
  actions: ReactNode;
}) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-x-0 top-10 bottom-0 z-40 grid place-items-center bg-black/55 data-[state=open]:animate-[fade-in_250ms_ease-out]">
          <RadixDialog.Content
            aria-describedby={undefined}
            className="w-[min(420px,calc(100vw-48px))] rounded-2xl border border-default bg-surface-elevated p-5 shadow-lg outline-none data-[state=open]:animate-[pop-in_250ms_cubic-bezier(0.22,1,0.36,1)]">
            <RadixDialog.Title className="heading-sm mb-2">{title}</RadixDialog.Title>
            <div className="text-sm text-secondary">{children}</div>
            <div className="mt-5 flex justify-end gap-2">{actions}</div>
          </RadixDialog.Content>
        </RadixDialog.Overlay>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
