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
  /** The wallpaper on screen for the chosen display. It is the highlighted tile. */
  active?: boolean;
  audioReactive?: boolean;
  canApply: boolean;
  applying?: boolean;
  /** Undefined hides the star (the Photography tile cannot be starred). */
  favorite?: boolean;
  tabIndex?: 0 | -1;
  onApply: (id: string) => void;
  onDetails?: (id: string) => void;
  onFavorite?: (id: string, next: boolean) => void;
}

const WallpaperCardBase: React.FC<WallpaperCardProps> = ({
  id, title, preview, kind, badge, active, audioReactive, canApply, applying, favorite, tabIndex = 0,
  onApply, onDetails, onFavorite,
}) => {
  const thumb = <Thumb src={preview} alt={title} height="auto" lazy />;
  const chips = <>
    {active && <span className="wallpaper-active"><Check size={11} />Current</span>}
    {badge && <span className="wallpaper-badge">{badge}</span>}
  </>;
  return (
    <article className={`wallpaper-card${active ? ' is-current' : ''}`}>
      <div className="wallpaper-media">
        {/* Clicking a tile does nothing: the highlight always marks what is on screen. It stays
            focusable for arrow-key moves, and Enter sets it as the wallpaper. */}
        <div className="wallpaper-select" data-grid-select="" tabIndex={tabIndex} role="button"
          aria-disabled={!canApply || active || !!applying} aria-keyshortcuts="Enter"
          aria-label={active ? `${title}, current wallpaper` : title}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && canApply && !active && !applying) { event.preventDefault(); onApply(id); }
          }}>
          {thumb}{chips}
        </div>
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

export interface DownloadCardProps {
  workshopId: string;
  title: string;
  preview: string | null | undefined;
  /** 0..100 */
  percent: number;
  failed?: boolean;
  error?: string | null;
  cancelling?: boolean;
  tabIndex?: 0 | -1;
  onCancel: (workshopId: string) => void;
  onRetry: (workshopId: string) => void;
  onDismiss: (workshopId: string) => void;
}

/** A Workshop download in the library grid: grayed out, with a progress bar until it is installed. */
const DownloadCardBase: React.FC<DownloadCardProps> = ({
  workshopId, title, preview, percent, failed, error, cancelling, tabIndex = 0, onCancel, onRetry, onDismiss,
}) => {
  const pct = Math.max(0, Math.min(100, Math.round(percent)));
  return (
    <article className={`wallpaper-card is-downloading${failed ? ' is-failed' : ''}`} aria-busy={!failed}>
      <div className="wallpaper-media">
        {/* Focusable for arrow-key moves through the grid; there is nothing to apply yet. */}
        <div className="wallpaper-select" data-grid-select="" tabIndex={tabIndex} role="img"
          aria-label={failed ? `${title}, download failed` : `${title}, downloading ${pct}%`}>
          <Thumb src={preview} alt="" height="auto" lazy />
        </div>
        <div className="wallpaper-progress" aria-hidden={failed ? 'true' : undefined}>
          <span className="wallpaper-progress-label">{failed ? 'Download failed' : `Downloading ${pct}%`}</span>
          {!failed && (
            <div className="progress" role="progressbar" aria-label={`Downloading ${title}`}
              aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
              <div style={{ width: `${pct}%` }} />
            </div>
          )}
        </div>
      </div>
      <div className="wallpaper-info">
        <div className="wallpaper-title" title={title}>{title}</div>
        <div className="wallpaper-meta" title={failed ? error ?? undefined : undefined}>
          {failed ? (error || 'Download failed') : 'Workshop download'}
        </div>
        <div className="wallpaper-actions">
          {failed ? (
            <>
              <button type="button" className="button wallpaper-apply" onClick={() => onRetry(workshopId)}>Retry</button>
              <button type="button" className="button" onClick={() => onDismiss(workshopId)}>Dismiss</button>
            </>
          ) : (
            <button type="button" className="button wallpaper-apply" disabled={cancelling}
              aria-label={`Cancel download: ${title}`} onClick={() => onCancel(workshopId)}>
              {cancelling ? 'Cancelling…' : 'Cancel download'}
            </button>
          )}
        </div>
      </div>
    </article>
  );
};

export const DownloadCard = React.memo(DownloadCardBase);
