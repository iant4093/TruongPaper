// @vitest-environment jsdom
// tests/uiFeatures.test.tsx — toasts and confirms, favorites, keyboard grid navigation, windowing,
// the per-display "Shows" picker, playlists, backup & restore, offline retry and Workshop changes.
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { installBrowserPreviewMocksIfNeeded } from '../src/dev/mockTauri';
import { App } from '../src/App';
import { TrayApp } from '../src/tray/TrayApp';
import { FeedbackHost } from '../src/components/Feedback';
import { VirtualGrid } from '../src/components/VirtualGrid';
import { WallpaperCard } from '../src/components/WallpaperCard';
import { useAppStore } from '../src/store/useAppStore';
import { confirmAction, toast, useFeedback } from '../src/store/feedback';

const initial = useAppStore.getState();

beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () => new Proxy({}, { get: (_t, prop) => (prop === 'createRadialGradient' ? () => ({ addColorStop() {} }) : () => {}) }) as never
  );
  installBrowserPreviewMocksIfNeeded();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  localStorage.clear();
  useFeedback.setState({ toasts: [], confirm: null });
  useAppStore.setState(initial, true);
});

const NEON = 'workshop:2841928371';
const LOFI = 'workshop:1966389027';
const nav = (label: string) => within(screen.getByRole('navigation', { name: 'Studio views' })).getByRole('button', { name: label });
const openApp = async () => {
  await act(async () => { render(<App />); });
  await screen.findByRole('button', { name: 'Select Neon Rain City' });
};
const goto = async (label: string, heading: RegExp | string) => {
  await act(async () => { fireEvent.click(nav(label)); });
  await screen.findByRole('heading', { level: 2, name: heading });
};

