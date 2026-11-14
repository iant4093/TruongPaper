// @vitest-environment jsdom
// tests/store.test.ts — Zustand store actions must call the §5.1 commands with the exact arg keys.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.fn();
const listeners = new Map<string, (e: { payload: unknown }) => void>();
const unlistenSpies: Array<ReturnType<typeof vi.fn>> = [];

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(event, cb);
    const un = vi.fn();
    unlistenSpies.push(un);
    return un;
  }),
  emit: vi.fn(async () => undefined),
}));

import { legacyAlbumsToMigrate, mergeNewWallpapers, useAppStore } from '../src/store/useAppStore';
import { useFeedback } from '../src/store/feedback';
import { IPC_COMMANDS } from '../src/types/ipc';

const status = { filterMode: 'all', featuredOnly: false, unusedQueue: [], favoritePhotos: [], currentPhotoByMonitor: {}, cachedBytes: 0, maxCacheBytes: 1, historyCount: 0, blacklistCount: 0, isRefilling: false, rateLimitRemaining: 100 };
const initial = useAppStore.getState();

const responses: Record<string, unknown> = {
  photo_get_status: status,
  monitors_list: [{ monitorId: 'M1', userPaused: false }],
  library_list_wallpapers: [{ wallpaperId: 'workshop:1', userProperties: {}, adjustments: {} }],
  settings_get: { transitionType: 'crossfade' },
  playlist_list: [],
  photo_add_album: { albumSlug: 'cape-horn-11-7', displayName: 'Cape Horn', albumUrl: '', isEnabled: true, addedAt: 0 },
  steam_inspect_url: { workshopId: '2841928371', title: 'Neon' },
  photo_fetch_public_catalog: [{ slug: 'a' }],
  steam_sync_local_folders: [],
  wallpaper_save_preset: { presetId: 'p1', wallpaperId: 'workshop:1', name: 'Night', properties: { a: 1 }, adjustments: {} },
  playlist_save: { playlistId: 'pl1', name: 'P', mode: 'mixed', intervalSeconds: 60, items: [] },
};

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string) => responses[cmd] ?? null);
  listeners.clear();
  unlistenSpies.length = 0;
  localStorage.clear();
  useAppStore.setState(initial, true);
});

const calls = () => invokeMock.mock.calls.map(([cmd, args]) => [cmd, args]);

it('sleep and unlock keep manual pause and overlapping system pause reasons', async () => {
  await useAppStore.getState().initAppListeners();
  listeners.get('playback://pause-changed')!({ payload: { monitorId: 'M1', paused: true } });
  const power = listeners.get('system-power')!;
  power({ payload: { kind: 'pause', reason: 'lock' } });
  power({ payload: { kind: 'pause', reason: 'sleep' } });
  power({ payload: { kind: 'resume', reason: 'sleep' } });
  expect(useAppStore.getState().systemPowerReasons).toEqual(['lock']);
  expect(useAppStore.getState().monitors[0].userPaused).toBe(true);
  power({ payload: { kind: 'resume', reason: 'lock' } });
  expect(useAppStore.getState().systemPowerReasons).toEqual([]);
  expect(useAppStore.getState().monitors[0].userPaused).toBe(true);
});

