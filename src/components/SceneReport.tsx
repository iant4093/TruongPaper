import React from 'react';

/** Collapsed list of scene diagnostics reported by the renderer for a wallpaper. */
export const SceneReport: React.FC<{ messages: string[] | undefined }> = ({ messages }) => {
  if (!messages || messages.length === 0) return null;
  return (
    <details className="scene-report">
      <summary>Scene report ({messages.length})</summary>
      <ul>{messages.map((message, index) => <li key={`${index}:${message}`}>{message}</li>)}</ul>
    </details>
  );
};
