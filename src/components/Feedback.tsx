// Toast stack and confirm dialog. Mount <FeedbackHost /> once per window.
import React, { useEffect, useRef } from 'react';
import { X } from 'lucide-react';
import { TOAST_ERROR_MS, TOAST_MS, useFeedback, type Toast } from '../store/feedback';

const ToastItem: React.FC<{ toast: Toast }> = ({ toast }) => {
  const dismiss = useFeedback((s) => s.dismiss);
  const hovered = useRef(false);
  useEffect(() => {
    let timer: number;
    const arm = () => {
      timer = window.setTimeout(() => {
        // Keep it while the pointer is over it, then give a short grace period.
        if (hovered.current) arm(); else dismiss(toast.id);
      }, toast.tone === 'error' ? TOAST_ERROR_MS : TOAST_MS);
    };
    arm();
    return () => window.clearTimeout(timer);
  }, [toast.id, toast.tone, dismiss]);
  return (
    <div className={`toast toast-${toast.tone}`} role={toast.tone === 'error' ? 'alert' : 'status'}
      onMouseEnter={() => { hovered.current = true; }} onMouseLeave={() => { hovered.current = false; }}>
      <span className="toast-text">{toast.message}</span>
      {toast.undo && (
        <button type="button" className="button toast-undo"
          onClick={() => { const undo = toast.undo; dismiss(toast.id); void Promise.resolve(undo?.()).catch(() => {}); }}>
          Undo
        </button>
      )}
      <button type="button" className="icon-button toast-close" aria-label="Dismiss message" onClick={() => dismiss(toast.id)}><X size={14} /></button>
    </div>
  );
};

const ConfirmDialog: React.FC = () => {
  const request = useFeedback((s) => s.confirm);
  const answer = useFeedback((s) => s.answer);
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!request) return;
    const previous = document.activeElement as HTMLElement | null;
    cancelRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); answer(false); }
    };
    document.addEventListener('keydown', onKey, true);
    return () => { document.removeEventListener('keydown', onKey, true); if (previous?.isConnected) previous.focus(); };
  }, [request, answer]);
  if (!request) return null;
  return (
    <div className="modal-backdrop" onClick={() => answer(false)}>
      <div className="panel confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title"
        aria-describedby={request.message ? 'confirm-message' : undefined} onClick={(e) => e.stopPropagation()}>
        <h3 id="confirm-title">{request.title}</h3>
        {request.message && <p id="confirm-message">{request.message}</p>}
        <div className="confirm-actions">
          <button type="button" ref={cancelRef} className="button" onClick={() => answer(false)}>Cancel</button>
          <button type="button" className="button button-danger" onClick={() => answer(true)}>{request.confirmLabel}</button>
        </div>
      </div>
    </div>
  );
};

export const FeedbackHost: React.FC<{ floatingToasts?: boolean }> = ({ floatingToasts = true }) => {
  const toasts = useFeedback((s) => s.toasts);
  return (
    <>
      {floatingToasts && (
        <div className="toast-stack" aria-live="polite">
          {toasts.map((t) => <ToastItem key={t.id} toast={t} />)}
        </div>
      )}
      <ConfirmDialog />
    </>
  );
};

/**
 * A quiet one-line status in the sidebar, above RAM and storage. Only the newest message
 * shows; it fades on its own (errors stay a little longer).
 */
export const StatusBanner: React.FC = () => {
  const latest = useFeedback((s) => s.toasts[s.toasts.length - 1]);
  const dismiss = useFeedback((s) => s.dismiss);
  useEffect(() => {
    if (!latest) return;
    const timer = window.setTimeout(() => dismiss(latest.id),
      latest.tone === 'error' ? TOAST_ERROR_MS : latest.undo ? TOAST_MS : STATUS_MS);
    return () => window.clearTimeout(timer);
  }, [latest, dismiss]);
  return (
    <div className="status-banner-slot" aria-live="polite">
      {latest && (
        <div key={latest.id} className={`status-banner status-${latest.tone}`} role={latest.tone === 'error' ? 'alert' : 'status'}
          title={latest.message}>
          <span className="status-banner-text">{latest.message}</span>
          {latest.undo && (
            <button type="button" className="status-banner-undo"
              onClick={() => { const undo = latest.undo; dismiss(latest.id); void Promise.resolve(undo?.()).catch(() => {}); }}>
              Undo
            </button>
          )}
          <button type="button" className="status-banner-close" aria-label="Dismiss message" onClick={() => dismiss(latest.id)}><X size={12} /></button>
        </div>
      )}
    </div>
  );
};

/** Plain confirmations ("Saved") leave quickly. */
const STATUS_MS = 2500;
