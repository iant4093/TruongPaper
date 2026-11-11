import React, { useState } from 'react';
import { Images, Search } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { WALLPAPER_TYPE_LABEL, formatBytes, formatOfflinePhotoCount } from '../../lib/format';
import type { WallpaperType } from '../../types/ipc';
import { SectionTitle, Thumb, ViewHeader, SegmentedControl } from '../ui';
import { WallpaperCard } from '../WallpaperCard';

type LibraryFilter = 'all' | WallpaperType;

const TYPE_FILTERS = [
  ['photo', 'Photos'],
  ['scene', 'Scenes'],
  ['video', 'Videos'],
  ['web', 'Web'],
  ['application', 'Applications'],
] as const satisfies ReadonlyArray<readonly [WallpaperType, string]>;

export const AllWallpapersView: React.FC = () => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const [selectedFilter, setFilter] = useState<LibraryFilter>('all');
  const [query, setQuery] = useState('');
  const [error, setError] = useState<string | null>(null);
  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const currentPhoto = activeMonitor ? photoStatus?.currentPhotoByMonitor?.[activeMonitor.monitorId] : null;
  const queue = photoStatus?.unusedQueue ?? [];
  const installedTypes = new Set(wallpapers.map((wallpaper) => wallpaper.type));
  const filters = TYPE_FILTERS.filter(([type]) => installedTypes.has(type));
  const filter = selectedFilter !== 'all' && installedTypes.has(selectedFilter) ? selectedFilter : 'all';
  const search = query.trim().toLowerCase();
  const items = wallpapers.filter((w) => (filter === 'all' || w.type === filter) && w.title.toLowerCase().includes(search));
  const showPhotos = filter === 'all' && 'ian truong photography photos'.includes(search);
  const showQueue = filter === 'all' && !search && !!photoStatus;
  const apply = async (wallpaperId?: string) => {
    if (!activeMonitor) return;
    setError(null);
    try { await applyMonitorSource(activeMonitor.monitorId, wallpaperId ? { kind: 'workshop', wallpaperId } : { kind: 'photo_api' }); }
    catch (err) { setError(String(err)); }
  };
  return (
    <div>
      <ViewHeader title="Library" subtitle="Your wallpapers, all in one place." />
      <div className="library-toolbar">
        {filters.length > 0 && (
          <SegmentedControl<LibraryFilter> value={filter} options={[[ 'all', 'All wallpapers' ], ...filters]} onChange={setFilter} />
        )}
        <label className="library-search"><Search size={14} /><input aria-label="Search library" placeholder="Search wallpapers" value={query} onChange={(e) => setQuery(e.target.value)} /></label>
      </div>
      {error && <div role="alert" className="error-banner">{error}</div>}
      <div className="wallpaper-grid">
        {showPhotos && <WallpaperCard title="Ian Truong Photography" preview={currentPhoto?.filePath} kind="Photography" detail="Photo API" active={activeMonitor?.activeSource.kind === 'photo_api'} canApply={!!activeMonitor} onApply={() => void apply()} />}
        {items.map((w) => <WallpaperCard key={w.wallpaperId} title={w.title} preview={w.previewPath} kind={WALLPAPER_TYPE_LABEL[w.type]} detail={w.windowsOnly ? 'Windows only' : formatBytes(w.bytes)} audioReactive={w.audioReactive} selected={w.wallpaperId === selectedWallpaperId} active={activeMonitor?.activeSource.kind === 'workshop' && activeMonitor.activeSource.wallpaperId === w.wallpaperId} canApply={!!activeMonitor} onSelect={() => selectWallpaper(w.wallpaperId)} onApply={() => void apply(w.wallpaperId)} />)}
      </div>
      {!showPhotos && items.length === 0 && <div className="empty-state"><Images size={24} />No wallpapers match your search.</div>}
      {showQueue && (
        <>
          <SectionTitle>{formatOfflinePhotoCount(queue.length)}</SectionTitle>
          {queue.length > 0 && (
            <div className="queue-strip">
              {queue.map((photo) => (
                <div key={photo.id} className="queue-item" title={`${photo.albumSlug} · ${formatBytes(photo.bytes)}`}>
                  <Thumb src={photo.filePath} alt={photo.albumSlug} height={80} />
                  <div className="queue-item-name">{photo.albumSlug.replace(/-/g, ' ')}</div>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
};
