// src/store/feedback.ts
// One place for "something happened" feedback: toasts (bottom center, auto-dismiss, optional
// Undo) and a promise-based confirm dialog for destructive actions.
import { create } from 'zustand';

export type ToastTone = 'info' | 'success' | 'error';

export interface Toast {
  id: number;
  message: string;
  tone: ToastTone;
  /** Shows an Undo button; the toast closes once it is pressed. */
  undo?: () => void | Promise<void>;
}

export interface ConfirmRequest {
  id: number;
  title: string;
  message?: string;
  confirmLabel: string;
  resolve: (ok: boolean) => void;
}

/** How long a toast stays: errors get longer so they can be read. */
export const TOAST_MS = 4000;
export const TOAST_ERROR_MS = 8000;

interface FeedbackState {
  toasts: Toast[];
  confirm: ConfirmRequest | null;
  push: (toast: Omit<Toast, 'id' | 'tone'> & { tone?: ToastTone }) => number;
  dismiss: (id: number) => void;
  ask: (request: Omit<ConfirmRequest, 'id' | 'resolve'>) => Promise<boolean>;
  answer: (ok: boolean) => void;
}

let nextId = 1;
const MAX_TOASTS = 3;

export const useFeedback = create<FeedbackState>((set, get) => ({
  toasts: [],
  confirm: null,
  push: (toast) => {
    const id = nextId++;
    set((state) => ({ toasts: [...state.toasts, { ...toast, tone: toast.tone ?? 'info', id }].slice(-MAX_TOASTS) }));
    return id;
  },
  dismiss: (id) => set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),
  ask: (request) => new Promise<boolean>((resolve) => {
    // A second question replaces the first, which counts as "no".
    get().confirm?.resolve(false);
    set({ confirm: { ...request, id: nextId++, resolve } });
  }),
  answer: (ok) => {
    const current = get().confirm;
    set({ confirm: null });
    current?.resolve(ok);
  },
}));

const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err));

/** Imperative helpers usable from event handlers and store actions. */
export const toast = {
  info: (message: string, undo?: Toast['undo']) => useFeedback.getState().push({ message, tone: 'info', undo }),
  success: (message: string, undo?: Toast['undo']) => useFeedback.getState().push({ message, tone: 'success', undo }),
  error: (err: unknown, prefix?: string) =>
    useFeedback.getState().push({ message: prefix ? `${prefix}: ${errorText(err)}` : errorText(err), tone: 'error' }),
};

/** Ask before a destructive action. Resolves false when the user cancels. */
export const confirmAction = (title: string, confirmLabel: string, message?: string) =>
  useFeedback.getState().ask({ title, confirmLabel, message });
