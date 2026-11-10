// src/store/useAppStore.ts
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AppSettings,
  FilterMode,
  MonitorInfo,
  PhotoItem,
  PhotoStatus,
  Playlist,
  PublicCatalogAlbum,
  SavedAlbum,
  TransitionType,
  UniversalAdjustments,
  WallpaperItem,
  WallpaperPreset,
  WallpaperSource,
  WorkshopPreview,
} from '../types/ipc';

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

export interface InitListenerOptions {
  /**
   * Handle the tray context-menu events (`tray://next-requested`,
   * `tray://toggle-pause-requested`). Only ONE window (the main studio window)
   * should pass `true`, otherwise each action would run once per window.
   */
  handleTrayMenuEvents?: boolean;
}

export interface AppState {
  photoStatus: PhotoStatus | null;
  publicCatalog: PublicCatalogAlbum[];
  savedAlbums: SavedAlbum[];
  wallpapers: WallpaperItem[];
  monitors: MonitorInfo[];
  selectedMonitorId: string | null;
  selectedWallpaperId: string | null;
  playlists: Playlist[];
  settings: AppSettings | null;
  isPaused: boolean;
  policyStateByMonitor: Record<string, string>;
  presetsByWallpaper: Record<string, WallpaperPreset[]>;

  inspectedWorkshop: WorkshopPreview | null;
  downloadsById: Record<string, SteamDownloadProgress>;
  authModalOpen: boolean;
  qrMatrix: boolean[][] | null;
  need2Fa: Steam2FaPrompt | null;
  steamAuthError: string | null;

  initAppListeners: (options?: InitListenerOptions) => Promise<UnlistenFn>;
  refreshAll: () => Promise<void>;
  rotateNextPhoto: (monitorId?: string) => Promise<void>;
  triggerPhotoRefill: () => Promise<void>;
  fetchPublicCatalog: () => Promise<PublicCatalogAlbum[]>;
  addAlbum: (urlOrSlug: string, displayName?: string) => Promise<SavedAlbum>;
  toggleAlbum: (albumSlug: string, enabled: boolean) => Promise<void>;
  removeAlbum: (albumSlug: string) => Promise<void>;
  setPhotoFilter: (mode: FilterMode, featuredOnly: boolean) => Promise<void>;
  setPhotoFavorite: (photoId: string, favorite: boolean) => Promise<void>;
  blacklistAndSkipPhoto: (photoId: string) => Promise<void>;
  setCacheLimit: (maxBytes: number) => Promise<void>;
  clearPhotoCache: () => Promise<void>;
  resetPhotoHistory: (albumSlug?: string) => Promise<void>;

  inspectWorkshopUrl: (urlOrId: string) => Promise<WorkshopPreview>;
  closeWorkshopPreview: () => void;
  downloadWorkshopItem: (urlOrId: string) => Promise<void>;
  syncLocalSteamFolders: () => Promise<void>;
  deleteWallpaper: (wallpaperId: string) => Promise<void>;
  openSteamAuthModal: () => void;
  closeSteamAuthModal: () => void;
  startSteamQrLogin: () => Promise<void>;
  submitSteamCredentials: (username: string, password: string) => Promise<void>;
  submitSteam2FaCode: (code: string) => Promise<void>;
  steamLogout: () => Promise<void>;

