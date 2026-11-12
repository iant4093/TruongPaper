import React, { useState } from 'react';
import { Check, Star, Trash2, X } from 'lucide-react';
import { useShallow } from 'zustand/react/shallow';
import { PresetBar } from './PresetBar';
import { PropertyInspector } from './PropertyInspector';
import { SceneReport } from './SceneReport';
import { Thumb } from './ui';
import { currentWallpaperForMonitor, useAppStore } from '../store/useAppStore';
import { confirmAction, toast } from '../store/feedback';
import { ROTATION_INTERVALS, WALLPAPER_TYPE_LABEL, formatBytes, photoPreviewSrc, wallpaperPreviewSrc } from '../lib/format';
import { Dropdown } from './Dropdown';
import type { WallpaperSource } from '../types/ipc';
import type { StudioTab } from '../App';

/** The details drawer content for the selected wallpaper or the Photography source. */
export const DetailsDrawer: React.FC<{
  closeRef: React.RefObject<HTMLButtonElement | null>;
  onClose: () => void;
  onNavigate: (tab: StudioTab) => void;
}> = ({ closeRef, onClose, onNavigate }) => {
  const photoSource = useAppStore((s) => s.selectedPhotoSource);
  const selected = useAppStore((s) => s.wallpapers.find((w) => w.wallpaperId === s.selectedWallpaperId));
  const monitor = useAppStore(useShallow((s) => {
    const m = s.monitors.find((x) => x.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    return { id: m?.monitorId, sourceKind: m?.activeSource.kind, sourceWallpaperId: m?.activeSource.kind === 'workshop' ? m.activeSource.wallpaperId : undefined };
  }));
  const photoPreview = useAppStore((s) => {
    const m = s.monitors.find((x) => x.monitorId === s.selectedMonitorId) ?? s.monitors[0];
    const { photo } = currentWallpaperForMonitor(m, s.wallpapers, s.photoStatus);
    return photoPreviewSrc(photo ?? s.photoStatus?.unusedQueue[0]);
  });
  const diagnostics = useAppStore((s) => (selected ? s.diagnosticsByWallpaper[selected.wallpaperId] : undefined));
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const deleteWallpaper = useAppStore((s) => s.deleteWallpaper);
  const setWallpaperFavorite = useAppStore((s) => s.setWallpaperFavorite);
  const recoverFromWriteError = useAppStore((s) => s.recoverFromWriteError);
  const setLocalWallpaperProperty = useAppStore((s) => s.setLocalWallpaperProperty);
  const setLocalWallpaperAdjustments = useAppStore((s) => s.setLocalWallpaperAdjustments);
  const [applying, setApplying] = useState(false);
  const rotationSeconds = useAppStore((s) => s.settings?.rotationIntervalSeconds ?? 1800);
  const updateSettings = useAppStore((s) => s.updateSettings);

  if (!photoSource && !selected) return null;
  const title = photoSource ? 'Ian Truong Photography' : selected!.title;
  const source: WallpaperSource = photoSource ? { kind: 'photo_api' } : { kind: 'workshop', wallpaperId: selected!.wallpaperId };
  const current = photoSource ? monitor.sourceKind === 'photo_api'
    : monitor.sourceKind === 'workshop' && monitor.sourceWallpaperId === selected!.wallpaperId;

  const apply = async () => {
    if (!monitor.id || current || applying) return;
    setApplying(true);
    try {
      await applyMonitorSource(monitor.id, source);
      toast.success(`Now showing ${title}`);
    } catch (err) { toast.error(err, `Could not set ${title}`); }
    finally { setApplying(false); }
  };
  const remove = async () => {
    if (!selected) return;
    const ok = await confirmAction(`Remove “${selected.title}” from your library?`, 'Remove', 'This deletes its files from this computer.');
    if (!ok) return;
    try {
      await deleteWallpaper(selected.wallpaperId);
      toast.success(`Removed ${selected.title}`);
      onClose();
    } catch (err) { toast.error(err, 'Could not remove'); }
  };

  return (
    <aside className="inspector-drawer" aria-label="Wallpaper details">
      <div className="inspector-heading">
        <span>Wallpaper details</span>
        <button type="button" ref={closeRef} className="icon-button" aria-label="Close wallpaper details" onClick={onClose}><X size={16} /></button>
      </div>
      <div className="inspector-selection">
        <h3>{title}</h3>
        {selected && (
          <div className="inspector-facts">
            {WALLPAPER_TYPE_LABEL[selected.type]} · {formatBytes(selected.bytes)}{selected.windowsOnly ? ' · Windows only' : ''}
          </div>
        )}
        <div className="inspector-buttons">
          {current ? (
            <span className="inspector-current"><Check size={14} /> Current on this display</span>
          ) : (
            <button type="button" className="button button-primary" disabled={!monitor.id || applying} onClick={() => void apply()}>
              {applying ? 'Applying…' : 'Set as wallpaper'}
            </button>
          )}
          {selected && (
            <button type="button" className="button" aria-pressed={!!selected.isFavorite}
              onClick={() => setWallpaperFavorite(selected.wallpaperId, !selected.isFavorite).catch((e) => toast.error(e, 'Could not update favorites'))}>
              <Star size={14} fill={selected.isFavorite ? 'currentColor' : 'none'} />{selected.isFavorite ? 'Favorited' : 'Add to favorites'}
            </button>
          )}
        </div>
      </div>
      {photoSource ? (
        <div className="photo-source-inspector">
          {photoPreview && <Thumb src={photoPreview} alt="Current photo" height={144} contain />}
          <div className="settings-field">
            <span>Change photo every</span>
            <Dropdown<number> label="Change photo every" value={rotationSeconds}
              options={ROTATION_INTERVALS.map((r) => [r.seconds, r.label] as const)}
              onChange={(rotationIntervalSeconds) => {
                updateSettings({ rotationIntervalSeconds }).then(() => toast.success('Saved'), (err) => toast.error(err));
              }} />
          </div>
          <p className="photo-source-hint">Photo pool, fit, orientation and storage are in Photography.</p>
          <button type="button" className="button" onClick={() => onNavigate('photos')}>Open Photography settings</button>
        </div>
      ) : selected && (
        <>
          <div className="inspector-preview"><Thumb src={wallpaperPreviewSrc(selected)} alt={selected.title} height={150} /></div>
          <SceneReport messages={diagnostics} />
          <PresetBar key={`presets:${selected.wallpaperId}`} wallpaperId={selected.wallpaperId} />
          <PropertyInspector key={`properties:${selected.wallpaperId}`} wallpaperId={selected.wallpaperId}
            project={selected.projectJson} values={selected.userProperties} adjustments={selected.adjustments}
            onWriteError={(error) => void recoverFromWriteError(error)}
            onPropertyChange={(k, v) => setLocalWallpaperProperty(selected.wallpaperId, k, v)}
            onAdjustmentChange={(next) => setLocalWallpaperAdjustments(selected.wallpaperId, next)} />
          <div className="inspector-danger">
            <button type="button" className="button button-danger-quiet" onClick={() => void remove()}>
              <Trash2 size={14} /> Remove from library…
            </button>
          </div>
        </>
      )}
    </aside>
  );
};