describe('toasts and confirmations', () => {
  it('shows a toast, closes it after 4 seconds and keeps errors longer', async () => {
    vi.useFakeTimers();
    render(<FeedbackHost />);
    act(() => { toast.success('Saved'); toast.error(new Error('Nope'), 'Could not save'); });
    expect(screen.getByText('Saved')).toBeTruthy();
    expect(screen.getByRole('alert').textContent).toContain('Could not save: Nope');
    await act(async () => { await vi.advanceTimersByTimeAsync(4100); });
    expect(screen.queryByText('Saved')).toBeNull();
    expect(screen.getByRole('alert')).toBeTruthy();
    await act(async () => { await vi.advanceTimersByTimeAsync(4100); });
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('runs Undo once and closes the toast', async () => {
    const undo = vi.fn();
    render(<FeedbackHost />);
    act(() => { toast.info('Deleted', undo); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Undo' })); });
    expect(undo).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Deleted')).toBeNull();
  });

  it('confirmAction resolves true on confirm, false on cancel and on Escape', async () => {
    render(<FeedbackHost />);
    let result!: Promise<boolean>;
    act(() => { result = confirmAction('Remove it?', 'Remove', 'This deletes files.'); });
    const dialog = screen.getByRole('alertdialog', { name: 'Remove it?' });
    expect(within(dialog).getByText('This deletes files.')).toBeTruthy();
    await act(async () => { fireEvent.click(within(dialog).getByRole('button', { name: 'Remove' })); });
    expect(await result).toBe(true);

    act(() => { result = confirmAction('Again?', 'Do it'); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); });
    expect(await result).toBe(false);

    act(() => { result = confirmAction('Third?', 'Do it'); });
    await act(async () => { fireEvent.keyDown(document, { key: 'Escape' }); });
    expect(await result).toBe(false);
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });
});

describe('Library cards', () => {
  it('applies from the full-width button and confirms with a toast', async () => {
    await openApp();
    const before = useAppStore.getState().monitors[0].activeSource;
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Set as wallpaper: Lo-fi Train Window' })); });
    await waitFor(() => expect(useAppStore.getState().monitors[0].activeWallpaperId).toBe(LOFI));
    expect(await screen.findByText('Now showing Lo-fi Train Window')).toBeTruthy();
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', before); });
  });

  it('no longer has a delete icon on tiles; removal is in the drawer behind a confirm', async () => {
    await openApp();
    expect(screen.queryByRole('button', { name: /^Delete / })).toBeNull();
    // Remove a throwaway import so the shared preview backend keeps its sample data.
    await act(async () => { await useAppStore.getState().importLocalWallpaper('/Users/me/Videos/temp.mp4'); });
    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: 'Details for temp' })); });
    const drawer = within(screen.getByRole('complementary', { name: 'Wallpaper details' }));
    await act(async () => { fireEvent.click(drawer.getByRole('button', { name: /Remove from library/ })); });
    const dialog = screen.getByRole('alertdialog');
    expect(within(dialog).getByText(/Remove “temp” from your library\?/)).toBeTruthy();
    await act(async () => { fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' })); });
    expect(useAppStore.getState().wallpapers.some((w) => w.title === 'temp')).toBe(true);

    await act(async () => { fireEvent.click(drawer.getByRole('button', { name: /Remove from library/ })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Remove' })); });
    await waitFor(() => expect(useAppStore.getState().wallpapers.some((w) => w.title === 'temp')).toBe(false));
    expect(await screen.findByText('Removed temp')).toBeTruthy();
  });

  it('stars a wallpaper from a tile and filters the Library to favorites', async () => {
    await openApp();
    expect(screen.getByText('Favorites')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Add Neon Rain City to favorites' })); });
    expect(useAppStore.getState().wallpapers.find((w) => w.wallpaperId === NEON)?.isFavorite).toBe(true);
    expect(screen.getByRole('button', { name: 'Remove Neon Rain City from favorites' }).getAttribute('aria-pressed')).toBe('true');

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /^Favorites/ })); });
    const names = screen.getAllByRole('button', { name: /^Select / }).map((b) => b.getAttribute('aria-label'));
    expect(names).toEqual(['Select Neon Rain City']);

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Remove Neon Rain City from favorites' })); });
    expect(screen.getByText(/No favorites yet/)).toBeTruthy();
    expect(useAppStore.getState().wallpapers.find((w) => w.wallpaperId === NEON)?.isFavorite).toBe(false);
  });

  it('stars from the details drawer', async () => {
    await openApp();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Details for Lo-fi Train Window' })); });
    const drawer = within(screen.getByRole('complementary', { name: 'Wallpaper details' }));
    await act(async () => { fireEvent.click(drawer.getByRole('button', { name: /Add to favorites/ })); });
    expect(useAppStore.getState().wallpapers.find((w) => w.wallpaperId === LOFI)?.isFavorite).toBe(true);
    expect(drawer.getByRole('button', { name: /Favorited/ })).toBeTruthy();
    await act(async () => { fireEvent.click(drawer.getByRole('button', { name: /Favorited/ })); });
    expect(useAppStore.getState().wallpapers.find((w) => w.wallpaperId === LOFI)?.isFavorite).toBe(false);
  });

  it('shows a helpful message and a way out when a search matches nothing', async () => {
    await openApp();
    fireEvent.change(screen.getByRole('textbox', { name: 'Search library' }), { target: { value: 'zzzz' } });
    expect(await screen.findByText(/No matches for/)).toBeTruthy();
    const empty = (await screen.findByText(/No matches for/)).closest('.empty-state') as HTMLElement;
    fireEvent.click(within(empty).getByRole('button', { name: 'Clear search' }));
    expect(screen.queryByText(/No matches for/)).toBeNull();
  });

  it('does not remember the search text between launches', async () => {
    await openApp();
    fireEvent.change(screen.getByRole('textbox', { name: 'Search library' }), { target: { value: 'neon' } });
    expect(JSON.parse(localStorage.getItem('truongpaper.library')!)).not.toHaveProperty('query');
  });

  it('guides an empty library to the Workshop', async () => {
    await openApp();
    await act(async () => { useAppStore.setState({ wallpapers: [] }); });
    expect(screen.getByText(/Your library is empty/)).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Browse Workshop' })); });
    expect(await screen.findByRole('heading', { level: 2, name: 'Workshop' })).toBeTruthy();
  });
});

describe('keyboard navigation in the grid', () => {
  it('moves with the arrow keys, applies with Enter and jumps to search with /', async () => {
    await openApp();
    const tiles = () => screen.getAllByRole('button', { name: /^Select / });
    const first = tiles()[0];
    expect(first.getAttribute('aria-label')).toBe('Select Ian Truong Photography');
    first.focus();
    fireEvent.keyDown(first, { key: 'ArrowRight' });
    expect(document.activeElement).toBe(tiles()[1]);
    fireEvent.keyDown(document.activeElement!, { key: 'End' });
    expect(document.activeElement).toBe(tiles()[tiles().length - 1]);
    fireEvent.keyDown(document.activeElement!, { key: 'Home' });
    expect(document.activeElement).toBe(tiles()[0]);
    // Only the focused tile is a tab stop.
    expect(tiles().filter((t) => t.tabIndex === 0)).toHaveLength(1);

    const before = useAppStore.getState().monitors[0].activeSource;
    const target = tiles()[1];
    target.focus();
    await act(async () => { fireEvent.keyDown(target, { key: 'Enter' }); });
    await waitFor(() => expect(useAppStore.getState().monitors[0].activeSource.kind).toBe('workshop'));
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', before); });

    (document.activeElement as HTMLElement).blur();
    fireEvent.keyDown(document.body, { key: '/' });
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Search library' }));
  });
});

