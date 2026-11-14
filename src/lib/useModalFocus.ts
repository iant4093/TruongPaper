// src/lib/useModalFocus.ts
// Keyboard behaviour shared by modal dialogs: focus moves in, Tab stays inside,
// Escape closes, and focus returns to the opener.
import { useEffect, type RefObject } from 'react';

const FOCUSABLE = 'button, input, select, textarea, [href], [tabindex]:not([tabindex="-1"])';

export function useModalFocus(
  active: boolean,
  dialogRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  initialFocusRef?: RefObject<HTMLElement | null>,
): void {
  useEffect(() => {
    if (!active) return;
    const previous = document.activeElement as HTMLElement | null;
    const focusable = () =>
      [...(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])].filter(
        (el) => !(el as HTMLButtonElement).disabled,
      );
    (initialFocusRef?.current ?? focusable()[0])?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); onClose(); return; }
      if (event.key !== 'Tab' || !dialogRef.current) return;
      const items = focusable();
      if (items.length === 0) return;
      const first = items[0], last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      if (previous?.isConnected) previous.focus();
    };
  }, [active, dialogRef, onClose, initialFocusRef]);
}