describe('store → invoke() contract', () => {
  it('refreshAll loads status, monitors, library, settings, playlists and the Steam account', async () => {
    await useAppStore.getState().refreshAll();
    expect(calls().map((c) => c[0]).sort()).toEqual(
      ['library_list_wallpapers', 'monitors_list', 'photo_get_status', 'photo_list_albums', 'playlist_list', 'settings_get', 'steam_account', 'wallpaper_list_presets'].sort()
    );
    expect(useAppStore.getState().selectedMonitorId).toBe('M1');
    expect(useAppStore.getState().selectedWallpaperId).toBeNull();
  });

  it.each([
    ['rotateNextPhoto', ['M1'], 'photo_rotate_next', { monitorId: 'M1' }],
    ['triggerPhotoRefill', [], 'photo_trigger_refill', undefined],
    ['fetchPublicCatalog', [], 'photo_fetch_public_catalog', undefined],
    ['addAlbum', ['cape-horn-11-7', 'Cape Horn'], 'photo_add_album', { urlOrSlug: 'cape-horn-11-7', displayName: 'Cape Horn' }],
    ['toggleAlbum', ['cape-horn-11-7', false], 'photo_toggle_album', { albumSlug: 'cape-horn-11-7', enabled: false }],
    ['removeAlbum', ['cape-horn-11-7'], 'photo_remove_album', { albumSlug: 'cape-horn-11-7' }],
    ['setPhotoFilter', ['albums', true], 'photo_set_filter', { mode: 'albums', featuredOnly: true }],
    ['setPhotoFavorite', ['p1', true], 'photo_set_favorite', { photoId: 'p1', favorite: true }],
    ['blacklistAndSkipPhoto', ['p1'], 'photo_blacklist_and_skip', { photoId: 'p1' }],
    ['setCacheLimit', [500], 'photo_set_cache_limit', { maxBytes: 500 }],
    ['clearPhotoCache', [], 'photo_clear_cache', undefined],
    ['resetPhotoHistory', ['a'], 'photo_reset_history', { albumSlug: 'a' }],
    ['inspectWorkshopUrl', ['2841928371'], 'steam_inspect_url', { urlOrId: '2841928371' }],
    ['downloadWorkshopItem', ['2841928371'], 'steam_download_workshop_item', { urlOrId: '2841928371' }],
    ['syncLocalSteamFolders', [], 'steam_sync_local_folders', undefined],
    ['deleteWallpaper', ['workshop:1'], 'library_delete_wallpaper', { wallpaperId: 'workshop:1' }],
    ['startSteamQrLogin', [], 'steam_login_qr', undefined],
    ['submitSteamCredentials', ['ian', 'pw'], 'steam_login_credentials', { username: 'ian', password: 'pw' }],
    ['submitSteam2FaCode', ['ABCDE'], 'steam_submit_2fa_code', { code: 'ABCDE' }],
    ['steamLogout', [], 'steam_logout', undefined],
    ['applyMonitorSource', ['M1', { kind: 'photo_api' }], 'monitor_apply_wallpaper', { monitorId: 'M1', source: { kind: 'photo_api' } }],
    ['updateWallpaperProperty', ['workshop:1', 'rain', 2], 'monitor_update_property', { wallpaperId: 'workshop:1', propKey: 'rain', propValue: 2 }],
    ['updateWallpaperAdjustments', ['workshop:1', { volume: 1 }], 'monitor_update_adjustments', { wallpaperId: 'workshop:1', adjustments: { volume: 1 } }],
    ['savePreset', ['workshop:1', 'Night'], 'wallpaper_save_preset', { wallpaperId: 'workshop:1', presetName: 'Night' }],
    ['applyPreset', ['workshop:1', 'p1'], 'wallpaper_apply_preset', { wallpaperId: 'workshop:1', presetId: 'p1' }],
    ['setPlaybackPaused', [true, 'M1'], 'playback_set_paused', { monitorId: 'M1', paused: true }],
    ['savePlaylist', [responses.playlist_save], 'playlist_save', { playlist: responses.playlist_save }],
    ['updateSettings', [{ fpsCap: 30 }], 'settings_update', { settings: { fpsCap: 30 } }],
  ] as const)('%s → %s', async (action, args, cmd, expectedArgs) => {
    const fn = useAppStore.getState()[action] as (...a: unknown[]) => Promise<unknown>;
    await fn(...args);
    expect(IPC_COMMANDS).toContain(cmd);
    expect(invokeMock.mock.calls[0]).toEqual([cmd, expectedArgs]);
  });

  it('PropertyInspector local updates do not invoke the backend', () => {
    useAppStore.setState({ wallpapers: [{ wallpaperId: 'w', userProperties: {}, adjustments: {} } as never] });
    useAppStore.getState().setLocalWallpaperProperty('w', 'k', true);
    expect(useAppStore.getState().wallpapers[0].userProperties).toEqual({ k: true });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('loads saved albums and presets from the backend in refreshAll', async () => {
    const album = { albumSlug: 'cape-horn-11-7', displayName: 'Cape Horn', albumUrl: '', isEnabled: true, addedAt: 1 };
    const preset = { presetId: 'p1', wallpaperId: 'workshop:1', name: 'Night', properties: {}, adjustments: {} };
    responses.photo_list_albums = [album];
    responses.wallpaper_list_presets = { 'workshop:1': [preset] };
    try {
      await useAppStore.getState().refreshAll();
      expect(useAppStore.getState().savedAlbums).toEqual([album]);
      expect(useAppStore.getState().presetsByWallpaper['workshop:1']).toEqual([preset]);
      expect(useAppStore.getState().loaded).toBe(true);
    } finally {
      delete responses.photo_list_albums;
      delete responses.wallpaper_list_presets;
    }
  });

  it('moves albums kept only in localStorage into the backend once', async () => {
    const local = { albumSlug: 'old-album-1', displayName: 'Old', albumUrl: '', isEnabled: false, addedAt: 1 };
    localStorage.setItem('truongpaper.savedAlbums', JSON.stringify([local]));
    responses.photo_list_albums = [];
    try {
      await useAppStore.getState().refreshAll();
      await vi.waitFor(() => expect(localStorage.getItem('truongpaper.savedAlbums')).toBeNull());
      expect(calls()).toContainEqual(['photo_add_album', { urlOrSlug: 'old-album-1', displayName: 'Old' }]);
      expect(calls()).toContainEqual(['photo_toggle_album', { albumSlug: 'old-album-1', enabled: false }]);
      // A second refresh does not import again.
      invokeMock.mockClear();
      await useAppStore.getState().refreshAll();
      expect(calls().some(([cmd]) => cmd === 'photo_add_album')).toBe(false);
    } finally {
      delete responses.photo_list_albums;
    }
  });

  it('keeps the old albums for the next launch when the import fails', async () => {
    localStorage.setItem('truongpaper.savedAlbums', JSON.stringify([{ albumSlug: 'old-album-1', displayName: 'Old' }]));
    responses.photo_list_albums = [];
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'photo_add_album') throw new Error('offline');
      return responses[cmd] ?? null;
    });
    try {
      await useAppStore.getState().refreshAll();
      await vi.waitFor(() => expect(calls().some(([cmd]) => cmd === 'photo_add_album')).toBe(true));
      await Promise.resolve();
      expect(localStorage.getItem('truongpaper.savedAlbums')).not.toBeNull();
    } finally {
      delete responses.photo_list_albums;
    }
  });

  it('adds and removes albums in the store without touching localStorage', async () => {
    await useAppStore.getState().addAlbum('cape-horn-11-7');
    expect(useAppStore.getState().savedAlbums).toHaveLength(1);
    expect(localStorage.getItem('truongpaper.savedAlbums')).toBeNull();
    await useAppStore.getState().savePreset('workshop:1', 'Night');
    expect(useAppStore.getState().presetsByWallpaper['workshop:1'][0].presetId).toBe('p1');
  });

  it('applyPreset re-lists wallpapers when the preset is not in the local list', async () => {
    await useAppStore.getState().applyPreset('workshop:1', 'unknown');
    expect(calls().map(([cmd]) => cmd)).toEqual(['wallpaper_apply_preset', 'library_list_wallpapers']);
  });

  it('only the newest settings write publishes its result', async () => {
    useAppStore.setState({ settings: { fpsCap: 30, qualityPreset: 'low' } as never });
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    let reads = 0;
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'settings_update') { await gate; return null; }
      if (cmd === 'settings_get') { reads += 1; return { fpsCap: reads === 1 ? 30 : 60, qualityPreset: 'high' }; }
      return null;
    });
    const first = useAppStore.getState().updateSettings({ fpsCap: 30 });
    expect(useAppStore.getState().settings).toMatchObject({ fpsCap: 30 });
    const second = useAppStore.getState().updateSettings({ fpsCap: 60 });
    expect(useAppStore.getState().settings).toMatchObject({ fpsCap: 60 });
    release();
    await Promise.all([first, second]);
    expect(useAppStore.getState().settings).toMatchObject({ fpsCap: 60 });
  });

  it('a failed settings write puts back what the backend has', async () => {
    useAppStore.setState({ settings: { fpsCap: 30 } as never });
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'settings_update') throw new Error('no');
      if (cmd === 'settings_get') return { fpsCap: 30 };
      return null;
    });
    await expect(useAppStore.getState().updateSettings({ fpsCap: 99 } as never)).rejects.toThrow('no');
    expect(useAppStore.getState().settings).toMatchObject({ fpsCap: 30 });
  });

  it('a failed apply still refreshes the display list', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'monitor_apply_wallpaper') throw new Error('bad scene');
      if (cmd === 'monitors_list') return [{ monitorId: 'M1', userPaused: false, name: 'fresh' }];
      return null;
    });
    await expect(useAppStore.getState().applyMonitorSource('M1', { kind: 'photo_api' })).rejects.toThrow('bad scene');
    expect(useAppStore.getState().monitors[0]).toMatchObject({ name: 'fresh' });
  });

  it('cancelWorkshopDownload and exportPlaylists call their commands', async () => {
    await useAppStore.getState().cancelWorkshopDownload('123');
    await useAppStore.getState().exportPlaylists('/tmp/p.json', '{}');
    expect(calls()).toEqual([
      ['steam_cancel_download', { urlOrId: '123' }],
      ['playlist_export', { path: '/tmp/p.json', contents: '{}' }],
    ]);
  });

  it('mergeNewWallpapers keeps local edits and appends only new items', () => {
    const edited = { wallpaperId: 'a', userProperties: { x: 1 } } as never;
    const merged = mergeNewWallpapers([edited], [{ wallpaperId: 'a', userProperties: {} } as never, { wallpaperId: 'b' } as never]);
    expect(merged).toHaveLength(2);
    expect(merged[0]).toBe(edited);
    const same = [edited];
    expect(mergeNewWallpapers(same, [{ wallpaperId: 'a' } as never])).toBe(same);
  });

  it('legacyAlbumsToMigrate skips albums the backend already has', () => {
    const a = { albumSlug: 'a' } as never;
    const b = { albumSlug: 'b' } as never;
    expect(legacyAlbumsToMigrate([a, b], [a])).toEqual([b]);
  });

  it('surfaces Steam auth failures in steamAuthError instead of throwing', async () => {
    invokeMock.mockRejectedValueOnce(new Error('bad password'));
    await useAppStore.getState().submitSteamCredentials('ian', 'x');
    expect(useAppStore.getState().steamAuthError).toBe('bad password');
  });
});

