import React from 'react';
import { Pause, Play, SkipForward } from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';
import { currentWallpaperForMonitor, useAppStore } from '../store/useAppStore';
import { toast } from '../store/feedback';
import { photoPreviewSrc, wallpaperPreviewSrc } from '../lib/format';
import type { WallpaperSource } from '../types/ipc';
import { LoadErrorCard } from './LoadErrorCard';
import { Thumb } from './ui';

/** Everything the bar shows, derived to primitives so unrelated store updates do not re-render it. */
function usePlaybackInfo() {
  return useAppStore(useShallow((s) => {
    const monitor = s.monitors.find((m) => m.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    const { wallpaper, photo } = currentWallpaperForMonitor(monitor, s.wallpapers, s.photoStatus);
    const policy = monitor ? s.policyStateByMonitor[monitor.monitorId] : undefined;
    const autoPaused = s.systemPowerReasons.length > 0 || (!!monitor && !!policy && policy !== 'playing');
    return {
      monitorId: monitor?.monitorId,
      monitorName: monitor?.name,
      paused: monitor?.userPaused ?? false,
      autoPaused,
      canRotate: monitor?.activeSource.kind === 'photo_api' || !!photo,
      title: wallpaper?.title ?? photo?.albumSlug.replace(/-/g, ' ') ?? 'No wallpaper selected',
      preview: wallpaperPreviewSrc(wallpaper) ?? photoPreviewSrc(photo),
    };
  }));
}

const LoadErrors: React.FC = () => {
  const wallpaperErrors = useAppStore((s) => s.wallpaperErrors);
  const monitors = useAppStore((s) => s.monitors);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const dismiss = useAppStore((s) => s.dismissWallpaperError);
  const apply = useAppStore((s) => s.applyMonitorSource);
  const retry = (monitorId: string, wallpaperId: string | null) => {
    const monitor = monitors.find((m) => m.monitorId === monitorId);
    const source: WallpaperSource | undefined = wallpaperId ? { kind: 'workshop', wallpaperId } : monitor?.activeSource;
    dismiss(monitorId);
    if (source) apply(monitorId, source).catch((err) => toast.error(err, 'Could not set wallpaper'));
  };
  return <>
    {Object.entries(wallpaperErrors).map(([monitorId, failure]) => (
      <LoadErrorCard key={monitorId}
        title={wallpapers.find((w) => w.wallpaperId === failure.wallpaperId)?.title ?? 'Wallpaper'}
        message={failure.message}
        monitorName={monitors.length > 1 ? monitors.find((m) => m.monitorId === monitorId)?.name : undefined}
        onRetry={() => retry(monitorId, failure.wallpaperId)} onDismiss={() => dismiss(monitorId)} />
    ))}
  </>;
};

export const PlaybackBar: React.FC = () => {
  const info = usePlaybackInfo();
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const state = info.paused ? 'Paused' : info.autoPaused ? 'Auto-paused' : 'Playing';
  return (
    <>
      <LoadErrors />
      <footer className="playback-bar">
        <div className="now-playing-image"><Thumb src={info.preview} alt={info.title} height={36} /></div>
        <div className="now-playing-copy">
          <div className="now-playing-title">{info.title}</div>
          <div className="now-playing-meta">{state}{info.monitorName ? ` on ${info.monitorName}` : ''}</div>
        </div>
        {!info.canRotate && <span className="playback-hint" id="next-photo-hint">Next photo works while this display shows photos.</span>}
        <button type="button" className="button playback-control" disabled={!info.monitorId}
          title={info.paused ? 'Resume playback on this display' : 'Pause playback on this display'}
          onClick={() => setPlaybackPaused(!info.paused, info.monitorId).catch((err) => toast.error(err, 'Could not change playback'))}>
          {info.paused ? <Play size={14} /> : <Pause size={14} />}{info.paused ? 'Resume' : 'Pause'}
        </button>
        <button type="button" className="button playback-control" disabled={!info.canRotate}
          aria-describedby={info.canRotate ? undefined : 'next-photo-hint'}
          onClick={() => rotateNextPhoto(info.monitorId).catch((err) => toast.error(err, 'Could not change photo'))}>
          Next photo<SkipForward size={14} />
        </button>
      </footer>
    </>
  );
};
