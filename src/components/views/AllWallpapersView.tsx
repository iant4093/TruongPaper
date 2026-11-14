import React, { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from 'react';
import { Images, Search, X } from 'lucide-react';
import { isCancelMessage, useAppStore, type SteamDownloadProgress } from '../../store/useAppStore';
import { toast } from '../../store/feedback';
import { WALLPAPER_TYPE_LABEL, OFFLINE_NOTICE, offlinePhotosForDisplay, photoPreviewSrc, plural, wallpaperPreviewSrc } from '../../lib/format';
import { useSlashToFocus } from '../../lib/hooks';
import type { Playlist, WallpaperItem, WallpaperSource, WallpaperType } from '../../types/ipc';
import { Dropdown } from '../Dropdown';
import { LocalImport } from '../LocalImport';
import { ViewHeader, SegmentedControl } from '../ui';
import { VirtualGrid } from '../VirtualGrid';
import { DownloadCard, WallpaperCard } from '../WallpaperCard';

type LibraryFilter = 'all' | 'favorites' | 'playlists' | WallpaperType;

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
/** Library tile ids for playlists, so they never clash with wallpaper ids. */
const PLAYLIST_PREFIX = 'playlist:';

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

/** Library tile ids for Workshop downloads in progress. */
const DOWNLOAD_PREFIX = 'download:';

type Entry = { id: string; kind: 'photos' } | { id: string; kind: 'wallpaper'; item: WallpaperItem } | { id: string; kind: 'playlist'; playlist: Playlist }
  | { id: string; kind: 'download'; download: SteamDownloadProgress };

export const AllWallpapersView: React.FC<{
  onOpenDetails: () => void;
  onNavigate: (tab: 'workshop') => void;
}> = ({ onOpenDetails, onNavigate }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const playlists = useAppStore((s) => s.playlists);
  const selectPlaylist = useAppStore((s) => s.selectPlaylist);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const matchOrientation = useAppStore((s) => s.settings?.skipMismatchedOrientation ?? true);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const selectPhotoSource = useAppStore((s) => s.selectPhotoSource);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const setWallpaperFavorite = useAppStore((s) => s.setWallpaperFavorite);
  const retryPhotoNow = useAppStore((s) => s.retryPhotoNow);
  const downloadsById = useAppStore((s) => s.downloadsById);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);
  const cancelWorkshopDownload = useAppStore((s) => s.cancelWorkshopDownload);
  const dismissDownload = useAppStore((s) => s.dismissDownload);
  const [cancelling, setCancelling] = useState<ReadonlySet<string>>(new Set());
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
  /** A playlist tile shows its first wallpaper, or the current photo for photo-only lists. */
  const playlistPreview = (playlist: Playlist) => {
    const first = playlist.items.find((i) => i.kind === 'workshop');
    const item = first && first.kind === 'workshop' ? wallpapers.find((w) => w.wallpaperId === first.wallpaperId) : undefined;
    return wallpaperPreviewSrc(item) ?? photoPreview;
  };
  const installedTypes = useMemo(() => new Set(wallpapers.map((w) => w.type)), [wallpapers]);
  const filters = TYPE_FILTERS.filter(([type]) => installedTypes.has(type));
  const filter = selectedFilter === 'all' || selectedFilter === 'favorites' || (selectedFilter === 'playlists' && playlists.length > 0) || installedTypes.has(selectedFilter as WallpaperType) ? selectedFilter : 'all';
  const search = deferredQuery.trim().toLowerCase();
  const favoriteCount = useMemo(() => wallpapers.filter((w) => w.isFavorite).length, [wallpapers]);

  const entries = useMemo<Entry[]>(() => {
    const matching = filter === 'playlists' ? [] : wallpapers.filter((w) =>
      (filter === 'all' || (filter === 'favorites' ? w.isFavorite : w.type === filter)) && matchesLibrarySearch(w, search));
    const list: Entry[] = sortLibrary(matching, sort).map((item) => ({ id: item.wallpaperId, kind: 'wallpaper', item }));
    // Downloads still running (or failed) come first among the wallpapers, grayed out.
    if (filter === 'all') {
      const downloads = Object.values(downloadsById)
        .filter((d) => d.state !== 'completed' && (!search || d.title.toLowerCase().includes(search) || d.workshopId.includes(search)))
        .map((download): Entry => ({ id: DOWNLOAD_PREFIX + download.workshopId, kind: 'download', download }));
      list.unshift(...downloads);
    }
    // Playlists sit right after Photography, so they are easy to set like any wallpaper.
    if (filter === 'all' || filter === 'playlists') {
      const lists = playlists
        .filter((p) => !search || p.name.toLowerCase().includes(search) || 'playlist'.includes(search))
        .map((playlist): Entry => ({ id: PLAYLIST_PREFIX + playlist.playlistId, kind: 'playlist', playlist }));
      list.unshift(...lists);
    }
    if (filter === 'all' && 'ian truong photography photos'.includes(search)) list.unshift({ id: PHOTOS_ID, kind: 'photos' });
    return list;
  }, [wallpapers, playlists, downloadsById, filter, search, sort]);

  // A finished or cancelled download forgets its "Cancelling…" mark.
  useEffect(() => {
    setCancelling((set) => {
      const kept = [...set].filter((id) => downloadsById[id]?.state === 'downloading');
      return kept.length === set.size ? set : new Set(kept);
    });
  }, [downloadsById]);
  const cancelDownload = useCallback((workshopId: string) => {
    setCancelling((set) => new Set(set).add(workshopId));
    cancelWorkshopDownload(workshopId)
      .then((found) => {
        if (found) return;
        setCancelling((set) => { const next = new Set(set); next.delete(workshopId); return next; });
        toast.info('That download is not running yet. Try again in a moment.');
      })
      .catch((e) => {
        setCancelling((set) => { const next = new Set(set); next.delete(workshopId); return next; });
        toast.error(e, 'Could not cancel the download');
      });
  }, [cancelWorkshopDownload]);
  const retryDownload = useCallback((workshopId: string) => {
    downloadWorkshopItem(workshopId).catch((e) => { if (!isCancelMessage(e)) toast.error(e, 'Download failed'); });
  }, [downloadWorkshopItem]);

  const applySeq = useRef(0);
  const apply = useCallback(async (id: string) => {
    if (!activeMonitorId) return;
    // Clicks are never blocked: the newest one wins, and older ones finish silently.
    const seq = ++applySeq.current;
    setApplying(id);
    const st = useAppStore.getState();
    const playlistId = id.startsWith(PLAYLIST_PREFIX) ? id.slice(PLAYLIST_PREFIX.length) : null;
    const title = id === PHOTOS_ID ? 'Photography'
      : playlistId ? `playlist “${st.playlists.find((p) => p.playlistId === playlistId)?.name ?? 'Playlist'}”`
        : st.wallpapers.find((w) => w.wallpaperId === id)?.title ?? 'Wallpaper';
    const source: WallpaperSource = id === PHOTOS_ID ? { kind: 'photo_api' }
      : playlistId ? { kind: 'playlist', playlistId } : { kind: 'workshop', wallpaperId: id };
    try {
      await applyMonitorSource(activeMonitorId, source);
      if (seq === applySeq.current) toast.success(`Now showing ${title}`);
    } catch (err) { if (seq === applySeq.current) toast.error(err, `Could not set ${title}`); }
    finally { if (seq === applySeq.current) setApplying(null); }
  }, [activeMonitorId, applyMonitorSource]);
  const select = useCallback((id: string) => {
    if (id === PHOTOS_ID) selectPhotoSource();
    else if (id.startsWith(PLAYLIST_PREFIX)) selectPlaylist(id.slice(PLAYLIST_PREFIX.length));
    else selectWallpaper(id);
  }, [selectPhotoSource, selectPlaylist, selectWallpaper]);
  const details = useCallback((id: string) => { select(id); onOpenDetails(); }, [select, onOpenDetails]);
  const favorite = useCallback((id: string, next: boolean) => {
    setWallpaperFavorite(id, next).then(
      () => toast.success(next ? 'Added to favorites' : 'Removed from favorites'),
      (err) => toast.error(err, 'Could not update favorites'),
    );
  }, [setWallpaperFavorite]);

  const canApply = !!activeMonitorId;
  const loaded = useAppStore((s) => s.loaded);
  const loadFailed = useAppStore((s) => s.loadFailed);
  const emptyLibrary = loaded && wallpapers.length === 0
    && !Object.values(downloadsById).some((d) => d.state !== 'completed');

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
          options={[['all', 'All'], ['favorites', favoriteCount ? `Favorites (${favoriteCount})` : 'Favorites'],
            ...(playlists.length ? [['playlists', `Playlists (${playlists.length})`] as const] : []), ...filters]} onChange={setFilter} />
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
      {!loaded && (loadFailed
        ? <div className="view-loading" role="status">Could not load your library. Use Retry above.</div>
        : <div className="view-loading" role="status">Loading…</div>)}
      <VirtualGrid items={entries} getKey={(e) => e.id} label="Wallpapers"
        renderItem={(entry, cell) => entry.kind === 'download' ? (
          <DownloadCard workshopId={entry.download.workshopId} title={entry.download.title || `Workshop item ${entry.download.workshopId}`}
            preview={entry.download.previewUrl} percent={entry.download.progressPercent}
            failed={entry.download.state === 'error'} error={entry.download.error}
            cancelling={cancelling.has(entry.download.workshopId)} tabIndex={cell.tabIndex}
            onCancel={cancelDownload} onRetry={retryDownload} onDismiss={dismissDownload} />
        ) : entry.kind === 'playlist' ? (
          <WallpaperCard id={entry.id} title={entry.playlist.name} preview={playlistPreview(entry.playlist)}
            kind={`Playlist · ${plural(entry.playlist.items.length, 'item')}`}
            active={activeSource?.kind === 'playlist' && activeSource.playlistId === entry.playlist.playlistId}
            canApply={canApply} applying={applying === entry.id} tabIndex={cell.tabIndex} onApply={apply} onDetails={details} />
        ) : entry.kind === 'photos' ? (
          <WallpaperCard id={PHOTOS_ID} title="Ian Truong Photography" preview={photoPreview}
            kind="Photography · rotating photos"
            active={activeSource?.kind === 'photo_api'} canApply={canApply} applying={applying === PHOTOS_ID}
            tabIndex={cell.tabIndex} onApply={apply} onDetails={details} />
        ) : (
          <WallpaperCard id={entry.id} title={entry.item.title} preview={wallpaperPreviewSrc(entry.item)}
            kind={WALLPAPER_TYPE_LABEL[entry.item.type]} badge={entry.item.windowsOnly ? 'Windows only' : undefined}
            audioReactive={entry.item.audioReactive} favorite={!!entry.item.isFavorite}
            active={activeSource?.kind === 'workshop' && activeSource.wallpaperId === entry.id}
            canApply={canApply} applying={applying === entry.id} tabIndex={cell.tabIndex}
            onApply={apply} onDetails={details} onFavorite={favorite} />
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
