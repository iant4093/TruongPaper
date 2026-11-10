// src/components/views/PlaylistsView.tsx
// §2.4 View 3: "Unified Playlists & Rotation" View
import React, { useRef, useState } from 'react';
import { Download, Plus, Save, Upload } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { ROTATION_INTERVALS, TRANSITION_LABEL } from '../../lib/format';
import type { Playlist, TransitionType, WallpaperPreset } from '../../types/ipc';
import { SectionTitle, SegmentedControl, ViewHeader } from '../ui';

type RotationMode = 'photo_api' | 'workshop' | 'playlist';
type Scope = 'monitor' | 'global';

export interface PlaylistExportFile {
  format: 'truongpaper-playlists';
  version: 1;
  exportedAt: number;
  playlists: Playlist[];
  presets: WallpaperPreset[];
  albumSlugs: string[];
}

const newPlaylistId = () =>
  typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `pl-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

export function parsePlaylistExport(raw: string): PlaylistExportFile {
  const data = JSON.parse(raw) as Partial<PlaylistExportFile>;
  if (data.format !== 'truongpaper-playlists' || !Array.isArray(data.playlists)) {
    throw new Error('Not a TruongPaper playlist export file');
  }
  const playlists = data.playlists.filter(
    (p): p is Playlist =>
      !!p &&
      typeof p.playlistId === 'string' &&
      typeof p.name === 'string' &&
      ['photos_only', 'workshop_only', 'mixed'].includes(p.mode) &&
      typeof p.intervalSeconds === 'number' &&
      Array.isArray(p.items)
  );
  return {
    format: 'truongpaper-playlists',
    version: 1,
    exportedAt: Number(data.exportedAt ?? Date.now()),
    playlists,
    presets: Array.isArray(data.presets) ? data.presets : [],
    albumSlugs: Array.isArray(data.albumSlugs) ? data.albumSlugs.filter((s) => typeof s === 'string') : [],
  };
}

export const PlaylistsView: React.FC = () => {
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const playlists = useAppStore((s) => s.playlists);
  const settings = useAppStore((s) => s.settings);
  const savedAlbums = useAppStore((s) => s.savedAlbums);
  const presetsByWallpaper = useAppStore((s) => s.presetsByWallpaper);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const savePlaylist = useAppStore((s) => s.savePlaylist);
  const addAlbum = useAppStore((s) => s.addAlbum);

  const activeMonitor = monitors.find((m) => m.monitorId === selectedMonitorId) ?? monitors[0] ?? null;
  const workshopItems = wallpapers.filter((w) => w.type !== 'photo');

  const [scope, setScope] = useState<Scope>('monitor');
  const [mode, setMode] = useState<RotationMode>(activeMonitor?.activeSource.kind ?? 'photo_api');
  const [singleId, setSingleId] = useState<string>(
    activeMonitor?.activeSource.kind === 'workshop' ? activeMonitor.activeSource.wallpaperId : workshopItems[0]?.wallpaperId ?? ''
  );
  const [playlistId, setPlaylistId] = useState<string>(
    activeMonitor?.activeSource.kind === 'playlist' ? activeMonitor.activeSource.playlistId : playlists[0]?.playlistId ?? ''
  );
  const [draft, setDraft] = useState<Playlist | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const applyRotation = async () => {
    const targets = scope === 'global' ? monitors : activeMonitor ? [activeMonitor] : [];
    const source =
      mode === 'photo_api'
        ? ({ kind: 'photo_api' } as const)
        : mode === 'workshop'
          ? ({ kind: 'workshop', wallpaperId: singleId } as const)
          : ({ kind: 'playlist', playlistId } as const);
    if ((mode === 'workshop' && !singleId) || (mode === 'playlist' && !playlistId)) {
      setMsg('Choose a wallpaper or playlist first');
      return;
    }
    try {
      for (const m of targets) {
        await applyMonitorSource(m.monitorId, source);
      }
      setMsg(`Applied to ${targets.length} monitor${targets.length === 1 ? '' : 's'}`);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const startNewPlaylist = () =>
    setDraft({
      playlistId: newPlaylistId(),
      name: 'New Mixed Playlist',
      mode: 'mixed',
      intervalSeconds: settings?.rotationIntervalSeconds || 1800,
      items: [{ kind: 'photo_api_draw' }],
    });

  const saveDraft = async () => {
    if (!draft) return;
    try {
      const saved = await savePlaylist(draft);
      setDraft(null);
      setPlaylistId(saved.playlistId);
      setMsg(`Saved playlist "${saved.name}"`);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const exportPlaylists = () => {
    const referenced = new Set(
      playlists.flatMap((p) => p.items.flatMap((i) => (i.kind === 'workshop' ? [i.wallpaperId] : [])))
    );
    const file: PlaylistExportFile = {
      format: 'truongpaper-playlists',
      version: 1,
      exportedAt: Date.now(),
      playlists,
      presets: Object.entries(presetsByWallpaper)
        .filter(([id]) => referenced.has(id))
        .flatMap(([, list]) => list),
      albumSlugs: savedAlbums.map((a) => a.albumSlug),
    };
    const blob = new Blob([JSON.stringify(file, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'truongpaper-playlists.json';
    a.click();
    URL.revokeObjectURL(url);
  };

  const importPlaylists = async (fileList: FileList | null) => {
    const file = fileList?.[0];
    if (!file) return;
    try {
      const parsed = parsePlaylistExport(await file.text());
      for (const p of parsed.playlists) await savePlaylist(p);
      for (const slug of parsed.albumSlugs) {
        if (!savedAlbums.some((a) => a.albumSlug === slug)) await addAlbum(slug);
      }
      setMsg(`Imported ${parsed.playlists.length} playlist(s) and ${parsed.albumSlugs.length} album slug(s)`);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : String(err));
    } finally {
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  const draftWorkshopIds = new Set(
    draft?.items.flatMap((i) => (i.kind === 'workshop' ? [i.wallpaperId] : [])) ?? []
  );
  const draftPhotoDraws = draft?.items.filter((i) => i.kind === 'photo_api_draw').length ?? 0;

  return (
    <div>
      <ViewHeader
        title="Unified Playlists & Rotation"
        subtitle={msg ?? 'Mix Ian Truong Photo API draws with Wallpaper Engine items'}
        actions={
          <>
            <button type="button" className="glass-button" onClick={exportPlaylists}>
              <Download size={14} /> Export .json
            </button>
            <button type="button" className="glass-button" onClick={() => fileRef.current?.click()}>
              <Upload size={14} /> Import .json
            </button>
            <input
              ref={fileRef}
              type="file"
              accept="application/json,.json"
              aria-label="Import playlist file"
              style={{ display: 'none' }}
              onChange={(e) => void importPlaylists(e.target.files)}
            />
          </>
        }
      />

      {/* Mode Selector */}
      <div className="glass-card" style={{ padding: 16 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12, gap: 12, flexWrap: 'wrap' }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Rotation Source</div>
          <SegmentedControl
            value={scope}
            options={[
              ['monitor', `Per Monitor (${activeMonitor?.name ?? 'none'})`],
              ['global', 'Global (All Monitors)'],
            ]}
            onChange={setScope}
          />
        </div>
        <SegmentedControl
          value={mode}
          options={[
            ['photo_api', "Ian's Photo API Only"],
            ['workshop', 'Single Wallpaper Engine Item'],
            ['playlist', 'Unified Mixed Playlist'],
          ]}
          onChange={setMode}
        />
        {mode === 'workshop' && (
          <select
            className="glass-button"
            aria-label="Wallpaper Engine item"
            style={{ width: '100%', marginTop: 12 }}
            value={singleId}
            onChange={(e) => setSingleId(e.target.value)}
          >
            {workshopItems.length === 0 && <option value="">No Workshop items in library</option>}
            {workshopItems.map((w) => (
              <option key={w.wallpaperId} value={w.wallpaperId}>
                {w.title}
              </option>
            ))}
          </select>
        )}
        {mode === 'playlist' && (
          <select
            className="glass-button"
            aria-label="Playlist"
            style={{ width: '100%', marginTop: 12 }}
            value={playlistId}
            onChange={(e) => setPlaylistId(e.target.value)}
          >
            {playlists.length === 0 && <option value="">No playlists yet</option>}
            {playlists.map((p) => (
              <option key={p.playlistId} value={p.playlistId}>
                {p.name}
              </option>
            ))}
          </select>
        )}
        <button type="button" className="glass-button glass-button-primary" style={{ marginTop: 12 }} onClick={() => void applyRotation()}>
          Apply Rotation Source
        </button>
      </div>

      {/* Rotation Interval & Trigger Controls */}
      <SectionTitle>Rotation Interval & Triggers</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
        <SegmentedControl
          value={String(settings?.rotationIntervalSeconds ?? 1800)}
          options={ROTATION_INTERVALS.map((r) => [String(r.seconds), r.label] as const)}
          onChange={(next) => void updateSettings({ rotationIntervalSeconds: Number(next) })}
        />
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 12 }}>
          Global hotkey for Next Wallpaper: <code style={{ fontFamily: 'var(--font-mono)' }}>Cmd+Shift+N</code> (macOS) /{' '}
          <code style={{ fontFamily: 'var(--font-mono)' }}>Ctrl+Shift+N</code> (Windows)
        </div>
      </div>

      {/* GPU Transition Selector */}
      <SectionTitle>GPU Transition</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
        <SegmentedControl
          value={settings?.transitionType ?? 'liquid_glass'}
          options={(Object.keys(TRANSITION_LABEL) as TransitionType[]).map((k) => [k, TRANSITION_LABEL[k]] as const)}
          onChange={(next) => void updateSettings({ transitionType: next })}
        />
        <label style={{ display: 'block', fontSize: 13, marginTop: 14 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Transition Duration</span>
            <span>{(settings?.transitionDurationSeconds ?? 1.2).toFixed(1)}s</span>
          </div>
          <input
            type="range"
            className="glass-slider"
            aria-label="Transition Duration"
            min={0.3}
            max={3.0}
            step={0.1}
            value={settings?.transitionDurationSeconds ?? 1.2}
            onChange={(e) => void updateSettings({ transitionDurationSeconds: Number(e.target.value) })}
          />
        </label>
      </div>

      {/* Playlists */}
      <SectionTitle>Playlists</SectionTitle>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {playlists.map((p) => (
          <div key={p.playlistId} className="glass-card" style={{ padding: 12, display: 'flex', alignItems: 'center', gap: 12 }}>
            <div style={{ flex: 1 }}>
              <div style={{ fontWeight: 600, fontSize: 13 }}>{p.name}</div>
              <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
                {p.mode.replace('_', ' ')} • {p.items.length} item(s) • every{' '}
                {ROTATION_INTERVALS.find((r) => r.seconds === p.intervalSeconds)?.label ?? `${p.intervalSeconds}s`}
              </div>
            </div>
            <button type="button" className="glass-button" onClick={() => setDraft(JSON.parse(JSON.stringify(p)) as Playlist)}>
              Edit
            </button>
          </div>
        ))}
        {!draft && (
          <button type="button" className="glass-button" style={{ alignSelf: 'flex-start' }} onClick={startNewPlaylist}>
            <Plus size={14} /> New Playlist
          </button>
        )}
      </div>

      {draft && (
        <div className="glass-card" style={{ padding: 16, marginTop: 12 }}>
          <input
            type="text"
            aria-label="Playlist name"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            className="glass-button"
            style={{ width: '100%', textAlign: 'left', cursor: 'text', marginBottom: 12 }}
          />
          <SegmentedControl
            value={draft.mode}
            options={[
              ['photos_only', 'Photos Only'],
              ['workshop_only', 'Workshop Only'],
              ['mixed', 'Mixed'],
            ]}
            onChange={(next) =>
              setDraft({
                ...draft,
                mode: next,
                items:
                  next === 'photos_only'
                    ? [{ kind: 'photo_api_draw' }]
                    : next === 'workshop_only'
                      ? draft.items.filter((i) => i.kind === 'workshop')
                      : draft.items,
              })
            }
          />
          <select
            className="glass-button"
            aria-label="Playlist interval"
            style={{ width: '100%', marginTop: 12 }}
            value={String(draft.intervalSeconds)}
            onChange={(e) => setDraft({ ...draft, intervalSeconds: Number(e.target.value) })}
          >
            {ROTATION_INTERVALS.map((r) => (
              <option key={r.seconds} value={r.seconds}>
                {r.label}
              </option>
            ))}
          </select>

          {draft.mode === 'mixed' && (
            <label style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13, marginTop: 12 }}>
              <span>Photo API draws per cycle</span>
              <input
                type="number"
                min={0}
                max={10}
                value={draftPhotoDraws}
                style={{ width: 60 }}
                onChange={(e) => {
                  const n = Math.max(0, Math.min(10, Number(e.target.value) || 0));
                  setDraft({
                    ...draft,
                    items: [
                      ...Array.from({ length: n }, () => ({ kind: 'photo_api_draw' as const })),
                      ...draft.items.filter((i) => i.kind === 'workshop'),
                    ],
                  });
                }}
              />
            </label>
          )}

          {draft.mode !== 'photos_only' && (
            <div style={{ marginTop: 12 }}>
              {workshopItems.map((w) => (
                <label key={w.wallpaperId} style={{ display: 'flex', gap: 8, fontSize: 13, marginBottom: 6 }}>
                  <input
                    type="checkbox"
                    checked={draftWorkshopIds.has(w.wallpaperId)}
                    onChange={(e) =>
                      setDraft({
                        ...draft,
                        items: e.target.checked
                          ? [...draft.items, { kind: 'workshop', wallpaperId: w.wallpaperId }]
                          : draft.items.filter((i) => !(i.kind === 'workshop' && i.wallpaperId === w.wallpaperId)),
                      })
                    }
                  />
                  {w.title}
                </label>
              ))}
              {workshopItems.length === 0 && (
                <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>No Workshop items in the library yet.</div>
              )}
            </div>
          )}

          <div style={{ display: 'flex', gap: 8, marginTop: 14 }}>
            <button
              type="button"
              className="glass-button glass-button-primary"
              disabled={draft.items.length === 0 || !draft.name.trim()}
              onClick={() => void saveDraft()}
            >
              <Save size={14} /> Save Playlist
            </button>
            <button type="button" className="glass-button" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
};
