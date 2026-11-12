// src/components/views/PhotosApiView.tsx
// §2.2 View 1: "Ian's Photos API" Studio View
import React, { useEffect, useRef, useState } from 'react';
import { open as openExternal } from '@tauri-apps/plugin-shell';
import { AnimatePresence, motion } from 'framer-motion';
import {
  BookOpen,
  ExternalLink,
  EyeOff,
  Heart,
  History,
  Plus,
  RefreshCw,
  SkipForward,
  Trash2,
} from 'lucide-react';
import { useAppStore } from '../../store/useAppStore';
import { parseImportLink } from '../../lib/importLinks';
import { GB, MB, OFFLINE_NOTICE, PHOTO_POOL_OPTIONS, formatBytes, formatOfflinePhotoCount, offlinePhotosForDisplay, photoPreviewSrc } from '../../lib/format';
import type { PhotoItem } from '../../types/ipc';
import { Badge, SectionTitle, SegmentedControl, Thumb, ToggleRow, ViewHeader, cardGrid } from '../ui';

const MIN_CACHE = 100 * MB;
const MAX_CACHE = 10 * GB;

const photoDims = (p: PhotoItem) =>
  p.width && p.height ? `${p.width} × ${p.height}` : 'Full Resolution';

const photoAlbumName = (photo: PhotoItem) => photo.albumSlug.replace(/-/g, ' ');

const PhotoBadges: React.FC<{ photo: PhotoItem }> = ({ photo }) => (
  <div className="photo-badges" title={photoAlbumName(photo)}>
    <Badge>{photoAlbumName(photo)}</Badge>
    {photo.featured && <Badge tone="warn">Featured</Badge>}
  </div>
);

const PhotoTile: React.FC<{ photo: PhotoItem; children: React.ReactNode }> = ({ photo, children }) => (
  <article className="card photo-tile">
    <Thumb src={photoPreviewSrc(photo)} alt={photoAlbumName(photo)} height={112} lazy contain />
    <PhotoBadges photo={photo} />
    <div className="photo-meta">{photoDims(photo)} · {formatBytes(photo.bytes)}</div>
    <div className="photo-actions">{children}</div>
  </article>
);

