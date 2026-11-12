import React, { useEffect, useState } from 'react';
import { Images, Search } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { WALLPAPER_TYPE_LABEL, formatBytes, offlinePhotosForDisplay, photoPreviewSrc } from '../../lib/format';
import type { WallpaperItem, WallpaperType } from '../../types/ipc';
import { Dropdown } from '../Dropdown';
import { ViewHeader, SegmentedControl } from '../ui';
import { WallpaperCard } from '../WallpaperCard';

type LibraryFilter = 'all' | WallpaperType;

const TYPE_FILTERS = [
  ['photo', 'Photos'],
  ['scene', 'Scenes'],
  ['video', 'Videos'],
  ['web', 'Web'],
  ['application', 'Applications'],
] as const satisfies ReadonlyArray<readonly [WallpaperType, string]>;

export type LibrarySort = 'recent' | 'name' | 'size' | 'type';
const SORT_OPTIONS = [
  ['recent', 'Recently added'],
  ['name', 'Name'],
  ['size', 'Size'],
  ['type', 'Type'],
] as const satisfies ReadonlyArray<readonly [LibrarySort, string]>;
const LIBRARY_PREFS_KEY = 'truongpaper.library';

interface LibraryPrefs { filter: LibraryFilter; sort: LibrarySort; query: string }

function loadPrefs(): LibraryPrefs {
  const defaults: LibraryPrefs = { filter: 'all', sort: 'recent', query: '' };
  try {
    const raw = JSON.parse(localStorage.getItem(LIBRARY_PREFS_KEY) ?? 'null') as Partial<LibraryPrefs> | null;
    if (!raw) return defaults;
    return {
      filter: typeof raw.filter === 'string' ? (raw.filter as LibraryFilter) : defaults.filter,
      sort: SORT_OPTIONS.some(([key]) => key === raw.sort) ? (raw.sort as LibrarySort) : defaults.sort,
      query: typeof raw.query === 'string' ? raw.query : '',
    };
  } catch {
    return defaults;
  }
}

/** Match against title, type (id and label) and Workshop id. */
export function matchesLibrarySearch(w: WallpaperItem, search: string): boolean {
  if (!search) return true;
  return [w.title, w.type, WALLPAPER_TYPE_LABEL[w.type], w.workshopId ?? '']
    .some((field) => field.toLowerCase().includes(search));
}

/** The backend lists wallpapers oldest first, so "recent" reverses that order. */
export function sortLibrary(items: WallpaperItem[], sort: LibrarySort): WallpaperItem[] {
  const byName = (a: WallpaperItem, b: WallpaperItem) => a.title.localeCompare(b.title, undefined, { sensitivity: 'base' });
  switch (sort) {
    case 'name': return [...items].sort(byName);
    case 'size': return [...items].sort((a, b) => b.bytes - a.bytes || byName(a, b));
    case 'type': return [...items].sort((a, b) => WALLPAPER_TYPE_LABEL[a.type].localeCompare(WALLPAPER_TYPE_LABEL[b.type]) || byName(a, b));
    default: return [...items].reverse();
  }
}

export const AllWallpapersView: React.FC<{ onSelectPhotoSource: () => void }> = ({ onSelectPhotoSource }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const matchOrientation = useAppStore((s) => s.settings?.skipMismatchedOrientation ?? true);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectedPhotoSource = useAppStore((s) => s.selectedPhotoSource);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const [prefs] = useState(loadPrefs);
  const [selectedFilter, setFilter] = useState<LibraryFilter>(prefs.filter);
  const [query, setQuery] = useState(prefs.query);
  const [sort, setSort] = useState<LibrarySort>(prefs.sort);
  useEffect(() => {
    try { localStorage.setItem(LIBRARY_PREFS_KEY, JSON.stringify({ filter: selectedFilter, sort, query })); }
    catch { /* storage unavailable: preferences just reset next launch */ }
  }, [selectedFilter, sort, query]);
  const [error, setError] = useState<string | null>(null);
  const [applying, setApplying] = useState<string | null>(null);
  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const currentPhoto = activeMonitor ? photoStatus?.currentPhotoByMonitor?.[activeMonitor.monitorId] : null;
  const queue = offlinePhotosForDisplay(photoStatus, activeMonitor, matchOrientation);
  const installedTypes = new Set(wallpapers.map((wallpaper) => wallpaper.type));
  const filters = TYPE_FILTERS.filter(([type]) => installedTypes.has(type));
  const filter = selectedFilter !== 'all' && installedTypes.has(selectedFilter) ? selectedFilter : 'all';
  const search = query.trim().toLowerCase();
  const items = sortLibrary(wallpapers.filter((w) => (filter === 'all' || w.type === filter) && matchesLibrarySearch(w, search)), sort);
  const showPhotos = filter === 'all' && 'ian truong photography photos'.includes(search);
  const apply = async (wallpaperId?: string) => {
    if (!activeMonitor || applying !== null) return;
    setError(null);
    setApplying(wallpaperId ?? 'photo_api');
    try { await applyMonitorSource(activeMonitor.monitorId, wallpaperId ? { kind: 'workshop', wallpaperId } : { kind: 'photo_api' }); }
    catch (err) { setError(String(err)); }
    finally { setApplying(null); }
  };
  return (
    <div>
      <ViewHeader title="Library" subtitle="Your wallpapers, all in one place." />
      <div className="library-toolbar">
        {filters.length > 0 && (
          <SegmentedControl<LibraryFilter> value={filter} options={[[ 'all', 'All wallpapers' ], ...filters]} onChange={setFilter} />
        )}
        <div className="library-sort"><Dropdown<LibrarySort> label="Sort library" value={sort} options={SORT_OPTIONS} onChange={setSort} /></div>
        <label className="library-search"><Search size={14} /><input aria-label="Search library" placeholder="Search title, type or ID" value={query} onChange={(e) => setQuery(e.target.value)} /></label>
      </div>
      {error && <div role="alert" className="error-banner">{error}</div>}
      <div className="wallpaper-grid">
        {showPhotos && (
          <WallpaperCard
            title="Ian Truong Photography" preview={photoPreviewSrc(currentPhoto ?? queue[0])}
            kind="Photography" detail="Photo API" selected={selectedPhotoSource}
            active={activeMonitor?.activeSource.kind === 'photo_api'} canApply={!!activeMonitor && applying === null}
            applying={applying === 'photo_api'} onSelect={onSelectPhotoSource} onApply={() => void apply()}
          />
        )}
        {items.map((w) => <WallpaperCard key={w.wallpaperId} title={w.title} preview={w.previewPath} kind={WALLPAPER_TYPE_LABEL[w.type]} detail={w.windowsOnly ? 'Windows only' : formatBytes(w.bytes)} audioReactive={w.audioReactive} selected={w.wallpaperId === selectedWallpaperId} active={activeMonitor?.activeSource.kind === 'workshop' && activeMonitor.activeSource.wallpaperId === w.wallpaperId} canApply={!!activeMonitor && applying === null} applying={applying === w.wallpaperId} onSelect={() => selectWallpaper(w.wallpaperId)} onApply={() => void apply(w.wallpaperId)} />)}
      </div>
      {!showPhotos && items.length === 0 && <div className="empty-state"><Images size={24} />No wallpapers match your search.</div>}
    </div>
  );
};
