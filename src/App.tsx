// src/App.tsx
import React, { useEffect, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import {
  Gamepad2,
  Images,
  LayoutGrid,
  ListMusic,
  Monitor,
  Pause,
  Play,
  Settings as SettingsIcon,
  SkipForward,
} from 'lucide-react';
import { AmbientBacklightCanvas } from './components/AmbientBacklightCanvas';
import { LiquidGlassFilters } from './components/LiquidGlassFilters';
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
import { formatBytes, toAssetUrl } from './lib/format';
import { useAppStore } from './store/useAppStore';

export type StudioTab = 'all' | 'photos' | 'workshop' | 'playlists' | 'monitors' | 'settings';

export const STUDIO_TABS = [
  ['all', 'All Wallpapers', LayoutGrid],
  ['photos', "Ian's Photos API", Images],
  ['workshop', 'Steam Workshop', Gamepad2],
  ['playlists', 'Unified Playlists', ListMusic],
  ['monitors', 'Monitors & Fit', Monitor],
  ['settings', 'Settings', SettingsIcon],
] as const;

const VIEWS: Record<StudioTab, React.FC> = {
  all: AllWallpapersView,
  photos: PhotosApiView,
  workshop: SteamWorkshopView,
  playlists: PlaylistsView,
  monitors: MonitorsView,
  settings: SettingsView,
};

export const App: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const isPaused = useAppStore((s) => s.isPaused);
  const policyStateByMonitor = useAppStore((s) => s.policyStateByMonitor);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const setLocalWallpaperProperty = useAppStore((s) => s.setLocalWallpaperProperty);
  const setLocalWallpaperAdjustments = useAppStore((s) => s.setLocalWallpaperAdjustments);

  const [activeTab, setActiveTab] = useState<StudioTab>('photos');
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    initAppListeners({ handleTrayMenuEvents: true })
      .then((fn) => {
        if (cancelled) fn();
        else cleanup = fn;
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, [initAppListeners]);

  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0] ?? null;
  const currentPhoto =
    (activeMonitor && photoStatus?.currentPhotoByMonitor?.[activeMonitor.monitorId]) ??
    photoStatus?.unusedQueue?.[0] ??
    null;
  const selectedWallpaper = wallpapers.find((w) => w.wallpaperId === selectedWallpaperId) ?? null;
  const showWallpaperInspector = !!selectedWallpaper && activeTab !== 'photos';
  const ActiveView = VIEWS[activeTab];
  const workshopCount = wallpapers.filter((w) => w.type !== 'photo').length;
  const pausedMonitors = Object.values(policyStateByMonitor).filter((s) => s !== 'playing').length;

  const backlightSource =
    activeMonitor?.activeSource.kind === 'workshop' && selectedWallpaper
      ? selectedWallpaper.previewPath
      : currentPhoto?.filePath ?? selectedWallpaper?.previewPath ?? null;

  return (
    <div className="app-window-frame" style={{ display: 'flex', flexDirection: 'column', padding: 16, gap: 14 }}>
      <LiquidGlassFilters />
      <AmbientBacklightCanvas activeThumbnailUrl={toAssetUrl(backlightSource)} />
      <SteamAuthModal />
      <WorkshopPreviewModal />

      {/* Top Liquid Glass Header + Universal Omnibar */}
      <header
        data-tauri-drag-region
        className="glass-panel"
        style={{ zIndex: 1, padding: '12px 18px', display: 'flex', flexDirection: 'column', gap: 10 }}
      >
        <div data-tauri-drag-region style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
          <WindowControls />
          <strong data-tauri-drag-region style={{ fontSize: 17, letterSpacing: '-0.02em', whiteSpace: 'nowrap' }}>
            TruongPaper
          </strong>
          <UniversalOmnibar />
        </div>

        <div data-tauri-drag-region style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ display: 'flex', gap: 8, flex: 1, overflowX: 'auto' }}>
            {monitors.map((m) => (
              <button
                key={m.monitorId}
                type="button"
                className={`glass-button ${m.monitorId === activeMonitor?.monitorId ? 'glass-button-primary' : ''}`}
                style={{ fontSize: 12, padding: '6px 12px', whiteSpace: 'nowrap' }}
                onClick={() => selectMonitor(m.monitorId)}
              >
                {m.name} ({m.width}×{m.height}){m.monitorId === activeMonitor?.monitorId ? ' • Active' : ''}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="glass-button"
            onClick={() => void setPlaybackPaused(!isPaused)}
          >
            {isPaused ? <Play size={14} /> : <Pause size={14} />}
            {isPaused ? 'Resume' : 'Pause'}
          </button>
          <button
            type="button"
            className="glass-button glass-button-primary"
            onClick={() => void rotateNextPhoto(activeMonitor?.monitorId)}
          >
            Next <SkipForward size={14} />
          </button>
        </div>
      </header>

      {/* Main 3-Column Studio Body */}
      <div style={{ zIndex: 1, flex: 1, display: 'grid', gridTemplateColumns: '220px 1fr 320px', gap: 14, minHeight: 0 }}>
        {/* Left Navigation Sidebar */}
        <nav
          aria-label="Studio views"
          className="glass-panel"
          style={{ padding: 14, display: 'flex', flexDirection: 'column', justifyContent: 'space-between' }}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {STUDIO_TABS.map(([key, label, Icon]) => (
              <button
                key={key}
                type="button"
                aria-current={activeTab === key ? 'page' : undefined}
                className={`glass-button ${activeTab === key ? 'glass-button-primary' : ''}`}
                style={{ justifyContent: 'flex-start', width: '100%' }}
                onClick={() => setActiveTab(key)}
              >
                <Icon size={15} />
                {label}
              </button>
            ))}
          </div>

          <div style={{ fontSize: 12, color: 'var(--text-secondary)', display: 'flex', flexDirection: 'column', gap: 4 }}>
            <div>Photo Queue: {photoStatus?.unusedQueue.length ?? 0}/5</div>
            <div>
              Cache: {Math.round((photoStatus?.cachedBytes ?? 0) / (1024 * 1024))} /{' '}
              {Math.round((photoStatus?.maxCacheBytes ?? 250 * 1024 * 1024) / (1024 * 1024))} MB
            </div>
            <div>Shown History: {photoStatus?.historyCount ?? 0} IDs</div>
            <div>Workshop: {workshopCount} items</div>
            <div>
              Playback: {isPaused ? 'Paused' : pausedMonitors > 0 ? `${pausedMonitors} monitor(s) auto-paused` : 'Active'}
            </div>
          </div>
        </nav>

        {/* Center Content Viewport */}
        <main className="glass-panel" style={{ padding: 20, overflowY: 'auto', minHeight: 0 }}>
          {loadError && (
            <div role="alert" style={{ fontSize: 13, color: '#fca5a5', marginBottom: 12 }}>
              Failed to load TruongPaper state: {loadError}
            </div>
          )}
          <AnimatePresence mode="wait">
            <motion.div
              key={activeTab}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -8 }}
              transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
            >
              <ActiveView />
            </motion.div>
          </AnimatePresence>
        </main>

        {/* Right Live Property Inspector Drawer */}
        <div style={{ display: 'flex', flexDirection: 'column', minHeight: 0, overflowY: 'auto' }}>
          {showWallpaperInspector && selectedWallpaper ? (
            <>
              <PresetBar wallpaperId={selectedWallpaper.wallpaperId} />
              <PropertyInspector
                key={selectedWallpaper.wallpaperId}
                wallpaperId={selectedWallpaper.wallpaperId}
                project={selectedWallpaper.projectJson}
                values={selectedWallpaper.userProperties}
                adjustments={selectedWallpaper.adjustments}
                presets={[]}
                // PropertyInspector already invokes monitor_update_property / monitor_update_adjustments;
                // the callbacks only apply the optimistic local update.
                onPropertyChange={(k, v) => setLocalWallpaperProperty(selectedWallpaper.wallpaperId, k, v)}
                onAdjustmentChange={(next) => setLocalWallpaperAdjustments(selectedWallpaper.wallpaperId, next)}
              />
            </>
          ) : (
            <aside className="glass-panel" style={{ padding: 20 }}>
              <h3 style={{ margin: '0 0 12px', fontSize: 16 }}>Active Photo Inspector</h3>
              {currentPhoto ? (
                <>
                  <Thumb src={currentPhoto.filePath} alt={currentPhoto.albumSlug} height={170} />
                  <div style={{ fontWeight: 600, marginTop: 10 }}>{currentPhoto.albumSlug}</div>
                  <p style={{ fontSize: 13, color: 'var(--text-secondary)', margin: '4px 0 0' }}>
                    {currentPhoto.width ?? '?'} × {currentPhoto.height ?? '?'} • {formatBytes(currentPhoto.bytes)}
                    {currentPhoto.featured ? ' • Featured' : ''}
                    {currentPhoto.isFavorite ? ' • Favorite' : ''}
                  </p>
                </>
              ) : (
                <p style={{ fontSize: 13, color: 'var(--text-secondary)' }}>
                  Select a Wallpaper Engine item or rotate a photo from Ian Truong Photography API.
                </p>
              )}
            </aside>
          )}
        </div>
      </div>
    </div>
  );
};

export default App;