  selectMonitor: (monitorId: string) => void;
  selectWallpaper: (wallpaperId: string | null) => void;
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

const errorMessage = (err: unknown) => (err instanceof Error ? err.message : String(err));

export const useAppStore = create<AppState>((set, get) => {
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
    selectedMonitorId: null,
    selectedWallpaperId: null,
    playlists: [],
    settings: null,
    isPaused: false,
    policyStateByMonitor: {},
    presetsByWallpaper: readJson<Record<string, WallpaperPreset[]>>(PRESETS_KEY, {}),

    inspectedWorkshop: null,
    downloadsById: {},
    authModalOpen: false,
    qrMatrix: null,
    need2Fa: null,
    steamAuthError: null,

    refreshAll: async () => {
      const [photoStatus, monitors, wallpapers, settings, playlists] = await Promise.all([
        invoke<PhotoStatus>('photo_get_status'),
        invoke<MonitorInfo[]>('monitors_list'),
        invoke<WallpaperItem[]>('library_list_wallpapers'),
        invoke<AppSettings>('settings_get'),
        invoke<Playlist[]>('playlist_list').catch(() => [] as Playlist[]),
      ]);
      set((state) => ({
        photoStatus,
        monitors,
        selectedMonitorId: state.selectedMonitorId ?? monitors[0]?.monitorId ?? null,
        wallpapers,
        selectedWallpaperId: state.selectedWallpaperId ?? wallpapers[0]?.wallpaperId ?? null,
        settings,
        playlists,
      }));
    },

    initAppListeners: async (options = {}) => {
      await get().refreshAll();

      const subscriptions: Array<Promise<UnlistenFn>> = [
        listen<{
          monitorId: string;
          currentItem: WallpaperItem | PhotoItem;
          transitionUsed: TransitionType;
        }>('wallpaper://changed', (event) => {
          const { monitorId, currentItem } = event.payload;
          if ('albumSlug' in currentItem) {
            set((state) =>
              state.photoStatus
                ? {
                    photoStatus: {
                      ...state.photoStatus,
                      currentPhotoByMonitor: {
                        ...state.photoStatus.currentPhotoByMonitor,
                        [monitorId]: currentItem,
                      },
                    },
                  }
                : state
            );
          } else {
            set((state) => ({
              monitors: state.monitors.map((m) =>
                m.monitorId === monitorId &&
                m.activeSource.kind !== 'playlist'
                  ? { ...m, activeSource: { kind: 'workshop', wallpaperId: currentItem.wallpaperId } }
                  : m
              ),
            }));
          }
        }),

        listen<PhotoStatus>('photo-queue://updated', (event) => {
          set({ photoStatus: event.payload });
        }),

        listen<SteamDownloadProgress>('steam-download://progress', (event) => {
          const prog = event.payload;
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
          set({ qrMatrix: event.payload.matrix, authModalOpen: true });
        }),

        listen<Steam2FaPrompt>('steam-auth://need-2fa', (event) => {
          set({ need2Fa: event.payload, authModalOpen: true });
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
          set({ isPaused: event.payload.kind === 'pause' });
        }),
      ];

      if (options.handleTrayMenuEvents) {
        subscriptions.push(
          listen('tray://next-requested', () => {
            void get().rotateNextPhoto(get().selectedMonitorId ?? undefined);
          }),
          listen('tray://toggle-pause-requested', () => {
            void get().setPlaybackPaused(!get().isPaused);
          })
        );
      }

      const unlisteners = await Promise.all(subscriptions);

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
      const preview = await invoke<WorkshopPreview>('steam_inspect_url', { urlOrId });
      set({ inspectedWorkshop: preview });
      return preview;
    },

    closeWorkshopPreview: () => set({ inspectedWorkshop: null }),

    downloadWorkshopItem: async (urlOrId) => {
      await invoke<string>('steam_download_workshop_item', { urlOrId });
    },

    syncLocalSteamFolders: async () => {
      const wallpapers = await invoke<WallpaperItem[]>('steam_sync_local_folders');
      set({ wallpapers });
    },

    deleteWallpaper: async (wallpaperId) => {
      await invoke('library_delete_wallpaper', { wallpaperId });
      set((state) => ({
        wallpapers: state.wallpapers.filter((w) => w.wallpaperId !== wallpaperId),
        selectedWallpaperId:
          state.selectedWallpaperId === wallpaperId ? null : state.selectedWallpaperId,
      }));
    },

    openSteamAuthModal: () => set({ authModalOpen: true, steamAuthError: null }),
    closeSteamAuthModal: () =>
      set({ authModalOpen: false, qrMatrix: null, need2Fa: null, steamAuthError: null }),

    startSteamQrLogin: async () => {
      set({ qrMatrix: null, need2Fa: null, authModalOpen: true, steamAuthError: null });
      try {
        await invoke('steam_login_qr');
      } catch (err) {
        set({ steamAuthError: errorMessage(err) });
      }
    },

    submitSteamCredentials: async (username, password) => {
      set({ need2Fa: null, steamAuthError: null });
      try {
        await invoke('steam_login_credentials', { username, password });
      } catch (err) {
        set({ steamAuthError: errorMessage(err) });
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
    },

    selectMonitor: (monitorId) => set({ selectedMonitorId: monitorId }),
    selectWallpaper: (wallpaperId) => set({ selectedWallpaperId: wallpaperId }),

    applyMonitorSource: async (monitorId, source) => {
      await invoke('monitor_apply_wallpaper', { monitorId, source });
      const monitors = await invoke<MonitorInfo[]>('monitors_list');
      set({ monitors });
    },

    updateWallpaperProperty: async (wallpaperId, propKey, propValue) => {
      get().setLocalWallpaperProperty(wallpaperId, propKey, propValue);
      await invoke('monitor_update_property', { wallpaperId, propKey, propValue });
    },

    updateWallpaperAdjustments: async (wallpaperId, adjustments) => {
      get().setLocalWallpaperAdjustments(wallpaperId, adjustments);
      await invoke('monitor_update_adjustments', { wallpaperId, adjustments });
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
      set({ isPaused: paused });
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