describe('VirtualGrid windowing', () => {
  const sizes = { w: 1000, h: 600 };
  let scrollTop = 0;
  let realRect: () => DOMRect;
  const defineSize = () => {
    scrollTop = 0;
    // Without layout every rect is 0; make the grid move up as its scroller scrolls, like in a browser.
    realRect = HTMLElement.prototype.getBoundingClientRect;
    HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
      const top = this.getAttribute('role') === 'group' ? -scrollTop : 0;
      return { top, bottom: top, left: 0, right: 0, width: 0, height: 0, x: 0, y: top, toJSON() {} } as DOMRect;
    };
    Object.defineProperty(HTMLElement.prototype, 'scrollTop', { configurable: true, get: () => scrollTop, set: (v: number) => { scrollTop = v; } });
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => sizes.w });
    Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => sizes.h });
  };
  const restoreSize = () => {
    HTMLElement.prototype.getBoundingClientRect = realRect;
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).scrollTop;
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientWidth;
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
  };
  const items = Array.from({ length: 200 }, (_, i) => ({ id: `item-${i}`, title: `Item ${i}` }));
  const Harness = () => (
    <div data-testid="scroller" style={{ overflowY: 'auto', height: 600 }}>
      <VirtualGrid items={items} getKey={(i) => i.id} label="Test grid"
        renderItem={(item, cell) => (
          <WallpaperCard id={item.id} title={item.title} preview={null} kind="Scene" canApply tabIndex={cell.tabIndex}
            onSelect={() => {}} onApply={() => {}} />
        )} />
    </div>
  );

  it('renders only the rows near the viewport and follows the scroll position', async () => {
    defineSize();
    try {
      render(<Harness />);
      await waitFor(() => expect(screen.getAllByRole('button', { name: /^Select / }).length).toBeLessThan(40));
      const rendered = () => screen.getAllByRole('button', { name: /^Select / });
      expect(rendered().length).toBeGreaterThanOrEqual(8);
      expect(rendered()[0].getAttribute('aria-label')).toBe('Select Item 0');
      const grid = screen.getByRole('group', { name: 'Test grid' });
      expect(parseFloat(grid.style.paddingBottom)).toBeGreaterThan(1000);

      const scroller = screen.getByTestId('scroller');
      scroller.scrollTop = 6000;
      fireEvent.scroll(scroller);
      await waitFor(() => expect(rendered()[0].getAttribute('aria-label')).not.toBe('Select Item 0'));
      expect(rendered().length).toBeLessThan(40);
    } finally { restoreSize(); }
  });

  it('keyboard End scrolls to and focuses a tile that was not rendered', async () => {
    defineSize();
    try {
      render(<Harness />);
      await waitFor(() => expect(screen.getAllByRole('button', { name: /^Select / }).length).toBeLessThan(40));
      const first = screen.getByRole('button', { name: 'Select Item 0' });
      first.focus();
      fireEvent.keyDown(first, { key: 'End' });
      fireEvent.scroll(screen.getByTestId('scroller'));
      await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('Select Item 199'));
    } finally { restoreSize(); }
  });

  it('renders small lists completely', () => {
    render(<VirtualGrid items={items.slice(0, 20)} getKey={(i) => i.id} label="Small"
      renderItem={(item) => <WallpaperCard id={item.id} title={item.title} preview={null} kind="Scene" canApply onSelect={() => {}} onApply={() => {}} />} />);
    expect(screen.getAllByRole('button', { name: /^Select / })).toHaveLength(20);
  });
});

