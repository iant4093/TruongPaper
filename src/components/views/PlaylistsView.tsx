// src/components/views/PlaylistsView.tsx
// §2.4 View 3: "Unified Playlists & Rotation" View
import React, { useRef, useState } from 'react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { Download, Pencil, Plus, Save, Trash2, Upload } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { PLAYLIST_MODE_LABEL, ROTATION_INTERVALS, plural } from '../../lib/format';
import { confirmAction, toast } from '../../store/feedback';
import type { Playlist, WallpaperPreset } from '../../types/ipc';
import { SectionTitle, ViewHeader } from '../ui';
import { PlaylistEditor, playlistIsValid } from '../PlaylistEditor';

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
  const loaded = useAppStore((s) => s.loaded);
  const loadFailed = useAppStore((s) => s.loadFailed);
  const monitors = useAppStore((s) => s.monitors);
  const playlists = useAppStore((s) => s.playlists);
  const settings = useAppStore((s) => s.settings);
  const savedAlbums = useAppStore((s) => s.savedAlbums);
  const presetsByWallpaper = useAppStore((s) => s.presetsByWallpaper);
  const savePlaylist = useAppStore((s) => s.savePlaylist);
  const exportPlaylistsFile = useAppStore((s) => s.exportPlaylists);
  const deletePlaylist = useAppStore((s) => s.deletePlaylist);
  const addAlbum = useAppStore((s) => s.addAlbum);

  /** Which displays currently play each playlist. */
  const usedBy = (playlistId: string) =>
    monitors.filter((m) => m.activeSource.kind === 'playlist' && m.activeSource.playlistId === playlistId).map((m) => m.name);

  const [draft, setDraft] = useState<Playlist | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const startNewPlaylist = () =>
    setDraft({
      playlistId: newPlaylistId(),
      name: 'New playlist',
      mode: 'mixed',
      intervalSeconds: settings?.rotationIntervalSeconds || 1800,
      items: [{ kind: 'photo_api_draw' }],
    });

  const saveDraft = async () => {
    if (!draft) return;
    try {
      const saved = await savePlaylist(draft);
      setDraft(null);
      toast.success(`Saved playlist “${saved.name}”. Set it from Library.`);
    } catch (err) {
      toast.error(err);
    }
  };

  const removePlaylist = async (p: Playlist) => {
    const users = usedBy(p.playlistId);
    const ok = await confirmAction(`Delete the playlist “${p.name}”?`, 'Delete playlist',
      users.length ? `${users.join(', ')} ${users.length === 1 ? 'is' : 'are'} playing it right now.` : undefined);
    if (!ok) return;
    try {
      await deletePlaylist(p.playlistId);
      if (draft?.playlistId === p.playlistId) setDraft(null);
      toast.info(`Deleted “${p.name}”`, async () => { await savePlaylist(p); });
    } catch (err) { toast.error(err, 'Could not delete'); }
  };

  const exportPlaylists = async () => {
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
    try {
      const path = await saveDialog({
        defaultPath: 'truongpaper-playlists.json',
        filters: [{ name: 'TruongPaper playlists', extensions: ['json'] }],
        title: 'Save playlists',
      });
      if (!path) return;
      await exportPlaylistsFile(path, JSON.stringify(file, null, 2));
      toast.success('Playlists saved to a file');
    } catch (err) { toast.error(err, 'Could not save the playlists'); }
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
      toast.success(`Loaded ${plural(parsed.playlists.length, 'playlist')} and ${plural(parsed.albumSlugs.length, 'album')}`);
    } catch (err) {
      toast.error(err);
    } finally {
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  const intervalLabel = (seconds: number) =>
    seconds === 0 ? 'manual only' : `every ${ROTATION_INTERVALS.find((r) => r.seconds === seconds)?.label ?? `${seconds}s`}`;

  return (
    <div>
      <ViewHeader
        title="Playlists"
        subtitle="Mix photos and wallpapers. Pick a playlist for each display in Displays."
        actions={
          <>
            <button type="button" className="button" onClick={() => void exportPlaylists()}>
              <Download size={14} /> Save playlists to a file
            </button>
            <button type="button" className="button" onClick={() => fileRef.current?.click()}>
              <Upload size={14} /> Load playlists from a file
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

      <SectionTitle>Your playlists</SectionTitle>
      <div className="stack">
        {playlists.map((p) => {
          const users = usedBy(p.playlistId);
          return (
            <div key={p.playlistId} className="card playlist-row">
              <div className="playlist-row-main">
                <div className="playlist-name">{p.name}</div>
                <div className="view-subtitle" style={{ marginTop: 2 }}>
                  {PLAYLIST_MODE_LABEL[p.mode]} · {plural(p.items.length, 'item')} · {intervalLabel(p.intervalSeconds)}
                </div>
                <div className="view-subtitle" style={{ marginTop: 2 }}>
                  {users.length ? `Playing on: ${users.join(', ')}` : 'Not used by any display'}
                </div>
              </div>
              <button type="button" className="button" aria-label={`Edit ${p.name}`} onClick={() => setDraft(JSON.parse(JSON.stringify(p)) as Playlist)}>
                <Pencil size={13} /> Edit
              </button>
              <button type="button" className="button button-quiet" aria-label={`Delete ${p.name}`} onClick={() => void removePlaylist(p)}>
                <Trash2 size={13} /> Delete
              </button>
            </div>
          );
        })}
        {playlists.length === 0 && !draft && !loaded && (
          <div className="view-loading" role="status">{loadFailed ? 'Could not load your playlists. Use Retry above.' : 'Loading…'}</div>
        )}
        {playlists.length === 0 && !draft && loaded && (
          <div className="empty-state" role="status">
            No playlists yet.
            <div className="empty-actions">
              <button type="button" className="button button-primary" onClick={startNewPlaylist}>Create your first playlist</button>
            </div>
          </div>
        )}
        {!draft && playlists.length > 0 && (
          <button type="button" className="button" style={{ alignSelf: 'flex-start' }} onClick={startNewPlaylist}>
            <Plus size={14} /> New playlist
          </button>
        )}
      </div>

      {draft && (
        <div className="card" style={{ padding: 16, marginTop: 12 }}>
          <PlaylistEditor value={draft} onChange={setDraft} />
          <div style={{ display: 'flex', gap: 8, marginTop: 14 }}>
            <button
              type="button"
              className="button button-primary"
              disabled={!playlistIsValid(draft)}
              onClick={() => void saveDraft()}
            >
              <Save size={14} /> Save playlist
            </button>
            <button type="button" className="button" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

    </div>
  );
};
