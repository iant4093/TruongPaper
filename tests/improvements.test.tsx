// @vitest-environment jsdom
// tests/improvements.test.tsx — load errors, scene report, drawer focus, library search/sort,
// offline notice and debounced writes, against the browser-preview mock backend.
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { emit } from '@tauri-apps/api/event';
import { installBrowserPreviewMocksIfNeeded } from '../src/dev/mockTauri';
import { App } from '../src/App';
import { TrayApp } from '../src/tray/TrayApp';
import { useAppStore } from '../src/store/useAppStore';
import { OFFLINE_NOTICE } from '../src/lib/format';
import { WriteQueue } from '../src/lib/writeQueue';

const initial = useAppStore.getState();

beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () => new Proxy({}, { get: (_t, prop) => (prop === 'createRadialGradient' ? () => ({ addColorStop() {} }) : () => {}) }) as never
  );
  installBrowserPreviewMocksIfNeeded();
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  useAppStore.setState(initial, true);
});

const NEON = 'workshop:2841928371';
const openDetails = (title: string) => fireEvent.click(screen.getByRole('button', { name: `Details for ${title}` }));
const tileNames = () => screen.getAllByRole('button', { name: /^Select / }).map((b) => b.getAttribute('aria-label')!.replace('Select ', ''));

describe('wallpaper load errors', () => {
  const fail = (message = 'Missing scene.pkg') =>
    act(async () => { await emit('wallpaper://error', { monitorId: 'DISPLAY-2', wallpaperId: NEON, message }); });

  it('shows a dismissible card in Studio and clears it on the next change for that display', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    await fail();
    const card = screen.getByText(/Neon Rain City couldn.t be set as wallpaper/);
    expect(card.closest('.load-error-card')).toBeTruthy();
    // The raw backend message sits behind "Show details".
    expect(card.closest('.load-error-card')!.querySelector('details')!.open).toBe(false);
    expect(screen.getByText('Missing scene.pkg')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText(/couldn.t be set as wallpaper/)).toBeNull();

    await fail('again');
    expect(screen.getByText('again')).toBeTruthy();
    await act(async () => {
      await emit('wallpaper://changed', { monitorId: 'DISPLAY-2', source: { kind: 'photo_api' }, currentItem: null, transitionUsed: 'crossfade' });
    });
    expect(screen.queryByText(/couldn.t be set as wallpaper/)).toBeNull();
  });

  it('wallpaper://loaded clears the card too', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    await fail();
    expect(screen.getByText(/couldn.t be set as wallpaper/)).toBeTruthy();
    await act(async () => { await emit('wallpaper://loaded', { monitorId: 'DISPLAY-2', wallpaperId: NEON }); });
    expect(screen.queryByText(/couldn.t be set as wallpaper/)).toBeNull();
  });

  it('keeps the error when a different display changes, and Try again re-applies the wallpaper', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    await fail();
    await act(async () => {
      await emit('wallpaper://changed', { monitorId: 'DISPLAY-1', source: { kind: 'photo_api' }, currentItem: null, transitionUsed: 'crossfade' });
    });
    expect(screen.getByText(/couldn.t be set as wallpaper/)).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Try again' })); });
    expect(screen.queryByText(/couldn.t be set as wallpaper/)).toBeNull();
    expect(useAppStore.getState().monitors.find((m) => m.monitorId === 'DISPLAY-2')?.activeWallpaperId).toBe(NEON);
  });

  it('shows the same line in the tray popover', async () => {
    await act(async () => { render(<TrayApp />); });
    await screen.findByRole('button', { name: 'Next photo' });
    await act(async () => { useAppStore.getState().selectMonitor('DISPLAY-2'); });
    await act(async () => { await emit('wallpaper://error', { monitorId: 'DISPLAY-2', wallpaperId: NEON, message: 'GPU lost' }); });
    expect(screen.getByText(/Neon Rain City couldn.t be set as wallpaper/)).toBeTruthy();
    expect(screen.getByText('GPU lost')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText(/couldn.t be set as wallpaper/)).toBeNull();
  });
});