describe('Displays: Shows picker', () => {
  it('lets each display show photos, one wallpaper or a playlist', async () => {
    await openApp();
    await goto('Displays', /^Displays$/);
    const picker = await screen.findByRole('combobox', { name: /Shows on Monitor 1/ });
    expect(picker.textContent).toBe('Photos');
    const before = useAppStore.getState().monitors[0].activeSource;
    try {
      fireEvent.click(picker);
      await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'Playlist: Evening Mix' })); });
      await waitFor(() => expect(useAppStore.getState().monitors[0].activeSource).toEqual({ kind: 'playlist', playlistId: 'demo-playlist-1' }));
      expect(await screen.findByText(/Monitor 1.* now shows Playlist: Evening Mix/)).toBeTruthy();

      fireEvent.click(screen.getByRole('combobox', { name: /Shows on Monitor 1/ }));
      await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'Lo-fi Train Window' })); });
      await waitFor(() => expect(useAppStore.getState().monitors[0].activeSource).toEqual({ kind: 'workshop', wallpaperId: LOFI }));

      fireEvent.click(screen.getByRole('combobox', { name: /Shows on Monitor 1/ }));
      await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'Photos' })); });
      await waitFor(() => expect(useAppStore.getState().monitors[0].activeSource.kind).toBe('photo_api'));
    } finally {
      await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', before); });
    }
  });
});

describe('Playlists', () => {
  it('creates several playlists, shows which displays use them and deletes with undo', async () => {
    await openApp();
    await goto('Playlists', /^Playlists$/);
    const evening = useAppStore.getState().playlists.find((p) => p.playlistId === 'demo-playlist-1')!;
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-2', { kind: 'playlist', playlistId: 'demo-playlist-1' }); });
    expect(await screen.findByText(/Playing on: Monitor 2/)).toBeTruthy();
    expect(screen.getByText(/Photos \+ wallpapers · 3 items · every 15 min/)).toBeTruthy();

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'New playlist' })); });
    fireEvent.change(screen.getByRole('textbox', { name: 'Playlist name' }), { target: { value: 'Calm' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Save playlist' })); });
    await waitFor(() => expect(useAppStore.getState().playlists.map((p) => p.name)).toContain('Calm'));
    expect(screen.getByText('Not used by any display')).toBeTruthy();

    // Rename an existing one.
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Edit Calm' })); });
    fireEvent.change(screen.getByRole('textbox', { name: 'Playlist name' }), { target: { value: 'Calmer' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Save playlist' })); });
    await waitFor(() => expect(useAppStore.getState().playlists.map((p) => p.name)).toContain('Calmer'));

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Delete Calmer' })); });
    expect(screen.getByRole('alertdialog')).toBeTruthy();
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Delete playlist' })); });
    await waitFor(() => expect(useAppStore.getState().playlists.map((p) => p.name)).not.toContain('Calmer'));

    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: 'Undo' })); });
    await waitFor(() => expect(useAppStore.getState().playlists.map((p) => p.name)).toContain('Calmer'));

    // Deleting the playlist a display plays sends that display back to photos.
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Delete Evening Mix' })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Delete playlist' })); });
    await waitFor(() => expect(useAppStore.getState().monitors[1].activeSource.kind).toBe('photo_api'));
    // Put the shared sample data back for later tests.
    await act(async () => {
      await useAppStore.getState().savePlaylist(evening);
      await useAppStore.getState().applyMonitorSource('DISPLAY-2', { kind: 'workshop', wallpaperId: NEON });
    });
  });

  it('invites you to create the first playlist', async () => {
    await openApp();
    await goto('Playlists', /^Playlists$/);
    await act(async () => { useAppStore.setState({ playlists: [] }); });
    expect(screen.getByText('No playlists yet.')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Create your first playlist' })); });
    expect(screen.getByRole('textbox', { name: 'Playlist name' })).toBeTruthy();
  });

  it('uses plain labels instead of jargon', async () => {
    await openApp();
    await goto('Playlists', /^Playlists$/);
    for (const word of [/Rotation Source/, /GPU Transition/, /Unified Mixed/, /Per Monitor/]) {
      expect(document.body.textContent).not.toMatch(word);
    }
    expect(screen.getByText('Transition effect')).toBeTruthy();
    expect(screen.getByText('Change photo every')).toBeTruthy();
  });
});

