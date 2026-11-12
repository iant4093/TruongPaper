// src/store/useAppStore.ts
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AppSettings,
  FilterMode,
  MonitorInfo,
  PhotoItem,
  GuestAvailability,
  PhotoStatus,
  PlaybackPauseChanged,
  Playlist,
  PublicCatalogAlbum,
  SavedAlbum,
  UniversalAdjustments,
  BackupImportSummary,
  WallpaperItem,
  WallpaperChangedPayload,
  WallpaperLoadedPayload,
  WallpaperDiagnosticsPayload,
  WallpaperErrorPayload,
  WallpaperPreset,
  WallpaperSource,
  WorkshopPreview,
  WorkshopDownloadSource,
} from '../types/ipc';
import { adjustmentWriteKey, propertyWriteKey, wallpaperWrites } from '../lib/writeQueue';

export interface SteamDownloadProgress {
  workshopId: string;
  title: string;
  progressPercent: number;
  state: 'downloading' | 'completed' | 'error';
  error?: string;
}

export interface Steam2FaPrompt {
  kind: 'totp' | 'email';
  prompt: string;
}

/** A wallpaper that failed to load on a display, shown until the next change there. */
export interface WallpaperLoadError {
  wallpaperId: string | null;
  message: string;
}

export function currentWallpaperForMonitor(
  monitor: MonitorInfo | undefined,
  wallpapers: WallpaperItem[],
  photoStatus: PhotoStatus | null,
) {
  const wallpaper = monitor?.activeWallpaperId
    ? wallpapers.find((item) => item.wallpaperId === monitor.activeWallpaperId)
    : undefined;
  const photo = monitor && !monitor.activeWallpaperId && monitor.activeSource?.kind !== 'workshop'
    ? photoStatus?.currentPhotoByMonitor?.[monitor.monitorId]
    : undefined;
  return { wallpaper, photo };
}

export interface AppState {
  photoStatus: PhotoStatus | null;
  publicCatalog: PublicCatalogAlbum[];
  savedAlbums: SavedAlbum[];
  wallpapers: WallpaperItem[];
  monitors: MonitorInfo[];
  selectedMonitorId: string | null;
  selectedWallpaperId: string | null;
  selectedPhotoSource: boolean;
  playlists: Playlist[];
  settings: AppSettings | null;
  systemPowerReasons: string[];
  policyStateByMonitor: Record<string, string>;
  presetsByWallpaper: Record<string, WallpaperPreset[]>;
  /** Load failures keyed by monitorId (`wallpaper://error`). */
  wallpaperErrors: Record<string, WallpaperLoadError>;
  /** Scene report messages keyed by wallpaperId (`wallpaper://diagnostics`). */
  diagnosticsByWallpaper: Record<string, string[]>;
  /** Last failed property/adjustment write, shown by Studio. */
  writeError: string | null;
  dismissWallpaperError: (monitorId: string) => void;
  dismissWriteError: () => void;
  /** Report a failed debounced write and re-sync the library from the backend. */
  recoverFromWriteError: (error: unknown) => Promise<void>;

  inspectedWorkshop: WorkshopPreview | null;
  /** GGNetwork (third-party mirror) availability by Workshop id; absent = not checked yet. */
  guestAvailability: Record<string, GuestAvailability>;
  downloadsById: Record<string, SteamDownloadProgress>;
  authModalOpen: boolean;
  qrMatrix: boolean[][] | null;
  need2Fa: Steam2FaPrompt | null;
  steamAuthError: string | null;
  /** Saved Steam account name; null when signed out. */
  steamAccount: string | null;
  /** True while a sign-in runs. QR and 2FA events from other runs are ignored. */
  steamSigningIn: boolean;
  initAppListeners: () => Promise<UnlistenFn>;
  refreshAll: () => Promise<void>;
  /** Cheap refresh for the tray popover: only the display list and photo status. */
  refreshLive: () => Promise<void>;
  rotateNextPhoto: (monitorId?: string) => Promise<void>;
  triggerPhotoRefill: () => Promise<void>;
  /** Offline mode: try the photo service again right now. */
  retryPhotoNow: () => Promise<void>;
  fetchPublicCatalog: () => Promise<PublicCatalogAlbum[]>;
  addAlbum: (urlOrSlug: string, displayName?: string) => Promise<SavedAlbum>;
  toggleAlbum: (albumSlug: string, enabled: boolean) => Promise<void>;
  removeAlbum: (albumSlug: string) => Promise<void>;
  setPhotoFilter: (mode: FilterMode, featuredOnly: boolean) => Promise<void>;
  setPhotoFavorite: (photoId: string, favorite: boolean) => Promise<void>;
  applyFavoritePhoto: (monitorId: string, photoId: string) => Promise<void>;
  blacklistAndSkipPhoto: (photoId: string) => Promise<void>;
  setCacheLimit: (maxBytes: number) => Promise<void>;
  clearPhotoCache: () => Promise<void>;
  resetPhotoHistory: (albumSlug?: string) => Promise<void>;

