// "Import from computer…": copies a Wallpaper Engine item folder or a video file into the
// library through `library_import_local`, then selects the new item. The two choices live in a
// small menu so opening it never shifts the page layout.
import React, { useEffect, useRef, useState } from 'react';
import { FolderInput } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { useAppStore } from '../store/useAppStore';

const VIDEO_FILTER = [{ name: 'Video', extensions: ['mp4', 'webm', 'mov', 'mkv'] }];

export const LocalImport: React.FC<{ onStatus?: (message: string | null) => void; label?: string }> = ({ onStatus, label = 'Import from computer…' }) => {
  const importLocalWallpaper = useAppStore((s) => s.importLocalWallpaper);
  const [choosing, setChoosing] = useState(false);
  const [busy, setBusy] = useState(false);
  const root = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (!choosing) return;
    const away = (event: Event) => { if (!root.current?.contains(event.target as Node)) setChoosing(false); };
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); setChoosing(false); } };
    document.addEventListener('pointerdown', away);
    document.addEventListener('keydown', key, true);
    return () => { document.removeEventListener('pointerdown', away); document.removeEventListener('keydown', key, true); };
  }, [choosing]);

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
    <span className="local-import" ref={root}>
      <button type="button" className="button" aria-haspopup="menu" aria-expanded={choosing} disabled={busy} onClick={() => setChoosing((v) => !v)}>
        <FolderInput size={14} />{busy ? 'Importing…' : label}
      </button>
      {choosing && (
        <div className="menu-popover" role="menu" aria-label="Import from computer">
          <button type="button" role="menuitem" className="menu-item" onClick={() => void pick('folder')}>Folder with project.json</button>
          <button type="button" role="menuitem" className="menu-item" onClick={() => void pick('video')}>Video file</button>
        </div>
      )}
    </span>
  );
};