describe('Settings: backup & restore', () => {
  it('saves a backup through the save dialog', async () => {
    await openApp();
    await goto('Settings', /^Settings$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Save backup/ })); });
    expect(await screen.findByText('Backup saved')).toBeTruthy();
  });

  it('restores, summarizes and downloads the missing Workshop wallpapers', async () => {
    await openApp();
    await goto('Settings', /^Settings$/);
    const downloads: string[] = [];
    const real = useAppStore.getState().downloadWorkshopItem;
    useAppStore.setState({ downloadWorkshopItem: async (id, source) => { downloads.push(id); return real(id, source); } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Restore from backup/ })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Restore' })); });
    const summary = await screen.findByText(/Restored settings, \d+ playlists?/);
    expect(summary.textContent).toMatch(/\d+ wallpapers?/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Download missing (2)' })); });
    expect(downloads).toEqual(['3800000001', '3800000002']);
  });

  it('does not restore when the confirmation is cancelled', async () => {
    await openApp();
    await goto('Settings', /^Settings$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Restore from backup/ })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Cancel' })); });
    expect(screen.queryByText(/Restored/)).toBeNull();
  });

  it('confirms before signing out of Steam and says when a setting was saved', async () => {
    await openApp();
    useAppStore.setState({ steamAccount: 'ian' });
    await goto('Settings', /^Settings$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Sign out' })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Cancel' })); });
    expect(useAppStore.getState().steamAccount).toBe('ian');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Match display' })); });
    expect(await screen.findByText('Saved')).toBeTruthy();
    expect(useAppStore.getState().settings!.fpsCap).toBe('display_native');
    await act(async () => { await useAppStore.getState().updateSettings({ fpsCap: 60 }); });
  });
});

describe('offline mode', () => {
  it('offers Retry now in Photography and the Library', async () => {
    await openApp();
    const retry = vi.fn(async () => {});
    useAppStore.setState({ retryPhotoNow: retry, photoStatus: { ...useAppStore.getState().photoStatus!, offline: true } });
    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: 'Retry now' })); });
    expect(retry).toHaveBeenCalledTimes(1);
    await goto('Photography', /^Ian Truong Photography$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry now' })); });
    expect(retry).toHaveBeenCalledTimes(2);
  });
});

describe('Photography page', () => {
  it('puts the current wallpaper first and offers "Use photos on this display" when needed', async () => {
    await openApp();
    await goto('Photography', /^Ian Truong Photography$/);
    const titles = [...document.querySelectorAll('.section-title')].map((e) => e.textContent);
    expect(titles[0]).toBe('Current wallpaper');
    expect(screen.queryByRole('button', { name: 'Use photos on this display' })).toBeNull();
    const before = useAppStore.getState().monitors[0].activeSource;
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', { kind: 'workshop', wallpaperId: LOFI }); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Use photos on this display' })); });
    await waitFor(() => expect(useAppStore.getState().monitors[0].activeSource.kind).toBe('photo_api'));
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', before); });
  });

  it('confirms before clearing the cache and renames the history reset', async () => {
    await openApp();
    await goto('Photography', /^Ian Truong Photography$/);
    expect(screen.getByRole('button', { name: /Show all photos again/ })).toBeTruthy();
    expect(screen.queryByText(/API requests/)).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Clear cache/ })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Cancel' })); });
    const bytes = useAppStore.getState().photoStatus!.cachedBytes;
    expect(bytes).toBeGreaterThan(0);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Clear cache/ })); });
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Clear cache' })); });
    expect(await screen.findByText('Cache cleared')).toBeTruthy();
  });

  it('confirms before removing a saved album', async () => {
    await openApp();
    useAppStore.setState({ savedAlbums: [{ albumSlug: 'cape-horn-11-7', displayName: 'Cape Horn', albumUrl: 'https://x', isEnabled: true, addedAt: 0 }] });
    await goto('Photography', /^Ian Truong Photography$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Remove Cape Horn' })); });
    expect(screen.getByRole('alertdialog')).toBeTruthy();
    await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Cancel' })); });
    expect(useAppStore.getState().savedAlbums.length).toBe(1);
  });
});

