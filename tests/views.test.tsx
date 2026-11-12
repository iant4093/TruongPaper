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
  photos: /^Ian Truong Photography$/,
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
      const workshopImport = screen.queryByRole('textbox', { name: 'Steam Workshop link or item ID' });
      expect(!!workshopImport).toBe(key === 'workshop');
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

  it('edits shared Photography settings from the Library details drawer', async () => {
    await act(async () => { render(<App />); });
    const selectPhotography = screen.getByRole('button', { name: 'Select Ian Truong Photography' });
    const fitBefore = useAppStore.getState().settings!.defaultFitMode;
    const sourcesBefore = useAppStore.getState().monitors.map((monitor) => monitor.activeSource);
    await act(async () => { fireEvent.click(selectPhotography); });
    const details = within(screen.getByRole('complementary', { name: 'Wallpaper details' }));
    expect(details.getByRole('heading', { name: 'Ian Truong Photography' })).toBeTruthy();
    expect(details.getByRole('combobox', { name: 'Photo pool' })).toBeTruthy();
    expect(details.getByRole('combobox', { name: 'Change photo every' })).toBeTruthy();
    expect(details.getByRole('checkbox', { name: 'Match photo orientation to display' })).toBeTruthy();

    const [fit, label] = fitBefore === 'fit_black' ? ['fill', 'Fill / Center Crop'] : ['fit_black', 'Fit + Black Bars'];
    fireEvent.click(details.getByRole('combobox', { name: 'Photo fit' }));
    try {
      await act(async () => { fireEvent.click(screen.getByRole('option', { name: label })); });
      expect(useAppStore.getState().settings!.defaultFitMode).toBe(fit);
      expect(useAppStore.getState().monitors.map((monitor) => monitor.activeSource)).toEqual(sourcesBefore);

      fireEvent.click(screen.getByRole('button', { name: 'Toggle wallpaper details' }));
      expect(screen.queryByRole('complementary', { name: 'Wallpaper details' })).toBeNull();
      fireEvent.click(selectPhotography);
      expect(screen.getByRole('complementary', { name: 'Wallpaper details' })).toBeTruthy();
      fireEvent.click(screen.getByRole('button', { name: 'Select Neon Rain City' }));
      expect(screen.getByText('Wallpaper properties')).toBeTruthy();
      expect(screen.queryByRole('combobox', { name: 'Photo pool' })).toBeNull();
    } finally {
      await act(async () => { await useAppStore.getState().updateSettings({ defaultFitMode: fitBefore }); });
    }
  });

  it('shows the offline queue only in Photography and filters Library by installed wallpaper types', async () => {
    await act(async () => { render(<App />); });
    await screen.findByRole('button', { name: 'Select Lo-fi Train Window' });
    const { photoStatus, wallpapers } = useAppStore.getState();
    const queuedPhoto = photoStatus!.unusedQueue[0];
    await act(async () => {
      useAppStore.setState({
        wallpapers: wallpapers.filter((wallpaper) => wallpaper.type === 'video'),
        photoStatus: {
          ...photoStatus!,
          unusedQueue: [
            ...Array.from({ length: 8 }, (_, i) => ({ ...queuedPhoto, id: `offline-${i}`, albumSlug: 'offline-landscape', width: 6000, height: 4000 })),
            { ...queuedPhoto, id: 'portrait-offline', width: 4000, height: 6000, albumSlug: 'portrait-only' },
          ],
        },
      });
    });
    expect(screen.queryByText(/photos ready offline/)).toBeNull();
    expect(screen.queryByRole('img', { name: /offline.landscape/ })).toBeNull();
    expect(screen.getByRole('button', { name: 'Videos' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Scenes' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Web' })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Photography' }));
    expect(screen.getByText('8 photos ready offline')).toBeTruthy();
    expect(screen.getAllByRole('img', { name: 'offline landscape' })).toHaveLength(8);
    expect(screen.queryByRole('img', { name: 'portrait only' })).toBeNull();
  });

  it('applies the inspected wallpaper and keeps Studio and tray pause controls in sync', async () => {
    const studio = await act(async () => render(<App />));
    const tray = await act(async () => render(<TrayApp />));
    const studioUi = within(studio.container);
    const trayUi = within(tray.container);
    const current = useAppStore.getState().monitors[0].activeSource;
    try {
      fireEvent.click(studioUi.getByRole('button', { name: 'Select Neon Rain City' }));
      const drawer = within(studioUi.getByRole('complementary', { name: 'Wallpaper details' }));
      await act(async () => { fireEvent.click(drawer.getByRole('button', { name: 'Apply wallpaper' })); });
      expect(drawer.getByText('Current on this display')).toBeTruthy();

      await act(async () => { fireEvent.click(studioUi.getByRole('button', { name: 'Pause' })); });
      expect(trayUi.getByRole('button', { name: 'Resume' })).toBeTruthy();
      expect(useAppStore.getState().monitors[1].userPaused).toBe(false);
      await act(async () => { fireEvent.click(trayUi.getByRole('button', { name: 'Resume' })); });
      expect(studioUi.getByRole('button', { name: 'Pause' })).toBeTruthy();
    } finally {
      await act(async () => {
        await useAppStore.getState().setPlaybackPaused(false, 'DISPLAY-1');
        await useAppStore.getState().applyMonitorSource('DISPLAY-1', current);
      });
    }
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

  it('shows a saved photo after rotation and reapplies it from Photography favorites', async () => {
    await act(async () => { render(<App />); });
    const before = useAppStore.getState().photoStatus!.currentPhotoByMonitor['DISPLAY-1']!;
    await act(async () => {
      await invoke('photo_set_favorite', { photoId: before.id, favorite: true });
      await invoke('photo_rotate_next', { monitorId: 'DISPLAY-1' });
      fireEvent.click(screen.getByRole('button', { name: 'Photography' }));
    });
    const favorites = screen.getByRole('region', { name: /^Favorites/ });
    const preview = within(favorites).getByRole('img', { name: before.albumSlug.replace(/-/g, ' ') });
    const tile = within(preview.closest('article')!);
    await act(async () => { fireEvent.click(tile.getByRole('button', { name: 'Apply' })); });
    expect(useAppStore.getState().photoStatus!.currentPhotoByMonitor['DISPLAY-1']?.id).toBe(before.id);
    expect(tile.getByRole('button', { name: 'Current' })).toBeTruthy();
  });
});

describe('Tray popover', () => {
  it('renders current wallpaper, controls and cache size', async () => {
    const view = render(<TrayApp />);
    expect(await screen.findByText('Current Wallpaper')).toBeTruthy();
    expect(screen.getByText('Next Wallpaper')).toBeTruthy();
    expect(view.container.querySelector('.tray-wallpaper-preview img')).toBeTruthy();
    expect(await screen.findByText(/MB cached$/)).toBeTruthy();
    expect(screen.queryByText(/photos? ready offline/)).toBeNull();
    expect(screen.queryByRole('textbox')).toBeNull();
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
