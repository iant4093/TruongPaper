// src/tray/tray-main.tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import { installBrowserPreviewMocksIfNeeded } from '../dev/mockTauri';
import { TrayApp } from './TrayApp';
import '../styles/theme.css';

installBrowserPreviewMocksIfNeeded();

ReactDOM.createRoot(document.getElementById('tray-root') as HTMLElement).render(
  <React.StrictMode>
    <TrayApp />
  </React.StrictMode>
);
