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
}));

import { useAppStore } from '../src/store/useAppStore';
import { IPC_COMMANDS } from '../src/types/ipc';

const status = { filterMode: 'all', featuredOnly: false, unusedQueue: [], currentPhotoByMonitor: {}, cachedBytes: 0, maxCacheBytes: 1, historyCount: 0, blacklistCount: 0, isRefilling: false, rateLimitRemaining: 100 };
const initial = useAppStore.getState();

const responses: Record<string, unknown> = {
  photo_get_status: status,
  monitors_list: [{ monitorId: 'M1' }],
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

describe('store → invoke() contract', () => {
  it('refreshAll loads status, monitors, library, settings and playlists', async () => {
    await useAppStore.getState().refreshAll();
    expect(calls().map((c) => c[0]).sort()).toEqual(
      ['library_list_wallpapers', 'monitors_list', 'photo_get_status', 'playlist_list', 'settings_get'].sort()
    );
    expect(useAppStore.getState().selectedMonitorId).toBe('M1');
    expect(useAppStore.getState().selectedWallpaperId).toBe('workshop:1');
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

  it('persists saved albums and presets across store instances via localStorage', async () => {
    await useAppStore.getState().addAlbum('cape-horn-11-7');
    expect(JSON.parse(localStorage.getItem('truongpaper.savedAlbums')!)).toHaveLength(1);
    await useAppStore.getState().savePreset('workshop:1', 'Night');
    expect(useAppStore.getState().presetsByWallpaper['workshop:1'][0].presetId).toBe('p1');
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
        'policy://state-changed',
        'steam-auth://need-2fa',
        'steam-auth://qr-matrix',
        'steam-download://progress',
        'system-power',
        'wallpaper://changed',
      ].sort()
    );

    listeners.get('steam-auth://qr-matrix')!({ payload: { matrix: [[true]] } });
    expect(useAppStore.getState()).toMatchObject({ qrMatrix: [[true]], authModalOpen: true });

    listeners.get('policy://state-changed')!({ payload: { monitorId: 'M1', state: 'paused_battery' } });
    expect(useAppStore.getState().policyStateByMonitor).toEqual({ M1: 'paused_battery' });

    listeners.get('system-power')!({ payload: { kind: 'pause', reason: 'lock' } });
    expect(useAppStore.getState().isPaused).toBe(true);

    const photo = { id: 'x', albumSlug: 'a' };
    listeners.get('wallpaper://changed')!({ payload: { monitorId: 'M1', currentItem: photo, transitionUsed: 'crossfade' } });
    expect(useAppStore.getState().photoStatus?.currentPhotoByMonitor.M1).toEqual(photo);

    invokeMock.mockClear();
    listeners.get('steam-download://progress')!({ payload: { workshopId: '1', title: 't', progressPercent: 100, state: 'completed' } });
    expect(useAppStore.getState().downloadsById['1'].state).toBe('completed');
    expect(invokeMock).toHaveBeenCalledWith('library_list_wallpapers', undefined);

    unlisten();
    expect(unlistenSpies.every((s) => s.mock.calls.length === 1)).toBe(true);
  });

  it('only handles tray menu events when asked to (main window)', async () => {
    await useAppStore.getState().initAppListeners({ handleTrayMenuEvents: true });
    expect(listeners.has('tray://next-requested')).toBe(true);
    expect(listeners.has('tray://toggle-pause-requested')).toBe(true);

    invokeMock.mockClear();
    listeners.get('tray://toggle-pause-requested')!({ payload: null });
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('playback_set_paused', { monitorId: undefined, paused: true }));

    invokeMock.mockClear();
    listeners.get('tray://next-requested')!({ payload: null });
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith('photo_rotate_next', { monitorId: 'M1' }));
  });
});
