// @vitest-environment jsdom
// tests/views.test.tsx — every studio view and the tray popover render against the
// browser-preview mock backend (real @tauri-apps/api modules + mockIPC).
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { installBrowserPreviewMocksIfNeeded } from '../src/dev/mockTauri';
import { App, STUDIO_TABS } from '../src/App';
import { TrayApp } from '../src/tray/TrayApp';
import { useAppStore } from '../src/store/useAppStore';

const HEADINGS: Record<string, RegExp> = {
  all: /^Library$/,
  photos: /^Photography$/,
  workshop: /^Workshop$/,
  playlists: /^Playlists$/,
  monitors: /^Displays$/,
  settings: /^Settings$/,
};

const initial = useAppStore.getState();

beforeAll(() => {
  // jsdom has no 2D canvas; the ambient backlight & QR canvas just need a no-op context.
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () =>
      new Proxy({}, { get: (_t, prop) => (prop === 'createRadialGradient' ? () => ({ addColorStop() {} }) : () => {}) }) as never
  );
  installBrowserPreviewMocksIfNeeded();
});

afterEach(() => {
  cleanup();
  useAppStore.setState(initial, true);
});

describe('Main studio window', () => {
  it('renders every sidebar view with mock data and no TODO stubs', async () => {
    await act(async () => {
      render(<App />);
    });
    // Mock data loaded in the display selector.
    expect(await screen.findByRole('combobox', { name: 'Active display' })).toBeTruthy();

    const nav = screen.getByRole('navigation', { name: 'Studio views' });
    for (const [key, label] of STUDIO_TABS) {
      await act(async () => {
        fireEvent.click(within(nav).getByRole('button', { name: label }));
      });
      const heading = await screen.findByRole('heading', { level: 2, name: HEADINGS[key] }, { timeout: 2000 });
      expect(heading).toBeTruthy();
      expect(document.body.textContent ?? '').not.toMatch(/TODO|FIXME|Not implemented/i);
    }
  });

  it('shows the Property Inspector for a selected Wallpaper Engine item', async () => {
    await act(async () => {
      render(<App />);
    });
    const nav = screen.getByRole('navigation', { name: 'Studio views' });
    await act(async () => {
      fireEvent.click(within(nav).getByRole('button', { name: 'Workshop' }));
    });
    await screen.findByRole('heading', { level: 2, name: HEADINGS.workshop }, { timeout: 2000 });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Installed' })); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Select Neon Rain City' })); });
    expect(await screen.findByText('Wallpaper properties')).toBeTruthy();
    expect(screen.getByText('Scheme Color')).toBeTruthy();
    expect(screen.getByText('Rain Speed')).toBeTruthy();
  });

  it('filters the library and opens details only after selecting a wallpaper', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Neon Rain City' });
    expect(screen.queryByRole('complementary', { name: 'Wallpaper details' })).toBeNull();
    await act(async () => { fireEvent.change(screen.getByRole('textbox', { name: 'Search library' }), { target: { value: 'Neon' } }); });
    expect(screen.queryByRole('button', { name: 'Select Lo-fi Train Window' })).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Select Neon Rain City' })); });
    expect(screen.getByRole('complementary', { name: 'Wallpaper details' })).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Toggle wallpaper details' })); });
    expect(screen.queryByRole('complementary', { name: 'Wallpaper details' })).toBeNull();
  });

  it('rotates to the next photo through the mock backend', async () => {
    await act(async () => {
      render(<App />);
    });
    await screen.findByRole('combobox', { name: 'Active display' });
    const before = useAppStore.getState().photoStatus!.historyCount;
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^Next/ }));
    });
    await vi.waitFor(() => expect(useAppStore.getState().photoStatus!.historyCount).toBe(before + 1));
  });
});

describe('Tray popover', () => {
  it('renders current wallpaper, controls and queue footer', async () => {
    const view = render(<TrayApp />);
    expect(await screen.findByText('Current Wallpaper')).toBeTruthy();
    expect(screen.getByText('Next Wallpaper')).toBeTruthy();
    expect(view.container.querySelector('.tray-wallpaper-preview img')).toBeTruthy();
    expect(await screen.findByText(/Offline Ready/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /Favorite/ })).toBeTruthy();
  });

  it('updates an open popover when Studio applies a video and can switch back to photos', async () => {
    await act(async () => { render(<TrayApp />); });
    await screen.findByRole('button', { name: 'Next Wallpaper' });

    await act(async () => {
      await invoke('monitor_apply_wallpaper', {
        monitorId: 'DISPLAY-1',
        source: { kind: 'workshop', wallpaperId: 'workshop:1966389027' },
      });
    });
    expect(screen.getByText('Lo-fi Train Window')).toBeTruthy();
    expect(screen.getByText(/^Video ·/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: /Favorite/ })).toBeNull();

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Switch to Photos' })); });
    expect(screen.queryByText('Lo-fi Train Window')).toBeNull();
    expect(screen.getByRole('button', { name: 'Next Wallpaper' })).toBeTruthy();
  });
});