  inspectWorkshopUrl: (urlOrId: string) => Promise<WorkshopPreview>;
  closeWorkshopPreview: () => void;
  downloadWorkshopItem: (urlOrId: string, source?: WorkshopDownloadSource) => Promise<void>;
  syncLocalSteamFolders: () => Promise<void>;
  loadGuestAvailability: (workshopIds: string[]) => Promise<void>;
  importLocalWallpaper: (path: string) => Promise<WallpaperItem>;
  deleteWallpaper: (wallpaperId: string) => Promise<void>;
  setWallpaperFavorite: (wallpaperId: string, favorite: boolean) => Promise<void>;
  dismissDownload: (workshopId: string) => void;
  deletePlaylist: (playlistId: string) => Promise<void>;
  exportBackup: (path: string) => Promise<void>;
  importBackup: (path: string) => Promise<BackupImportSummary>;
  openSteamAuthModal: () => void;
  closeSteamAuthModal: () => void;
  startSteamQrLogin: () => Promise<void>;
  submitSteamCredentials: (username: string, password: string) => Promise<void>;
  submitSteam2FaCode: (code: string) => Promise<void>;
  steamLogout: () => Promise<void>;

  selectMonitor: (monitorId: string) => void;
  selectWallpaper: (wallpaperId: string | null) => void;
  selectPhotoSource: () => void;
  applyMonitorSource: (monitorId: string, source: WallpaperSource) => Promise<void>;
  updateWallpaperProperty: (wallpaperId: string, key: string, value: string | number | boolean) => Promise<void>;
  updateWallpaperAdjustments: (wallpaperId: string, adjustments: UniversalAdjustments) => Promise<void>;
  /** Local-only optimistic updates (used by PropertyInspector, which invokes the backend itself). */
  setLocalWallpaperProperty: (wallpaperId: string, key: string, value: string | number | boolean) => void;
  setLocalWallpaperAdjustments: (wallpaperId: string, adjustments: UniversalAdjustments) => void;
  savePreset: (wallpaperId: string, presetName: string) => Promise<WallpaperPreset>;
  applyPreset: (wallpaperId: string, presetId: string) => Promise<void>;
  setPlaybackPaused: (paused: boolean, monitorId?: string) => Promise<void>;
  savePlaylist: (playlist: Playlist) => Promise<Playlist>;
  updateSettings: (partial: Partial<AppSettings>) => Promise<void>;
}

// There is no `photo_list_albums` command in the §5.1 contract, so the saved
// album list is mirrored in localStorage (the backend upserts on
// `photo_add_album`, so re-adding is idempotent).
const SAVED_ALBUMS_KEY = 'truongpaper.savedAlbums';
const PRESETS_KEY = 'truongpaper.presets';
const SELECTED_MONITOR_KEY = 'truongpaper.selectedMonitor';
/** Cross-window event keeping Studio and the tray popover on the same display. */
export const SELECTED_MONITOR_EVENT = 'ui://selected-monitor';
/** Counts sign-in attempts so a replaced attempt cannot change the sign-in window. */
let steamLoginRun = 0;

function readJson<T>(key: string, fallback: T): T {
  try {
    if (typeof localStorage === 'undefined') return fallback;
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full or unavailable: the in-memory store remains authoritative.
  }
}

function readSelectedMonitor(): string | null {
  const stored = readJson<string | null>(SELECTED_MONITOR_KEY, null);
  return typeof stored === 'string' ? stored : null;
}

const errorMessage = (err: unknown) => (err instanceof Error ? err.message : String(err));

