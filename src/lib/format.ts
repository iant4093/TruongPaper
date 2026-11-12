// src/lib/format.ts
import { convertFileSrc } from '@tauri-apps/api/core';
import type { FilterMode, MonitorInfo, PhotoItem, PhotoStatus, WallpaperType } from '../types/ipc';

export const MB = 1024 * 1024;
export const GB = 1024 * MB;

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB';
  if (bytes >= GB) return `${(bytes / GB).toFixed(2)} GB`;
  return `${(bytes / MB).toFixed(1)} MB`;
}

export function formatOfflinePhotoCount(count: number): string {
  return `${count} ${count === 1 ? 'photo' : 'photos'} ready offline`;
}

export function offlinePhotosForDisplay(
  status: PhotoStatus | null,
  monitor: MonitorInfo | undefined,
  matchOrientation: boolean,
): PhotoItem[] {
  const queue = status?.unusedQueue ?? [];
  if (!monitor || !matchOrientation) return queue;
  return queue.filter(({ width, height }) =>
    width !== null && height !== null && (height > width) === monitor.isPortrait,
  );
}

export function photoPreviewSrc(photo: PhotoItem | null | undefined): string | undefined {
  if (!photo) return undefined;
  // The browser demo supplies inline previews and has no local photo cache.
  if (photo.filePath.startsWith('data:')) return photo.filePath;
  return `${convertFileSrc(photo.id, 'photo-preview')}?v=${photo.downloadedAt}`;
}

/**
 * Converts an on-disk path returned by the Rust backend into a URL the webview
 * can load through Tauri's `asset:` protocol. Remote, data and blob URLs pass through.
 */
export function toAssetUrl(pathOrUrl: string | null | undefined): string | null {
  if (!pathOrUrl) return null;
  if (/^(https?:|data:|blob:|asset:|photo-preview:)/i.test(pathOrUrl)) return pathOrUrl;
  try {
    const internals = (window as unknown as { __TAURI_INTERNALS__?: { convertFileSrc?: unknown } })
      .__TAURI_INTERNALS__;
    if (internals && typeof internals.convertFileSrc === 'function') {
      return convertFileSrc(pathOrUrl);
    }
  } catch {
    // Fall through to the raw path.
  }
  return pathOrUrl;
}

export const WALLPAPER_TYPE_LABEL: Record<WallpaperType, string> = {
  photo: 'Photo',
  scene: 'Scene',
  video: 'Video',
  web: 'Web',
  application: 'Application',
};

/** Wallpaper Engine Workshop items carry their type as a tag ("Scene", "Video", "Web", "Application"). */
export function workshopTypeFromTags(tags: string[]): string {
  const known = ['Scene', 'Video', 'Web', 'Application'];
  const hit = tags.find((t) => known.some((k) => k.toLowerCase() === t.toLowerCase()));
  return hit ? hit[0].toUpperCase() + hit.slice(1).toLowerCase() : 'Wallpaper';
}

export const PHOTO_POOL_OPTIONS = [
  ['all', 'All public photos'],
  ['featured', 'Featured photos only'],
  ['albums', 'Selected saved albums'],
] as const satisfies ReadonlyArray<readonly [FilterMode, string]>;

export const ROTATION_INTERVALS: Array<{ label: string; seconds: number }> = [
  { label: '1 min', seconds: 60 },
  { label: '5 min', seconds: 300 },
  { label: '15 min', seconds: 900 },
  { label: '30 min', seconds: 1800 },
  { label: '1 hour', seconds: 3600 },
  { label: '4 hours', seconds: 14400 },
  { label: '12 hours', seconds: 43200 },
  { label: '24 hours', seconds: 86400 },
  { label: 'Never (Manual Only)', seconds: 0 },
];

export const FIT_MODE_LABEL = {
  fit_blur: 'Fit + Blurred Backdrop',
  fill: 'Fill / Center Crop',
  fit_black: 'Fit + Black Bars',
  stretch: 'Stretch',
} as const;

export const TRANSITION_LABEL = {
  crossfade: 'Smooth Crossfade',
  blur_dissolve: 'Blur-Dissolve',
  liquid_glass: 'Liquid Glass Morph',
} as const;

/** Shown in Photography and the tray while the photo service is unreachable. */
export const OFFLINE_NOTICE = 'Offline - showing cached photos';
