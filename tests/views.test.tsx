// @vitest-environment jsdom
// tests/views.test.tsx — every studio view and the tray popover render against the
// browser-preview mock backend (real @tauri-apps/api modules + mockIPC).
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { installBrowserPreviewMocksIfNeeded } from '../src/dev/mockTauri';
import { App, STUDIO_TABS } from '../src/App';
import { TrayApp } from '../src/tray/TrayApp';
import { useAppStore } from '../src/store/useAppStore';

const HEADINGS: Record<string, RegExp> = {
  all: /^All Wallpapers$/,
  photos: /^Ian Truong Photography API$/,
  workshop: /^Steam Workshop Library$/,
  playlists: /^Unified Playlists & Rotation$/,
  monitors: /^Monitors & Fit$/,
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
    // Mock data loaded (two monitors in the header).
    expect(await screen.findByText(/Monitor 2: 24" Vertical/)).toBeTruthy();

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
      fireEvent.click(within(nav).getByRole('button', { name: 'Steam Workshop' }));
    });
    await screen.findByRole('heading', { level: 2, name: HEADINGS.workshop }, { timeout: 2000 });
    expect(await screen.findByText('WALLPAPER PROPERTIES')).toBeTruthy();
    expect(screen.getByText('Scheme Color')).toBeTruthy();
    expect(screen.getByText('Rain Speed')).toBeTruthy();
  });

  it('rotates to the next photo through the mock backend', async () => {
    await act(async () => {
      render(<App />);
    });
    await screen.findByText(/Monitor 2: 24" Vertical/);
    const before = useAppStore.getState().photoStatus!.historyCount;
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /^Next/ }));
    });
    await vi.waitFor(() => expect(useAppStore.getState().photoStatus!.historyCount).toBe(before + 1));
  });
});

describe('Tray popover', () => {
  it('renders current wallpaper, controls and queue footer', async () => {
    await act(async () => {
      render(<TrayApp />);
    });
    expect(await screen.findByText('Current Wallpaper')).toBeTruthy();
    expect(screen.getByText('Next Wallpaper')).toBeTruthy();
    expect(await screen.findByText(/Offline Ready/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /Favorite/ })).toBeTruthy();
  });
});
