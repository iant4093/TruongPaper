// src/components/views/PhotosApiView.tsx
// §2.2 View 1: "Ian's Photos API" Studio View
import React, { useEffect, useState } from 'react';
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
import { GB, MB, formatBytes, formatOfflinePhotoCount } from '../../lib/format';
import type { FilterMode, PhotoItem } from '../../types/ipc';
import { Badge, SectionTitle, SegmentedControl, Thumb, ToggleRow, ViewHeader, cardGrid } from '../ui';

const MIN_CACHE = 100 * MB;
const MAX_CACHE = 10 * GB;

const POOL_OPTIONS = [
  ['all', 'All Public Photos'],
  ['featured', 'Featured Photos Only'],
  ['albums', 'Selected Saved albums'],
] as const satisfies ReadonlyArray<readonly [FilterMode, string]>;

const photoDims = (p: PhotoItem) =>
  p.width && p.height ? `${p.width} × ${p.height}` : 'Full Resolution';

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

  const mode = photoStatus?.filterMode ?? 'all';
  const featuredOnly = photoStatus?.featuredOnly ?? false;
  const maxCache = photoStatus?.maxCacheBytes ?? settings?.maxPhotoCacheBytes ?? 250 * MB;
  const cached = photoStatus?.cachedBytes ?? 0;
  const activeMonitorId = selectedMonitorId ?? monitors[0]?.monitorId;
  const currentPhoto =
    (activeMonitorId ? photoStatus?.currentPhotoByMonitor?.[activeMonitorId] : null) ?? null;
  const queue = photoStatus?.unusedQueue ?? [];

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
          <button type="button" className="button" onClick={() => void openCatalog()}>
            <BookOpen size={14} /> {catalogOpen ? 'Close catalog' : 'Browse albums'}
          </button>
        }
      />

      {/* Filter Controls Card */}
      <div className="card" style={{ padding: 16 }}>
        <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 10 }}>Photo source</div>
        <SegmentedControl
          value={mode}
          options={POOL_OPTIONS}
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
            description="Landscape displays use landscape photos. Portrait displays use portrait photos."
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
      <form onSubmit={submitAlbum} style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
        <input
          type="text"
          aria-label="Ian Truong Photography album link or slug"
          value={albumInput}
          onChange={(e) => setAlbumInput(e.target.value)}
          placeholder="Paste an Ian Truong Photography album link or slug"
          className="button"
          style={{ flex: 1, textAlign: 'left', cursor: 'text', justifyContent: 'flex-start' }}
        />
        <button type="submit" className="button">
          <Plus size={14} /> Add Album
        </button>
      </form>
      {albumMsg && <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 8 }}>{albumMsg}</div>}
      <div style={cardGrid}>
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
        {savedAlbums.length === 0 && (
          <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
            No saved albums yet. Paste an album link above or browse the official catalog.
          </div>
        )}
      </div>

      {/* Current wallpaper Photo Card */}
      <SectionTitle>Current wallpaper</SectionTitle>
      {currentPhoto ? (
        <div className="card" style={{ padding: 14, display: 'grid', gridTemplateColumns: '1.4fr 1fr', gap: 16 }}>
          <Thumb src={currentPhoto.filePath} alt={currentPhoto.albumSlug} height={200} />
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <div style={{ display: 'flex', gap: 6 }}>
              <Badge>{currentPhoto.albumSlug}</Badge>
              {currentPhoto.featured && <Badge tone="warn">Featured</Badge>}
            </div>
            <div style={{ fontSize: 13, color: 'var(--text-secondary)' }}>
              {photoDims(currentPhoto)} • {formatBytes(currentPhoto.bytes)}
            </div>
            <button
              type="button"
              className="button"
              onClick={() => void setPhotoFavorite(currentPhoto.id, !currentPhoto.isFavorite)}
            >
              <Heart size={14} fill={currentPhoto.isFavorite ? 'currentColor' : 'none'} />
              {currentPhoto.isFavorite ? 'Favorited' : 'Favorite / Keep on Disk'}
            </button>
            <button type="button" className="button" onClick={() => void blacklistAndSkipPhoto(currentPhoto.id)}>
              <EyeOff size={14} /> Hide photo
            </button>
            <button type="button" className="button" onClick={() => void openExternal(currentPhoto.albumUrl)}>
              <ExternalLink size={14} /> View album
            </button>
          </div>
        </div>
      ) : (
        <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
          No photo is displayed on the selected monitor yet.
        </div>
      )}

      {/* Up Next Queue */}
      <SectionTitle>{formatOfflinePhotoCount(queue.length)}</SectionTitle>
      <div style={cardGrid}>
        {queue.map((photo, idx) => (
          <div key={photo.id} className="card" style={{ padding: 12 }}>
            <Thumb src={photo.filePath} alt={photo.albumSlug} height={100} />
            <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
              <Badge>{photo.albumSlug}</Badge>
              {photo.featured && <Badge tone="warn">Featured</Badge>}
            </div>
            <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 6 }}>
              {photoDims(photo)} • {formatBytes(photo.bytes)}
            </div>
            <div style={{ display: 'flex', gap: 6, marginTop: 10 }}>
              {idx === 0 && (
                <button
                  type="button"
                  className="button button-primary"
                  style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                  onClick={() => void rotateNextPhoto(activeMonitorId)}
                >
                  <SkipForward size={12} /> Apply Now
                </button>
              )}
              <button
                type="button"
                className="button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void blacklistAndSkipPhoto(photo.id)}
              >
                <EyeOff size={12} /> Skip/Blacklist
              </button>
            </div>
          </div>
        ))}
        {queue.length === 0 && (
          <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Queue is empty. Refill to download new photos.</div>
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
            description="When off, TruongPaper holds on the last photo once the pool is exhausted."
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
