// The playlist form, shared by the Playlists page and the details drawer.
import React from 'react';
import { Check } from 'lucide-react';
import { ROTATION_INTERVALS, TRANSITION_LABEL, WALLPAPER_TYPE_LABEL, wallpaperPreviewSrc } from '../lib/format';
import { useAppStore } from '../store/useAppStore';
import type { Playlist, TransitionType } from '../types/ipc';
import { Dropdown } from './Dropdown';
import { SegmentedControl, Thumb } from './ui';

/** "Same as photos" uses the Photography transition. */
type TransitionChoice = 'photos' | TransitionType;
export const TRANSITION_LENGTHS: ReadonlyArray<readonly [number, string]> = [
  [0.5, '0.5 s'], [1, '1 s'], [1.5, '1.5 s'], [2, '2 s'], [3, '3 s'],
];

export const PlaylistEditor: React.FC<{
  value: Playlist;
  onChange: (next: Playlist) => void;
  /** Narrow layout for the details drawer. */
  compact?: boolean;
}> = ({ value: draft, onChange, compact }) => {
  const wallpapers = useAppStore((s) => s.wallpapers);
  const photoSeconds = useAppStore((s) => s.settings?.transitionDurationSeconds ?? 1.2);
  const workshopItems = wallpapers.filter((w) => w.type !== 'photo');
  const chosen = new Set(draft.items.flatMap((i) => (i.kind === 'workshop' ? [i.wallpaperId] : [])));
  const photoDraws = draft.items.filter((i) => i.kind === 'photo_api_draw').length;
  const transition: TransitionChoice = draft.transition?.type ?? 'photos';
  const toggle = (wallpaperId: string) => onChange({
    ...draft,
    items: chosen.has(wallpaperId)
      ? draft.items.filter((i) => !(i.kind === 'workshop' && i.wallpaperId === wallpaperId))
      : [...draft.items, { kind: 'workshop', wallpaperId }],
  });

  return (
    <div className={`playlist-editor${compact ? ' is-compact' : ''}`}>
      <label className="settings-field">
        <span>Name</span>
        <input type="text" className="text-input" aria-label="Playlist name" value={draft.name}
          onChange={(e) => onChange({ ...draft, name: e.target.value })} />
      </label>

      <div className="settings-field">
        <span>Shows</span>
        <SegmentedControl
          value={draft.mode}
          options={[
            ['photos_only', 'Photos'],
            ['workshop_only', 'Wallpapers'],
            ['mixed', 'Both'],
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

      <div className="playlist-editor-row">
        <div className="settings-field">
          <span>Change every</span>
          <Dropdown<number> label="Change every" value={draft.intervalSeconds}
            options={ROTATION_INTERVALS.map((r) => [r.seconds, r.label] as const)}
            onChange={(intervalSeconds) => onChange({ ...draft, intervalSeconds })} />
        </div>
        {draft.mode === 'mixed' && (
          <label className="settings-field">
            <span>Photos per round</span>
            <input type="number" className="text-input" min={0} max={10} value={photoDraws} aria-label="Photos per round"
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
      </div>

      <div className="playlist-editor-row">
        <div className="settings-field">
          <span>Transition</span>
          <Dropdown<TransitionChoice> label="Playlist transition" value={transition}
            options={[['photos', 'Same as Photography'], ...(Object.keys(TRANSITION_LABEL) as TransitionType[]).map((k) => [k, TRANSITION_LABEL[k]] as const)]}
            onChange={(choice) => onChange({
              ...draft,
              transition: choice === 'photos' ? undefined : { type: choice, durationSeconds: draft.transition?.durationSeconds ?? photoSeconds },
            })} />
        </div>
        {draft.transition && (
          <div className="settings-field">
            <span>Effect length</span>
            <Dropdown<number> label="Playlist effect length" value={nearestLength(draft.transition.durationSeconds)}
              options={TRANSITION_LENGTHS}
              onChange={(durationSeconds) => onChange({ ...draft, transition: { ...draft.transition!, durationSeconds } })} />
          </div>
        )}
      </div>

      {draft.mode !== 'photos_only' && (
        <div className="settings-field">
          <span>Wallpapers · {chosen.size} chosen</span>
          {workshopItems.length === 0 ? (
            <div className="view-subtitle">No wallpapers in the library yet.</div>
          ) : (
            <div className="playlist-picks" role="group" aria-label="Wallpapers in this playlist">
              {workshopItems.map((w) => {
                const on = chosen.has(w.wallpaperId);
                return (
                  <button key={w.wallpaperId} type="button" className={`playlist-pick${on ? ' is-on' : ''}`}
                    aria-pressed={on} aria-label={`${on ? 'Remove' : 'Add'} ${w.title}`} title={w.title}
                    onClick={() => toggle(w.wallpaperId)}>
                    <span className="playlist-pick-thumb">
                      <Thumb src={wallpaperPreviewSrc(w)} alt="" height="auto" lazy />
                      <span className="playlist-pick-check" aria-hidden="true">{on && <Check size={12} strokeWidth={3} />}</span>
                    </span>
                    <span className="playlist-pick-title">{w.title}</span>
                    <span className="playlist-pick-kind">{WALLPAPER_TYPE_LABEL[w.type]}</span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
};

function nearestLength(seconds: number): number {
  return TRANSITION_LENGTHS.reduce((best, [s]) => (Math.abs(s - seconds) < Math.abs(best - seconds) ? s : best), 1);
}

/** True when the playlist can be saved. */
export const playlistIsValid = (p: Playlist) => p.items.length > 0 && !!p.name.trim();
