// src/components/views/SettingsView.tsx
// Settings: performance, power policy, startup, Steam account & signed auto-updates (docs/06 §5).
import React, { useEffect, useRef, useState } from 'react';
import { Download, LogIn, LogOut, RefreshCw, Upload } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { errorMessage, useAppStore } from '../../store/useAppStore';
import { confirmAction, toast } from '../../store/feedback';
import { plural } from '../../lib/format';
import {
  appVersion,
  availableUpdate,
  checkForUpdate,
  installUpdate,
  onUpdateChange,
  type AvailableUpdate,
} from '../../services/updates';
import type { AppSettings, BackupImportSummary } from '../../types/ipc';
import { SectionTitle, SegmentedControl, ToggleRow, ViewHeader } from '../ui';

const FPS_OPTIONS = [
  ['30', 'Battery saver (30)'],
  ['60', 'Smooth (60)'],
  ['120', 'Very smooth (120)'],
  ['display_native', 'Match display'],
] as const;

const QUALITY_OPTIONS = [
  ['low', 'Low'],
  ['medium', 'Medium'],
  ['high', 'High'],
  ['ultra', 'Ultra'],
] as const satisfies ReadonlyArray<readonly [AppSettings['qualityPreset'], string]>;

const BATTERY_OPTIONS = [
  ['continue', 'Keep playing'],
  ['pause_animations', 'Freeze the picture'],
  ['switch_to_photo', 'Switch to a photo'],
] as const satisfies ReadonlyArray<readonly [AppSettings['batteryPolicy'], string]>;

const parseFps = (v: string): AppSettings['fpsCap'] =>
  v === 'display_native' ? 'display_native' : (Number(v) as 30 | 60 | 120);

/** Two accelerator fields saved together. */
const GlobalShortcuts: React.FC<{ nextHotkey: string; pauseHotkey: string }> = ({ nextHotkey, pauseHotkey }) => {
  const updateSettings = useAppStore((s) => s.updateSettings);
  const [next, setNext] = useState(nextHotkey);
  const [pause, setPause] = useState(pauseHotkey);
  const [status, setStatus] = useState<{ error: boolean; text: string } | null>(null);
  const dirty = next !== nextHotkey || pause !== pauseHotkey;
  /** True once the user typed; a settings event then must not overwrite the draft. */
  const editedRef = useRef(false);
  useEffect(() => { if (!editedRef.current) { setNext(nextHotkey); setPause(pauseHotkey); } }, [nextHotkey, pauseHotkey]);
  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setStatus(null);
    try {
      await updateSettings({ nextHotkey: next.trim(), pauseHotkey: pause.trim() });
      editedRef.current = false;
      setStatus({ error: false, text: 'Shortcuts saved' });
      toast.success('Shortcuts saved');
    } catch (err) {
      setStatus({ error: true, text: errorMessage(err) });
    }
  };
  return (
    <>
      <SectionTitle>Keyboard shortcuts</SectionTitle>
      <form className="card" style={{ padding: 16 }} onSubmit={(e) => void save(e)}>
        <label className="settings-field">
          <span>Next wallpaper</span>
          <input type="text" className="button" placeholder="CmdOrCtrl+Alt+Right" value={next} onChange={(e) => { editedRef.current = true; setNext(e.target.value); }} />
        </label>
        <label className="settings-field">
          <span>Pause / resume</span>
          <input type="text" className="button" placeholder="CmdOrCtrl+Alt+P" value={pause} onChange={(e) => { editedRef.current = true; setPause(e.target.value); }} />
        </label>
        <div className="view-subtitle" style={{ marginBottom: 10 }}>
          Type a combination such as CmdOrCtrl+Alt+Right. Work anywhere, even when TruongPaper is in the background. Leave empty to turn off.
        </div>
        <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
          <button type="submit" className="button" disabled={!dirty}>Save shortcuts</button>
          {status && <span role={status.error ? 'alert' : 'status'} style={{ fontSize: 12, color: status.error ? '#e3b1b1' : 'var(--text-secondary)' }}>{status.text}</span>}
        </div>
      </form>
    </>
  );
};

const BACKUP_FILTER = [{ name: 'TruongPaper backup', extensions: ['json'] }];

