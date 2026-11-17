// @vitest-environment jsdom
// tests/contextMenu.test.tsx — the app's own right-click menu.
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ContextMenu, contextMenuItems } from '../src/components/ContextMenu';

const actions = (over: Partial<Parameters<typeof contextMenuItems>[1]> = {}) => ({
  next: vi.fn(),
  togglePause: vi.fn(),
  paused: false,
  canNext: true,
  openSettings: vi.fn(),
  ...over,
});

describe('context menu', () => {
  afterEach(cleanup);

  it('offers text editing on fields and wallpaper controls everywhere', () => {
    const input = document.createElement('input');
    input.value = 'hello';
    expect(contextMenuItems(input, actions()).map((i) => i.label)).toEqual([
      'Cut', 'Copy', 'Paste', 'Select all', 'Next wallpaper', 'Pause all', 'Settings',
    ]);
    const labels = contextMenuItems(document.body, actions({ paused: true })).map((i) => i.label);
    expect(labels).toEqual(['Next wallpaper', 'Resume all', 'Settings']);
  });

  it('disables Next when the display cannot rotate', () => {
    const next = contextMenuItems(document.body, actions({ canNext: false })).find((i) => i.label === 'Next wallpaper');
    expect(next?.disabled).toBe(true);
  });

  it('replaces the browser menu and runs the chosen entry', async () => {
    const openSettings = vi.fn();
    render(<ContextMenu onOpenSettings={openSettings} />);
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 20, clientY: 20 });
    await act(async () => { document.body.dispatchEvent(event); });
    expect(event.defaultPrevented).toBe(true);
    await act(async () => { fireEvent.click(screen.getByRole('menuitem', { name: 'Settings' })); });
    expect(openSettings).toHaveBeenCalled();
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('closes with Escape', async () => {
    render(<ContextMenu onOpenSettings={() => {}} />);
    await act(async () => { fireEvent.contextMenu(document.body); });
    await act(async () => { fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' }); });
    expect(screen.queryByRole('menu')).toBeNull();
  });
});
