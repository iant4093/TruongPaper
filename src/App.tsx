import { Dropdown } from './components/Dropdown';
import React, { useEffect, useState } from 'react';
import { Check, Gamepad2, Images, LayoutGrid, ListMusic, Monitor, Pause, Play, Settings as SettingsIcon, SkipForward, SlidersHorizontal, X } from 'lucide-react';
import { PresetBar } from './components/PresetBar';
import { PhotoSourceInspector } from './components/PhotoSourceInspector';
import { PropertyInspector } from './components/PropertyInspector';
import { SteamAuthModal } from './components/SteamAuthModal';
import { WindowControls } from './components/WindowControls';
import { WorkshopPreviewModal } from './components/WorkshopPreviewModal';
import { AllWallpapersView } from './components/views/AllWallpapersView';
import { MonitorsView } from './components/views/MonitorsView';
import { PhotosApiView } from './components/views/PhotosApiView';
import { PlaylistsView } from './components/views/PlaylistsView';
import { SettingsView } from './components/views/SettingsView';
import { SteamWorkshopView } from './components/views/SteamWorkshopView';
import { Thumb } from './components/ui';
import { ResourceUsage } from './components/ResourceUsage';
import { currentWallpaperForMonitor, useAppStore } from './store/useAppStore';
import { photoPreviewSrc } from './lib/format';
import type { WallpaperSource } from './types/ipc';

export type StudioTab = 'all' | 'photos' | 'workshop' | 'playlists' | 'monitors' | 'settings';
export const STUDIO_TABS = [
  ['all', 'Library', LayoutGrid],
  ['photos', 'Photography', Images],
  ['workshop', 'Workshop', Gamepad2],
  ['playlists', 'Playlists', ListMusic],
  ['monitors', 'Displays', Monitor],
  ['settings', 'Settings', SettingsIcon],
] as const;
const VIEWS: Record<Exclude<StudioTab, 'all'>, React.FC> = {
  photos: PhotosApiView, workshop: SteamWorkshopView,
  playlists: PlaylistsView, monitors: MonitorsView, settings: SettingsView,
};
const nativeMacTitlebar = /Mac/.test(navigator.platform);

