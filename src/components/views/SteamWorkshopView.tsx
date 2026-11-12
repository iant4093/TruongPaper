// src/components/views/SteamWorkshopView.tsx
// §2.3 View 2: "Steam Workshop" Library View
import React, { useCallback, useMemo, useRef, useState } from 'react';
import { RefreshCw, Search } from 'lucide-react';
import { WorkshopBrowser } from '../WorkshopBrowser';
import { WorkshopImport } from '../WorkshopImport';
import { WallpaperCard } from '../WallpaperCard';
import { VirtualGrid } from '../VirtualGrid';
import { useAppStore } from '../../store/useAppStore';
import { toast } from '../../store/feedback';
import { WALLPAPER_TYPE_LABEL } from '../../lib/format';
import { useSlashToFocus } from '../../lib/hooks';
import type { WallpaperItem } from '../../types/ipc';
import { SectionTitle, SegmentedControl, ViewHeader } from '../ui';

export type WorkshopFilter =
  | 'all'
  | 'scene'
  | 'video'
  | 'web'
  | 'application'
  | 'audio'
  | 'local_steam'
  | 'downloaded';

const FILTERS = [
  ['all', 'All'],
  ['scene', 'Scene'],
  ['video', 'Video'],
  ['web', 'Web'],
  ['application', 'Application'],
  ['audio', 'Reacts to sound'],
  ['local_steam', 'From Steam app'],
  ['downloaded', 'Downloaded'],
] as const satisfies ReadonlyArray<readonly [WorkshopFilter, string]>;

/** Items synced from the local Steam client live under `steamapps/workshop/content/431960`. */
export const isLocalSteamItem = (w: WallpaperItem) =>
  /steamapps[\\/]workshop[\\/]content[\\/]431960/i.test(w.installDir);

export function filterWorkshopItems(
  items: WallpaperItem[],
  filter: WorkshopFilter,
  query: string
): WallpaperItem[] {
  const q = query.trim().toLowerCase();
  return items.filter((w) => {
    if (w.type === 'photo') return false;
    switch (filter) {
      case 'scene':
      case 'video':
      case 'web':
      case 'application':
        if (w.type !== filter) return false;
        break;
      case 'audio':
        if (!w.audioReactive) return false;
        break;
      case 'local_steam':
        if (!isLocalSteamItem(w)) return false;
        break;
      case 'downloaded':
        if (isLocalSteamItem(w)) return false;
        break;
      default:
        break;
    }
    if (!q) return true;
    const haystack = [
      w.title,
      w.workshopId ?? '',
      w.wallpaperId,
      ...(w.projectJson.tags ?? []),
      w.projectJson.description ?? '',
    ]
      .join(' ')
      .toLowerCase();
    return haystack.includes(q);
  });
}

/** The only Steam account hint on this page; the full controls live in Settings. */
export const SteamStatusLine: React.FC = () => {
  const steamAccount = useAppStore((s) => s.steamAccount);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);
  return (
    <div className="steam-status" role="status">
      {steamAccount ? <>Steam: signed in as {steamAccount}</> : <>Not signed in to Steam.</>}
      {!steamAccount && <button type="button" className="button button-quiet" onClick={openSteamAuthModal}>Sign in</button>}
    </div>
  );
};

const DownloadsList: React.FC = () => {
  const downloadsById = useAppStore((s) => s.downloadsById);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);
  const dismissDownload = useAppStore((s) => s.dismissDownload);
  const active = Object.values(downloadsById).filter((d) => d.state !== 'completed');
  if (active.length === 0) return null;
  return (
    <>
      <SectionTitle>Downloads</SectionTitle>
      {active.map((d) => (
        <div key={d.workshopId} className="card download-card">
          <div className="download-row">
            <span>{d.title}</span>
            <span>{d.state === 'error' ? 'Failed' : `${Math.round(d.progressPercent)}%`}</span>
          </div>
          {d.state === 'error' ? (
            <>
              <div className="download-error">{d.error}</div>
              <div className="download-actions">
                <button type="button" className="button" onClick={() => {
                  dismissDownload(d.workshopId);
                  downloadWorkshopItem(d.workshopId).catch((e) => toast.error(e, 'Download failed'));
                }}>Retry</button>
                <button type="button" className="button" onClick={() => dismissDownload(d.workshopId)}>Dismiss</button>
              </div>
            </>
          ) : (
            <div className="progress"><div style={{ width: `${Math.min(100, d.progressPercent)}%` }} /></div>
          )}
        </div>
      ))}
    </>
  );
};

