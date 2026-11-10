import React, { useEffect, useState } from 'react';
import { Gamepad2, Images, LayoutGrid, ListMusic, Monitor, Pause, Play, Settings as SettingsIcon, SkipForward, SlidersHorizontal, Wallpaper } from 'lucide-react';
import { PresetBar } from './components/PresetBar';
import { PropertyInspector } from './components/PropertyInspector';
import { SteamAuthModal } from './components/SteamAuthModal';
import { UniversalOmnibar } from './components/UniversalOmnibar';
import { WindowControls } from './components/WindowControls';
import { WorkshopPreviewModal } from './components/WorkshopPreviewModal';
import { AllWallpapersView } from './components/views/AllWallpapersView';
import { MonitorsView } from './components/views/MonitorsView';
import { PhotosApiView } from './components/views/PhotosApiView';
import { PlaylistsView } from './components/views/PlaylistsView';
import { SettingsView } from './components/views/SettingsView';
import { SteamWorkshopView } from './components/views/SteamWorkshopView';
import { Thumb } from './components/ui';
import { formatBytes } from './lib/format';
import { useAppStore } from './store/useAppStore';

export type StudioTab = 'all' | 'photos' | 'workshop' | 'playlists' | 'monitors' | 'settings';
export const STUDIO_TABS = [
  ['all', 'Library', LayoutGrid],
  ['photos', 'Photography', Images],
  ['workshop', 'Workshop', Gamepad2],
  ['playlists', 'Playlists', ListMusic],
  ['monitors', 'Displays', Monitor],
  ['settings', 'Settings', SettingsIcon],
] as const;
const VIEWS: Record<StudioTab, React.FC> = {
  all: AllWallpapersView, photos: PhotosApiView, workshop: SteamWorkshopView,
  playlists: PlaylistsView, monitors: MonitorsView, settings: SettingsView,
};

