// src/main.tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import { installBrowserPreviewMocksIfNeeded } from './dev/mockTauri';
import App from './App';
import './styles/theme.css';

installBrowserPreviewMocksIfNeeded();

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
