// src/components/views/AllWallpapersView.tsx
// §2 "All Wallpapers": unified grid of Ian's Photo API + every Wallpaper Engine item, plus the Active Queue strip.
import React from 'react';
import { AudioLines, Image as ImageIcon, MonitorSmartphone } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { WALLPAPER_TYPE_LABEL, formatBytes } from '../../lib/format';
import { Badge, SectionTitle, Thumb, ViewHeader, cardGrid, spotlightHandlers } from '../ui';

export const AllWallpapersView: React.FC = () => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const selectedWallpaperId = useAppStore((s) => s.selectedWallpaperId);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);

  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0] ?? null;
  const currentPhoto =
    (activeMonitor ? photoStatus?.currentPhotoByMonitor?.[activeMonitor.monitorId] : null) ??
    photoStatus?.unusedQueue?.[0] ??
    null;
  const queue = photoStatus?.unusedQueue ?? [];
  const isActive = (wallpaperId: string) =>
    activeMonitor?.activeSource.kind === 'workshop' && activeMonitor.activeSource.wallpaperId === wallpaperId;

  return (
    <div>
      <ViewHeader
        title="All Wallpapers"
        subtitle={`${wallpapers.length + 1} sources • applying to ${activeMonitor?.name ?? 'no monitor'}`}
      />

      <div style={cardGrid}>
        {/* Ian's Photo API is always available as a dynamic source */}
        <div
          className="glass-card"
          style={{
            padding: 12,
            outline: activeMonitor?.activeSource.kind === 'photo_api' ? '2px solid var(--accent-cyan)' : 'none',
          }}
          {...spotlightHandlers()}
        >
          <Thumb src={currentPhoto?.filePath} alt="Ian Truong Photography" height={110} />
          <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
            <Badge>Photo</Badge>
            <Badge tone="muted">{queue.length}/5 Ready</Badge>
          </div>
          <div style={{ fontWeight: 600, marginTop: 6, fontSize: 13 }}>Ian Truong Photography API</div>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
            {currentPhoto ? currentPhoto.albumSlug : 'Rotating public photo pool'}
          </div>
          <button
            type="button"
            className="glass-button glass-button-primary"
            style={{ width: '100%', marginTop: 10, fontSize: 12 }}
            disabled={!activeMonitor}
            onClick={() => activeMonitor && void applyMonitorSource(activeMonitor.monitorId, { kind: 'photo_api' })}
          >
            <ImageIcon size={12} /> Use Photo API
          </button>
        </div>

        {wallpapers.map((w) => (
          <div
            key={w.wallpaperId}
            className="glass-card"
            role="button"
            tabIndex={0}
            aria-pressed={w.wallpaperId === selectedWallpaperId}
            onClick={() => selectWallpaper(w.wallpaperId)}
            onKeyDown={(e) => e.key === 'Enter' && selectWallpaper(w.wallpaperId)}
            style={{
              padding: 12,
              cursor: 'pointer',
              outline: isActive(w.wallpaperId)
                ? '2px solid var(--accent-cyan)'
                : w.wallpaperId === selectedWallpaperId
                  ? '1px solid rgba(255,255,255,0.5)'
                  : 'none',
            }}
            {...spotlightHandlers()}
          >
            <Thumb src={w.previewPath} alt={w.title} height={110} />
            <div style={{ display: 'flex', gap: 6, marginTop: 8, alignItems: 'center' }}>
              <Badge tone={w.windowsOnly ? 'warn' : 'cyan'}>{WALLPAPER_TYPE_LABEL[w.type]}</Badge>
              {w.audioReactive && (
                <span title="Audio reactive" style={{ color: 'var(--accent-cyan)', display: 'inline-flex' }}>
                  <AudioLines size={14} />
                </span>
              )}
            </div>
            <div style={{ fontWeight: 600, marginTop: 6, fontSize: 13 }}>{w.title}</div>
            <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{formatBytes(w.bytes)}</div>
            <button
              type="button"
              className="glass-button"
              style={{ width: '100%', marginTop: 10, fontSize: 12 }}
              disabled={!activeMonitor}
              onClick={(e) => {
                e.stopPropagation();
                if (activeMonitor) {
                  void applyMonitorSource(activeMonitor.monitorId, { kind: 'workshop', wallpaperId: w.wallpaperId });
                }
              }}
            >
              <MonitorSmartphone size={12} /> {isActive(w.wallpaperId) ? 'Active' : 'Apply'}
            </button>
          </div>
        ))}
      </div>

      <SectionTitle>
        Active Queue ({queue.length}/5 Ready Offline)
      </SectionTitle>
      <div style={{ display: 'flex', gap: 10, overflowX: 'auto', paddingBottom: 4 }}>
        {queue.map((p) => (
          <div key={p.id} style={{ width: 120, flex: '0 0 auto' }} title={`${p.albumSlug} • ${formatBytes(p.bytes)}`}>
            <Thumb src={p.filePath} alt={p.albumSlug} height={72} />
            <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 4, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {p.albumSlug}
            </div>
          </div>
        ))}
        {queue.length === 0 && <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Queue empty.</div>}
      </div>
    </div>
  );
};
