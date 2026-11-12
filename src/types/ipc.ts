// src/types/ipc.ts

export type FilterMode = 'all' | 'featured' | 'albums';
export type WallpaperType = 'photo' | 'scene' | 'video' | 'web' | 'application';
export type FitMode = 'fill' | 'fit_blur' | 'fit_black' | 'stretch';
export type TransitionType = 'crossfade' | 'blur_dissolve' | 'liquid_glass';

export interface RuntimeMemoryUsage {
  residentBytes: number;
}

export interface PhotoItem {
  id: string;
  albumSlug: string;
  albumUrl: string;
  filePath: string;
  width: number | null;
  height: number | null;
  bytes: number;
  featured: boolean;
  isFavorite: boolean;
  downloadedAt: number;
  lastShownAt: number | null;
}

export interface SavedAlbum {
  albumSlug: string;
  displayName: string;
  albumUrl: string;
  isEnabled: boolean;
  coverThumbnailUrl?: string;
  imageCount?: number;
  addedAt: number;
}

export interface PublicCatalogAlbum {
  albumId: string;
  type: 'photo';
  title: string;
  description: string;
  category: string;
  imageCount: number;
  coverImageUrl: string;
  coverThumbnailUrl: string;
  coverBlurhash: string;
  slug: string;
}

export interface PhotoStatus {
  filterMode: FilterMode;
  featuredOnly: boolean;
  unusedQueue: PhotoItem[];
  favoritePhotos: PhotoItem[];
  currentPhotoByMonitor: Record<string, PhotoItem | null>;
  cachedBytes: number;
  maxCacheBytes: number;
  historyCount: number;
  blacklistCount: number;
  isRefilling: boolean;
  rateLimitRemaining: number;
  offline: boolean;
  showingCachedOnly: boolean;
}

export interface ProjectJsonComboOption {
  label: string;
  value: string | number;
}

export interface ProjectJsonProperty {
  type: 'slider' | 'bool' | 'color' | 'combo' | 'text' | 'file' | 'scenetexture' | 'directory' | 'group';
  text?: string;
  value?: string | number | boolean;
  order?: number;
  min?: number;
  max?: number;
  step?: number;
  fraction?: boolean;
  options?: ProjectJsonComboOption[];
  condition?: string;
}

export interface ProjectJsonSchema {
  title: string;
  type: 'scene' | 'video' | 'web' | 'application';
  file: string;
  preview?: string;
  description?: string;
  tags?: string[];
  workshopid?: string | number;
  general?: {
    supportsaudioprocessing?: boolean;
    properties?: Record<string, ProjectJsonProperty>;
    localization?: Record<string, Record<string, string>>;
  };
}

export interface UniversalAdjustments {
  volume: number;         // 0.0 .. 1.0
  muted: boolean;
  audioSensitivity: number; // 0.0 .. 5.0
  playbackSpeed: number;  // 0.1 .. 2.0
  fitMode: FitMode;
  zoom: number;           // 0.5 .. 2.0
  offsetX: number;        // -1.0 .. 1.0
  offsetY: number;        // -1.0 .. 1.0
  brightness: number;     // -0.5 .. 0.5 (default 0.0)
  contrast: number;       // 0.5 .. 2.0  (default 1.0)
  saturation: number;     // 0.0 .. 2.0  (default 1.0)
  hueShift: number;       // -3.14159 .. 3.14159 (default 0.0)
}

export interface WallpaperItem {
  wallpaperId: string;    // "workshop:<id>" or "local:<hash>"
  workshopId?: string;
  title: string;
  type: WallpaperType;
  previewPath: string;
  installDir: string;
  bytes: number;
  audioReactive: boolean;
  windowsOnly: boolean;
  projectJson: ProjectJsonSchema;
  userProperties: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
}

export interface WallpaperPreset {
  presetId: string;
  wallpaperId: string;
  name: string;
  properties: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
}

export type WorkshopDownloadSource = 'steam' | 'ggnetwork';

export type WorkshopSort = 'trending' | 'most_subscribed' | 'newest' | 'top_rated';
export interface WorkshopBrowseQuery {
  search: string;
  sort: WorkshopSort;
  page: number;
  days: number;
  tags: string[];
}
export interface WorkshopBrowseItem extends WorkshopPreview {
  author: string;
  subscribers: number;
  rating: number;
}
export interface WorkshopBrowsePage {
  items: WorkshopBrowseItem[];
  page: number;
  totalPages: number;
  totalItems: number;
  tagGroups: Array<{name: string; tags: Array<{tag: string; label: string}>}>;
}