export const App: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectedPhotoSource = useAppStore((s) => s.selectedPhotoSource);
  const selectPhotoSource = useAppStore((s) => s.selectPhotoSource);
  const systemPowerReasons = useAppStore((s) => s.systemPowerReasons);
  const policyStateByMonitor = useAppStore((s) => s.policyStateByMonitor);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const setLocalWallpaperProperty = useAppStore((s) => s.setLocalWallpaperProperty);
  const setLocalWallpaperAdjustments = useAppStore((s) => s.setLocalWallpaperAdjustments);
  const [activeTab, setActiveTab] = useState<StudioTab>('all');
  const [detailsOpen, setDetailsOpen] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [applyingDetails, setApplyingDetails] = useState(false);
  const [detailsError, setDetailsError] = useState<string | null>(null);

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

  useEffect(() => { setDetailsError(null); }, [selectedWallpaperId, selectedPhotoSource, selectedMonitorId]);

  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0];
  const isPaused = activeMonitor?.userPaused ?? false;
  const { wallpaper: activeWallpaper, photo: currentPhoto } =
    currentWallpaperForMonitor(activeMonitor, wallpapers, photoStatus);
  const selectedWallpaper = wallpapers.find((w) => w.wallpaperId === selectedWallpaperId);
  const detailsTitle = selectedPhotoSource ? 'Ian Truong Photography' : selectedWallpaper?.title;
  const detailsSource: WallpaperSource | undefined = selectedPhotoSource ? { kind: 'photo_api' }
    : selectedWallpaper ? { kind: 'workshop', wallpaperId: selectedWallpaper.wallpaperId } : undefined;
  const detailsCurrent = detailsSource?.kind === 'photo_api'
    ? activeMonitor?.activeSource.kind === 'photo_api'
    : activeMonitor?.activeSource.kind === 'workshop' && activeMonitor.activeSource.wallpaperId === selectedWallpaperId;
  const canInspect = (selectedPhotoSource && activeTab === 'all') ||
    (!!selectedWallpaper && (activeTab === 'all' || activeTab === 'workshop'));
  const showInspector = detailsOpen && canInspect;
  const ActiveView = activeTab === 'all' ? null : VIEWS[activeTab];
  const activeLabel = STUDIO_TABS.find(([key]) => key === activeTab)?.[1];
  const autoPaused = systemPowerReasons.length > 0 || (activeMonitor && policyStateByMonitor[activeMonitor.monitorId] && policyStateByMonitor[activeMonitor.monitorId] !== 'playing');
  const playingLabel = isPaused ? 'Paused' : autoPaused ? 'Auto-paused' : 'Playing';
  const currentTitle = activeWallpaper?.title ?? currentPhoto?.albumSlug.replace(/-/g, ' ') ?? 'No wallpaper selected';
  const currentPreview = activeWallpaper?.previewPath ?? photoPreviewSrc(currentPhoto);
  const canRotate = activeMonitor?.activeSource.kind === 'photo_api' || !!currentPhoto;
  const [actionError, setActionError] = useState<string | null>(null);
  const runAction = async (action: () => Promise<void>) => {
    setActionError(null);
    try { await action(); } catch (err) { setActionError(String(err)); }
  };
  const applyDetails = async () => {
    if (!activeMonitor || !detailsSource || detailsCurrent || applyingDetails) return;
    setApplyingDetails(true);
    setDetailsError(null);
    try { await applyMonitorSource(activeMonitor.monitorId, detailsSource); }
    catch (err) { setDetailsError(String(err)); }
    finally { setApplyingDetails(false); }
  };

  return (
    <div className={`app-window-frame studio-shell${nativeMacTitlebar ? ' native-titlebar' : ''}`}>
      <SteamAuthModal />
      <WorkshopPreviewModal />
      <aside className="app-sidebar">
        <div className="sidebar-titlebar" data-tauri-drag-region>{!nativeMacTitlebar && <WindowControls />}</div>
        <div className="app-brand" data-tauri-drag-region><img src="/app-icon.png" width={23} height={23} alt="" /><span>TruongPaper</span></div>
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
          <ResourceUsage cachedBytes={photoStatus?.cachedBytes ?? 0} />
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
            <Dropdown className="display-select" label="Active display" value={activeMonitor?.monitorId ?? ''}
              options={monitors.map((monitor) => [monitor.monitorId, monitor.name])}
              onChange={selectMonitor} placeholder="No displays connected" />
            {canInspect && <button type="button" className={`icon-button ${detailsOpen ? 'is-active' : ''}`} aria-label="Toggle wallpaper details" aria-pressed={detailsOpen} onClick={() => setDetailsOpen(!detailsOpen)}><SlidersHorizontal size={16} /></button>}
          </div>
        </header>
        <div className={`workspace-body ${showInspector ? 'with-inspector' : ''}`}>
          <main className="content-viewport">
            {(loadError || actionError) && <div role="alert" className="error-banner">{loadError ?? actionError}</div>}
            <div key={activeTab} className={`view-content${activeTab === 'settings' ? ' settings-enter' : ''}`}>
              {ActiveView ? <ActiveView /> : (
                <AllWallpapersView onSelectPhotoSource={() => { selectPhotoSource(); setDetailsOpen(true); }} />
              )}
            </div>
          </main>
          <div className="inspector-slot" inert={!showInspector} aria-hidden={!showInspector}>
            {(selectedPhotoSource || selectedWallpaper) && (
              <aside className="inspector-drawer" aria-label="Wallpaper details">
                <div className="inspector-heading">
                  <span>Wallpaper details</span>
                  <button type="button" className="icon-button" aria-label="Close wallpaper details" onClick={() => setDetailsOpen(false)}><X size={16} /></button>
                </div>
                <div className="inspector-selection">
                  <h3>{detailsTitle}</h3>
                  {detailsCurrent ? (
                    <span className="inspector-current"><Check size={14} /> Current on this display</span>
                  ) : (
                    <button type="button" className="button button-primary" disabled={!activeMonitor || applyingDetails} onClick={() => void applyDetails()}>
                      {applyingDetails ? 'Applying…' : 'Apply wallpaper'}
                    </button>
                  )}
                  {detailsError && <div role="alert" className="error-banner">{detailsError}</div>}
                </div>
                {selectedPhotoSource ? <PhotoSourceInspector monitor={activeMonitor} /> : selectedWallpaper && (
                  <>
                    <div className="inspector-preview"><Thumb src={selectedWallpaper.previewPath} alt={selectedWallpaper.title} height={150} /></div>
                    <PresetBar key={selectedWallpaper.wallpaperId} wallpaperId={selectedWallpaper.wallpaperId} />
                    <PropertyInspector key={selectedWallpaper.wallpaperId} wallpaperId={selectedWallpaper.wallpaperId}
                      project={selectedWallpaper.projectJson} values={selectedWallpaper.userProperties} adjustments={selectedWallpaper.adjustments} presets={[]}
                      onPropertyChange={(k, v) => setLocalWallpaperProperty(selectedWallpaper.wallpaperId, k, v)}
                      onAdjustmentChange={(next) => setLocalWallpaperAdjustments(selectedWallpaper.wallpaperId, next)} />
                  </>
                )}
              </aside>
            )}
          </div>
        </div>
        <footer className="playback-bar">
          <div className="now-playing-image"><Thumb src={currentPreview} alt={currentTitle} height={36} /></div>
          <div className="now-playing-copy"><div className="now-playing-title">{currentTitle}</div><div className="now-playing-meta">{playingLabel}{activeMonitor ? ` on ${activeMonitor.name}` : ''}</div></div>
          <button type="button" className="button playback-control" disabled={!activeMonitor} onClick={() => void runAction(() => setPlaybackPaused(!isPaused, activeMonitor?.monitorId))}>{isPaused ? <Play size={14} /> : <Pause size={14} />}{isPaused ? 'Resume' : 'Pause'}</button>
          <button type="button" className="button playback-control" disabled={!canRotate} title={canRotate ? 'Show the next photo' : 'Select Photography on this display to rotate photos'} onClick={() => void runAction(() => rotateNextPhoto(activeMonitor?.monitorId))}>Next<SkipForward size={14} /></button>
        </footer>
      </div>
    </div>
  );
};
export default App;
