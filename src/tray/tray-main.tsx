// src/tray/tray-main.tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import { TrayApp } from './TrayApp';
import '../styles/theme.css';

async function boot() {
  // The browser-preview mock is dev-only; production builds do not carry it.
  if (import.meta.env.DEV) (await import('../dev/mockTauri')).installBrowserPreviewMocksIfNeeded();
  ReactDOM.createRoot(document.getElementById('tray-root') as HTMLElement).render(
    <React.StrictMode>
      <TrayApp />
    </React.StrictMode>
  );
}

void boot();