describe('store → listen() event wiring', () => {
  it('subscribes to all §5.1 events and applies payloads', async () => {
    const unlisten = await useAppStore.getState().initAppListeners();
    expect([...listeners.keys()].sort()).toEqual(
      [
        'photo-queue://updated',
        'playback://pause-changed',
        'policy://state-changed',
        'settings://changed',
        'steam-auth://need-2fa',
        'steam-auth://qr-matrix',
        'steam-download://progress',
        'system-power',
        'ui://selected-monitor',
        'wallpaper://changed',
        'wallpaper://diagnostics',
        'wallpaper://error',
        'wallpaper://loaded',
        'wallpaper://span-unsupported',
      ].sort()
    );

    // A QR from a closed or replaced sign-in must not reopen the window.
    listeners.get('steam-auth://qr-matrix')!({ payload: { matrix: [[true]] } });
    expect(useAppStore.getState()).toMatchObject({ qrMatrix: null, authModalOpen: false });
    useAppStore.setState({ steamSigningIn: true, authModalOpen: true });
    listeners.get('steam-auth://qr-matrix')!({ payload: { matrix: [[true]] } });
    expect(useAppStore.getState()).toMatchObject({ qrMatrix: [[true]], authModalOpen: true });

    listeners.get('policy://state-changed')!({ payload: { monitorId: 'M1', state: 'paused_battery' } });
    expect(useAppStore.getState().policyStateByMonitor).toEqual({ M1: 'paused_battery' });

    listeners.get('settings://changed')!({ payload: { skipMismatchedOrientation: false } });
    expect(useAppStore.getState().settings?.skipMismatchedOrientation).toBe(false);

    listeners.get('wallpaper://span-unsupported')!({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', message: 'x' } });
    expect(useFeedback.getState().toasts.at(-1)).toMatchObject({ tone: 'info', message: expect.stringContaining("can't span") });

    listeners.get('system-power')!({ payload: { kind: 'pause', reason: 'lock' } });
    expect(useAppStore.getState().monitors[0].userPaused).toBe(false);
    expect(useAppStore.getState().systemPowerReasons).toEqual(['lock']);

    const photo = { id: 'x', albumSlug: 'a' };
    const playlist = { kind: 'playlist', playlistId: 'mixed' };
    const video = { wallpaperId: 'workshop:3836199874', title: 'Video test', type: 'video' };
    const wallpaperChanged = listeners.get('wallpaper://changed')!;
    wallpaperChanged({ payload: { monitorId: 'M1', source: playlist, currentItem: video, transitionUsed: 'crossfade' } });
    expect(useAppStore.getState().monitors[0]).toMatchObject({ activeSource: playlist, activeWallpaperId: video.wallpaperId });
    expect(useAppStore.getState().photoStatus?.currentPhotoByMonitor.M1).toBeNull();
    expect(useAppStore.getState().wallpapers).toContainEqual(video);

    const photos = { kind: 'photo_api' };
    wallpaperChanged({ payload: { monitorId: 'M1', source: photos, currentItem: photo, transitionUsed: 'crossfade' } });
    expect(useAppStore.getState().monitors[0]).toMatchObject({ activeSource: photos, activeWallpaperId: null });
    expect(useAppStore.getState().photoStatus?.currentPhotoByMonitor.M1).toEqual(photo);

    invokeMock.mockClear();
    listeners.get('steam-download://progress')!({ payload: { workshopId: '1', title: 't', progressPercent: 100, state: 'completed' } });
    expect(useAppStore.getState().downloadsById['1'].state).toBe('completed');
    expect(invokeMock).toHaveBeenCalledWith('library_list_wallpapers', undefined);

    unlisten();
    expect(unlistenSpies.every((s) => s.mock.calls.length === 1)).toBe(true);
  });

  it('does not listen for tray menu events (the Rust side acts on them directly)', async () => {
    await useAppStore.getState().initAppListeners();
    expect(listeners.has('tray://next-requested')).toBe(false);
    expect(listeners.has('tray://toggle-pause-requested')).toBe(false);
  });
});

describe('shared display selection', () => {
  it('persists, emits to other windows and applies incoming selections without echoing', async () => {
    const { emit } = await import('@tauri-apps/api/event');
    const emitMock = vi.mocked(emit);
    await useAppStore.getState().initAppListeners();
    emitMock.mockClear();

    useAppStore.getState().selectMonitor('M2');
    expect(useAppStore.getState().selectedMonitorId).toBe('M2');
    expect(emitMock).toHaveBeenCalledWith('ui://selected-monitor', { monitorId: 'M2' });
    expect(JSON.parse(localStorage.getItem('truongpaper.selectedMonitor')!)).toBe('M2');

    emitMock.mockClear();
    listeners.get('ui://selected-monitor')!({ payload: { monitorId: 'M3' } });
    expect(useAppStore.getState().selectedMonitorId).toBe('M3');
    expect(emitMock).not.toHaveBeenCalled();
    // The originating window also receives its own event: a no-op.
    listeners.get('ui://selected-monitor')!({ payload: { monitorId: 'M3' } });
    expect(emitMock).not.toHaveBeenCalled();
  });

  it('records wallpaper errors and scene reports from events', async () => {
    await useAppStore.getState().initAppListeners();
    listeners.get('wallpaper://error')!({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', message: 'bad' } });
    expect(useAppStore.getState().wallpaperErrors.M1).toEqual({ wallpaperId: 'workshop:1', message: 'bad' });
    listeners.get('wallpaper://diagnostics')!({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', messages: ['a'] } });
    expect(useAppStore.getState().diagnosticsByWallpaper['workshop:1']).toEqual(['a']);
  });
});

describe('error cards, diagnostics and downloads', () => {
  it('wallpaper://loaded clears only that display\'s error card', async () => {
    await useAppStore.getState().initAppListeners();
    listeners.get('wallpaper://error')!({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', message: 'bad' } });
    listeners.get('wallpaper://error')!({ payload: { monitorId: 'M2', wallpaperId: 'workshop:2', message: 'worse' } });
    listeners.get('wallpaper://loaded')!({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1' } });
    expect(Object.keys(useAppStore.getState().wallpaperErrors)).toEqual(['M2']);
  });

  it('each diagnostics event replaces the previous list and an empty list clears it', async () => {
    await useAppStore.getState().initAppListeners();
    const diagnostics = listeners.get('wallpaper://diagnostics')!;
    diagnostics({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', messages: ['a', 'b'] } });
    diagnostics({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', messages: ['c'] } });
    expect(useAppStore.getState().diagnosticsByWallpaper['workshop:1']).toEqual(['c']);
    diagnostics({ payload: { monitorId: 'M1', wallpaperId: 'workshop:1', messages: [] } });
    expect(useAppStore.getState().diagnosticsByWallpaper).toEqual({});
  });

  it('skips download progress updates that do not change the whole percent', async () => {
    await useAppStore.getState().initAppListeners();
    const progress = listeners.get('steam-download://progress')!;
    progress({ payload: { workshopId: '1', title: 't', progressPercent: 10.1, state: 'downloading' } });
    const first = useAppStore.getState().downloadsById;
    progress({ payload: { workshopId: '1', title: 't', progressPercent: 10.6, state: 'downloading' } });
    expect(useAppStore.getState().downloadsById).toBe(first);
    progress({ payload: { workshopId: '1', title: 't', progressPercent: 11, state: 'downloading' } });
    expect(useAppStore.getState().downloadsById['1'].progressPercent).toBe(11);
    progress({ payload: { workshopId: '1', title: 't', progressPercent: 11, state: 'error', error: 'x' } });
    expect(useAppStore.getState().downloadsById['1'].state).toBe('error');
    useAppStore.getState().dismissDownload('1');
    expect(useAppStore.getState().downloadsById).toEqual({});
  });
});

describe('new contracts: favorites, playlists, backup, offline retry', () => {
  it('setWallpaperFavorite updates optimistically and calls library_set_favorite', async () => {
    useAppStore.setState({ wallpapers: [{ wallpaperId: 'workshop:1', userProperties: {}, adjustments: {} } as never] });
    const pending = useAppStore.getState().setWallpaperFavorite('workshop:1', true);
    expect(useAppStore.getState().wallpapers[0].isFavorite).toBe(true);
    await pending;
    expect(invokeMock).toHaveBeenCalledWith('library_set_favorite', { wallpaperId: 'workshop:1', favorite: true });
  });

  it('setWallpaperFavorite rolls back when the backend refuses', async () => {
    useAppStore.setState({ wallpapers: [{ wallpaperId: 'workshop:1', isFavorite: false, userProperties: {}, adjustments: {} } as never] });
    invokeMock.mockRejectedValueOnce(new Error('nope'));
    await expect(useAppStore.getState().setWallpaperFavorite('workshop:1', true)).rejects.toThrow('nope');
    expect(useAppStore.getState().wallpapers[0].isFavorite).toBe(false);
  });

  it('deletePlaylist calls playlist_delete, drops the playlist and reloads the displays', async () => {
    useAppStore.setState({ playlists: [{ playlistId: 'a', name: 'A' } as never, { playlistId: 'b', name: 'B' } as never] });
    await useAppStore.getState().deletePlaylist('a');
    expect(invokeMock).toHaveBeenCalledWith('playlist_delete', { playlistId: 'a' });
    expect(useAppStore.getState().playlists.map((p) => p.playlistId)).toEqual(['b']);
    expect(invokeMock).toHaveBeenCalledWith('monitors_list', undefined);
  });

  it('exportBackup and importBackup use the chosen path; import refreshes everything', async () => {
    const summary = { settings: true, playlists: 1, favorites: 2, wallpapers: 3, missingWorkshopIds: ['9'] };
    invokeMock.mockImplementation(async (cmd: string) => (cmd === 'backup_import' ? summary : responses[cmd] ?? null));
    await useAppStore.getState().exportBackup('/tmp/b.json');
    expect(invokeMock).toHaveBeenCalledWith('backup_export', { path: '/tmp/b.json' });
    expect(await useAppStore.getState().importBackup('/tmp/b.json')).toEqual(summary);
    expect(invokeMock).toHaveBeenCalledWith('backup_import', { path: '/tmp/b.json' });
    expect(invokeMock).toHaveBeenCalledWith('settings_get', undefined);
  });

  it('retryPhotoNow calls photo_retry_now then reloads the photo status', async () => {
    await useAppStore.getState().retryPhotoNow();
    expect(invokeMock.mock.calls.map((c) => c[0])).toEqual(['photo_retry_now', 'photo_get_status']);
  });

  it('refreshLive reloads only the displays and the photo status', async () => {
    await useAppStore.getState().refreshLive();
    expect(invokeMock.mock.calls.map((c) => c[0]).sort()).toEqual(['monitors_list', 'photo_get_status']);
  });
});
