import { useState } from 'react';
import { FIT_MODE_LABEL, PHOTO_POOL_OPTIONS, ROTATION_INTERVALS, TRANSITION_LABEL, offlinePhotosForDisplay, photoPreviewSrc } from '../lib/format';
import { useAppStore } from '../store/useAppStore';
import type { FilterMode, FitMode, MonitorInfo, TransitionType } from '../types/ipc';
import { Dropdown } from './Dropdown';
import { Thumb, ToggleRow } from './ui';

const fitOptions = Object.entries(FIT_MODE_LABEL) as [FitMode, string][];
const transitionOptions = Object.entries(TRANSITION_LABEL) as [TransitionType, string][];
const rotationOptions = ROTATION_INTERVALS.map(({ seconds, label }) => [seconds, label] as const);

export function PhotoSourceInspector({ monitor }: { monitor?: MonitorInfo }) {
  const settings = useAppStore((s) => s.settings);
  const status = useAppStore((s) => s.photoStatus);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const setPhotoFilter = useAppStore((s) => s.setPhotoFilter);
  const [error, setError] = useState<string | null>(null);
  const [savingPool, setSavingPool] = useState(false);
  const run = async (action: () => Promise<void>) => {
    setError(null);
    try { await action(); }
    catch (err) { setError(String(err)); }
  };
  const updatePool = async (mode: FilterMode, featuredOnly: boolean) => {
    setSavingPool(true);
    try { await run(() => setPhotoFilter(mode, featuredOnly)); }
    finally { setSavingPool(false); }
  };
  const current = monitor ? status?.currentPhotoByMonitor[monitor.monitorId] : null;
  const photo = current ?? offlinePhotosForDisplay(status, monitor, settings?.skipMismatchedOrientation ?? true)[0];

  return (
    <div className="photo-source-inspector">
      {photo && <Thumb src={photoPreviewSrc(photo)} alt={photo.albumSlug.replace(/-/g, ' ')} height={144} contain />}
      <p className="photo-source-hint">These settings are shared across displays using Photography.</p>
      {error && <div role="alert" className="error-banner">{error}</div>}
      {settings && status ? (
        <>
          <fieldset className="photo-source-fields" aria-label="Photo pool" disabled={savingPool} aria-busy={savingPool}>
            <div className="photo-source-field">
              <span>Photo pool</span>
              <Dropdown<FilterMode> label="Photo pool" value={status.filterMode} options={PHOTO_POOL_OPTIONS}
                onChange={(mode) => void updatePool(mode, mode === 'featured' || status.featuredOnly)} />
            </div>
            {status.filterMode === 'albums' && (
              <ToggleRow label="Featured only" description="Only use featured photos from enabled albums."
                checked={status.featuredOnly} onChange={(checked) => void updatePool('albums', checked)} />
            )}
          </fieldset>
          <div className="photo-source-field">
            <span>Photo fit</span>
            <Dropdown label="Photo fit" value={settings.defaultFitMode} options={fitOptions}
              onChange={(defaultFitMode) => void run(() => updateSettings({ defaultFitMode }))} />
          </div>
          <ToggleRow label="Match photo orientation to display" checked={settings.skipMismatchedOrientation}
            onChange={(skipMismatchedOrientation) => void run(() => updateSettings({ skipMismatchedOrientation }))} />
          <div className="photo-source-field">
            <span>Change photo every</span>
            <Dropdown label="Change photo every" value={settings.rotationIntervalSeconds} options={rotationOptions}
              placeholder={`${settings.rotationIntervalSeconds} seconds`}
              onChange={(rotationIntervalSeconds) => void run(() => updateSettings({ rotationIntervalSeconds }))} />
          </div>
          <div className="photo-source-field">
            <span>Transition</span>
            <Dropdown label="Transition" value={settings.transitionType} options={transitionOptions}
              onChange={(transitionType) => void run(() => updateSettings({ transitionType }))} />
          </div>
        </>
      ) : <p className="photo-source-hint">Loading photo settings…</p>}
    </div>
  );
}