export const PhotosApiView: React.FC = () => {
  const photoStatus = useAppStore((s) => s.photoStatus);
  const settings = useAppStore((s) => s.settings);
  const savedAlbums = useAppStore((s) => s.savedAlbums);
  const publicCatalog = useAppStore((s) => s.publicCatalog);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const setPhotoFilter = useAppStore((s) => s.setPhotoFilter);
  const fetchPublicCatalog = useAppStore((s) => s.fetchPublicCatalog);
  const addAlbum = useAppStore((s) => s.addAlbum);
  const toggleAlbum = useAppStore((s) => s.toggleAlbum);
  const removeAlbum = useAppStore((s) => s.removeAlbum);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const setPhotoFavorite = useAppStore((s) => s.setPhotoFavorite);
  const applyFavoritePhoto = useAppStore((s) => s.applyFavoritePhoto);
  const blacklistAndSkipPhoto = useAppStore((s) => s.blacklistAndSkipPhoto);
  const setCacheLimit = useAppStore((s) => s.setCacheLimit);
  const clearPhotoCache = useAppStore((s) => s.clearPhotoCache);
  const resetPhotoHistory = useAppStore((s) => s.resetPhotoHistory);
  const triggerPhotoRefill = useAppStore((s) => s.triggerPhotoRefill);
  const updateSettings = useAppStore((s) => s.updateSettings);

  const [catalogOpen, setCatalogOpen] = useState(false);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [albumInput, setAlbumInput] = useState('');
  const [albumMsg, setAlbumMsg] = useState<string | null>(null);
  const [cacheDraft, setCacheDraft] = useState<number | null>(null);
  const [photoActionPending, setPhotoActionPending] = useState(false);
  const [photoActionError, setPhotoActionError] = useState<string | null>(null);
  const favoritesSection = useRef<HTMLElement>(null);

  const mode = photoStatus?.filterMode ?? 'all';
  const featuredOnly = photoStatus?.featuredOnly ?? false;
  const maxCache = photoStatus?.maxCacheBytes ?? settings?.maxPhotoCacheBytes ?? 250 * MB;
  const cached = photoStatus?.cachedBytes ?? 0;
  const activeMonitor = monitors.find((monitor) => monitor.monitorId === selectedMonitorId) ?? monitors[0];
  const activeMonitorId = activeMonitor?.monitorId;
  const currentPhoto =
    (activeMonitorId ? photoStatus?.currentPhotoByMonitor?.[activeMonitorId] : null) ?? null;
  const queue = offlinePhotosForDisplay(photoStatus, activeMonitor, settings?.skipMismatchedOrientation ?? true);
  const favorites = photoStatus?.favoritePhotos ?? [];

  const runPhotoAction = async (action: () => Promise<void>) => {
    setPhotoActionPending(true);
    setPhotoActionError(null);
    try {
      await action();
    } catch (error) {
      setPhotoActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setPhotoActionPending(false);
    }
  };

  // Debounce cache-limit slider commits so we don't spam the backend while dragging.
  useEffect(() => {
    if (cacheDraft === null) return;
    const t = window.setTimeout(() => {
      void setCacheLimit(cacheDraft).finally(() => setCacheDraft(null));
    }, 400);
    return () => window.clearTimeout(t);
  }, [cacheDraft, setCacheLimit]);

  const openCatalog = async () => {
    const next = !catalogOpen;
    setCatalogOpen(next);
    if (next) {
      setCatalogError(null);
      try {
        await fetchPublicCatalog();
      } catch (err) {
        setCatalogError(err instanceof Error ? err.message : String(err));
      }
    }
  };

  const submitAlbum = async (e: React.FormEvent) => {
    e.preventDefault();
    const parsed = parseImportLink(albumInput);
    if (!parsed || parsed.kind !== 'photo_album') {
      setAlbumMsg('Enter an iantruongphotography.com album URL or slug');
      return;
    }
    try {
      const saved = await addAlbum(parsed.albumSlug);
      setAlbumMsg(`Saved "${saved.displayName}"`);
      setAlbumInput('');
    } catch (err) {
      setAlbumMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const savedSlugs = new Set(savedAlbums.map((a) => a.albumSlug));
  const displayedCache = cacheDraft ?? maxCache;

  return (
    <div>
      <ViewHeader
        title="Ian Truong Photography"
        subtitle="Photo wallpapers from the Ian Truong Photography API."
        actions={
          <>
            <button type="button" className="button" onClick={() => favoritesSection.current?.scrollIntoView({ block: 'start' })}>
              <Heart size={14} /> Favorites
            </button>
            <button type="button" className="button" onClick={() => void openCatalog()}>
              <BookOpen size={14} /> {catalogOpen ? 'Close catalog' : 'Browse albums'}
            </button>
          </>
        }
      />
      {photoStatus?.offline && <div role="status" className="offline-notice">{OFFLINE_NOTICE}</div>}

      {/* Filter Controls Card */}
      <div className="card" style={{ padding: 16 }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 10 }}>Photo source</div>
        <SegmentedControl
          value={mode}
          options={PHOTO_POOL_OPTIONS}
          onChange={(next) => void setPhotoFilter(next, next === 'featured' ? true : featuredOnly)}
        />
        {mode === 'albums' && (
          <div style={{ marginTop: 12 }}>
            <ToggleRow
              label="Featured Only"
              description="Only draw featured photos from the enabled albums"
              checked={featuredOnly}
              onChange={(next) => void setPhotoFilter('albums', next)}
            />
          </div>
        )}
        <div style={{ marginTop: 12 }}>
          <ToggleRow
            label="Match photo orientation to display"
            description="Offline photos and automatic rotation match the selected display."
            checked={settings?.skipMismatchedOrientation ?? true}
            onChange={(next) => void updateSettings({ skipMismatchedOrientation: next })}
          />
        </div>
      </div>

      {/* Browse Official Album Catalog Drawer */}
      <AnimatePresence initial={false}>
        {catalogOpen && (
          <motion.div
            key="catalog"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            style={{ overflow: 'hidden' }}
          >
            <SectionTitle>Official Album Catalog</SectionTitle>
            {catalogError && <div style={{ fontSize: 12, color: '#fca5a5', marginBottom: 8 }}>{catalogError}</div>}
            <div style={cardGrid}>
              {publicCatalog.map((album) => (
                <div key={album.albumId} className="card" style={{ padding: 12 }}>
                  <Thumb src={album.coverThumbnailUrl} alt={album.title} height={110} />
                  <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
                    <Badge>{album.category}</Badge>
                    <Badge tone="muted">{album.imageCount} photos</Badge>
                  </div>
                  <div style={{ fontWeight: 600, marginTop: 6 }}>{album.title}</div>
                  <button
                    type="button"
                    className="button button-primary"
                    style={{ width: '100%', marginTop: 10, fontSize: 12 }}
                    disabled={savedSlugs.has(album.slug)}
                    onClick={() => void addAlbum(album.slug, album.title)}
                  >
                    {savedSlugs.has(album.slug) ? 'Saved' : 'Save & Enable Album'}
                  </button>
                </div>
              ))}
              {publicCatalog.length === 0 && !catalogError && (
                <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading catalog…</div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Saved albums Grid */}
      <SectionTitle>Saved albums</SectionTitle>
      <form onSubmit={submitAlbum} className="saved-album-form">
        <input
          type="text"
          aria-label="Ian Truong Photography album link or slug"
          value={albumInput}
          onChange={(e) => setAlbumInput(e.target.value)}
          placeholder="Paste an album link or slug"
          className="button"
        />
        <button type="submit" className="button">
          <Plus size={14} /> Add Album
        </button>
      </form>
      {albumMsg && <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 8 }}>{albumMsg}</div>}
      {savedAlbums.length ? <div style={cardGrid}>
        {savedAlbums.map((album) => (
          <div key={album.albumSlug} className="card" style={{ padding: 12 }}>
            <Thumb src={album.coverThumbnailUrl} alt={album.displayName} height={90} />
            <label style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 8, fontSize: 13, fontWeight: 600 }}>
              <input
                type="checkbox"
                checked={album.isEnabled}
                onChange={(e) => void toggleAlbum(album.albumSlug, e.target.checked)}
              />
              {album.displayName}
            </label>
            <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 2 }}>
              {album.albumSlug}
              {album.imageCount !== undefined ? ` • ${album.imageCount} photos` : ''}
            </div>
            <div style={{ display: 'flex', gap: 6, marginTop: 10 }}>
              <button
                type="button"
                className="button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void openExternal(album.albumUrl)}
              >
                <ExternalLink size={12} /> Open on Website
              </button>
              <button
                type="button"
                className="button"
                aria-label={`Remove ${album.displayName}`}
                style={{ fontSize: 11, padding: '6px 10px' }}
                onClick={() => void removeAlbum(album.albumSlug)}
              >
                <Trash2 size={12} />
              </button>
            </div>
          </div>
        ))}
      </div> : <p className="saved-album-hint">No saved albums yet. Add a link above or browse the catalog.</p>}

      {/* Current wallpaper Photo Card */}
      <SectionTitle>Current wallpaper</SectionTitle>
      {photoActionError && <div role="alert" className="error-banner">{photoActionError}</div>}
      {currentPhoto ? (
        <div className="card photo-current">
          <Thumb src={photoPreviewSrc(currentPhoto)} alt={photoAlbumName(currentPhoto)} height={260} contain lazy />
          <div className="photo-current-details">
            <PhotoBadges photo={currentPhoto} />
            <div className="photo-meta">
              {photoDims(currentPhoto)} · {formatBytes(currentPhoto.bytes)}
            </div>
            <div className="photo-actions">
              <button type="button" className="button" disabled={!activeMonitorId || photoActionPending}
                onClick={() => void runPhotoAction(() => rotateNextPhoto(activeMonitorId))}>
                <SkipForward size={14} /> Next photo
              </button>
              <button
                type="button"
                className="button"
                aria-pressed={currentPhoto.isFavorite}
                disabled={photoActionPending}
                onClick={() => void runPhotoAction(() => setPhotoFavorite(currentPhoto.id, !currentPhoto.isFavorite))}
              >
                <Heart size={14} fill={currentPhoto.isFavorite ? 'currentColor' : 'none'} />
                {currentPhoto.isFavorite ? 'Favorited' : 'Add to favorites'}
              </button>
              <button type="button" className="button" disabled={photoActionPending}
                onClick={() => void runPhotoAction(() => blacklistAndSkipPhoto(currentPhoto.id))}>
                <EyeOff size={14} /> Hide photo
              </button>
              <button type="button" className="button" onClick={() => void openExternal(currentPhoto.albumUrl)}>
                <ExternalLink size={14} /> View album
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
          No photo is displayed on the selected monitor yet.
        </div>
      )}

      <section ref={favoritesSection} className="photo-favorites" aria-labelledby="photo-favorites-title">
        <h3 className="section-title" id="photo-favorites-title">Favorites{favorites.length ? ` · ${favorites.length}` : ''}</h3>
        <p className="photo-section-description">Favorites stay on disk and can come up again when they match your photo filters.</p>
        {favorites.length ? (
          <div style={cardGrid}>
            {favorites.map((photo) => (
              <PhotoTile key={photo.id} photo={photo}>
                <button type="button" className="button button-primary"
                  disabled={!activeMonitorId || photoActionPending || currentPhoto?.id === photo.id}
                  onClick={() => {
                    if (activeMonitorId) void runPhotoAction(() => applyFavoritePhoto(activeMonitorId, photo.id));
                  }}>
                  {currentPhoto?.id === photo.id ? 'Current' : 'Apply'}
                </button>
                <button type="button" className="icon-button" disabled={photoActionPending}
                  aria-label={`Remove ${photoAlbumName(photo)} from favorites`}
                  onClick={() => void runPhotoAction(() => setPhotoFavorite(photo.id, false))}>
                  <Heart size={14} fill="currentColor" />
                </button>
              </PhotoTile>
            ))}
          </div>
        ) : <div className="view-subtitle">Add a wallpaper to favorites to find it here later.</div>}
      </section>

      {/* Up Next Queue */}
      <SectionTitle>{formatOfflinePhotoCount(queue.length)}</SectionTitle>
      <div style={cardGrid}>
        {queue.map((photo, idx) => (
          <PhotoTile key={photo.id} photo={photo}>
              {idx === 0 && (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={photoActionPending}
                  onClick={() => void runPhotoAction(() => rotateNextPhoto(activeMonitorId))}
                >
                  <SkipForward size={12} /> Apply Now
                </button>
              )}
              <button
                type="button"
                className="button"
                disabled={photoActionPending}
                onClick={() => void runPhotoAction(() => blacklistAndSkipPhoto(photo.id))}
              >
                <EyeOff size={12} /> Hide photo
              </button>
          </PhotoTile>
        ))}
        {queue.length === 0 && (
          <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
            {settings?.skipMismatchedOrientation ?? true
              ? 'No offline photos match this display yet.'
              : 'No photos are ready offline yet.'}
          </div>
        )}
      </div>

      {/* Storage & history Bar */}
      <SectionTitle>Storage & history</SectionTitle>
      <div className="card" style={{ padding: 16 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13, marginBottom: 6 }}>
          <span>Image Cache</span>
          <span>
            {formatBytes(cached)} / {formatBytes(maxCache)}
          </span>
        </div>
        <div
          role="progressbar"
          aria-valuenow={Math.round((cached / Math.max(maxCache, 1)) * 100)}
          aria-valuemin={0}
          aria-valuemax={100}
          style={{ height: 8, borderRadius: 999, background: 'rgba(255,255,255,0.12)', overflow: 'hidden' }}
        >
          <div
            style={{
              width: `${Math.min(100, (cached / Math.max(maxCache, 1)) * 100)}%`,
              height: '100%',
              background: 'linear-gradient(90deg, var(--accent-blue), var(--accent-cyan))',
            }}
          />
        </div>

        <label style={{ display: 'block', fontSize: 13, marginTop: 16 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Cache Size Limit</span>
            <span>{formatBytes(displayedCache)}</span>
          </div>
          <input
            type="range"
            className="slider"
            aria-label="Cache Size Limit"
            min={MIN_CACHE}
            max={MAX_CACHE}
            step={50 * MB}
            value={displayedCache}
            onChange={(e) => setCacheDraft(Number(e.target.value))}
          />
        </label>

        <div style={{ marginTop: 14 }}>
          <ToggleRow
            label="Auto-reset album/filter history when all photos have been shown (Recommended)"
            description="When off, only new photos and matching favorites rotate."
            checked={settings?.exhaustionAutoReset ?? true}
            onChange={(next) => void updateSettings({ exhaustionAutoReset: next })}
          />
        </div>

        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 12 }}>
          Shown history: {photoStatus?.historyCount ?? 0} IDs • Blacklisted: {photoStatus?.blacklistCount ?? 0}
          {photoStatus && <div>API requests available: {photoStatus.rateLimitRemaining}</div>}
        </div>

        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <button type="button" className="button button-primary" disabled={photoStatus?.isRefilling} onClick={() => void triggerPhotoRefill()}>
            <RefreshCw size={14} /> {photoStatus?.isRefilling ? 'Refilling queue…' : 'Refill queue'}
          </button>
          <button type="button" className="button" onClick={() => void clearPhotoCache()}>
            <Trash2 size={14} /> Clear cache
          </button>
          <button type="button" className="button" onClick={() => void resetPhotoHistory()}>
            <History size={14} /> Reset history
          </button>
        </div>
      </div>
    </div>
  );
};
