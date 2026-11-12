// Details-drawer panels for the Photography source and for playlists, so their settings can be
// changed right there instead of on another page.
import React, { useEffect, useState } from 'react';
import { SkipForward, Trash2 } from 'lucide-react';
import { FIT_MODE_HINT, FIT_MODE_LABEL, PHOTO_POOL_OPTIONS, ROTATION_INTERVALS, plural } from '../lib/format';
import { useAppStore } from '../store/useAppStore';
import { confirmAction, toast } from '../store/feedback';
import type { FitMode, Playlist } from '../types/ipc';
import { Dropdown } from './Dropdown';
import { PlaylistEditor, playlistIsValid } from './PlaylistEditor';
import { SegmentedControl, ToggleRow } from './ui';

const saved = (action: Promise<unknown>) => action.then(() => toast.success('Saved'), (err) => toast.error(err));

export const PhotoSourceSettings: React.FC<{ onMore: () => void }> = ({ onMore }) => {
  const settings = useAppStore((s) => s.settings);
  const mode = useAppStore((s) => s.photoStatus?.filterMode ?? 'all');
  const featuredOnly = useAppStore((s) => s.photoStatus?.featuredOnly ?? false);
  const albumCount = useAppStore((s) => s.savedAlbums.filter((a) => a.isEnabled).length);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const setPhotoFilter = useAppStore((s) => s.setPhotoFilter);
  const fit = settings?.defaultFitMode ?? 'fill';
  return (
    <div className="source-settings">
      <div className="settings-field">
        <span>Photos from</span>
        <SegmentedControl value={mode} options={PHOTO_POOL_OPTIONS}
          onChange={(next) => void saved(setPhotoFilter(next, next === 'featured' ? true : featuredOnly))} />
        {mode === 'albums' && (
          <span className="view-subtitle">{albumCount ? plural(albumCount, 'album') + ' turned on.' : 'No albums turned on yet.'}</span>
        )}
      </div>
      <div className="settings-field">
        <span>Change photo every</span>
        <Dropdown<number> label="Change photo every" value={settings?.rotationIntervalSeconds ?? 1800}
          options={ROTATION_INTERVALS.map((r) => [r.seconds, r.label] as const)}
          onChange={(rotationIntervalSeconds) => void saved(updateSettings({ rotationIntervalSeconds }))} />
      </div>
      <div className="settings-field">
        <span>Photo fit</span>
        <Dropdown<FitMode> label="Photo fit" value={fit}
          options={(Object.keys(FIT_MODE_LABEL) as FitMode[]).map((k) => [k, FIT_MODE_LABEL[k]] as const)}
          onChange={(defaultFitMode) => void saved(updateSettings({ defaultFitMode }))} />
        <span className="view-subtitle">{FIT_MODE_HINT[fit]}</span>
      </div>
      <ToggleRow label="Match photo orientation to display"
        checked={settings?.skipMismatchedOrientation ?? true}
        onChange={(skipMismatchedOrientation) => void saved(updateSettings({ skipMismatchedOrientation }))} />
      <button type="button" className="button" onClick={onMore}>Albums, storage and history…</button>
    </div>
  );
};

export const PlaylistDetails: React.FC<{ playlist: Playlist; onClose: () => void; onMore: () => void }> = ({ playlist, onClose, onMore }) => {
  const savePlaylist = useAppStore((s) => s.savePlaylist);
  const deletePlaylist = useAppStore((s) => s.deletePlaylist);
  const playbackNext = useAppStore((s) => s.playbackNext);
  const monitorId = useAppStore((s) => (s.monitors.find((m) => m.monitorId === s.selectedMonitorId) ?? s.monitors[0])?.monitorId);
  const playingHere = useAppStore((s) => {
    const m = s.monitors.find((x) => x.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    return m?.activeSource.kind === 'playlist' && m.activeSource.playlistId === playlist.playlistId;
  });
  const [draft, setDraft] = useState<Playlist>(playlist);
  const [saving, setSaving] = useState(false);
  // A different playlist, or a save from elsewhere, resets the form.
  useEffect(() => setDraft(playlist), [playlist]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(playlist);

  const save = async () => {
    setSaving(true);
    try { await savePlaylist(draft); toast.success(`Saved “${draft.name}”`); }
    catch (err) { toast.error(err, 'Could not save the playlist'); }
    finally { setSaving(false); }
  };
  const remove = async () => {
    if (!(await confirmAction(`Delete the playlist “${playlist.name}”?`, 'Delete playlist'))) return;
    try {
      await deletePlaylist(playlist.playlistId);
      onClose();
      toast.info(`Deleted “${playlist.name}”`, async () => { await savePlaylist(playlist); });
    } catch (err) { toast.error(err, 'Could not delete'); }
  };

  return (
    <div className="source-settings">
      {playingHere && monitorId && (
        <button type="button" className="button" onClick={() => playbackNext(monitorId).catch((e) => toast.error(e, 'Could not go to the next one'))}>
          Next in playlist<SkipForward size={14} />
        </button>
      )}
      <PlaylistEditor value={draft} onChange={setDraft} compact />
      <div className="inspector-buttons">
        <button type="button" className="button button-primary" disabled={!dirty || saving || !playlistIsValid(draft)} onClick={() => void save()}>
          {saving ? 'Saving…' : 'Save changes'}
        </button>
        {dirty && <button type="button" className="button" onClick={() => setDraft(playlist)}>Undo changes</button>}
      </div>
      <button type="button" className="button" onClick={onMore}>All playlists…</button>
      <div className="inspector-danger">
        <button type="button" className="button button-danger-quiet" onClick={() => void remove()}>
          <Trash2 size={14} /> Delete playlist…
        </button>
      </div>
    </div>
  );
};