describe('scene report', () => {
  it('lists diagnostics in a collapsed section of the details drawer', async () => {
    await act(async () => { render(<App />); });
    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: 'Details for Neon Rain City' })); });
    expect(screen.queryByText(/Scene report/)).toBeNull();
    await act(async () => { await emit('wallpaper://diagnostics', { monitorId: 'DISPLAY-2', wallpaperId: NEON, messages: ['Shader foo skipped', 'Texture bar missing'] }); });
    const summary = screen.getByText('Scene report (2)');
    const details = summary.closest('details')!;
    expect(details.open).toBe(false);
    expect(within(details).getByText('Shader foo skipped')).toBeTruthy();
    expect(within(details).getByText('Texture bar missing')).toBeTruthy();
  });
});

describe('details drawer', () => {
  it('focuses the close button on open, closes on Escape and returns focus to the tile', async () => {
    await act(async () => { render(<App />); });
    const tile = await screen.findByRole('button', { name: 'Details for Neon Rain City' });
    await act(async () => { fireEvent.click(tile); });
    const close = screen.getByRole('button', { name: 'Close wallpaper details' });
    expect(document.activeElement).toBe(close);
    await act(async () => { fireEvent.keyDown(close, { key: 'Escape' }); });
    expect(screen.queryByRole('complementary', { name: 'Wallpaper details' })).toBeNull();
    expect(document.activeElement).toBe(tile);
  });

  it('adds a scrim for the overlay layout that closes the drawer', async () => {
    await act(async () => { render(<App />); });
    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: 'Details for Neon Rain City' })); });
    const scrim = document.querySelector('.inspector-scrim')!;
    expect(scrim).toBeTruthy();
    await act(async () => { fireEvent.click(scrim); });
    expect(screen.queryByRole('complementary', { name: 'Wallpaper details' })).toBeNull();
  });
});

describe('library search and sort', () => {
  it('searches by title, type and workshop id', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    const search = screen.getByRole('textbox', { name: 'Search library' });
    await act(async () => { fireEvent.change(search, { target: { value: '1966389027' } }); });
    expect(tileNames()).toEqual(['Lo-fi Train Window']);
    await act(async () => { fireEvent.change(search, { target: { value: 'scene' } }); });
    expect(tileNames()).toContain('Neon Rain City');
    expect(tileNames()).not.toContain('Lo-fi Train Window');
  });

  it('sorts by name and size and remembers the choice', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    const pick = async (label: string) => {
      fireEvent.click(screen.getByRole('combobox', { name: 'Sort library' }));
      await act(async () => { fireEvent.click(screen.getByRole('option', { name: label })); });
    };
    await pick('Name');
    const names = tileNames().filter((n) => n !== 'Ian Truong Photography');
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
    await pick('Size');
    const { wallpapers } = useAppStore.getState();
    const bySize = [...wallpapers].sort((a, b) => b.bytes - a.bytes).map((w) => w.title);
    expect(tileNames().filter((n) => n !== 'Ian Truong Photography')).toEqual(bySize);
    expect(JSON.parse(localStorage.getItem('truongpaper.library')!).sort).toBe('size');

    cleanup();
    await act(async () => { render(<App />); });
    expect(screen.getByRole('combobox', { name: 'Sort library' }).textContent).toBe('Size');
  });
});

describe('offline notice', () => {
  it('appears in Photography and the tray only while offline', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    fireEvent.click(screen.getByRole('button', { name: 'Photography' }));
    await screen.findByRole('heading', { level: 2, name: 'Ian Truong Photography' });
    expect(screen.queryByText(OFFLINE_NOTICE)).toBeNull();
    await act(async () => { useAppStore.setState({ photoStatus: { ...useAppStore.getState().photoStatus!, offline: true } }); });
    expect(screen.getByText('Offline - showing cached photos')).toBeTruthy();
    cleanup();
    await act(async () => { render(<TrayApp />); });
    await screen.findByRole('button', { name: 'Next photo' });
    expect(screen.queryByText(OFFLINE_NOTICE)).toBeNull();
    await act(async () => { useAppStore.setState({ photoStatus: { ...useAppStore.getState().photoStatus!, offline: true } }); });
    expect(screen.getByText('Offline - showing cached photos')).toBeTruthy();
  });
});

