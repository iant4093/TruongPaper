import { Dropdown } from './components/Dropdown';
import React, { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Gamepad2, Images, LayoutGrid, ListMusic, Monitor, Settings as SettingsIcon } from 'lucide-react';
import { DetailsDrawer } from './components/DetailsDrawer';
import { FeedbackHost, StatusBanner } from './components/Feedback';
import { PlaybackBar } from './components/PlaybackBar';
import { ResourceUsage } from './components/ResourceUsage';
import { WindowControls } from './components/WindowControls';
import { AllWallpapersView } from './components/views/AllWallpapersView';
import { errorMessage, useAppStore } from './store/useAppStore';
import { toast } from './store/feedback';
import { usePageVisible } from './lib/hooks';

// Everything except the Library is split out, so the first screen loads (and parses) less code.
const lazyView = <K extends string>(load: () => Promise<Record<K, React.ComponentType<any>>>, name: K) =>
  lazy(() => load().then((m) => ({ default: m[name] })));
const loaders = {
  photos: () => import('./components/views/PhotosApiView'),
  workshop: () => import('./components/views/SteamWorkshopView'),
  playlists: () => import('./components/views/PlaylistsView'),
  monitors: () => import('./components/views/MonitorsView'),
  settings: () => import('./components/views/SettingsView'),
};
const PhotosApiView = lazyView(loaders.photos, 'PhotosApiView');
const SteamWorkshopView = lazyView(loaders.workshop, 'SteamWorkshopView');
const PlaylistsView = lazyView(loaders.playlists, 'PlaylistsView');
const MonitorsView = lazyView(loaders.monitors, 'MonitorsView');
const SettingsView = lazyView(loaders.settings, 'SettingsView');
const SteamAuthModal = lazyView(() => import('./components/SteamAuthModal'), 'SteamAuthModal');
const WorkshopPreviewModal = lazyView(() => import('./components/WorkshopPreviewModal'), 'WorkshopPreviewModal');

export type StudioTab = 'all' | 'photos' | 'workshop' | 'playlists' | 'monitors' | 'settings';
export const STUDIO_TABS = [
  ['all', 'Library', LayoutGrid],
  ['photos', 'Photography', Images],
  ['workshop', 'Workshop', Gamepad2],
  ['playlists', 'Playlists', ListMusic],
  ['monitors', 'Displays', Monitor],
  ['settings', 'Settings', SettingsIcon],
] as const;
const nativeMacTitlebar = /Mac/.test(navigator.platform);

/** True from the first time `flag` is true: keeps a lazily loaded modal mounted after it closes. */
function useEverTrue(flag: boolean) {
  const [seen, setSeen] = useState(flag);
  useEffect(() => { if (flag) setSeen(true); }, [flag]);
  return seen || flag;
}

