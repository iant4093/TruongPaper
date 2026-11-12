import React from 'react';

/** A wallpaper that failed to load: a plain sentence first, the raw backend message behind "Show details". */
export const LoadErrorCard: React.FC<{
  title: string;
  message: string;
  monitorName?: string;
  className?: string;
  onRetry: () => void;
  onDismiss: () => void;
}> = ({ title, message, monitorName, className = 'load-error-card', onRetry, onDismiss }) => (
  <div role="alert" className={className}>
    <div className="load-error-text">
      <div>{title} couldn’t be set as wallpaper.{monitorName ? ` (${monitorName})` : ''}</div>
      <details className="load-error-details">
        <summary>Show details</summary>
        <div>{message}</div>
      </details>
    </div>
    <div className="load-error-actions">
      <button type="button" className="button" onClick={onRetry}>Try again</button>
      <button type="button" className="button" onClick={onDismiss}>Dismiss</button>
    </div>
  </div>
);
