// "Import Folder or Video": copies a Wallpaper Engine item folder or a video file into the
// library through `library_import_local`, then selects the new item.
import React, { useState } from 'react';
import { FolderInput } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { useAppStore } from '../store/useAppStore';

const VIDEO_FILTER = [{ name: 'Video', extensions: ['mp4', 'webm', 'mov', 'mkv'] }];

export const LocalImport: React.FC<{ onStatus?: (message: string | null) => void }> = ({ onStatus }) => {
  const importLocalWallpaper = useAppStore((s) => s.importLocalWallpaper);
  const [choosing, setChoosing] = useState(false);
  const [busy, setBusy] = useState(false);

  const pick = async (kind: 'folder' | 'video') => {
    setChoosing(false);
    let selected: string | string[] | null;
    try {
      selected = kind === 'folder'
        ? await open({ directory: true, multiple: false, title: 'Choose a Wallpaper Engine item folder' })
        : await open({ directory: false, multiple: false, filters: VIDEO_FILTER, title: 'Choose a video' });
    } catch (err) {
      onStatus?.(err instanceof Error ? err.message : String(err));
      return;
    }
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (!path) return;
    setBusy(true);
    onStatus?.('Copying into your library…');
    try {
      const item = await importLocalWallpaper(path);
      onStatus?.(`Imported “${item.title}”.`);
    } catch (err) {
      onStatus?.(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <span className="local-import">
      <button type="button" className="button" aria-expanded={choosing} disabled={busy} onClick={() => setChoosing((v) => !v)}>
        <FolderInput size={14} />Import Folder or Video
      </button>
      {choosing && (
        <>
          <button type="button" className="button" onClick={() => void pick('folder')}>Folder with project.json</button>
          <button type="button" className="button" onClick={() => void pick('video')}>Video file</button>
        </>
      )}
    </span>
  );
};
