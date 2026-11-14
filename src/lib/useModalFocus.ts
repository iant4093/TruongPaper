// src/lib/useModalFocus.ts
// Keyboard behaviour shared by modal dialogs: focus moves in, Tab stays inside,
// Escape closes, and focus returns to the opener.
import { useEffect, useRef, type RefObject } from 'react';

const FOCUSABLE = 'button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])';

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
    const focusable = () =>
      [...(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter(
        (el) => !(el as HTMLButtonElement).disabled,
      );
    (initialFocusRef?.current ?? focusable()[0])?.focus();
    const onKey = (event: KeyboardEvent) => {
      // Capture phase + stopPropagation: one Escape closes only the top layer.
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeRef.current(); return; }
      if (event.key !== 'Tab' || !dialogRef.current) return;
      const items = focusable();
      if (items.length === 0) return;
      const first = items[0], last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('keydown', onKey, true);
      if (previous?.isConnected) previous.focus();
    };
  }, [active, dialogRef, initialFocusRef]);
}
