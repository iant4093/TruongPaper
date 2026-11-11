// src/tray/TrayApp.tsx
import React, { useEffect, useState } from 'react';
import { WebviewWindow, getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { open } from '@tauri-apps/plugin-shell';
import { parseOmnibarInput } from '../components/UniversalOmnibar';
import { Thumb } from '../components/ui';
import { formatBytes, WALLPAPER_TYPE_LABEL } from '../lib/format';
import { currentWallpaperForMonitor, useAppStore } from '../store/useAppStore';
import type { TransitionType } from '../types/ipc';

export const TrayApp: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const monitors = useAppStore((s) => s.monitors);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const settings = useAppStore((s) => s.settings);
  const isPaused = useAppStore((s) => s.isPaused);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const setPhotoFavorite = useAppStore((s) => s.setPhotoFavorite);
  const blacklistAndSkipPhoto = useAppStore((s) => s.blacklistAndSkipPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const addAlbum = useAppStore((s) => s.addAlbum);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);

  const [quickLink, setQuickLink] = useState('');
  const [switching, setSwitching] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    void initAppListeners()
      .then((fn) => {
        if (cancelled) fn();
        else cleanup = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, [initAppListeners]);

  const activeMonitor = monitors.find((monitor) => monitor.monitorId === selectedMonitorId) ?? monitors[0];
  const activeMonitorId = activeMonitor?.monitorId;
  const { wallpaper: currentWallpaper, photo: currentPhoto } =
    currentWallpaperForMonitor(activeMonitor, wallpapers, photoStatus);
  const canRotate = !!currentPhoto || activeMonitor?.activeSource.kind === 'photo_api';
  const currentTitle = currentWallpaper?.title ?? currentPhoto?.albumSlug.replace(/-/g, ' ') ?? 'No wallpaper selected';
  const currentDetails = currentWallpaper
    ? `${WALLPAPER_TYPE_LABEL[currentWallpaper.type]} · ${formatBytes(currentWallpaper.bytes)}`
    : currentPhoto?.width && currentPhoto?.height
      ? `${currentPhoto.width} × ${currentPhoto.height} · ${formatBytes(currentPhoto.bytes)}`
      : null;

  const showPhotos = async () => {
    if (!activeMonitorId) return;
    setSwitching(true);
    setActionError(null);
    try {
      if (canRotate) await rotateNextPhoto(activeMonitorId);
      else await applyMonitorSource(activeMonitorId, { kind: 'photo_api' });
    } catch (error) {
      setActionError(String(error));
    } finally {
      setSwitching(false);
    }
  };

  const handleQuickPaste = async (e: React.FormEvent) => {
    e.preventDefault();
    const parsed = parseOmnibarInput(quickLink);
    if (!parsed) return;
    try {
      if (parsed.kind === 'steam_workshop') {
        await downloadWorkshopItem(parsed.workshopId);
      } else {
        await addAlbum(parsed.albumSlug);
      }
      setQuickLink('');
    } catch {
      // Errors (e.g. Steam not signed in) are surfaced in the studio window; keep the link for retry.
    }
  };

  return (
    <div className="app-window-frame tray-shell" style={{ width: 360, height: 480, padding: 14, display: 'flex', flexDirection: 'column', gap: 10 }}>

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <strong style={{ fontSize: 15, letterSpacing: '-0.01em' }}>TruongPaper</strong>
        <div style={{ display: 'flex', gap: 6 }}>
          <button
            type="button"
            className="button"
            style={{ padding: '5px 12px', fontSize: 12 }}
            onClick={async () => {
              // Bring the main studio window forward, then dismiss this popover.
              const studio = await WebviewWindow.getByLabel('main');
              if (studio) {
                await studio.unminimize().catch(() => {});
                await studio.show();
                await studio.setFocus();
              }
              await getCurrentWebviewWindow().hide();
            }}
          >
            Studio
          </button>
        </div>
      </div>

      <form onSubmit={handleQuickPaste} style={{ position: 'relative', zIndex: 1 }}>
        <input
          type="text"
          value={quickLink}
          onChange={(e) => setQuickLink(e.target.value)}
          placeholder="Paste Workshop or Photo Album Link..."
          className="button"
          style={{ width: '100%', textAlign: 'left', fontSize: 12, padding: '7px 12px', cursor: 'text' }}
        />
      </form>

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', gap: 6, overflowX: 'auto' }}>
        {monitors.map((m) => (
          <button
            key={m.monitorId}
            type="button"
            className={`button ${m.monitorId === activeMonitorId ? 'button-primary' : ''}`}
            style={{ fontSize: 11, padding: '4px 10px' }}
            onClick={() => useAppStore.setState({ selectedMonitorId: m.monitorId })}
          >
            {m.name}
          </button>
        ))}
      </div>

      <div className="card" style={{ position: 'relative', zIndex: 1, padding: 14, flex: 1, display: 'flex', flexDirection: 'column', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 11, color: 'var(--accent-cyan)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            Current Wallpaper
          </div>
          <div className="tray-wallpaper-title" title={currentTitle}>
            {currentTitle}
          </div>
          {currentDetails && <div className="tray-wallpaper-meta">{currentDetails}</div>}
          {currentWallpaper?.previewPath && (
            <div className="tray-wallpaper-preview">
              <Thumb src={currentWallpaper.previewPath} alt="" height={84} />
            </div>
          )}
        </div>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              type="button"
              className="button"
              style={{ flex: 1 }}
              disabled={!activeMonitorId}
              onClick={() => void setPlaybackPaused(!isPaused, activeMonitorId)}
            >
              {isPaused ? 'Resume' : 'Pause'}
            </button>
            <button
              type="button"
              className="button button-primary"
              style={{ flex: 1 }}
              disabled={!activeMonitorId || switching}
              onClick={() => void showPhotos()}
            >
              {switching ? 'Loading photo…' : canRotate ? 'Next Wallpaper' : 'Switch to Photos'}
            </button>
          </div>

          {currentPhoto && (
            <div style={{ display: 'flex', gap: 6 }}>
              <button
                type="button"
                className="button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void setPhotoFavorite(currentPhoto.id, !currentPhoto.isFavorite)}
              >
                {currentPhoto.isFavorite ? '★ Favorited' : '☆ Favorite'}
              </button>
              <button
                type="button"
                className="button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void blacklistAndSkipPhoto(currentPhoto.id)}
              >
                Hide
              </button>
              <button
                type="button"
                className="button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void open(currentPhoto.albumUrl)}
              >
                Open Album
              </button>
            </div>
          )}
          {actionError && <div className="tray-wallpaper-error" role="alert" title={actionError}>{actionError}</div>}
        </div>
      </div>

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', alignItems: 'center', justifyContent: 'space-between', fontSize: 12 }}>
        <span style={{ color: 'var(--text-secondary)' }}>GPU Transition</span>
        <select
          className="button"
          style={{ fontSize: 12, padding: '4px 10px' }}
          value={settings?.transitionType ?? 'liquid_glass'}
          onChange={(e) => void updateSettings({ transitionType: e.target.value as TransitionType })}
        >
          <option value="liquid_glass">Liquid Glass Morph</option>
          <option value="blur_dissolve">Blur-Dissolve</option>
          <option value="crossfade">Smooth Crossfade</option>
        </select>
      </div>

      <div style={{ position: 'relative', zIndex: 1, fontSize: 11, color: 'var(--text-muted)', textAlign: 'center' }}>
        Queue: {photoStatus?.unusedQueue.length ?? 0}/5 Offline Ready •{' '}
        {Math.round((photoStatus?.cachedBytes ?? 0) / (1024 * 1024))} /{' '}
        {Math.round((photoStatus?.maxCacheBytes ?? 250 * 1024 * 1024) / (1024 * 1024))} MB
      </div>
    </div>
  );
};
