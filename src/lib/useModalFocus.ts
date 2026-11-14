// src/lib/useModalFocus.ts
// Keyboard behaviour shared by modal dialogs: focus moves in, Tab stays inside,
// Escape closes, and focus returns to the opener.
import { useEffect, useRef, type RefObject } from 'react';
import { isTopEscapeLayer, popEscapeLayer, pushEscapeLayer } from './escapeStack';

const FOCUSABLE =
  'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])';

export function useModalFocus(
  active: boolean,
  dialogRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  initialFocusRef?: RefObject<HTMLElement | null>,
) {
  // Read through a ref so a caller's inline function never re-runs the effect (and steals focus).
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    if (!active) return;
    const previous = document.activeElement as HTMLElement | null;
    const layer = pushEscapeLayer();
    const focusable = () => [...(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
    (initialFocusRef?.current ?? focusable()[0])?.focus();
    const onKey = (event: KeyboardEvent) => {
      // Capture phase + stopImmediatePropagation: one Escape closes only the top layer.
      if (event.key === 'Escape') {
        if (!isTopEscapeLayer(layer)) return; // a newer layer (a confirm dialog) owns this key press
        event.preventDefault(); event.stopPropagation(); event.stopImmediatePropagation(); closeRef.current(); return;
      }
      if (event.key !== 'Tab' || !dialogRef.current || !isTopEscapeLayer(layer)) return;
      const items = focusable();
      const dialog = dialogRef.current;
      // The focused button may have just become disabled, which drops focus to <body>: pull it back in.
      if (!dialog.contains(document.activeElement)) {
        event.preventDefault();
        (event.shiftKey ? items[items.length - 1] : items[0])?.focus();
        return;
      }
      if (items.length === 0) return;
      const first = items[0], last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('keydown', onKey, true);
      popEscapeLayer(layer);
      if (previous?.isConnected) previous.focus();
    };
  }, [active, dialogRef, initialFocusRef]);
}