const InstalledWorkshopView: React.FC<{ onBrowse: () => void; onOpenDetails: () => void }> = ({ onBrowse, onOpenDetails }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const monitors = useAppStore((s) => s.monitors);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const syncLocalSteamFolders = useAppStore((s) => s.syncLocalSteamFolders);
  const setWallpaperFavorite = useAppStore((s) => s.setWallpaperFavorite);

  const [filter, setFilter] = useState<WorkshopFilter>('all');
  const [query, setQuery] = useState('');
  const [syncing, setSyncing] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  useSlashToFocus(searchRef);

  const items = useMemo(() => filterWorkshopItems(wallpapers, filter, query), [wallpapers, filter, query]);
  const workshopCount = useMemo(() => wallpapers.filter((w) => w.type !== 'photo').length, [wallpapers]);
  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const activeMonitorId = activeMonitor?.monitorId ?? null;
  const activeSource = activeMonitor?.activeSource;

  const runSync = async () => {
    setSyncing(true);
    try {
      await syncLocalSteamFolders();
      toast.success('Library refreshed from the Steam app');
    } catch (err) {
      toast.error(err, 'Could not refresh');
    } finally { setSyncing(false); }
  };
  const apply = useCallback((id: string) => {
    if (!activeMonitorId) return;
    const title = useAppStore.getState().wallpapers.find((w) => w.wallpaperId === id)?.title ?? 'Wallpaper';
    applyMonitorSource(activeMonitorId, { kind: 'workshop', wallpaperId: id }).then(
      () => toast.success(`Now showing ${title}`),
      (err) => toast.error(err, `Could not set ${title}`),
    );
  }, [activeMonitorId, applyMonitorSource]);
  const details = useCallback((id: string) => { selectWallpaper(id); onOpenDetails(); }, [selectWallpaper, onOpenDetails]);
  const favorite = useCallback((id: string, next: boolean) => {
    setWallpaperFavorite(id, next).catch((err) => toast.error(err, 'Could not update favorites'));
  }, [setWallpaperFavorite]);

  return (
    <div>
      <div className="workshop-installed-summary">{workshopCount} {workshopCount === 1 ? 'wallpaper' : 'wallpapers'}</div>
      <DownloadsList />
      <div className="library-filters">
        <SegmentedControl value={filter} options={FILTERS} onChange={setFilter} />
      </div>
      <div className="library-toolbar">
        <label className="field-box library-search">
          <Search size={14} aria-hidden="true" />
          <input ref={searchRef} type="search" aria-label="Search Workshop library" value={query}
            onChange={(e) => setQuery(e.target.value)} placeholder="Search title, tags or Workshop ID…" />
        </label>
        <button type="button" className="button" disabled={syncing} onClick={() => void runSync()}>
          <RefreshCw size={14} /> {syncing ? 'Refreshing…' : 'Refresh library'}
        </button>
      </div>

      <VirtualGrid items={items} getKey={(w) => w.wallpaperId} label="Installed Workshop wallpapers"
        renderItem={(w, cell) => (
          <WallpaperCard id={w.wallpaperId} title={w.title} preview={w.previewPath}
            kind={WALLPAPER_TYPE_LABEL[w.type]} badge={isLocalSteamItem(w) ? 'Steam app' : w.windowsOnly ? 'Windows only' : undefined}
            selected={w.wallpaperId === selectedWallpaperId} audioReactive={w.audioReactive}
            active={activeSource?.kind === 'workshop' && activeSource.wallpaperId === w.wallpaperId}
            favorite={!!w.isFavorite} canApply={!!activeMonitorId} tabIndex={cell.tabIndex}
            onSelect={selectWallpaper} onApply={apply} onDetails={details} onFavorite={favorite} />
        )} />
      {items.length === 0 && (
        <div className="empty-state" role="status">
          {workshopCount === 0 ? 'No Workshop wallpapers installed yet.' : 'No installed wallpapers match.'}
          <div className="empty-actions">
            <button type="button" className="button button-primary" onClick={onBrowse}>Browse Discover</button>
          </div>
        </div>
      )}
    </div>
  );
};

export const SteamWorkshopView: React.FC<{ onOpenDetails?: () => void }> = ({ onOpenDetails = () => {} }) => {
  const [tab, setTab] = useState<'discover' | 'installed'>('discover');
  return <div>
    <ViewHeader title="Workshop" subtitle="Find your next wallpaper." actions={<WorkshopImport />} />
    <SteamStatusLine />
    <div className="workshop-tabs"><SegmentedControl value={tab} options={[["discover", "Discover"], ["installed", "Installed"]]} onChange={setTab} /></div>
    {tab === 'discover' ? <WorkshopBrowser /> : <InstalledWorkshopView onBrowse={() => setTab('discover')} onOpenDetails={onOpenDetails} />}
  </div>;
};
