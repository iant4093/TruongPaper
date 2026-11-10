// src/components/views/SteamWorkshopView.tsx
// §2.3 View 2: "Steam Workshop" Library View
import React, { useMemo, useState } from 'react';
import { AudioLines, LogIn, LogOut, MonitorSmartphone, RefreshCw, Search, Trash2 } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { WALLPAPER_TYPE_LABEL } from '../../lib/format';
import type { WallpaperItem } from '../../types/ipc';
import { Badge, SectionTitle, SegmentedControl, Thumb, ViewHeader, cardGrid } from '../ui';

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
  ['audio', 'Audio Reactive'],
  ['local_steam', 'Local Steam Synced'],
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

export const SteamWorkshopView: React.FC = () => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const downloadsById = useAppStore((s) => s.downloadsById);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const monitors = useAppStore((s) => s.monitors);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const deleteWallpaper = useAppStore((s) => s.deleteWallpaper);
  const syncLocalSteamFolders = useAppStore((s) => s.syncLocalSteamFolders);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);
  const steamLogout = useAppStore((s) => s.steamLogout);

  const [filter, setFilter] = useState<WorkshopFilter>('all');
  const [query, setQuery] = useState('');
  const [syncMsg, setSyncMsg] = useState<string | null>(null);

  const items = useMemo(() => filterWorkshopItems(wallpapers, filter, query), [wallpapers, filter, query]);
  const workshopCount = wallpapers.filter((w) => w.type !== 'photo').length;
  const localCount = wallpapers.filter(isLocalSteamItem).length;
  const activeDownloads = Object.values(downloadsById).filter((d) => d.state !== 'completed');
  const activeMonitorId = selectedMonitorId ?? monitors[0]?.monitorId ?? null;

  const runSync = async () => {
    setSyncMsg('Syncing local Steam library…');
    try {
      await syncLocalSteamFolders();
      setSyncMsg('Local Steam Library: Synced');
    } catch (err) {
      setSyncMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div>
      <ViewHeader
        title="Workshop"
        subtitle={`${workshopCount} Wallpaper Engine items • ${localCount} from the local Steam library`}
      />

      {/* Steam Connection Pill */}
      <div className="card" style={{ padding: 12, display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
        <div style={{ flex: 1, fontSize: 13 }}>
          <div style={{ fontWeight: 600 }}>Steam Account</div>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
            {syncMsg ?? 'Sign in once to download Workshop items via DepotDownloader.'}
          </div>
        </div>
        <button type="button" className="button" onClick={openSteamAuthModal}>
          <LogIn size={14} /> Manage Steam Account
        </button>
        <button type="button" className="button" onClick={() => void steamLogout()}>
          <LogOut size={14} /> Sign Out
        </button>
        <button type="button" className="button button-primary" onClick={() => void runSync()}>
          <RefreshCw size={14} /> Sync Local Steam Library
        </button>
      </div>

      {activeDownloads.length > 0 && (
        <>
          <SectionTitle>Downloads</SectionTitle>
          {activeDownloads.map((d) => (
            <div key={d.workshopId} className="card" style={{ padding: 12, marginBottom: 8 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13 }}>
                <span>{d.title}</span>
                <span>{d.state === 'error' ? 'Failed' : `${Math.round(d.progressPercent)}%`}</span>
              </div>
              {d.state === 'error' ? (
                <div style={{ fontSize: 12, color: '#fca5a5', marginTop: 4 }}>{d.error}</div>
              ) : (
                <div style={{ height: 6, borderRadius: 999, background: 'rgba(255,255,255,0.12)', marginTop: 6 }}>
                  <div
                    style={{
                      width: `${Math.min(100, d.progressPercent)}%`,
                      height: '100%',
                      borderRadius: 999,
                      background: 'linear-gradient(90deg, var(--accent-blue), var(--accent-cyan))',
                    }}
                  />
                </div>
              )}
            </div>
          ))}
        </>
      )}

      <div style={{ display: 'flex', gap: 12, alignItems: 'center', margin: '16px 0', flexWrap: 'wrap' }}>
        <SegmentedControl value={filter} options={FILTERS} onChange={setFilter} />
        <div className="button" style={{ flex: 1, minWidth: 200, cursor: 'text', justifyContent: 'flex-start' }}>
          <Search size={14} />
          <input
            type="search"
            aria-label="Search Workshop library"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search title, tags or Workshop ID…"
            style={{ flex: 1, background: 'transparent', border: 'none', outline: 'none', color: 'inherit', font: 'inherit' }}
          />
        </div>
      </div>

      <div style={cardGrid}>
        {items.map((w) => (
          <div
            key={w.wallpaperId}
            className="card"
            role="button"
            tabIndex={0}
            aria-pressed={w.wallpaperId === selectedWallpaperId}
            style={{
              padding: 12,
              cursor: 'pointer',
              outline: w.wallpaperId === selectedWallpaperId ? '2px solid var(--accent-cyan)' : 'none',
            }}
            onClick={() => selectWallpaper(w.wallpaperId)}
            onKeyDown={(e) => e.key === 'Enter' && selectWallpaper(w.wallpaperId)}

          >
            {/* preview.gif / preview.jpg from the Workshop item (GIF previews animate natively) */}
            <Thumb src={w.previewPath} alt={w.title} height={110} />
            <div style={{ display: 'flex', gap: 6, marginTop: 8, alignItems: 'center' }}>
              <Badge tone={w.windowsOnly ? 'warn' : 'cyan'}>{WALLPAPER_TYPE_LABEL[w.type]}</Badge>
              {w.audioReactive && (
                <span title="Audio reactive" style={{ color: 'var(--accent-cyan)', display: 'inline-flex' }}>
                  <AudioLines size={14} />
                </span>
              )}
              {isLocalSteamItem(w) && <Badge tone="muted">Steam</Badge>}
            </div>
            <div style={{ fontWeight: 600, marginTop: 6, fontSize: 13 }}>{w.title}</div>
            {w.workshopId && (
              <div style={{ fontSize: 11, color: 'var(--text-muted)', fontFamily: 'var(--font-mono)' }}>#{w.workshopId}</div>
            )}
            <div style={{ display: 'flex', gap: 6, marginTop: 10 }}>
              <button
                type="button"
                className="button button-primary"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                disabled={!activeMonitorId}
                onClick={(e) => {
                  e.stopPropagation();
                  if (activeMonitorId) {
                    void applyMonitorSource(activeMonitorId, { kind: 'workshop', wallpaperId: w.wallpaperId });
                  }
                }}
              >
                <MonitorSmartphone size={12} /> Apply
              </button>
              <button
                type="button"
                className="button"
                aria-label={`Delete ${w.title}`}
                style={{ fontSize: 11, padding: '6px 10px' }}
                onClick={(e) => {
                  e.stopPropagation();
                  void deleteWallpaper(w.wallpaperId);
                }}
              >
                <Trash2 size={12} />
              </button>
            </div>
          </div>
        ))}
        {items.length === 0 && (
          <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
            No Workshop wallpapers match. Paste a Workshop URL into the omnibar to download one.
          </div>
        )}
      </div>
    </div>
  );
};