describe('debounced slider writes', () => {
  it('coalesces drag ticks, flushes on pointer up and keeps the last value', async () => {
    vi.useFakeTimers();
    try {
      const sent: number[] = [];
      const queue = new WriteQueue(150);
      for (const v of [1, 2, 3]) void queue.schedule('k', async () => { sent.push(v); });
      expect(sent).toEqual([]);
      await vi.advanceTimersByTimeAsync(149);
      expect(sent).toEqual([]);
      await vi.advanceTimersByTimeAsync(2);
      expect(sent).toEqual([3]);
      void queue.schedule('k', async () => { sent.push(4); });
      queue.flush('k');
      expect(sent).toEqual([3, 4]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores the failure of a superseded write but reports the latest', async () => {
    const queue = new WriteQueue(10);
    let failFirst!: (e: Error) => void;
    const first = queue.schedule('k', () => new Promise((_, reject) => { failFirst = reject; }));
    queue.flush('k');
    const second = queue.schedule('k', async () => { throw new Error('latest failed'); });
    failFirst(new Error('old failed'));
    await expect(first).resolves.toBeUndefined();
    queue.flush('k');
    await expect(second).rejects.toThrow('latest failed');
  });

  it('writes adjustments once after the drag and flushes on unmount', async () => {
    vi.useFakeTimers();
    try {
      await act(async () => { render(<App />); });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      const scene = useAppStore.getState().wallpapers.find((w) => w.wallpaperId === NEON)!;
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Details for Neon Rain City' })); });
      const volume = document.querySelectorAll<HTMLInputElement>('.property-inspector input[type="range"]')[0];
      for (const v of ['0.1', '0.2', '0.3']) fireEvent.change(volume, { target: { value: v } });
      expect(useAppStore.getState().wallpapers.find((w) => w.wallpaperId === NEON)!.adjustments.volume).toBe(0.3);
      // Backend (mock) still holds the old value until the debounce fires.
      const { invoke: realInvoke } = await import('@tauri-apps/api/core');
      let saved = await realInvoke<typeof scene[]>('library_list_wallpapers');
      expect(saved.find((w) => w.wallpaperId === NEON)!.adjustments.volume).toBe(scene.adjustments.volume);
      cleanup(); // unmount flushes the trailing write
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      saved = await realInvoke<typeof scene[]>('library_list_wallpapers');
      expect(saved.find((w) => w.wallpaperId === NEON)!.adjustments.volume).toBe(0.3);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('playbackSpeed above 2', () => {
  it('shows the stored value instead of capping it', async () => {
    await act(async () => { render(<App />); });
    await act(async () => {
      useAppStore.getState().setLocalWallpaperAdjustments(NEON, { ...useAppStore.getState().wallpapers.find((w) => w.wallpaperId === NEON)!.adjustments, playbackSpeed: 3 });
      fireEvent.click(await screen.findByRole('button', { name: 'Details for Neon Rain City' }));
    });
    expect(screen.getByText('3.00x')).toBeTruthy();
  });
});

describe('settings and displays additions', () => {
  const open = async (label: string) => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    await act(async () => { fireEvent.click(within(screen.getByRole('navigation', { name: 'Studio views' })).getByRole('button', { name: label })); });
  };

  it('saves keyboard shortcuts', async () => {
    await open('Settings');
    const next = await screen.findByPlaceholderText('CmdOrCtrl+Alt+Right');
    fireEvent.change(next, { target: { value: 'CmdOrCtrl+Alt+N' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Save shortcuts' })); });
    expect(useAppStore.getState().settings!.nextHotkey).toBe('CmdOrCtrl+Alt+N');
    expect(screen.getByText('Keyboard shortcuts')).toBeTruthy();
  });

  it('toggles span mode and writes per-display volume once after the drag', async () => {
    await open('Displays');
    await act(async () => { fireEvent.click(await screen.findByRole('checkbox', { name: /Span one wallpaper/ }, { timeout: 5000 })); });
    expect(screen.getByText('Works for photos and scenes. Videos and web wallpapers show on each display.')).toBeTruthy();
    expect(useAppStore.getState().settings!.multiMonitorMode).toBe('span');
    const volume = screen.getByRole('slider', { name: /^Volume for / });
    fireEvent.change(volume, { target: { value: '0.25' } });
    expect(useAppStore.getState().monitors[0].volume).toBe(0.25);
    await act(async () => { fireEvent.pointerUp(volume); });
  });
});