describe('Workshop page', () => {
  it('has one Steam status line and no duplicate account buttons', async () => {
    await openApp();
    await goto('Workshop', /^Workshop$/);
    expect(screen.getByText('Not signed in to Steam.')).toBeTruthy();
    expect(screen.getAllByRole('button', { name: 'Sign in' })).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Steam account' })).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Installed' })); });
    expect(screen.queryByText('Steam Account')).toBeNull();
    expect(screen.getByRole('button', { name: /Refresh library/ })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'From Steam app' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Reacts to sound' })).toBeTruthy();
  });

  it('never pops the Steam sign-in on its own; it offers a Sign in button instead', async () => {
    await openApp();
    await goto('Workshop', /^Workshop$/);
    const input = await screen.findByRole('searchbox', { name: 'Search Steam Workshop' });
    const original = useAppStore.getState().inspectWorkshopUrl;
    useAppStore.setState({ inspectWorkshopUrl: async () => { throw new Error('Steam login required'); } });
    fireEvent.change(input, { target: { value: '2841928371' } });
    await act(async () => { fireEvent.submit(input.closest('form')!); });
    expect(await screen.findByText('Steam login required')).toBeTruthy();
    expect(useAppStore.getState().authModalOpen).toBe(false);
    useAppStore.setState({ inspectWorkshopUrl: original });
    const signIn = screen.getAllByRole('button', { name: 'Sign in' });
    await act(async () => { fireEvent.click(signIn[signIn.length - 1]); });
    expect(useAppStore.getState().authModalOpen).toBe(true);
  });

  it('pages with Previous and Next only', async () => {
    await openApp();
    await goto('Workshop', /^Workshop$/);
    await screen.findAllByRole('button', { name: /^Preview / });
    expect(screen.getByRole('button', { name: 'Next Workshop page' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'First' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Last' })).toBeNull();
    expect(screen.queryByRole('spinbutton')).toBeNull();
    expect(screen.queryByText('Public catalog')).toBeNull();
  });

  it('shows an empty Installed tab with a way to browse', async () => {
    await openApp();
    await goto('Workshop', /^Workshop$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Installed' })); });
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search Workshop library' }), { target: { value: 'zzzz' } });
    expect(screen.getByText('No installed wallpapers match.')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Browse Discover' })); });
    expect(await screen.findByRole('searchbox', { name: 'Search Steam Workshop' })).toBeTruthy();
  });

  it('retries or dismisses a failed download', async () => {
    await openApp();
    await goto('Workshop', /^Workshop$/);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Installed' })); });
    await act(async () => { useAppStore.setState({ downloadsById: { '77': { workshopId: '77', title: 'Broken', progressPercent: 10, state: 'error', error: 'Disk full' } } }); });
    expect(screen.getByText('Disk full')).toBeTruthy();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Dismiss' })); });
    // Mock downloads started by earlier tests can still report progress, so check only this one.
    expect(useAppStore.getState().downloadsById['77']).toBeUndefined();
  });
});

describe('shell', () => {
  it('has no breadcrumb and a labelled display picker; the drawer overlays instead of resizing the grid', async () => {
    await openApp();
    expect(document.querySelector('.breadcrumb')).toBeNull();
    expect(screen.getByText('Display:')).toBeTruthy();
    expect(screen.queryByText('Workspace')).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Details for Neon Rain City' })); });
    expect(document.querySelector('.inspector-scrim')).toBeTruthy();
  });

  it('explains why Next photo is unavailable', async () => {
    await openApp();
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', { kind: 'workshop', wallpaperId: LOFI }); });
    useAppStore.setState((s) => ({ photoStatus: { ...s.photoStatus!, currentPhotoByMonitor: { ...s.photoStatus!.currentPhotoByMonitor, 'DISPLAY-1': null } } }));
    const next = await screen.findByRole('button', { name: 'Next photo' });
    expect((next as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('Next photo works while this display shows photos.')).toBeTruthy();
    await act(async () => { await useAppStore.getState().applyMonitorSource('DISPLAY-1', { kind: 'photo_api' }); });
  });

  it('reports a failed debounced write as a toast', async () => {
    await openApp();
    await act(async () => { useAppStore.setState({ writeError: 'Could not save the change: boom' }); });
    expect(await screen.findByText('Could not save the change: boom')).toBeTruthy();
    expect(useAppStore.getState().writeError).toBeNull();
  });
});

describe('Tray popover', () => {
  it('uses a display dropdown, a bigger preview slot, wrapped errors and a retry for offline mode', async () => {
    await act(async () => { render(<TrayApp />); });
    await screen.findByRole('button', { name: 'Next photo' });
    expect(screen.getByRole('combobox', { name: 'Display' })).toBeTruthy();
    expect(screen.queryByRole('combobox', { name: /Transition/ })).toBeNull();
    const retry = vi.fn(async () => {});
    await act(async () => { useAppStore.setState({ retryPhotoNow: retry, photoStatus: { ...useAppStore.getState().photoStatus!, offline: true } }); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Retry now' })); });
    expect(retry).toHaveBeenCalled();
  });
});