export const App: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const isPaused = useAppStore((s) => s.isPaused);
  const systemPowerReasons = useAppStore((s) => s.systemPowerReasons);
  const policyStateByMonitor = useAppStore((s) => s.policyStateByMonitor);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const setLocalWallpaperProperty = useAppStore((s) => s.setLocalWallpaperProperty);
  const setLocalWallpaperAdjustments = useAppStore((s) => s.setLocalWallpaperAdjustments);
  const [activeTab, setActiveTab] = useState<StudioTab>('all');
  const [detailsOpen, setDetailsOpen] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    initAppListeners({ handleTrayMenuEvents: true })
      .then((fn) => { if (cancelled) fn(); else cleanup = fn; })
      .catch((err) => setLoadError(String(err)));
    return () => { cancelled = true; cleanup?.(); };
  }, [initAppListeners]);

  useEffect(() => {
    if (selectedWallpaperId) setDetailsOpen(true);
  }, [selectedWallpaperId]);

  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const currentPhoto = activeMonitor ? photoStatus?.currentPhotoByMonitor?.[activeMonitor.monitorId] : null;
  const activeWallpaper = activeMonitor?.activeSource.kind === 'workshop'
    ? wallpapers.find((w) => activeMonitor.activeSource.kind === 'workshop' && w.wallpaperId === activeMonitor.activeSource.wallpaperId)
    : null;
  const selectedWallpaper = wallpapers.find((w) => w.wallpaperId === selectedWallpaperId);
  const canInspect = !!selectedWallpaper && (activeTab === 'all' || activeTab === 'workshop');
  const showInspector = detailsOpen && canInspect;
  const ActiveView = VIEWS[activeTab];
  const activeLabel = STUDIO_TABS.find(([key]) => key === activeTab)?.[1];
  const autoPaused = systemPowerReasons.length > 0 || (activeMonitor && policyStateByMonitor[activeMonitor.monitorId] && policyStateByMonitor[activeMonitor.monitorId] !== 'playing');
  const playingLabel = isPaused ? 'Paused' : autoPaused ? 'Auto-paused' : 'Playing';
  const currentTitle = activeWallpaper?.title ?? currentPhoto?.albumSlug.replace(/-/g, ' ') ?? 'No wallpaper selected';
  const currentPreview = activeWallpaper?.previewPath ?? currentPhoto?.filePath;
  const canRotate = activeMonitor?.activeSource.kind === 'photo_api';
  const [actionError, setActionError] = useState<string | null>(null);
  const runAction = async (action: () => Promise<void>) => {
    setActionError(null);
    try { await action(); } catch (err) { setActionError(String(err)); }
  };

  return (
    <div className="app-window-frame studio-shell">
      <SteamAuthModal />
      <WorkshopPreviewModal />
      <aside className="app-sidebar">
        <div className="sidebar-titlebar" data-tauri-drag-region><WindowControls /></div>
        <div className="app-brand" data-tauri-drag-region><Wallpaper size={20} strokeWidth={1.5} /><span>TruongPaper</span></div>
        <nav aria-label="Studio views" className="sidebar-nav">
          <div className="sidebar-label">Workspace</div>
          {STUDIO_TABS.filter(([key]) => key !== 'settings').map(([key, label, Icon]) => (
            <button key={key} type="button" aria-current={activeTab === key ? 'page' : undefined}
              aria-label={label} className={`nav-item ${activeTab === key ? 'is-active' : ''}`} onClick={() => setActiveTab(key)}>
              <Icon size={16} strokeWidth={1.6} /><span>{label}</span>
              {key === 'all' && <span className="nav-count">{wallpapers.length + 1}</span>}
            </button>
          ))}
          <div className="sidebar-spacer" />
          <div className="sidebar-status"><span className={`status-dot ${isPaused || autoPaused ? 'is-paused' : ''}`} />{playingLabel}<span className="sidebar-cache">{formatBytes(photoStatus?.cachedBytes ?? 0)}</span></div>
          <button type="button" className={`nav-item ${activeTab === 'settings' ? 'is-active' : ''}`}
            aria-current={activeTab === 'settings' ? 'page' : undefined} onClick={() => setActiveTab('settings')}>
            <SettingsIcon size={16} strokeWidth={1.6} /><span>Settings</span>
          </button>
        </nav>
      </aside>

      <div className="workspace">
        <header className="workspace-titlebar" data-tauri-drag-region>
          <div className="breadcrumb" data-tauri-drag-region><span>Workspace</span><span className="breadcrumb-separator">/</span><span>{activeLabel}</span></div>
          <div className="workspace-controls">
            <Monitor size={14} aria-hidden="true" />
            <select className="display-select" aria-label="Active display" value={activeMonitor?.monitorId ?? ''} onChange={(e) => selectMonitor(e.target.value)}>
              {monitors.map((m) => <option key={m.monitorId} value={m.monitorId}>{m.name}</option>)}
              {!monitors.length && <option value="">No displays connected</option>}
            </select>
            {canInspect && <button type="button" className={`icon-button ${detailsOpen ? 'is-active' : ''}`} aria-label="Toggle wallpaper details" aria-pressed={detailsOpen} onClick={() => setDetailsOpen(!detailsOpen)}><SlidersHorizontal size={16} /></button>}
          </div>
        </header>
        <div className="import-toolbar"><UniversalOmnibar /></div>
        <div className={`workspace-body ${showInspector ? 'with-inspector' : ''}`}>
          <main className="content-viewport">
            {(loadError || actionError) && <div role="alert" className="error-banner">{loadError ?? actionError}</div>}
            <div className="view-content"><ActiveView /></div>
          </main>
          {showInspector && selectedWallpaper && (
            <aside className="inspector-drawer" aria-label="Wallpaper details">
              <div className="inspector-heading">Wallpaper details</div>
              <div className="inspector-preview"><Thumb src={selectedWallpaper.previewPath} alt={selectedWallpaper.title} height={150} /></div>
              <PresetBar wallpaperId={selectedWallpaper.wallpaperId} />
              <PropertyInspector key={selectedWallpaper.wallpaperId} wallpaperId={selectedWallpaper.wallpaperId}
                project={selectedWallpaper.projectJson} values={selectedWallpaper.userProperties} adjustments={selectedWallpaper.adjustments} presets={[]}
                onPropertyChange={(k, v) => setLocalWallpaperProperty(selectedWallpaper.wallpaperId, k, v)}
                onAdjustmentChange={(next) => setLocalWallpaperAdjustments(selectedWallpaper.wallpaperId, next)} />
            </aside>
          )}
        </div>
        <footer className="playback-bar">
          <div className="now-playing-image"><Thumb src={currentPreview} alt={currentTitle} height={36} /></div>
          <div className="now-playing-copy"><div className="now-playing-title">{currentTitle}</div><div className="now-playing-meta">{playingLabel}{activeMonitor ? ` on ${activeMonitor.name}` : ''}</div></div>
          <button type="button" className="button playback-control" onClick={() => void runAction(() => setPlaybackPaused(!isPaused))}>{isPaused ? <Play size={14} /> : <Pause size={14} />}{isPaused ? 'Resume' : 'Pause'}</button>
          <button type="button" className="button playback-control" disabled={!canRotate} title={canRotate ? 'Show the next photo' : 'Select Photography on this display to rotate photos'} onClick={() => void runAction(() => rotateNextPhoto(activeMonitor?.monitorId))}>Next<SkipForward size={14} /></button>
        </footer>
      </div>
    </div>
  );
};
export default App;
