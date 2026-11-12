import { Dropdown } from '../components/Dropdown';
// src/tray/TrayApp.tsx
import React, { useEffect, useRef, useState } from 'react';
import { WebviewWindow, getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { open } from '@tauri-apps/plugin-shell';
import { Thumb } from '../components/ui';
import { OFFLINE_NOTICE, formatBytes, photoPreviewSrc, WALLPAPER_TYPE_LABEL } from '../lib/format';
import { currentWallpaperForMonitor, useAppStore } from '../store/useAppStore';
import type { WallpaperSource } from '../types/ipc';
import { LoadErrorCard } from '../components/LoadErrorCard';

export const TrayApp: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const monitors = useAppStore((s) => s.monitors);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const setPhotoFavorite = useAppStore((s) => s.setPhotoFavorite);
  const blacklistAndSkipPhoto = useAppStore((s) => s.blacklistAndSkipPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const retryPhotoNow = useAppStore((s) => s.retryPhotoNow);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const wallpaperErrors = useAppStore((s) => s.wallpaperErrors);
  const dismissWallpaperError = useAppStore((s) => s.dismissWallpaperError);
  const refreshLive = useAppStore((s) => s.refreshLive);
  const nextRef = useRef<HTMLButtonElement>(null);
  const [switching, setSwitching] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const run = (action: () => Promise<unknown>) => {
    setActionError(null);
    action().catch((error) => setActionError(String(error)));
  };

  useEffect(() => {
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    void initAppListeners()
      .then((fn) => {
        if (cancelled) fn();
        else cleanup = fn;
      })
      .catch((error) => {
        if (!cancelled) setActionError(`Could not load TruongPaper status: ${String(error)}`);
      });
    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, [initAppListeners]);

  // The popover keeps its own store, so reload edits made in Studio each time it opens.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebviewWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) {
          // Only what changes while the popover is closed; library and settings arrive by event.
          refreshLive().then(() => setActionError(null)).catch((error) => setActionError(String(error)));
          nextRef.current?.focus();
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refreshLive]);

  // Escape closes the popover.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void getCurrentWebviewWindow().hide().catch(() => {});
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  const activeMonitor = monitors.find((monitor) => monitor.monitorId === selectedMonitorId) ?? monitors[0];
  const activeMonitorId = activeMonitor?.monitorId;
  const isPaused = activeMonitor?.userPaused ?? false;
  const { wallpaper: currentWallpaper, photo: currentPhoto } =
    currentWallpaperForMonitor(activeMonitor, wallpapers, photoStatus);
  const canRotate = !!currentPhoto || activeMonitor?.activeSource.kind === 'photo_api';
  const currentTitle = currentWallpaper?.title ?? currentPhoto?.albumSlug.replace(/-/g, ' ') ?? 'No wallpaper selected';
  const currentPreview = currentWallpaper?.previewPath || photoPreviewSrc(currentPhoto);
  const currentDetails = currentWallpaper
    ? `${WALLPAPER_TYPE_LABEL[currentWallpaper.type]} · ${formatBytes(currentWallpaper.bytes)}`
    : currentPhoto?.width && currentPhoto?.height
      ? `${currentPhoto.width} × ${currentPhoto.height} · ${formatBytes(currentPhoto.bytes)}`
      : null;

  const loadFailure = activeMonitorId ? wallpaperErrors[activeMonitorId] : undefined;
  const failedTitle = loadFailure
    ? wallpapers.find((w) => w.wallpaperId === loadFailure.wallpaperId)?.title ?? 'Wallpaper'
    : '';
  const retryLoad = () => {
    if (!activeMonitor || !loadFailure) return;
    const source: WallpaperSource = loadFailure.wallpaperId
      ? { kind: 'workshop', wallpaperId: loadFailure.wallpaperId }
      : activeMonitor.activeSource;
    dismissWallpaperError(activeMonitor.monitorId);
    run(() => applyMonitorSource(activeMonitor.monitorId, source));
  };

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
  const togglePause = async () => {
    if (!activeMonitorId) return;
    setActionError(null);
    try { await setPlaybackPaused(!isPaused, activeMonitorId); }
    catch (error) { setActionError(String(error)); }
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

      {monitors.length > 1 && (
        <Dropdown label="Display" className="tray-display" value={activeMonitorId ?? ''}
          options={monitors.map((m) => [m.monitorId, m.name] as const)} onChange={selectMonitor} />
      )}

      <div className="card tray-wallpaper-card">
        <div className="tray-wallpaper-content">
          <div className="tray-wallpaper-title" title={currentTitle}>
            {currentTitle}
          </div>
          {currentDetails && <div className="tray-wallpaper-meta">{currentDetails}</div>}
          {currentPreview && (
            <div className="tray-wallpaper-preview">
              <Thumb src={currentPreview} alt="" height={160} contain />
            </div>
          )}
        </div>

        <div className="tray-actions">
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              type="button"
              className="button"
              style={{ flex: 1 }}
              disabled={!activeMonitorId}
              onClick={() => void togglePause()}
            >
              {isPaused ? 'Resume this display' : 'Pause this display'}
            </button>
            <button
              type="button"
              ref={nextRef}
              className="button button-primary"
              style={{ flex: 1 }}
              disabled={!activeMonitorId || switching}
              onClick={() => void showPhotos()}
            >
              {switching ? 'Loading photo…' : canRotate ? 'Next photo' : 'Use photos'}
            </button>
          </div>

          {currentPhoto && (
            <div style={{ display: 'flex', gap: 6 }}>
              <button
                type="button"
                className="button"
                style={{ flex: 1 }}
                onClick={() => run(() => setPhotoFavorite(currentPhoto.id, !currentPhoto.isFavorite))}
              >
                {currentPhoto.isFavorite ? '★ Favorited' : '☆ Favorite'}
              </button>
              <button
                type="button"
                className="button"
                style={{ flex: 1 }}
                onClick={() => run(() => blacklistAndSkipPhoto(currentPhoto.id))}
              >
                Hide
              </button>
              <button
                type="button"
                className="button"
                style={{ flex: 1 }}
                onClick={() => run(() => open(currentPhoto.albumUrl))}
              >
                Open album
              </button>
            </div>
          )}
          {loadFailure && activeMonitorId && (
            <LoadErrorCard className="tray-load-error" title={failedTitle} message={loadFailure.message}
              onRetry={retryLoad} onDismiss={() => dismissWallpaperError(activeMonitorId)} />
          )}
          {actionError && <div className="tray-wallpaper-error" role="alert" title={actionError}>{actionError}</div>}
        </div>
      </div>

      {photoStatus && (
        <div className="tray-cache-status">
          {formatBytes(photoStatus.cachedBytes)} cached
          {photoStatus.offline && (
            <div className="tray-offline" role="status">
              {OFFLINE_NOTICE}{' '}
              <button type="button" className="button button-quiet" onClick={() => run(retryPhotoNow)}>Retry now</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