/** Save everything to one file, or restore it on this or another computer. */
const BackupSection: React.FC = () => {
  const exportBackup = useAppStore((s) => s.exportBackup);
  const importBackup = useAppStore((s) => s.importBackup);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);
  const [busy, setBusy] = useState(false);
  const [summary, setSummary] = useState<BackupImportSummary | null>(null);

  const doExport = async () => {
    setBusy(true);
    try {
      const path = await saveDialog({ defaultPath: 'truongpaper-backup.json', filters: BACKUP_FILTER, title: 'Save backup' });
      if (!path) return;
      await exportBackup(path);
      toast.success('Backup saved');
    } catch (err) { toast.error(err, 'Could not save the backup'); }
    finally { setBusy(false); }
  };
  const doImport = async () => {
    setBusy(true);
    try {
      const picked = await openDialog({ multiple: false, directory: false, filters: BACKUP_FILTER, title: 'Restore from backup' });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (!path) return;
      if (!await confirmAction('Restore from this backup?', 'Restore', 'Your current settings and playlists are replaced by the ones in the file.')) return;
      setSummary(await importBackup(path));
      toast.success('Backup restored');
    } catch (err) { toast.error(err, 'Could not restore the backup'); }
    finally { setBusy(false); }
  };
  const downloadMissing = async () => {
    if (!summary) return;
    let started = 0;
    let failure: unknown = null;
    for (const id of summary.missingWorkshopIds) {
      try { await downloadWorkshopItem(id); started += 1; } catch (err) { failure = err; }
    }
    if (started) toast.success(`Started ${plural(started, 'download')}`);
    if (failure) toast.error(failure, 'Some downloads could not start');
  };

  return (
    <>
      <SectionTitle>Backup & restore</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <div className="view-subtitle" style={{ marginTop: 0, marginBottom: 12 }}>
          One file with your settings, playlists and favorites. Wallpaper files are not included; missing Workshop items can be downloaded again.
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button type="button" className="button" disabled={busy} onClick={() => void doExport()}><Download size={14} /> Save backup…</button>
          <button type="button" className="button" disabled={busy} onClick={() => void doImport()}><Upload size={14} /> Restore from backup…</button>
        </div>
        {summary && (
          <div className="backup-summary" role="status">
            <div>Restored {summary.settings ? 'settings, ' : ''}{plural(summary.playlists, 'playlist')}, {plural(summary.favorites, 'favorite')} and {plural(summary.wallpapers, 'wallpaper')}.</div>
            {summary.missingWorkshopIds.length > 0 && (
              <div className="backup-missing">
                <span>{plural(summary.missingWorkshopIds.length, 'Workshop wallpaper')} {summary.missingWorkshopIds.length === 1 ? 'is' : 'are'} not on this computer.</span>
                <button type="button" className="button button-primary" onClick={() => void downloadMissing()}>
                  Download missing ({summary.missingWorkshopIds.length})
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );
};

export const SettingsView: React.FC = () => {
  const settings = useAppStore((s) => s.settings);
  const store = useAppStore((s) => s.updateSettings);
  /** Every change says it was saved, or why it was not. */
  const updateSettings = (partial: Partial<AppSettings>) =>
    store(partial).then(() => toast.success('Saved'), (err) => toast.error(err, 'Could not save'));
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);
  const steamLogout = useAppStore((s) => s.steamLogout);
  const steamAccount = useAppStore((s) => s.steamAccount);

  const [version, setVersion] = useState<string | null>(null);
  const [update, setUpdate] = useState<AvailableUpdate | null>(availableUpdate());
  const [updateMsg, setUpdateMsg] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [progress, setProgress] = useState<{ downloaded: number; total: number | null } | null>(null);

  useEffect(() => {
    let alive = true;
    appVersion()
      .then((v) => alive && setVersion(v))
      .catch(() => alive && setVersion(null));
    const off = onUpdateChange(setUpdate);
    return () => {
      alive = false;
      off();
    };
  }, []);

  const runCheck = async () => {
    setChecking(true);
    setUpdateMsg(null);
    try {
      const found = await checkForUpdate();
      setUpdateMsg(found ? `TruongPaper ${found.version} is available` : 'You are on the latest version');
    } catch (err) {
      setUpdateMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  };

  const runInstall = async () => {
    setUpdateMsg('Downloading update…');
    try {
      await installUpdate((downloaded, total) => setProgress({ downloaded, total }));
    } catch (err) {
      setUpdateMsg(err instanceof Error ? err.message : String(err));
      setProgress(null);
    }
  };

  if (!settings) {
    return (
      <div className="settings-view">
        <ViewHeader title="Settings" />
        <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading settings…</div>
      </div>
    );
  }

  return (
    <div className="settings-view">
      <ViewHeader title="Settings" subtitle={version ? `TruongPaper v${version}` : undefined} />

      <SectionTitle>Performance</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <div style={{ fontSize: 13, marginBottom: 2 }}>Animation smoothness</div>
        <div className="view-subtitle" style={{ marginTop: 0, marginBottom: 8 }}>Higher looks smoother but uses more power.</div>
        <SegmentedControl
          value={String(settings.fpsCap)}
          options={FPS_OPTIONS}
          onChange={(next) => void updateSettings({ fpsCap: parseFps(next) })}
        />
        <div style={{ fontSize: 13, margin: '14px 0 2px' }}>Picture quality</div>
        <div className="view-subtitle" style={{ marginTop: 0, marginBottom: 8 }}>Lower it if wallpapers stutter.</div>
        <SegmentedControl
          value={settings.qualityPreset}
          options={QUALITY_OPTIONS}
          onChange={(next) => void updateSettings({ qualityPreset: next })}
        />
      </div>

      <SectionTitle>Smart pause</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <ToggleRow
          label="Pause when a fullscreen app or game is active"
          checked={settings.pauseOnFullscreen}
          onChange={(next) => void updateSettings({ pauseOnFullscreen: next })}
        />
        <ToggleRow
          label="Pause when a window is maximized"
          checked={settings.pauseOnMaximized}
          onChange={(next) => void updateSettings({ pauseOnMaximized: next })}
        />
        <div style={{ fontSize: 13, marginBottom: 2 }}>When on battery or in Low Power Mode</div>
        <div className="view-subtitle" style={{ marginTop: 0, marginBottom: 8 }}>“Freeze the picture” stops animation but keeps the wallpaper visible.</div>
        <SegmentedControl
          value={settings.batteryPolicy}
          options={BATTERY_OPTIONS}
          onChange={(next) => void updateSettings({ batteryPolicy: next })}
        />
      </div>

      <SectionTitle>Startup</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <ToggleRow
          label="Launch TruongPaper at login"
          checked={settings.launchAtLogin}
          onChange={(next) => void updateSettings({ launchAtLogin: next })}
        />
      </div>

      <GlobalShortcuts nextHotkey={settings.nextHotkey} pauseHotkey={settings.pauseHotkey} />

      <BackupSection />

      <SectionTitle>Steam account</SectionTitle>
      <div className="card" style={{ padding: 16, display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
        <div style={{ flex: '1 1 100%', fontSize: 13, color: 'var(--text-secondary)' }}>
          {steamAccount ? `Signed in as ${steamAccount}.` : 'Not signed in.'}
        </div>
        <button type="button" className="button" onClick={openSteamAuthModal}>
          <LogIn size={14} /> {steamAccount ? 'Switch account' : 'Sign in to Steam'}
        </button>
        {steamAccount && (
          <button type="button" className="button" onClick={async () => {
            if (!await confirmAction('Sign out of Steam?', 'Sign out', 'You will need to sign in again to download Workshop wallpapers.')) return;
            try { await steamLogout(); toast.success('Signed out of Steam'); } catch (err) { toast.error(err); }
          }}>
            <LogOut size={14} /> Sign out
          </button>
        )}
      </div>

      <SectionTitle>Updates</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <button type="button" className="button" disabled={checking} onClick={() => void runCheck()}>
            <RefreshCw size={14} /> {checking ? 'Checking…' : 'Check for Updates'}
          </button>
          {update && (
            <button type="button" className="button button-primary" disabled={!!progress} onClick={() => void runInstall()}>
              <Download size={14} /> Install {update.version} & Restart
            </button>
          )}
          <span role="status" style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{updateMsg}</span>
        </div>
        {update?.notes && (
          <pre style={{ whiteSpace: 'pre-wrap', fontSize: 12, color: 'var(--text-secondary)', fontFamily: 'var(--font-sans)', marginTop: 12 }}>
            {update.notes}
          </pre>
        )}
        {progress && (
          <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 10 }}>
            {progress.total
              ? `${Math.round((progress.downloaded / progress.total) * 100)}% downloaded`
              : `${Math.round(progress.downloaded / 1024)} KB downloaded`}
          </div>
        )}
      </div>
    </div>
  );
};
