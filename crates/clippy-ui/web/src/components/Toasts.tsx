import { Alert } from '@openai/apps-sdk-ui/components/Alert';
import { Button } from '@openai/apps-sdk-ui/components/Button';
import { Close } from './icons';
import { app, useApp } from '../lib/app';

export function Toasts() {
  const a = useApp();
  return (
    <div className="fixed right-[18px] bottom-[18px] z-50 grid gap-2">
      {a.toasts.map((toast) => (
        <div key={toast.id} className="selectable max-w-[46ch] animate-[toast-in_250ms_cubic-bezier(0.22,1,0.36,1)]">
          <Alert
            color={toast.kind === 'error' ? 'danger' : 'success'}
            variant="outline"
            className="bg-surface-elevated shadow-lg"
            description={toast.text}
            actions={
              <Button color="secondary" variant="ghost" size="2xs" uniform aria-label="Fermer la notification" onClick={() => app.dismissToast(toast.id)}>
                <Close />
              </Button>
            }
            actionsPlacement="end"
          />
        </div>
      ))}
    </div>
  );
}
