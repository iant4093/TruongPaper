// @vitest-environment jsdom
// tests/windowsCaption.test.tsx — Windows-style minimize / maximize / close buttons (issue #7).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

const win = {
  minimize: vi.fn(async () => {}),
  toggleMaximize: vi.fn(async () => { win.max = !win.max; }),
  isMaximized: vi.fn(async () => win.max),
  hide: vi.fn(async () => {}),
  onResized: vi.fn(async () => () => {}),
  max: false,
};
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => win }));

import { WindowsCaptionButtons } from '../src/components/WindowControls';

afterEach(() => { cleanup(); vi.clearAllMocks(); win.max = false; });

describe('WindowsCaptionButtons', () => {
  it('shows minimize, maximize and close in Windows order', async () => {
    await act(async () => { render(<WindowsCaptionButtons />); });
    const labels = screen.getAllByRole('button').map((b) => b.getAttribute('aria-label'));
    expect(labels).toEqual(['Minimize window', 'Maximize window', 'Close window']);
  });

  it('minimizes, toggles maximize to restore, and close hides to the tray', async () => {
    await act(async () => { render(<WindowsCaptionButtons />); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Minimize window' })); });
    expect(win.minimize).toHaveBeenCalledTimes(1);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Maximize window' })); });
    expect(win.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole('button', { name: 'Restore window' })).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Close window' })); });
    expect(win.hide).toHaveBeenCalledTimes(1);
  });
});
