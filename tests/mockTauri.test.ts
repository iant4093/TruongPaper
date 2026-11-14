// tests/mockTauri.test.ts — the browser-preview mock must implement every §5.1 command,
// and the frontend registry must match docs/04 §5.1 exactly.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { createMockBackend } from '../src/dev/mockTauri';
import { IPC_COMMANDS, IPC_EVENTS } from '../src/types/ipc';

const doc = readFileSync(resolve(__dirname, '../docs/04-UI-UX-LIQUID-GLASS-SPEC.md'), 'utf8');

function docCommands(): string[] {
  const section = doc.slice(doc.indexOf('### 5.1'), doc.indexOf('## 6.'));
  return [...section.matchAll(/invoke<[^>]*>\('([a-z0-9_]+)'/g)].map((m) => m[1]);
}

function docEvents(): string[] {
  const section = doc.slice(doc.indexOf('#### Rust'), doc.indexOf('## 6.'));
  const fromList = [...section.matchAll(/^- `([a-z0-9:/_-]+)`:/gm)].map((m) => m[1]);
  const fromTray = [...doc.matchAll(/app\.emit\("([^"]+)"/g)].map((m) => m[1]);
  return [...fromList, ...fromTray];
}

/** Representative, contract-shaped args for every command. */
const SAMPLE_ARGS: Record<string, Record<string, unknown>> = {
  photo_rotate_next: { monitorId: 'DISPLAY-1' },
  photo_add_album: { urlOrSlug: 'https://iantruongphotography.com/album/cape-horn-11-7' },
  photo_toggle_album: { albumSlug: 'bald-mountain-11-1', enabled: false },
  photo_remove_album: { albumSlug: 'bald-mountain-11-1' },
  photo_set_filter: { mode: 'albums', featuredOnly: true },
  photo_set_favorite: { photoId: 'demo-photo-0003', favorite: true },
  photo_apply_favorite: { monitorId: 'DISPLAY-1', photoId: 'demo-photo-0001' },
  photo_blacklist_and_skip: { photoId: 'demo-photo-0003' },
  photo_set_cache_limit: { maxBytes: 500 * 1024 * 1024 },
  photo_reset_history: { albumSlug: 'bald-mountain-11-1' },
  steam_browse_workshop: {query: {search:'',sort:'trending',page:1,days:7,tags:[]}},
  steam_guest_availability: { workshopIds: ['3800000001', '3800000002'] },
  library_import_local: { path: '/Users/me/Videos/clip.mp4' },
  steam_inspect_url: { urlOrId: 'https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371' },
  steam_download_workshop_item: { urlOrId: '3000000001' },
  steam_login_credentials: { username: 'ian', password: 'secret' },
  steam_submit_2fa_code: { code: 'AB12C' },
  library_delete_wallpaper: { wallpaperId: 'workshop:1100550912' },
  library_set_favorite: { wallpaperId: 'workshop:2841928371', favorite: true },
  playlist_delete: { playlistId: 'demo-playlist-1' },
  backup_export: { path: '/tmp/truongpaper-backup.json' },
  backup_import: { path: '/tmp/truongpaper-backup.json' },
  monitor_apply_wallpaper: { monitorId: 'DISPLAY-1', source: { kind: 'workshop', wallpaperId: 'workshop:2841928371' } },
  wallpaper_set_lock_screen: { monitorId: 'DISPLAY-1' },
  monitor_update_property: { wallpaperId: 'workshop:2841928371', propKey: 'rain_speed', propValue: 1.7 },
  monitor_update_adjustments: {
    wallpaperId: 'workshop:2841928371',
    adjustments: { volume: 0.2, muted: false, audioSensitivity: 1, playbackSpeed: 1, fitMode: 'fill', zoom: 1, offsetX: 0, offsetY: 0, brightness: 0, contrast: 1, saturation: 1, hueShift: 0 },
  },
  wallpaper_save_preset: { wallpaperId: 'workshop:2841928371', presetName: 'Night' },
  wallpaper_apply_preset: { wallpaperId: 'workshop:2841928371', presetId: 'missing' },
  playback_set_paused: { paused: true },
  playback_next: { monitorId: 'DISPLAY-1' },
  monitor_set_volume: { monitorId: 'DISPLAY-1', volume: 0.5 },
  playlist_save: { playlist: { playlistId: 'x', name: 'X', mode: 'photos_only', intervalSeconds: 60, items: [{ kind: 'photo_api_draw' }] } },
  settings_update: { settings: { fpsCap: 120 } },
};

describe('IPC contract registry', () => {
  it('IPC_COMMANDS matches docs/04 §5.1 exactly', () => {
    expect([...IPC_COMMANDS].sort()).toEqual([...new Set(docCommands())].sort());
  });

  it('IPC_EVENTS matches docs/04 §5.1 events + §4.1 tray events', () => {
    expect([...IPC_EVENTS].sort()).toEqual([...new Set(docEvents())].sort());
  });
});

describe('createMockBackend (browser preview)', () => {
  it.each([...IPC_COMMANDS])('handles %s without throwing', (cmd) => {
    const backend = createMockBackend(() => {}, () => {});
    expect(() => backend.handle(cmd, SAMPLE_ARGS[cmd] ?? {})).not.toThrow();
  });

  it('rejects commands outside the contract', () => {
    const backend = createMockBackend();
    expect(() => backend.handle('photo_unknown_command')).toThrow(/Unhandled command/);
  });

  it('answers the plugin commands used by the UI', () => {
    const backend = createMockBackend();
    expect(backend.handle('plugin:app|version')).toBe('1.0.0');
    expect(backend.handle('plugin:updater|check', {})).toBeNull();
    expect(backend.handle('plugin:window|get_all_windows')).toEqual(['main', 'tray-popover']);
    expect(backend.handle('plugin:window|hide')).toBeNull();
  });

  it('returns contract-shaped data for the read commands', () => {
    const backend = createMockBackend();
    const status = backend.handle('photo_get_status') as { unusedQueue: unknown[]; maxCacheBytes: number };
    expect(status.unusedQueue).toHaveLength(5);
    expect(status.maxCacheBytes).toBe(250 * 1024 * 1024);
    expect((backend.handle('monitors_list') as unknown[]).length).toBeGreaterThanOrEqual(2);
    expect((backend.handle('library_list_wallpapers') as Array<{ type: string }>).map((w) => w.type).sort()).toEqual(
      ['application', 'scene', 'video', 'web']
    );
    expect(backend.handle('update_feed_status')).toBe('ok');
  });

  it('rotates from the local queue and emits wallpaper://changed + photo-queue://updated', () => {
    const events: string[] = [];
    const backend = createMockBackend((e) => events.push(e), (fn) => fn());
    const before = (backend.handle('photo_get_status') as { unusedQueue: Array<{ id: string }> }).unusedQueue[0].id;
    const shown = backend.handle('photo_rotate_next', { monitorId: 'DISPLAY-1' }) as { id: string };
    expect(shown.id).toBe(before);
    expect(events).toContain('wallpaper://changed');
    expect(events).toContain('photo-queue://updated');
  });

  it('simulates download progress and adds the item to the library', () => {
    const progress: Array<{ progressPercent: number; state: string }> = [];
    const backend = createMockBackend(
      (e, p) => e === 'steam-download://progress' && progress.push(p as never),
      (fn) => fn()
    );
    const id = backend.handle('steam_download_workshop_item', { urlOrId: '3000000001' });
    expect(id).toBe('workshop:3000000001');
    expect(progress.at(-1)).toMatchObject({ progressPercent: 100, state: 'completed' });
    expect(backend.state().wallpapers.some((w) => w.workshopId === '3000000001')).toBe(true);
  });

  it('emits a QR matrix and a 2FA prompt for the Steam sign-in flows', () => {
    const events: Array<[string, unknown]> = [];
    const backend = createMockBackend((e, p) => events.push([e, p]), (fn) => fn());
    backend.handle('steam_login_qr');
    backend.handle('steam_login_credentials', { username: 'a', password: 'b' });
    const qr = events.find(([e]) => e === 'steam-auth://qr-matrix')![1] as { matrix: boolean[][] };
    expect(qr.matrix.length).toBe(29);
    expect(events.some(([e]) => e === 'steam-auth://need-2fa')).toBe(true);
    expect(() => backend.handle('steam_submit_2fa_code', { code: '1' })).toThrow(/Invalid/);
  });

  it('validates album slugs like the backend', () => {
    const backend = createMockBackend();
    expect(() => backend.handle('photo_add_album', { urlOrSlug: 'not a slug!' })).toThrow();
    const album = backend.handle('photo_add_album', { urlOrSlug: 'cape-horn-11-7' }) as { displayName: string };
    expect(album.displayName).toBe('Cape Horn 11/7');
  });
});
