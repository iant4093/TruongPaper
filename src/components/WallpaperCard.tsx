import React from 'react';
import { AudioLines, Check, Info, Star } from 'lucide-react';
import { Thumb } from './ui';

export interface WallpaperCardProps {
  /** Passed back to every handler, so parents can use one stable callback for all cards. */
  id: string;
  title: string;
  preview: string | null | undefined;
  kind: string;
  /** Short badge on the thumbnail, e.g. "Windows only". */
  badge?: string;
  active?: boolean;
  selected?: boolean;
  audioReactive?: boolean;
  canApply: boolean;
  applying?: boolean;
  /** Undefined hides the star (the Photography tile cannot be starred). */
  favorite?: boolean;
  tabIndex?: 0 | -1;
  onSelect?: (id: string) => void;
  onApply: (id: string) => void;
  onDetails?: (id: string) => void;
  onFavorite?: (id: string, next: boolean) => void;
}

const WallpaperCardBase: React.FC<WallpaperCardProps> = ({
  id, title, preview, kind, badge, active, selected, audioReactive, canApply, applying, favorite, tabIndex = 0,
  onSelect, onApply, onDetails, onFavorite,
}) => {
  const thumb = <Thumb src={preview} alt={title} height="auto" lazy />;
  const chips = <>
    {active && <span className="wallpaper-active"><Check size={11} />Current</span>}
    {badge && <span className="wallpaper-badge">{badge}</span>}
  </>;
  return (
    <article className="wallpaper-card">
      <div className="wallpaper-media">
        {onSelect ? (
          <button type="button" className="wallpaper-select" data-grid-select="" tabIndex={tabIndex}
            aria-label={`Select ${title}`} aria-pressed={!!selected}
            onClick={() => onSelect(id)} onDoubleClick={() => onDetails?.(id)}
            onKeyDown={(event) => {
              // Enter sets the wallpaper; Space still selects.
              if (event.key === 'Enter' && canApply && !active && !applying) { event.preventDefault(); onApply(id); }
            }}>
            {thumb}{chips}
          </button>
        ) : <div className="wallpaper-select">{thumb}{chips}</div>}
        {favorite !== undefined && onFavorite && (
          <button type="button" className={`wallpaper-star${favorite ? ' is-on' : ''}`} aria-pressed={favorite}
            aria-label={favorite ? `Remove ${title} from favorites` : `Add ${title} to favorites`}
            onClick={() => onFavorite(id, !favorite)}>
            <Star size={14} fill={favorite ? 'currentColor' : 'none'} />
          </button>
        )}
      </div>
      <div className="wallpaper-info">
        <div className="wallpaper-title" title={title}>{title}</div>
        <div className="wallpaper-meta">{kind}{audioReactive && <AudioLines size={12} aria-label="Reacts to sound" />}</div>
        <div className="wallpaper-actions">
          <button type="button" className="button wallpaper-apply" disabled={!canApply || active || applying}
            aria-label={`${active ? 'Currently using' : applying ? 'Applying' : 'Set as wallpaper:'} ${title}`}
            onClick={() => onApply(id)}>
            {active ? <><Check size={13} />Current</> : applying ? 'Applying…' : 'Set as wallpaper'}
          </button>
          {onDetails && (
            <button type="button" className="icon-button wallpaper-details-button" aria-label={`Details for ${title}`}
              title="Details" onClick={() => onDetails(id)}>
              <Info size={15} />
            </button>
          )}
        </div>
      </div>
    </article>
  );
};

/** Memoized: with stable callbacks a store update only re-renders the cards whose data changed. */
export const WallpaperCard = React.memo(WallpaperCardBase);
