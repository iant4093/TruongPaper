import React from 'react';
import { AudioLines, Check, Trash2 } from 'lucide-react';
import { Thumb } from './ui';

export const WallpaperCard: React.FC<{
  title: string; preview: string | null | undefined; kind: string; detail?: string;
  active?: boolean; selected?: boolean; audioReactive?: boolean; canApply: boolean; applying?: boolean;
  onSelect?: () => void; onApply: () => void; onDelete?: () => void;
}> = ({ title, preview, kind, detail, active, selected, audioReactive, canApply, applying, onSelect, onApply, onDelete }) => (
  <article className="wallpaper-card">
    {onSelect ? <button type="button" className="wallpaper-select" aria-label={`Select ${title}`} aria-pressed={!!selected} onClick={onSelect}>
      <Thumb src={preview} alt={title} height={142} lazy />
      {active && <span className="wallpaper-active"><Check size={11} />Current</span>}
    </button> : <div className="wallpaper-select">
      <Thumb src={preview} alt={title} height={142} lazy />
      {active && <span className="wallpaper-active"><Check size={11} />Current</span>}
    </div>}
    <div className="wallpaper-info">
      <div className="wallpaper-copy"><div className="wallpaper-title" title={title}>{title}</div><div className="wallpaper-meta">{kind}{audioReactive && <AudioLines size={11} aria-label="Audio reactive" />}{detail && <><span>·</span>{detail}</>}</div></div>
      <button type="button" className="button wallpaper-apply" disabled={!canApply || active || applying} aria-label={`${active ? 'Currently using' : applying ? 'Applying' : 'Apply'} ${title}`} onClick={onApply}>{active ? <Check size={12} /> : applying ? 'Applying…' : 'Apply'}</button>
      {onDelete && <button type="button" className="icon-button wallpaper-delete" aria-label={`Delete ${title}`} onClick={onDelete}><Trash2 size={12} /></button>}
    </div>
  </article>
);