export const useAppStore = create<AppState>((set, get) => {
  let previewRequest = 0;
  const refreshPhotoStatus = async () => {
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  };

  const setSavedAlbums = (savedAlbums: SavedAlbum[]) => {
    writeJson(SAVED_ALBUMS_KEY, savedAlbums);
    set({ savedAlbums });
  };

  return {
    photoStatus: null,
    publicCatalog: [],
    savedAlbums: readJson<SavedAlbum[]>(SAVED_ALBUMS_KEY, []),
    wallpapers: [],
    monitors: [],
    selectedMonitorId: readSelectedMonitor(),
    selectedWallpaperId: null,
    selectedPhotoSource: false,
    playlists: [],
    settings: null,
    systemPowerReasons: [],
    policyStateByMonitor: {},
    presetsByWallpaper: readJson<Record<string, WallpaperPreset[]>>(PRESETS_KEY, {}),
    wallpaperErrors: {},
    diagnosticsByWallpaper: {},
    writeError: null,
    dismissWallpaperError: (monitorId) => set((state) => {
      const { [monitorId]: _dismissed, ...rest } = state.wallpaperErrors;
      return { wallpaperErrors: rest };
    }),
    dismissWriteError: () => set({ writeError: null }),
    recoverFromWriteError: async (error) => {
      set({ writeError: `Could not save the change: ${errorMessage(error)}` });
      try {
        const wallpapers = await invoke<WallpaperItem[]>('library_list_wallpapers');
        set({ wallpapers });
      } catch {
        // Keep the optimistic values if the backend cannot be reached either.
      }
    },

    inspectedWorkshop: null,
    guestAvailability: {},
    downloadsById: {},
    authModalOpen: false,
    qrMatrix: null,
    need2Fa: null,
    steamAuthError: null,
    steamAccount: null,
    steamSigningIn: false,
    refreshAll: async () => {
      const [photoStatus, monitors, wallpapers, settings, playlists, steamAccount] = await Promise.all([
        invoke<PhotoStatus>('photo_get_status'),
        invoke<MonitorInfo[]>('monitors_list'),
        invoke<WallpaperItem[]>('library_list_wallpapers'),
        invoke<AppSettings>('settings_get'),
        invoke<Playlist[]>('playlist_list').catch(() => [] as Playlist[]),
        invoke<string | null>('steam_account').catch(() => null),
      ]);
      set((state) => ({
        steamAccount,
        photoStatus,
        monitors,
        // A display can disappear between refreshes, so drop a stale selection.
        selectedMonitorId: monitors.some((m) => m.monitorId === state.selectedMonitorId)
          ? state.selectedMonitorId
          : monitors[0]?.monitorId ?? null,
        wallpapers,
        selectedWallpaperId: wallpapers.some((w) => w.wallpaperId === state.selectedWallpaperId) ? state.selectedWallpaperId : null,
        settings,
        playlists,
      }));
    },

    refreshLive: async () => {
      const [photoStatus, monitors] = await Promise.all([
        invoke<PhotoStatus>('photo_get_status'),
        invoke<MonitorInfo[]>('monitors_list'),
      ]);
      set((state) => ({
        photoStatus,
        monitors,
        selectedMonitorId: monitors.some((m) => m.monitorId === state.selectedMonitorId)
          ? state.selectedMonitorId
          : monitors[0]?.monitorId ?? null,
      }));
    },

    initAppListeners: async () => {
      let pendingDisplayUpdates: Array<() => void> | null = [];
      const updateDisplay = (update: () => void) => {
        if (pendingDisplayUpdates) pendingDisplayUpdates.push(update);
        else update();
      };

      const subscriptions: Array<Promise<UnlistenFn>> = [
        listen<WallpaperChangedPayload>('wallpaper://changed', ({ payload }) => {
          const { monitorId, source, currentItem } = payload;
          const wallpaper = currentItem && 'wallpaperId' in currentItem ? currentItem : null;
          const photo = currentItem && 'albumSlug' in currentItem ? currentItem : null;
          updateDisplay(() =>
            set((state) => ({
              wallpaperErrors: Object.fromEntries(
                Object.entries(state.wallpaperErrors).filter(([id]) => id !== monitorId),
              ),
              monitors: state.monitors.map((m) =>
                m.monitorId === monitorId
                  ? { ...m, activeSource: source, activeWallpaperId: wallpaper?.wallpaperId ?? null }
                  : m
              ),
              wallpapers: wallpaper && !state.wallpapers.some((w) => w.wallpaperId === wallpaper.wallpaperId)
                ? [...state.wallpapers, wallpaper]
                : state.wallpapers,
              photoStatus: state.photoStatus && {
                ...state.photoStatus,
                currentPhotoByMonitor: {
                  ...state.photoStatus.currentPhotoByMonitor,
                  [monitorId]: photo,
                },
              },
            })),
          );
        }),

        listen<WallpaperErrorPayload>('wallpaper://error', ({ payload }) => {
          set((state) => ({
            wallpaperErrors: {
              ...state.wallpaperErrors,
              [payload.monitorId]: { wallpaperId: payload.wallpaperId, message: payload.message },
            },
          }));
        }),
        // A wallpaper loaded fine on that display, so its error card is stale.
        listen<WallpaperLoadedPayload>('wallpaper://loaded', ({ payload }) => {
          set((state) => state.wallpaperErrors[payload.monitorId]
            ? { wallpaperErrors: Object.fromEntries(Object.entries(state.wallpaperErrors).filter(([id]) => id !== payload.monitorId)) }
            : state);
        }),
        // Every load reports its own list, so it replaces the previous one (an empty list clears it).
        listen<WallpaperDiagnosticsPayload>('wallpaper://diagnostics', ({ payload }) => {
          if (!payload.wallpaperId) return;
          const wallpaperId = payload.wallpaperId;
          set((state) => {
            const { [wallpaperId]: _old, ...rest } = state.diagnosticsByWallpaper;
            return { diagnosticsByWallpaper: payload.messages.length > 0 ? { ...rest, [wallpaperId]: payload.messages } : rest };
          });
        }),
        // Another window changed the display; apply it without re-emitting (no echo loop).
        listen<{ monitorId: string }>(SELECTED_MONITOR_EVENT, ({ payload }) => {
          if (!payload?.monitorId || payload.monitorId === get().selectedMonitorId) return;
          writeJson(SELECTED_MONITOR_KEY, payload.monitorId);
          set({ selectedMonitorId: payload.monitorId });
        }),

        listen<PhotoStatus>('photo-queue://updated', (event) => {
          updateDisplay(() => set({ photoStatus: event.payload }));
        }),
        listen<AppSettings>('settings://changed', ({ payload }) => {
          updateDisplay(() => set({ settings: payload }));
        }),
        listen<PlaybackPauseChanged>('playback://pause-changed', ({ payload }) => {
          updateDisplay(() => set((state) => ({
            monitors: state.monitors.map((monitor) => payload.monitorId === null || monitor.monitorId === payload.monitorId
              ? { ...monitor, userPaused: payload.paused }
              : monitor),
          })));
        }),

        listen<SteamDownloadProgress>('steam-download://progress', (event) => {
          const prog = event.payload;
          const previous = get().downloadsById[prog.workshopId];
          // Whole-percent steps are enough for the bar; skipping the rest avoids re-rendering on every chunk.
          if (previous && previous.state === prog.state && Math.floor(previous.progressPercent) === Math.floor(prog.progressPercent)) return;
          set((state) => ({
            downloadsById: { ...state.downloadsById, [prog.workshopId]: prog },
          }));
          if (prog.state === 'completed') {
            void invoke<WallpaperItem[]>('library_list_wallpapers').then((wallpapers) =>
              set({ wallpapers })
            );
          }
        }),

        listen<{ matrix: boolean[][] }>('steam-auth://qr-matrix', (event) => {
          // Only the running sign-in may show a QR code, and it never reopens a closed window.
          if (get().steamSigningIn) set({ qrMatrix: event.payload.matrix });
        }),

        listen<Steam2FaPrompt>('steam-auth://need-2fa', (event) => {
          if (get().steamSigningIn) set({ need2Fa: event.payload });
        }),

        listen<{ monitorId: string; state: string }>('policy://state-changed', (event) => {
          set((state) => ({
            policyStateByMonitor: {
              ...state.policyStateByMonitor,
              [event.payload.monitorId]: event.payload.state,
            },
          }));
        }),

        listen<{ kind: 'pause' | 'resume'; reason: string }>('system-power', (event) => {
          // System sleep/lock must not erase the user's explicit pause on resume.
          const { kind, reason } = event.payload;
          set((state) => ({ systemPowerReasons: kind === 'pause'
            ? [...new Set([...state.systemPowerReasons, reason])]
            : state.systemPowerReasons.filter((r) => r !== reason) }));
        }),
      ];

      const unlisteners = await Promise.all(subscriptions);
      try {
        await get().refreshAll();
        for (const update of pendingDisplayUpdates) update();
        pendingDisplayUpdates = null;
      } catch (error) {
        unlisteners.forEach((unlisten) => unlisten());
        throw error;
      }

      return () => {
        for (const unlisten of unlisteners) {
          unlisten();
        }
      };
    },

    rotateNextPhoto: async (monitorId) => {
      await invoke<PhotoItem>('photo_rotate_next', { monitorId });
      await refreshPhotoStatus();
    },

    triggerPhotoRefill: async () => {
      await invoke('photo_trigger_refill');
    },

    retryPhotoNow: async () => {
      await invoke('photo_retry_now');
      await refreshPhotoStatus();
    },

    fetchPublicCatalog: async () => {
      const publicCatalog = await invoke<PublicCatalogAlbum[]>('photo_fetch_public_catalog');
      set({ publicCatalog });
      return publicCatalog;
    },

    addAlbum: async (urlOrSlug, displayName) => {
      const album = await invoke<SavedAlbum>('photo_add_album', { urlOrSlug, displayName });
      setSavedAlbums([
        ...get().savedAlbums.filter((a) => a.albumSlug !== album.albumSlug),
        album,
      ]);
      return album;
    },

    toggleAlbum: async (albumSlug, enabled) => {
      await invoke('photo_toggle_album', { albumSlug, enabled });
      setSavedAlbums(
        get().savedAlbums.map((a) => (a.albumSlug === albumSlug ? { ...a, isEnabled: enabled } : a))
      );
    },

    removeAlbum: async (albumSlug) => {
      await invoke('photo_remove_album', { albumSlug });
      setSavedAlbums(get().savedAlbums.filter((a) => a.albumSlug !== albumSlug));
    },

    setPhotoFilter: async (mode, featuredOnly) => {
      await invoke('photo_set_filter', { mode, featuredOnly });
      await refreshPhotoStatus();
    },

    setPhotoFavorite: async (photoId, favorite) => {
      await invoke('photo_set_favorite', { photoId, favorite });
      await refreshPhotoStatus();
    },

    applyFavoritePhoto: async (monitorId, photoId) => {
      await invoke('photo_apply_favorite', { monitorId, photoId });
      await refreshPhotoStatus();
    },

    blacklistAndSkipPhoto: async (photoId) => {
      await invoke('photo_blacklist_and_skip', { photoId });
      await refreshPhotoStatus();
    },

    setCacheLimit: async (maxBytes) => {
      await invoke('photo_set_cache_limit', { maxBytes });
      await refreshPhotoStatus();
    },

    clearPhotoCache: async () => {
      await invoke('photo_clear_cache');
      await refreshPhotoStatus();
    },

    resetPhotoHistory: async (albumSlug) => {
      await invoke('photo_reset_history', { albumSlug });
      await refreshPhotoStatus();
    },

    inspectWorkshopUrl: async (urlOrId) => {
      const request = ++previewRequest;
      const preview = await invoke<WorkshopPreview>('steam_inspect_url', { urlOrId });
      if (request === previewRequest) set({ inspectedWorkshop: preview });
      return preview;
    },

    closeWorkshopPreview: () => { previewRequest++; set({ inspectedWorkshop: null }); },

    downloadWorkshopItem: async (urlOrId, source) => {
      await invoke<string>('steam_download_workshop_item', source ? { urlOrId, source } : { urlOrId });
    },

    syncLocalSteamFolders: async () => {
      const wallpapers = await invoke<WallpaperItem[]>('steam_sync_local_folders');
      set({ wallpapers });
    },

    // --- GGNetwork availability & local import (self-contained) ---
    loadGuestAvailability: async (workshopIds) => {
      const known = get().guestAvailability;
      const wanted = [...new Set(workshopIds)].filter((id) => known[id] !== 'available' && known[id] !== 'unavailable').slice(0, 30);
      if (wanted.length === 0) return;
      try {
        const result = await invoke<Record<string, GuestAvailability>>('steam_guest_availability', { workshopIds: wanted });
        set((state) => ({ guestAvailability: { ...state.guestAvailability, ...result } }));
      } catch {
        // The badge is a hint only; a failed check leaves the item unmarked.
      }
    },

    importLocalWallpaper: async (path) => {
      const item = await invoke<WallpaperItem>('library_import_local', { path });
      set((state) => ({
        wallpapers: [...state.wallpapers.filter((w) => w.wallpaperId !== item.wallpaperId), item],
        selectedWallpaperId: item.wallpaperId,
        selectedPhotoSource: false,
      }));
      return item;
    },

    deleteWallpaper: async (wallpaperId) => {
      await invoke('library_delete_wallpaper', { wallpaperId });
      set((state) => ({
        wallpapers: state.wallpapers.filter((w) => w.wallpaperId !== wallpaperId),
        selectedWallpaperId:
          state.selectedWallpaperId === wallpaperId ? null : state.selectedWallpaperId,
      }));
    },

    setWallpaperFavorite: async (wallpaperId, favorite) => {
      const mark = (value: boolean | undefined) => set((state) => ({
        wallpapers: state.wallpapers.map((w) => (w.wallpaperId === wallpaperId ? { ...w, isFavorite: value } : w)),
      }));
      const before = get().wallpapers.find((w) => w.wallpaperId === wallpaperId)?.isFavorite;
      mark(favorite);
      try {
        await invoke('library_set_favorite', { wallpaperId, favorite });
      } catch (error) {
        mark(before);
        throw error;
      }
    },

    dismissDownload: (workshopId) => set((state) => {
      const { [workshopId]: _gone, ...rest } = state.downloadsById;
      return { downloadsById: rest };
    }),

    deletePlaylist: async (playlistId) => {
      await invoke('playlist_delete', { playlistId });
      set((state) => ({ playlists: state.playlists.filter((p) => p.playlistId !== playlistId) }));
      // Displays that played it fall back to something else on the backend.
      try {
        const monitors = await invoke<MonitorInfo[]>('monitors_list');
        set({ monitors });
      } catch { /* the next refresh will catch up */ }
    },

    exportBackup: async (path) => {
      await invoke('backup_export', { path });
    },

    importBackup: async (path) => {
      const summary = await invoke<BackupImportSummary>('backup_import', { path });
      await get().refreshAll();
      return summary;
    },

    openSteamAuthModal: () => set({ authModalOpen: true, steamAuthError: null }),
    closeSteamAuthModal: () => {
      const signingIn = get().steamSigningIn;
      steamLoginRun += 1;
      set({ authModalOpen: false, qrMatrix: null, need2Fa: null, steamAuthError: null, steamSigningIn: false });
      // Stop the downloader too, or its refreshed QR codes keep arriving.
      if (signingIn) void invoke('steam_cancel_login').catch(() => {});
    },

    startSteamQrLogin: async () => {
      const run = ++steamLoginRun;
      set({ qrMatrix: null, need2Fa: null, authModalOpen: true, steamAuthError: null, steamSigningIn: true });
      try {
        const account = await invoke<string>('steam_login_qr');
        if (run === steamLoginRun) set({ steamAccount: account, steamSigningIn: false, authModalOpen: false, qrMatrix: null });
      } catch (err) {
        // A newer sign-in or a closed window replaced this run, so its error is stale.
        if (run === steamLoginRun) set({ steamAuthError: errorMessage(err), steamSigningIn: false, qrMatrix: null });
      }
    },

    submitSteamCredentials: async (username, password) => {
      const run = ++steamLoginRun;
      set({ need2Fa: null, steamAuthError: null, steamSigningIn: true });
      try {
        const account = await invoke<string>('steam_login_credentials', { username, password });
        if (run === steamLoginRun) set({ steamAccount: account, steamSigningIn: false, authModalOpen: false, need2Fa: null });
      } catch (err) {
        if (run === steamLoginRun) set({ steamAuthError: errorMessage(err), steamSigningIn: false, need2Fa: null });
      }
    },

    submitSteam2FaCode: async (code) => {
      try {
        await invoke('steam_submit_2fa_code', { code });
        set({ need2Fa: null, steamAuthError: null });
      } catch (err) {
        set({ steamAuthError: errorMessage(err) });
      }
    },

    steamLogout: async () => {
      await invoke('steam_logout');
      set({ steamAccount: null, steamSigningIn: false });
    },

    selectMonitor: (monitorId) => {
      if (monitorId === get().selectedMonitorId) return;
      writeJson(SELECTED_MONITOR_KEY, monitorId);
      set({ selectedMonitorId: monitorId });
      try {
        void emit(SELECTED_MONITOR_EVENT, { monitorId }).catch(() => {});
      } catch {
        // No Tauri event bridge (plain browser): single window, nothing to sync.
      }
    },
    selectWallpaper: (wallpaperId) => set({ selectedWallpaperId: wallpaperId, selectedPhotoSource: false }),
    selectPhotoSource: () => set({ selectedWallpaperId: null, selectedPhotoSource: true }),

    applyMonitorSource: async (monitorId, source) => {
      await invoke('monitor_apply_wallpaper', { monitorId, source });
      const monitors = await invoke<MonitorInfo[]>('monitors_list');
      set((state) => ({
        monitors: monitors.map((monitor) => ({
          ...monitor,
          userPaused: state.monitors.find((existing) => existing.monitorId === monitor.monitorId)?.userPaused ?? monitor.userPaused,
        })),
      }));
    },

    updateWallpaperProperty: async (wallpaperId, propKey, propValue) => {
      get().setLocalWallpaperProperty(wallpaperId, propKey, propValue);
      try {
        await wallpaperWrites.schedule(propertyWriteKey(wallpaperId, propKey), () =>
          invoke('monitor_update_property', { wallpaperId, propKey, propValue }));
      } catch (error) {
        await get().recoverFromWriteError(error);
        throw error;
      }
    },

    updateWallpaperAdjustments: async (wallpaperId, adjustments) => {
      get().setLocalWallpaperAdjustments(wallpaperId, adjustments);
      try {
        await wallpaperWrites.schedule(adjustmentWriteKey(wallpaperId), () =>
          invoke('monitor_update_adjustments', { wallpaperId, adjustments }));
      } catch (error) {
        await get().recoverFromWriteError(error);
        throw error;
      }
    },

    setLocalWallpaperProperty: (wallpaperId, propKey, propValue) => {
      set((state) => ({
        wallpapers: state.wallpapers.map((w) =>
          w.wallpaperId === wallpaperId
            ? { ...w, userProperties: { ...w.userProperties, [propKey]: propValue } }
            : w
        ),
      }));
    },

    setLocalWallpaperAdjustments: (wallpaperId, adjustments) => {
      set((state) => ({
        wallpapers: state.wallpapers.map((w) =>
          w.wallpaperId === wallpaperId ? { ...w, adjustments } : w
        ),
      }));
    },

    savePreset: async (wallpaperId, presetName) => {
      const preset = await invoke<WallpaperPreset>('wallpaper_save_preset', {
        wallpaperId,
        presetName,
      });
      const presetsByWallpaper = {
        ...get().presetsByWallpaper,
        [wallpaperId]: [
          ...(get().presetsByWallpaper[wallpaperId] ?? []).filter(
            (p) => p.presetId !== preset.presetId
          ),
          preset,
        ],
      };
      writeJson(PRESETS_KEY, presetsByWallpaper);
      set({ presetsByWallpaper });
      return preset;
    },

    applyPreset: async (wallpaperId, presetId) => {
      await invoke('wallpaper_apply_preset', { wallpaperId, presetId });
      const preset = get().presetsByWallpaper[wallpaperId]?.find((p) => p.presetId === presetId);
      if (preset) {
        set((state) => ({
          wallpapers: state.wallpapers.map((w) =>
            w.wallpaperId === wallpaperId
              ? { ...w, userProperties: { ...preset.properties }, adjustments: preset.adjustments }
              : w
          ),
        }));
      }
    },

    setPlaybackPaused: async (paused, monitorId) => {
      await invoke('playback_set_paused', { monitorId, paused });
    },

    savePlaylist: async (playlist) => {
      const saved = await invoke<Playlist>('playlist_save', { playlist });
      set((state) => ({
        playlists: [
          ...state.playlists.filter((p) => p.playlistId !== saved.playlistId),
          saved,
        ],
      }));
      return saved;
    },

    updateSettings: async (partial) => {
      await invoke('settings_update', { settings: partial });
      const settings = await invoke<AppSettings>('settings_get');
      set({ settings });
    },
  };
});

export const useWallpaperStore = useAppStore;
