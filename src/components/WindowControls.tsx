// src/components/WindowControls.tsx
// macOS-style "traffic light" controls for the frameless (decorations: false) studio window.
import React from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

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

export const WindowControls: React.FC = () => {
  const run = (fn: (w: ReturnType<typeof getCurrentWindow>) => Promise<void>) => () => {
    void fn(getCurrentWindow()).catch(() => {});
  };
  return (
    <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
      {/* Closing the studio only hides it; TruongPaper keeps running in the menu bar / system tray. */}
      <button type="button" aria-label="Close window" style={dot('#ff5f57')} onClick={run((w) => w.hide())} />
      <button type="button" aria-label="Minimize window" style={dot('#febc2e')} onClick={run((w) => w.minimize())} />
      <button type="button" aria-label="Maximize window" style={dot('#28c840')} onClick={run((w) => w.toggleMaximize())} />
    </div>
  );
};
