// src/components/ContextMenu.tsx
// The app's own right-click menu (replaces the web view's Reload / Inspect menu): text editing
// on fields and selections, and quick wallpaper controls everywhere else.
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ClipboardPaste, Copy, Pause, Play, Scissors, Settings, SkipForward, TextSelect } from 'lucide-react';
import { useAppStore } from '../store/useAppStore';
import { toast } from '../store/feedback';

export interface ContextMenuItem {
  label: string;
  icon: React.ReactNode;
  run: () => void | Promise<void>;
  disabled?: boolean;
}

type Editable = HTMLInputElement | HTMLTextAreaElement;

const TEXT_INPUT_TYPES = new Set(['text', 'search', 'url', 'email', 'tel', 'password', 'number', '']);

/** The text field the click landed in, if any. */
export function editableTarget(target: EventTarget | null): Editable | null {
  if (target instanceof HTMLTextAreaElement) return target;
  if (target instanceof HTMLInputElement && TEXT_INPUT_TYPES.has(target.type)) return target;
  return null;
}

function hasSelection(field: Editable): boolean {
  try {
    return (field.selectionStart ?? 0) !== (field.selectionEnd ?? 0);
  } catch {
    return false; // number inputs have no selection API
  }
}

/** Replaces the field's selection with `text` the way typing would (React sees the change). */
function insertText(field: Editable, text: string) {
  field.focus();
  if (!document.execCommand('insertText', false, text)) {
    const start = field.selectionStart ?? field.value.length;
    const end = field.selectionEnd ?? start;
    field.setRangeText(text, start, end, 'end');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  }
}

/** Menu entries for a right-click on `target`. */
export function contextMenuItems(
  target: EventTarget | null,
  actions: { next: () => void; togglePause: () => void; paused: boolean; canNext: boolean; openSettings: () => void },
): ContextMenuItem[] {
  const items: ContextMenuItem[] = [];
  const field = editableTarget(target);
  const selected = field ? '' : window.getSelection()?.toString() ?? '';
  if (field) {
    const readOnly = field.readOnly || field.disabled;
    const some = hasSelection(field);
    items.push(
      { label: 'Cut', icon: <Scissors size={14} />, disabled: readOnly || !some || field.type === 'password', run: () => { field.focus(); document.execCommand('cut'); } },
      { label: 'Copy', icon: <Copy size={14} />, disabled: !some || field.type === 'password', run: () => { field.focus(); document.execCommand('copy'); } },
      {
        label: 'Paste', icon: <ClipboardPaste size={14} />, disabled: readOnly,
        run: async () => {
          try { insertText(field, await navigator.clipboard.readText()); } catch (err) { toast.error(err, 'Could not paste'); }
        },
      },
      { label: 'Select all', icon: <TextSelect size={14} />, run: () => { field.focus(); field.select(); } },
    );
  } else if (selected.trim()) {
    items.push({
      label: 'Copy', icon: <Copy size={14} />,
      run: () => { navigator.clipboard.writeText(selected).catch((err) => toast.error(err, 'Could not copy')); },
    });
  }
  items.push(
    { label: 'Next wallpaper', icon: <SkipForward size={14} />, disabled: !actions.canNext, run: actions.next },
    { label: actions.paused ? 'Resume all' : 'Pause all', icon: actions.paused ? <Play size={14} /> : <Pause size={14} />, run: actions.togglePause },
    { label: 'Settings', icon: <Settings size={14} />, run: actions.openSettings },
  );
  return items;
}

export const ContextMenu: React.FC<{ onOpenSettings: () => void }> = ({ onOpenSettings }) => {
  const [menu, setMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);
  const [highlight, setHighlight] = useState(-1);
  const listRef = useRef<HTMLUListElement>(null);
  const close = useCallback(() => setMenu(null), []);

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      event.preventDefault();
      const s = useAppStore.getState();
      const monitor = s.monitors.find((m) => m.monitorId === s.selectedMonitorId) ?? s.monitors[0];
      const kind = monitor?.activeSource.kind;
      const paused = s.monitors.length > 0 && s.monitors.every((m) => m.userPaused);
      const items = contextMenuItems(event.target, {
        canNext: !!monitor && (kind === 'photo_api' || kind === 'playlist'),
        next: () => { if (monitor) s.playbackNext(monitor.monitorId).catch((err) => toast.error(err, 'Could not go to the next one')); },
        paused,
        togglePause: () => { s.setPlaybackPaused(!paused).catch((err) => toast.error(err, 'Could not change playback')); },
        openSettings: onOpenSettings,
      });
      setHighlight(-1);
      setMenu({ x: event.clientX, y: event.clientY, items });
    };
    document.addEventListener('contextmenu', onContextMenu);
    return () => document.removeEventListener('contextmenu', onContextMenu);
  }, [onOpenSettings]);

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!menu || !list) return;
    const rect = list.getBoundingClientRect();
    const x = Math.min(menu.x, window.innerWidth - rect.width - 4);
    const y = Math.min(menu.y, window.innerHeight - rect.height - 4);
    list.style.left = `${Math.max(4, x)}px`;
    list.style.top = `${Math.max(4, y)}px`;
    list.focus();
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const onPointer = (event: PointerEvent) => {
      if (!listRef.current?.contains(event.target as Node)) close();
    };
    window.addEventListener('pointerdown', onPointer, true);
    window.addEventListener('blur', close);
    window.addEventListener('resize', close);
    document.addEventListener('scroll', close, true);
    return () => {
      window.removeEventListener('pointerdown', onPointer, true);
      window.removeEventListener('blur', close);
      window.removeEventListener('resize', close);
      document.removeEventListener('scroll', close, true);
    };
  }, [menu, close]);

  if (!menu) return null;
  const enabled = menu.items.map((item, i) => (item.disabled ? -1 : i)).filter((i) => i >= 0);
  const run = (item: ContextMenuItem) => {
    if (item.disabled) return;
    close();
    void item.run();
  };
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'Escape') { event.preventDefault(); close(); return; }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const at = enabled.indexOf(highlight);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setHighlight(enabled[(at + step + enabled.length) % enabled.length] ?? -1);
    }
    if ((event.key === 'Enter' || event.key === ' ') && highlight >= 0) {
      event.preventDefault();
      run(menu.items[highlight]);
    }
  };
  return (
    <ul ref={listRef} role="menu" aria-label="Context menu" tabIndex={-1} className="dropdown-menu context-menu"
      style={{ left: menu.x, top: menu.y }} onKeyDown={onKeyDown}>
      {menu.items.map((item, i) => (
        <React.Fragment key={item.label}>
          {/* Text editing above, wallpaper controls below. */}
          {item.label === 'Next wallpaper' && i > 0 && <li role="separator" className="context-menu-separator" />}
          <li role="menuitem" aria-disabled={item.disabled || undefined}
            className={`${i === highlight ? 'is-highlighted' : ''}${item.disabled ? ' is-disabled' : ''}`}
            onMouseEnter={() => setHighlight(item.disabled ? -1 : i)}
            onClick={() => run(item)}>
            <span className="context-menu-label">{item.icon}{item.label}</span>
          </li>
        </React.Fragment>
      ))}
    </ul>
  );
};