export interface WorkshopPreview {
  workshopId: string;
  title: string;
  description: string;
  previewUrl: string;
  fileSizeBytes: number;
  tags: string[];
  timeUpdated: number;
}

export interface MonitorInfo {
  monitorId: string;
  name: string;
  width: number;
  height: number;
  scaleFactor: number;
  refreshRateHz: number;
  isPrimary: boolean;
  isPortrait: boolean;
  userPaused: boolean;
  activeSource: WallpaperSource;
  activeWallpaperId: string | null;
}

export interface PlaybackPauseChanged {
  monitorId: string | null;
  paused: boolean;
}

export type WallpaperSource =
  | { kind: 'photo_api' }
  | { kind: 'workshop'; wallpaperId: string }
  | { kind: 'playlist'; playlistId: string };

export interface Playlist {
  playlistId: string;
  name: string;
  mode: 'photos_only' | 'workshop_only' | 'mixed';
  intervalSeconds: number;
  items: Array<{ kind: 'photo_api_draw' } | { kind: 'workshop'; wallpaperId: string }>;
}

export interface AppSettings {
  rotationIntervalSeconds: number;
  transitionType: TransitionType;
  transitionDurationSeconds: number;
  skipMismatchedOrientation: boolean;
  defaultFitMode: FitMode;
  exhaustionAutoReset: boolean;
  maxPhotoCacheBytes: number;
  fpsCap: 30 | 60 | 120 | 'display_native';
  qualityPreset: 'low' | 'medium' | 'high' | 'ultra';
  pauseOnFullscreen: boolean;
  pauseOnMaximized: boolean;
  batteryPolicy: 'continue' | 'pause_animations' | 'switch_to_photo';
  launchAtLogin: boolean;
  syncLockScreen: boolean;
}

// ---------------------------------------------------------------------------
// §5.1 Tauri v2 IPC command & event registry (single source of truth for the
// frontend, the browser-preview mock and the contract tests).
// ---------------------------------------------------------------------------

export const IPC_COMMANDS = [
  // Photo API & Cache
  'photo_get_status',
  'photo_fetch_public_catalog',
  'photo_rotate_next',
  'photo_trigger_refill',
  'photo_add_album',
  'photo_toggle_album',
  'photo_remove_album',
  'photo_set_filter',
  'photo_set_favorite',
  'photo_apply_favorite',
  'photo_blacklist_and_skip',
  'photo_set_cache_limit',
  'photo_clear_cache',
  'photo_reset_history',
  // Steam Workshop & Wallpaper Engine
  'steam_browse_workshop',
  'steam_inspect_url',
  'steam_download_workshop_item',
  'steam_login_qr',
  'steam_login_credentials',
  'steam_submit_2fa_code',
  'steam_logout',
  'steam_cancel_login',
  'steam_account',
  'steam_sync_local_folders',
  'library_list_wallpapers',
  'library_delete_wallpaper',
  // Monitor, Playback & Property Customization
  'monitors_list',
  'monitor_apply_wallpaper',
  'wallpaper_set_lock_screen',
  'monitor_update_property',
  'monitor_update_adjustments',
  'wallpaper_save_preset',
  'wallpaper_apply_preset',
  'playback_set_paused',
  // Playlists, Auto-Updater & Global Settings
  'playlist_list',
  'playlist_save',
  'settings_get',
  'settings_update',
  'update_feed_status',
  'runtime_shutdown_for_update',
  'runtime_get_memory_usage',
] as const;

export type IpcCommand = (typeof IPC_COMMANDS)[number];

/** Rust → frontend event channels (§5.1) plus the tray menu events emitted by `tray.rs` (§4.1). */
export const IPC_EVENTS = [
  'wallpaper://changed',
  'photo-queue://updated',
  'settings://changed',
  'steam-download://progress',
  'steam-auth://qr-matrix',
  'steam-auth://need-2fa',
  'policy://state-changed',
  'playback://pause-changed',
  'system-power',
  'tray://next-requested',
  'tray://toggle-pause-requested',
] as const;

export type IpcEvent = (typeof IPC_EVENTS)[number];

export type PolicyState =
  | 'playing'
  | 'paused_fullscreen'
  | 'paused_maximized'
  | 'paused_battery'
  | 'paused_display_sleep';

export interface WallpaperChangedPayload {
  monitorId: string;
  source: WallpaperSource;
  currentItem: WallpaperItem | PhotoItem | null;
  transitionUsed: TransitionType;
}

export interface SystemPowerPayload {
  kind: 'pause' | 'resume';
  reason: 'sleep' | 'display' | 'lock';
}
