import React, { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { Images, Search, X } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { toast } from '../../store/feedback';
import { WALLPAPER_TYPE_LABEL, OFFLINE_NOTICE, offlinePhotosForDisplay, photoPreviewSrc } from '../../lib/format';
import { useSlashToFocus } from '../../lib/hooks';
import type { WallpaperItem, WallpaperType } from '../../types/ipc';
import { Dropdown } from '../Dropdown';
import { LocalImport } from '../LocalImport';
import { ViewHeader, SegmentedControl } from '../ui';
import { VirtualGrid } from '../VirtualGrid';
import { WallpaperCard } from '../WallpaperCard';

type LibraryFilter = 'all' | 'favorites' | WallpaperType;

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
const PHOTOS_ID = '__photography__';

interface LibraryPrefs { filter: LibraryFilter; sort: LibrarySort }

// The search text is deliberately not remembered: a leftover query makes the library look empty.
function loadPrefs(): LibraryPrefs {
  const defaults: LibraryPrefs = { filter: 'all', sort: 'recent' };
  try {
    const raw = JSON.parse(localStorage.getItem(LIBRARY_PREFS_KEY) ?? 'null') as Partial<LibraryPrefs> | null;
    if (!raw) return defaults;
    return {
      filter: typeof raw.filter === 'string' ? (raw.filter as LibraryFilter) : defaults.filter,
      sort: SORT_OPTIONS.some(([key]) => key === raw.sort) ? (raw.sort as LibrarySort) : defaults.sort,
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

type Entry = { id: string; kind: 'photos' } | { id: string; kind: 'wallpaper'; item: WallpaperItem };

export const AllWallpapersView: React.FC<{
  onOpenDetails: () => void;
  onNavigate: (tab: 'workshop') => void;
}> = ({ onOpenDetails, onNavigate }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const matchOrientation = useAppStore((s) => s.settings?.skipMismatchedOrientation ?? true);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectedPhotoSource = useAppStore((s) => s.selectedPhotoSource);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const selectPhotoSource = useAppStore((s) => s.selectPhotoSource);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const setWallpaperFavorite = useAppStore((s) => s.setWallpaperFavorite);
  const retryPhotoNow = useAppStore((s) => s.retryPhotoNow);
  const [prefs] = useState(loadPrefs);
  const [selectedFilter, setFilter] = useState<LibraryFilter>(prefs.filter);
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<LibrarySort>(prefs.sort);
  const [applying, setApplying] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  useSlashToFocus(searchRef);
  useEffect(() => {
    try { localStorage.setItem(LIBRARY_PREFS_KEY, JSON.stringify({ filter: selectedFilter, sort })); }
    catch { /* storage unavailable: preferences just reset next launch */ }
  }, [selectedFilter, sort]);

  const deferredQuery = useDeferredValue(query);
  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const activeMonitorId = activeMonitor?.monitorId;
  const activeSource = activeMonitor?.activeSource;
  const currentPhoto = activeMonitorId ? photoStatus?.currentPhotoByMonitor?.[activeMonitorId] : null;
  const photoPreview = useMemo(
    () => photoPreviewSrc(currentPhoto ?? offlinePhotosForDisplay(photoStatus, activeMonitor, matchOrientation)[0]),
    [currentPhoto, photoStatus, activeMonitor, matchOrientation],
  );
  const installedTypes = useMemo(() => new Set(wallpapers.map((w) => w.type)), [wallpapers]);
  const filters = TYPE_FILTERS.filter(([type]) => installedTypes.has(type));
  const filter = selectedFilter === 'all' || selectedFilter === 'favorites' || installedTypes.has(selectedFilter) ? selectedFilter : 'all';
  const search = deferredQuery.trim().toLowerCase();
  const favoriteCount = useMemo(() => wallpapers.filter((w) => w.isFavorite).length, [wallpapers]);

  const entries = useMemo<Entry[]>(() => {
    const matching = wallpapers.filter((w) =>
      (filter === 'all' || (filter === 'favorites' ? w.isFavorite : w.type === filter)) && matchesLibrarySearch(w, search));
    const list: Entry[] = sortLibrary(matching, sort).map((item) => ({ id: item.wallpaperId, kind: 'wallpaper', item }));
    if (filter === 'all' && 'ian truong photography photos'.includes(search)) list.unshift({ id: PHOTOS_ID, kind: 'photos' });
    return list;
  }, [wallpapers, filter, search, sort]);

  const apply = useCallback(async (id: string) => {
    if (!activeMonitorId) return;
    setApplying(id);
    const title = id === PHOTOS_ID ? 'Photography' : useAppStore.getState().wallpapers.find((w) => w.wallpaperId === id)?.title ?? 'Wallpaper';
    try {
      await applyMonitorSource(activeMonitorId, id === PHOTOS_ID ? { kind: 'photo_api' } : { kind: 'workshop', wallpaperId: id });
      toast.success(`Now showing ${title}`);
    } catch (err) { toast.error(err, `Could not set ${title}`); }
    finally { setApplying(null); }
  }, [activeMonitorId, applyMonitorSource]);
  const select = useCallback((id: string) => { if (id === PHOTOS_ID) selectPhotoSource(); else selectWallpaper(id); }, [selectPhotoSource, selectWallpaper]);
  const details = useCallback((id: string) => { select(id); onOpenDetails(); }, [select, onOpenDetails]);
  const favorite = useCallback((id: string, next: boolean) => {
    setWallpaperFavorite(id, next).then(
      () => toast.success(next ? 'Added to favorites' : 'Removed from favorites'),
      (err) => toast.error(err, 'Could not update favorites'),
    );
  }, [setWallpaperFavorite]);

  const canApply = !!activeMonitorId && applying === null;
  const emptyLibrary = wallpapers.length === 0;

  return (
    <div>
      <ViewHeader title="Library" subtitle="Your wallpapers, all in one place."
        actions={<LocalImport onStatus={(m) => { if (m && !/^Copying/.test(m)) toast.info(m); }} />} />
      {photoStatus?.offline && (
        <div role="status" className="offline-notice">
          <span>{OFFLINE_NOTICE}</span>
          <button type="button" className="button" onClick={() => void retryPhotoNow().catch((e) => toast.error(e, 'Could not reconnect'))}>Retry now</button>
        </div>
      )}
      <div className="library-toolbar">
        <label className="field-box library-search">
          <Search size={14} aria-hidden="true" />
          <input ref={searchRef} aria-label="Search library" placeholder="Search title or type  (press /)" value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Escape' && query) { e.stopPropagation(); setQuery(''); } }} />
          {query && <button type="button" className="icon-button" aria-label="Clear search" onClick={() => { setQuery(''); searchRef.current?.focus(); }}><X size={13} /></button>}
        </label>
        <div className="library-sort"><Dropdown<LibrarySort> label="Sort library" value={sort} options={SORT_OPTIONS} onChange={setSort} /></div>
      </div>
      <div className="library-filters">
        <SegmentedControl<LibraryFilter> value={filter}
          options={[['all', 'All'], ['favorites', favoriteCount ? `Favorites (${favoriteCount})` : 'Favorites'], ...filters]} onChange={setFilter} />
      </div>
      {emptyLibrary && (
        <div className="empty-state" role="status">
          <Images size={24} />
          <div>Your library is empty besides Photography.</div>
          <div className="empty-actions">
            <button type="button" className="button button-primary" onClick={() => onNavigate('workshop')}>Browse Workshop</button>
            <LocalImport label="Import a video or folder" onStatus={(m) => { if (m && !/^Copying/.test(m)) toast.info(m); }} />
          </div>
        </div>
      )}
      <VirtualGrid items={entries} getKey={(e) => e.id} label="Wallpapers"
        renderItem={(entry, cell) => entry.kind === 'photos' ? (
          <WallpaperCard id={PHOTOS_ID} title="Ian Truong Photography" preview={photoPreview}
            kind="Photography · rotating photos" selected={selectedPhotoSource}
            active={activeSource?.kind === 'photo_api'} canApply={canApply} applying={applying === PHOTOS_ID}
            tabIndex={cell.tabIndex} onSelect={select} onApply={apply} onDetails={details} />
        ) : (
          <WallpaperCard id={entry.id} title={entry.item.title} preview={entry.item.previewPath}
            kind={WALLPAPER_TYPE_LABEL[entry.item.type]} badge={entry.item.windowsOnly ? 'Windows only' : undefined}
            audioReactive={entry.item.audioReactive} favorite={!!entry.item.isFavorite}
            selected={entry.id === selectedWallpaperId}
            active={activeSource?.kind === 'workshop' && activeSource.wallpaperId === entry.id}
            canApply={canApply} applying={applying === entry.id} tabIndex={cell.tabIndex}
            onSelect={select} onApply={apply} onDetails={details} onFavorite={favorite} />
        )} />
      {!emptyLibrary && !entries.some((e) => e.kind === 'wallpaper') && entries.length <= 1 && (search || filter !== 'all') && (
        <div className="empty-state" role="status">
          {filter === 'favorites' && !search
            ? <>No favorites yet. Press the star on a wallpaper to keep it here.</>
            : <>No matches{search ? <> for “{query.trim()}”</> : null}.</>}
          <div className="empty-actions">
            {search && <button type="button" className="button" onClick={() => setQuery('')}>Clear search</button>}
            {filter !== 'all' && <button type="button" className="button" onClick={() => setFilter('all')}>Show all</button>}
          </div>
        </div>
      )}
    </div>
  );
};
