// src/main.tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './styles/theme.css';

async function boot() {
  // The browser-preview mock is dev-only; production builds do not carry it.
  if (import.meta.env.DEV) (await import('./dev/mockTauri')).installBrowserPreviewMocksIfNeeded();
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
}

void boot();