export const App: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  // While Studio sits hidden in the menu bar, drop the open view (and its decoded images)
  // after a short delay. The web view then gives that memory back; reopening rebuilds it.
  const pageVisible = usePageVisible();
  const [dormant, setDormant] = useState(false);
  useEffect(() => {
    if (pageVisible) { setDormant(false); return; }
    const timer = window.setTimeout(() => setDormant(true), 20_000);
    return () => window.clearTimeout(timer);
  }, [pageVisible]);
  // A primitive key keeps this from re-rendering on unrelated display changes (pause, volume, ...).
  const monitorKey = useAppStore((s) => s.monitors.map((m) => `${m.monitorId}\u0000${m.name}`).join('\u0001'));
  const monitors = useMemo(
    () => (monitorKey ? monitorKey.split('\u0001').map((entry) => entry.split('\u0000') as [string, string]) : []),
    [monitorKey],
  );
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const writeError = useAppStore((s) => s.writeError);
  const dismissWriteError = useAppStore((s) => s.dismissWriteError);
  const photoSourceSelected = useAppStore((s) => s.selectedPhotoSource);
  const wallpaperSelected = useAppStore((s) => s.wallpapers.some((w) => w.wallpaperId === s.selectedWallpaperId));
  const playlistSelected = useAppStore((s) => s.playlists.some((p) => p.playlistId === s.selectedPlaylistId));
  const cachedBytes = useAppStore((s) => s.photoStatus?.cachedBytes ?? 0);
  const authOpen = useEverTrue(useAppStore((s) => s.authModalOpen));
  const previewOpen = useEverTrue(useAppStore((s) => !!s.inspectedWorkshop));
  const [activeTab, setActiveTab] = useState<StudioTab>('all');
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    setLoadError(null);
    initAppListeners()
      .then((fn) => { if (cancelled) fn(); else cleanup = fn; })
      .catch((err) => setLoadError(errorMessage(err)));
    return () => { cancelled = true; cleanup?.(); };
  }, [initAppListeners, loadAttempt]);

  // Warm the other views once the first screen is up, so switching tabs is instant.
  useEffect(() => {
    const timer = window.setTimeout(() => { Object.values(loaders).forEach((load) => void load()); }, 1500);
    return () => window.clearTimeout(timer);
  }, []);

  // A failed debounced write is reported once, as a message.
  useEffect(() => {
    if (!writeError) return;
    toast.error(writeError);
    dismissWriteError();
  }, [writeError, dismissWriteError]);

  // The control that opened the drawer gets focus back when it closes.
  const openerRef = useRef<HTMLElement | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const closeDetails = useCallback(() => {
    setDetailsOpen(false);
    const opener = openerRef.current;
    if (opener?.isConnected) opener.focus();
  }, []);
  const openDetails = useCallback(() => setDetailsOpen(true), []);
  // The top-right button always shows the wallpaper on screen for the chosen display.
  const showingCurrent = useAppStore((s) => {
    const m = s.monitors.find((x) => x.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    if (!m) return false;
    if (m.activeSource.kind === 'playlist') return s.selectedPlaylistId === m.activeSource.playlistId;
    if (m.activeSource.kind === 'workshop') return s.selectedWallpaperId === m.activeSource.wallpaperId;
    return s.selectedPhotoSource;
  });
  const openCurrentDetails = useCallback(() => {
    const s = useAppStore.getState();
    const m = s.monitors.find((x) => x.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    const source = m?.activeSource;
    if (source?.kind === 'workshop') s.selectWallpaper(source.wallpaperId);
    else if (source?.kind === 'playlist') s.selectPlaylist(source.playlistId);
    else s.selectPhotoSource();
    setDetailsOpen(true);
  }, []);

  const canInspect = photoSourceSelected || wallpaperSelected || playlistSelected;
  const showInspector = detailsOpen && canInspect;
  const wasInspectorOpen = useRef(false);
  useEffect(() => {
    if (showInspector && !wasInspectorOpen.current) closeButtonRef.current?.focus();
    wasInspectorOpen.current = showInspector;
  }, [showInspector]);
  useEffect(() => {
    if (!showInspector) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.defaultPrevented) closeDetails();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [showInspector, closeDetails]);

  const activeId = monitors.find(([id]) => id === selectedMonitorId)?.[0] ?? monitors[0]?.[0] ?? '';
  const navigate = (tab: StudioTab) => { setActiveTab(tab); };

  const view = (() => {
    switch (activeTab) {
      case 'all': return <AllWallpapersView onOpenDetails={openDetails} onNavigate={navigate} />;
      case 'photos': return <PhotosApiView />;
      case 'workshop': return <SteamWorkshopView onOpenDetails={openDetails} />;
      case 'playlists': return <PlaylistsView />;
      case 'monitors': return <MonitorsView />;
      case 'settings': return <SettingsView />;
    }
  })();

  const navButton = ([key, label, Icon]: (typeof STUDIO_TABS)[number]) => (
    <button key={key} type="button" aria-current={activeTab === key ? 'page' : undefined} title={label}
      className={`nav-item ${activeTab === key ? 'is-active' : ''}`} onClick={() => setActiveTab(key)}>
      <Icon size={16} strokeWidth={1.6} /><span>{label}</span>
    </button>
  );

  return (
    <div className={`app-window-frame studio-shell${nativeMacTitlebar ? ' native-titlebar' : ''}`}>
      <Suspense fallback={null}>
        {authOpen && <SteamAuthModal />}
        {previewOpen && <WorkshopPreviewModal />}
      </Suspense>
      <FeedbackHost floatingToasts={false} />
      <aside className="app-sidebar">
        <div className="sidebar-titlebar" data-tauri-drag-region>{!nativeMacTitlebar && <WindowControls />}</div>
        <div className="app-brand" data-tauri-drag-region><img src="/app-icon.png" width={23} height={23} alt="" /><span>TruongPaper</span></div>
        <nav aria-label="Studio views" className="sidebar-nav">
          {STUDIO_TABS.filter(([key]) => key !== 'settings').map(navButton)}
          <div className="sidebar-spacer" />
          <StatusBanner />
          <ResourceUsage cachedBytes={cachedBytes} />
          {navButton(STUDIO_TABS[STUDIO_TABS.length - 1])}
        </nav>
      </aside>

      <div className="workspace">
        <header className="workspace-titlebar" data-tauri-drag-region>
          <div className="workspace-controls">
            <Monitor size={14} aria-hidden="true" />
            <span className="workspace-label">Display:</span>
            <Dropdown className="display-select" label="Active display" value={activeId} options={monitors}
              onChange={selectMonitor} placeholder="No displays connected" />
          </div>
          {(() => {
            const hide = showInspector && showingCurrent;
            return (
              <button type="button" className={`button details-toggle ${hide ? 'is-active' : ''}`} aria-pressed={hide}
                disabled={monitors.length === 0}
                onClick={(event) => { openerRef.current = event.currentTarget; if (hide) closeDetails(); else openCurrentDetails(); }}>
                {hide ? 'Hide details' : 'Current wallpaper'}
              </button>
            );
          })()}
        </header>
        <div className={`workspace-body ${showInspector ? 'with-inspector' : ''}`}>
          <main className="content-viewport" onClickCapture={(event) => {
            const opener = (event.target as Element).closest?.('.wallpaper-details-button, .wallpaper-select');
            if (opener instanceof HTMLElement) openerRef.current = opener;
          }}>
            {loadError && (
              <div role="alert" className="error-banner error-banner-dismissible">
                <span>{loadError}</span>
                <button type="button" className="button" onClick={() => setLoadAttempt((n) => n + 1)}>Retry</button>
              </div>
            )}
            <div key={activeTab} className={`view-content${activeTab === 'settings' ? ' settings-enter' : ''}`}>
              <Suspense fallback={<div className="view-loading" role="status">Loading…</div>}>{dormant ? null : view}</Suspense>
            </div>
          </main>
          {showInspector && <div className="inspector-scrim" aria-hidden="true" onClick={closeDetails} />}
          <div className="inspector-slot" inert={!showInspector} aria-hidden={!showInspector}>
            <DetailsDrawer closeRef={closeButtonRef} onClose={closeDetails} onNavigate={(tab) => { closeDetails(); navigate(tab); }} />
          </div>
        </div>
        <PlaybackBar />
      </div>
    </div>
  );
};
export default App;
