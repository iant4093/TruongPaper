// The playlist form, shared by the Playlists page and the details drawer.
import React from 'react';
import { ROTATION_INTERVALS } from '../lib/format';
import { useAppStore } from '../store/useAppStore';
import type { Playlist } from '../types/ipc';
import { Dropdown } from './Dropdown';
import { SegmentedControl } from './ui';

export const PlaylistEditor: React.FC<{
  value: Playlist;
  onChange: (next: Playlist) => void;
  /** Narrow layout for the details drawer. */
  compact?: boolean;
}> = ({ value: draft, onChange, compact }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const workshopItems = wallpapers.filter((w) => w.type !== 'photo');
  const chosen = new Set(draft.items.flatMap((i) => (i.kind === 'workshop' ? [i.wallpaperId] : [])));
  const photoDraws = draft.items.filter((i) => i.kind === 'photo_api_draw').length;

  return (
    <div className={`playlist-editor${compact ? ' is-compact' : ''}`}>
      <label className="settings-field">
        <span>Name</span>
        <input type="text" aria-label="Playlist name" value={draft.name}
          onChange={(e) => onChange({ ...draft, name: e.target.value })} />
      </label>

      <div className="settings-field">
        <span>Shows</span>
        <SegmentedControl
          value={draft.mode}
          options={[
            ['photos_only', 'Photos only'],
            ['workshop_only', 'Wallpapers only'],
            ['mixed', 'Mixed'],
          ]}
          onChange={(mode) => onChange({
            ...draft,
            mode,
            items: mode === 'photos_only'
              ? [{ kind: 'photo_api_draw' }]
              : mode === 'workshop_only'
                ? draft.items.filter((i) => i.kind === 'workshop')
                : draft.items.some((i) => i.kind === 'photo_api_draw') ? draft.items : [{ kind: 'photo_api_draw' }, ...draft.items],
          })}
        />
      </div>

      <div className="settings-field">
        <span>Change every</span>
        <Dropdown<number> label="Change every" value={draft.intervalSeconds}
          options={ROTATION_INTERVALS.map((r) => [r.seconds, r.label] as const)}
          onChange={(intervalSeconds) => onChange({ ...draft, intervalSeconds })} />
      </div>

      {draft.mode === 'mixed' && (
        <label className="settings-field playlist-photo-count">
          <span>Photos per round</span>
          <input type="number" min={0} max={10} value={photoDraws} aria-label="Photos per round"
            onChange={(e) => {
              const n = Math.max(0, Math.min(10, Number(e.target.value) || 0));
              onChange({
                ...draft,
                items: [
                  ...Array.from({ length: n }, () => ({ kind: 'photo_api_draw' as const })),
                  ...draft.items.filter((i) => i.kind === 'workshop'),
                ],
              });
            }} />
        </label>
      )}

      {draft.mode !== 'photos_only' && (
        <div className="settings-field">
          <span>Wallpapers ({chosen.size})</span>
          <div className="playlist-picks">
            {workshopItems.map((w) => (
              <label key={w.wallpaperId} className="playlist-pick">
                <input type="checkbox" checked={chosen.has(w.wallpaperId)}
                  onChange={(e) => onChange({
                    ...draft,
                    items: e.target.checked
                      ? [...draft.items, { kind: 'workshop', wallpaperId: w.wallpaperId }]
                      : draft.items.filter((i) => !(i.kind === 'workshop' && i.wallpaperId === w.wallpaperId)),
                  })} />
                <span>{w.title}</span>
              </label>
            ))}
            {workshopItems.length === 0 && <div className="view-subtitle">No wallpapers in the library yet.</div>}
          </div>
        </div>
      )}
    </div>
  );
};

/** True when the playlist can be saved. */
export const playlistIsValid = (p: Playlist) => p.items.length > 0 && !!p.name.trim();
