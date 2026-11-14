// src/components/WindowControls.tsx
// Window controls for the frameless (decorations: false) studio window: Windows-style caption
// buttons (minimize, maximize/restore, close) on Windows, "traffic lights" elsewhere. macOS uses
// its native overlay title bar and shows neither.
import React, { useEffect, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

export const isWindowsPlatform = () => /Win/.test(navigator.platform);

type AppWindow = ReturnType<typeof getCurrentWindow>;
const run = (fn: (w: AppWindow) => Promise<unknown>) => () => {
  void fn(getCurrentWindow()).catch(() => {});
};

const dot = (color: string): React.CSSProperties => ({
  width: 12,
  height: 12,
  borderRadius: '50%',
  border: 'none',
  padding: 0,
  background: color,
  boxShadow: 'inset 0 0 0 0.5px rgba(0,0,0,0.35)',
  cursor: 'pointer',
});

export const WindowControls: React.FC = () => (
  <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
    {/* Closing the studio only hides it; TruongPaper keeps running in the menu bar / system tray. */}
    <button type="button" aria-label="Close window" style={dot('#ff5f57')} onClick={run((w) => w.hide())} />
    <button type="button" aria-label="Minimize window" style={dot('#febc2e')} onClick={run((w) => w.minimize())} />
    <button type="button" aria-label="Maximize window" style={dot('#28c840')} onClick={run((w) => w.toggleMaximize())} />
  </div>
);

/** 10x10 glyphs drawn like Windows 11's Segoe Fluent caption icons. */
const Glyph: React.FC<{ kind: 'minimize' | 'maximize' | 'restore' | 'close' }> = ({ kind }) => (
  <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1"
    shapeRendering="crispEdges">
    {kind === 'minimize' && <path d="M0 5.5h10" />}
    {kind === 'maximize' && <rect x="0.5" y="0.5" width="9" height="9" rx="1" />}
    {kind === 'restore' && <><rect x="0.5" y="2.5" width="7" height="7" rx="1" /><path d="M2.5 2.5V1.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1" /></>}
    {kind === 'close' && <path d="M0.5 0.5l9 9M9.5 0.5l-9 9" shapeRendering="geometricPrecision" />}
  </svg>
);

/** Windows 11 caption buttons for the top-right corner of the studio window. */
export const WindowsCaptionButtons: React.FC = () => {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    const sync = () => { void getCurrentWindow().isMaximized().then((m) => { if (alive) setMaximized(m); }).catch(() => {}); };
    sync();
    getCurrentWindow().onResized(sync).then((fn) => { if (alive) unlisten = fn; else fn(); }).catch(() => {});
    return () => { alive = false; unlisten?.(); };
  }, []);
  return (
    <div className="caption-buttons">
      <button type="button" className="caption-button" aria-label="Minimize window" title="Minimize"
        onClick={run((w) => w.minimize())}><Glyph kind="minimize" /></button>
      <button type="button" className="caption-button" aria-label={maximized ? 'Restore window' : 'Maximize window'}
        title={maximized ? 'Restore Down' : 'Maximize'}
        onClick={run(async (w) => { await w.toggleMaximize(); setMaximized(await w.isMaximized()); })}>
        <Glyph kind={maximized ? 'restore' : 'maximize'} />
      </button>
      {/* Closing the studio only hides it; TruongPaper keeps running in the system tray. */}
      <button type="button" className="caption-button caption-close" aria-label="Close window" title="Close"
        onClick={run((w) => w.hide())}><Glyph kind="close" /></button>
    </div>
  );
};
