// src/components/views/MonitorsView.tsx
// §2.5 View 4: "Monitors & Fit" View
import React, { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Copy, Image as ImageIcon, Lock } from 'lucide-react';
import { currentWallpaperForMonitor, useAppStore } from '../../store/useAppStore';
import { FIT_MODE_LABEL } from '../../lib/format';
import type { FitMode, MonitorInfo, WallpaperSource } from '../../types/ipc';
import { Badge, SectionTitle, SegmentedControl, ToggleRow, ViewHeader } from '../ui';

export function describeSource(
  source: WallpaperSource,
  lookup: { wallpaperTitle: (id: string) => string | undefined; playlistName: (id: string) => string | undefined }
): string {
  switch (source.kind) {
    case 'photo_api':
      return "Ian's Photo API";
    case 'workshop':
      return lookup.wallpaperTitle(source.wallpaperId) ?? source.wallpaperId;
    case 'playlist':
      return `Playlist: ${lookup.playlistName(source.playlistId) ?? source.playlistId}`;
  }
}

const DIAGRAM_HEIGHT = 170;

export const MonitorsView: React.FC = () => {
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const settings = useAppStore((s) => s.settings);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const playlists = useAppStore((s) => s.playlists);
  const policyStateByMonitor = useAppStore((s) => s.policyStateByMonitor);
  const selectMonitor = useAppStore((s) => s.selectMonitor);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const [msg, setMsg] = useState<string | null>(null);
  const [lockScreenPending, setLockScreenPending] = useState(false);
  const [lockScreenStatus, setLockScreenStatus] = useState<string | null>(null);

  const active = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0] ?? null;
  const { wallpaper, photo } = currentWallpaperForMonitor(active ?? undefined, wallpapers, photoStatus);
  const mac = /Mac/.test(navigator.platform);
  const lookup = {
    wallpaperTitle: (id: string) => wallpapers.find((w) => w.wallpaperId === id)?.title,
    playlistName: (id: string) => playlists.find((p) => p.playlistId === id)?.name,
  };

  // Scale every display by its logical (DPI-independent) size so the diagram is proportional.
  const logical = (m: MonitorInfo) => ({
    w: m.width / (m.scaleFactor || 1),
    h: m.height / (m.scaleFactor || 1),
  });
  const maxLogicalH = Math.max(1, ...monitors.map((m) => logical(m).h));

  const cloneToAll = async () => {
    if (!active) return;
    try {
      for (const m of monitors) {
        if (m.monitorId !== active.monitorId) await applyMonitorSource(m.monitorId, active.activeSource);
      }
      setMsg(`Applied "${describeSource(active.activeSource, lookup)}" to ${monitors.length - 1} other monitor(s)`);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : String(err));
    }
  };
  const setLockScreen = async () => {
    if (!active) return;
    setLockScreenPending(true);
    setLockScreenStatus(null);
    try {
      await invoke('wallpaper_set_lock_screen', { monitorId: active.monitorId });
      setLockScreenStatus(mac ? 'System wallpaper set. Lock your Mac to check the image.' : 'Lock screen image set.');
    } catch (err) {
      setLockScreenStatus(String(err));
    } finally {
      setLockScreenPending(false);
    }
  };

  return (
    <div>
      <ViewHeader
        title="Displays"
        subtitle={msg ?? `${monitors.length} display${monitors.length === 1 ? '' : 's'} connected`}
      />

      {/* Interactive visual diagram */}
      <div
        className="card"
        style={{ padding: 20, display: 'flex', alignItems: 'flex-end', justifyContent: 'center', gap: 18, minHeight: DIAGRAM_HEIGHT + 40, flexWrap: 'wrap' }}
      >
        {monitors.map((m) => {
          const { w, h } = logical(m);
          const scale = DIAGRAM_HEIGHT / maxLogicalH;
          const selected = m.monitorId === active?.monitorId;
          return (
            <button
              key={m.monitorId}
              type="button"
              aria-pressed={selected}
              onClick={() => selectMonitor(m.monitorId)}
              className="button"
              style={{
                width: Math.max(90, w * scale),
                height: Math.max(60, h * scale),
                borderRadius: 12,
                flexDirection: 'column',
                gap: 4,
                padding: 8,
                outline: selected ? '2px solid var(--accent-cyan)' : 'none',
              }}
            >
              <strong style={{ fontSize: 12 }}>{m.name}</strong>
              <span style={{ fontSize: 11, color: 'var(--text-secondary)' }}>
                {m.width}×{m.height}
              </span>
              {m.isPrimary && <Badge>Primary</Badge>}
            </button>
          );
        })}
        {monitors.length === 0 && <div style={{ color: 'var(--text-muted)', fontSize: 13 }}>No displays detected.</div>}
      </div>

      {active && (
        <>
          <SectionTitle>{active.name}</SectionTitle>
          <div className="card" style={{ padding: 16, display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))', gap: 12, fontSize: 13 }}>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>Resolution</div>
              {active.width} × {active.height}
            </div>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>DPI Scale</div>
              {Math.round(active.scaleFactor * 100)}%
            </div>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>Refresh Rate</div>
              {active.refreshRateHz} Hz
            </div>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>Orientation</div>
              {active.isPortrait ? 'Portrait' : 'Landscape'}
            </div>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>Playback</div>
              {(policyStateByMonitor[active.monitorId] ?? 'playing').replace(/_/g, ' ')}
            </div>
            <div>
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>Active Source</div>
              {describeSource(active.activeSource, lookup)}
            </div>
          </div>
          {active.activeSource.kind !== 'photo_api' && (
            <button
              type="button"
              className="button"
              style={{ marginTop: 10 }}
              onClick={() => void applyMonitorSource(active.monitorId, { kind: 'photo_api' })}
            >
              <ImageIcon size={14} /> Switch to Ian's Photo API
            </button>
          )}
        </>
      )}

      <SectionTitle>Lock Screen</SectionTitle>
      <div className="card lock-screen-card">
        <div className="view-subtitle">
          Use a still of the selected display’s wallpaper. Workshop items use their preview image.
          {mac && ' This also sets the system wallpaper on the primary display.'}
        </div>
        <button type="button" className="button" disabled={lockScreenPending || !(photo || wallpaper?.previewPath)}
          onClick={() => void setLockScreen()}>
          <Lock size={14} /> {lockScreenPending ? 'Preparing image…' : 'Set Lock Screen Still'}
        </button>
        {lockScreenStatus && <div role="status">{lockScreenStatus}</div>}
      </div>

      <SectionTitle>Orientation Matching for Photo API</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <ToggleRow
          label="Skip portrait photos on landscape monitors (and vice-versa)"
          checked={settings?.skipMismatchedOrientation ?? true}
          onChange={(next) => void updateSettings({ skipMismatchedOrientation: next })}
        />
        <div style={{ fontSize: 13, marginBottom: 8 }}>Photo fit mode</div>
        <SegmentedControl
          value={settings?.defaultFitMode ?? 'fill'}
          options={(Object.keys(FIT_MODE_LABEL) as FitMode[]).map((k) => [k, FIT_MODE_LABEL[k]] as const)}
          onChange={(next) => void updateSettings({ defaultFitMode: next })}
        />
      </div>

      <SectionTitle>Multi-Monitor Mode</SectionTitle>
      <div className="card" style={{ padding: 16, fontSize: 13 }}>
        <div style={{ color: 'var(--text-secondary)', marginBottom: 12 }}>
          Each display keeps its own source. Apply the selected source to all displays to share a Workshop
          wallpaper or photo pool. Photo draws remain independent.
        </div>
        <button
          type="button"
          className="button button-primary"
          disabled={!active || monitors.length < 2}
          onClick={() => void cloneToAll()}
        >
          <Copy size={14} /> Use source on all displays
        </button>
      </div>
    </div>
  );
};
