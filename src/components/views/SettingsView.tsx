// src/components/views/SettingsView.tsx
// Settings: performance, power policy, startup, Steam account & signed auto-updates (docs/06 §5).
import React, { useEffect, useState } from 'react';
import { Download, LogIn, LogOut, RefreshCw } from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import {
  appVersion,
  availableUpdate,
  checkForUpdate,
  installUpdate,
  onUpdateChange,
  type AvailableUpdate,
} from '../../services/updates';
import type { AppSettings } from '../../types/ipc';
import { SectionTitle, SegmentedControl, ToggleRow, ViewHeader } from '../ui';

const FPS_OPTIONS = [
  ['30', '30 FPS'],
  ['60', '60 FPS'],
  ['120', '120 FPS'],
  ['display_native', 'Display Native'],
] as const;

const QUALITY_OPTIONS = [
  ['low', 'Low'],
  ['medium', 'Medium'],
  ['high', 'High'],
  ['ultra', 'Ultra'],
] as const satisfies ReadonlyArray<readonly [AppSettings['qualityPreset'], string]>;

const BATTERY_OPTIONS = [
  ['continue', 'Keep Playing'],
  ['pause_animations', 'Pause Animations'],
  ['switch_to_photo', 'Switch to Photo'],
] as const satisfies ReadonlyArray<readonly [AppSettings['batteryPolicy'], string]>;

const parseFps = (v: string): AppSettings['fpsCap'] =>
  v === 'display_native' ? 'display_native' : (Number(v) as 30 | 60 | 120);

export const SettingsView: React.FC = () => {
  const settings = useAppStore((s) => s.settings);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);
  const steamLogout = useAppStore((s) => s.steamLogout);

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
      <div>
        <ViewHeader title="Settings" />
        <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading settings…</div>
      </div>
    );
  }

  return (
    <div>
      <ViewHeader title="Settings" subtitle={version ? `TruongPaper v${version}` : undefined} />

      <SectionTitle>Performance</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
        <div style={{ fontSize: 13, marginBottom: 8 }}>Frame rate cap</div>
        <SegmentedControl
          value={String(settings.fpsCap)}
          options={FPS_OPTIONS}
          onChange={(next) => void updateSettings({ fpsCap: parseFps(next) })}
        />
        <div style={{ fontSize: 13, margin: '14px 0 8px' }}>Render quality</div>
        <SegmentedControl
          value={settings.qualityPreset}
          options={QUALITY_OPTIONS}
          onChange={(next) => void updateSettings({ qualityPreset: next })}
        />
      </div>

      <SectionTitle>Smart Pause</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
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
        <div style={{ fontSize: 13, marginBottom: 8 }}>On battery / Low Power Mode</div>
        <SegmentedControl
          value={settings.batteryPolicy}
          options={BATTERY_OPTIONS}
          onChange={(next) => void updateSettings({ batteryPolicy: next })}
        />
      </div>

      <SectionTitle>Startup</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
        <ToggleRow
          label="Launch TruongPaper at login"
          checked={settings.launchAtLogin}
          onChange={(next) => void updateSettings({ launchAtLogin: next })}
        />
      </div>

      <SectionTitle>Steam Account</SectionTitle>
      <div className="glass-card" style={{ padding: 16, display: 'flex', gap: 8, flexWrap: 'wrap' }}>
        <button type="button" className="glass-button" onClick={openSteamAuthModal}>
          <LogIn size={14} /> Sign in to Steam
        </button>
        <button type="button" className="glass-button" onClick={() => void steamLogout()}>
          <LogOut size={14} /> Sign Out & Forget Refresh Token
        </button>
      </div>

      <SectionTitle>Updates</SectionTitle>
      <div className="glass-card" style={{ padding: 16 }}>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
          <button type="button" className="glass-button" disabled={checking} onClick={() => void runCheck()}>
            <RefreshCw size={14} /> {checking ? 'Checking…' : 'Check for Updates'}
          </button>
          {update && (
            <button type="button" className="glass-button glass-button-primary" disabled={!!progress} onClick={() => void runInstall()}>
              <Download size={14} /> Install {update.version} & Restart
            </button>
          )}
          {updateMsg && <span style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{updateMsg}</span>}
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
