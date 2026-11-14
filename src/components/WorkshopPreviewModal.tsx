// src/components/WorkshopPreviewModal.tsx
// §2.1 Floating Liquid Glass "Workshop Preview & Download" modal driven by
// `steam_inspect_url` + `steam-download://progress` events.
import React, { useRef, useState } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { MonitorSmartphone, X } from 'lucide-react';
import { errorMessage, isCancelMessage, useAppStore } from '../store/useAppStore';
import { formatBytes, workshopTypeFromTags } from '../lib/format';
import { Badge, Thumb } from './ui';
import { useModalFocus } from '../lib/useModalFocus';
import { toast } from '../store/feedback';
import type { WorkshopDownloadSource } from '../types/ipc';

export const WorkshopPreviewModal: React.FC = () => {
  const preview = useAppStore((s) => s.inspectedWorkshop);
  const authModalOpen = useAppStore((s) => s.authModalOpen);
  const downloadsById = useAppStore((s) => s.downloadsById);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const closeWorkshopPreview = useAppStore((s) => s.closeWorkshopPreview);
  const applyMonitorSource = useAppStore((s) => s.applyMonitorSource);
  const selectWallpaper = useAppStore((s) => s.selectWallpaper);
  const guestStatus = useAppStore((s) => (preview ? s.guestAvailability[preview.workshopId] : undefined));
  const loadGuestAvailability = useAppStore((s) => s.loadGuestAvailability);

  const download = useAppStore((s) => s.downloadWorkshopItem);
  const cancelDownload = useAppStore((s) => s.cancelWorkshopDownload);
  const [cancelling, setCancelling] = useState(false);
  const openAuth = useAppStore((s) => s.openSteamAuthModal);
  const [starting, setStarting] = useState<WorkshopDownloadSource | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  React.useEffect(() => { setDownloadError(null); setStarting(null); setCancelling(false); }, [preview?.workshopId]);
  React.useEffect(() => {
    if (preview) void loadGuestAvailability([preview.workshopId]);
  }, [preview?.workshopId, loadGuestAvailability]);
  const startDownload = async (source: WorkshopDownloadSource) => {
    if (!preview) return;
    setStarting(source); setDownloadError(null);
    try { await download(preview.workshopId, source === 'ggnetwork' ? source : undefined); }
    catch (err) {
      if (isCancelMessage(err)) return; // the user stopped it
      const message = errorMessage(err);
      setDownloadError(message);
      if (source === 'steam' && /^(sign in to steam|steam login required)/i.test(message)) openAuth();
    } finally { setStarting(null); }
  };

  const open = !!preview && !authModalOpen;
  const dialogRef = useRef<HTMLDivElement | null>(null);
  useModalFocus(open, dialogRef, closeWorkshopPreview);
  const progress = preview ? downloadsById[preview.workshopId] : undefined;
  const error = downloadError ?? (!starting && progress?.state === 'error' ? progress.error : null);
  const installed = preview ? wallpapers.find((w) => w.workshopId === preview.workshopId) : undefined;
  const monitorId = selectedMonitorId ?? monitors[0]?.monitorId;

  const downloadedBytes = preview && progress ? (preview.fileSizeBytes * progress.progressPercent) / 100 : 0;

  return (
    <AnimatePresence>
      {open && preview && (
        <motion.div
          key="workshop-preview"
          role="dialog"
          aria-modal="true"
          aria-label="Workshop Preview"
          onMouseDown={(event) => { if (event.target === event.currentTarget) closeWorkshopPreview(); }}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 90,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: 'rgba(0, 0, 0, 0.65)',
          }}
        >
          <motion.div
            className="panel"
            ref={dialogRef}
            initial={{ scale: 0.96, y: 12 }}
            animate={{ scale: 1, y: 0 }}
            exit={{ scale: 0.96, y: 12 }}
            style={{ width: 460, maxWidth: 'calc(100vw - 32px)', maxHeight: 'calc(100vh - 32px)', overflowY: 'auto', padding: 22 }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
              <div style={{ display: 'flex', gap: 6 }}>
                <Badge>{workshopTypeFromTags(preview.tags)}</Badge>
                <Badge tone="muted">#{preview.workshopId}</Badge>
              </div>
              <button type="button" className="button" aria-label="Close preview" style={{ padding: '6px 10px' }} onClick={closeWorkshopPreview}>
                <X size={14} />
              </button>
            </div>
            <Thumb src={preview.previewUrl} alt={preview.title} height={200} />
            <h3 style={{ margin: '14px 0 4px', fontSize: 17 }}>{preview.title}</h3>
            <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>
              {formatBytes(preview.fileSizeBytes)}
              {preview.timeUpdated ? ` • Updated ${new Date(preview.timeUpdated * 1000).toLocaleDateString()}` : ''}
            </div>
            {preview.description && (
              <p style={{ fontSize: 12, color: 'var(--text-muted)', maxHeight: 64, overflow: 'hidden', margin: '8px 0 0' }}>
                {preview.description.replace(/\[[^\]]+\]/g, '')}
              </p>
            )}

            <div style={{ marginTop: 16 }}>
              {!installed && progress?.state !== 'downloading' && progress?.state !== 'completed' && (
                <div className="workshop-download-actions">
                  {workshopTypeFromTags(preview.tags) !== 'Application' && (
                    <button className="button button-primary" disabled={!!starting || guestStatus === 'unavailable'} onClick={() => void startDownload('ggnetwork')}>
                      {guestStatus === 'unavailable' ? 'Not on GGNetwork' : starting === 'ggnetwork' ? 'Checking GGNetwork…' : 'Download via GGNetwork'}
                    </button>
                  )}
                  <button className="button" disabled={!!starting} onClick={() => void startDownload('steam')}>
                    {starting === 'steam' ? 'Starting download…' : progress?.state === 'error' ? 'Retry with Steam' : 'Download with Steam'}
                  </button>
                </div>
              )}
              {!installed && progress?.state !== 'downloading' && progress?.state !== 'completed' && workshopTypeFromTags(preview.tags) !== 'Application' && (
                <p style={{ fontSize: 11, color: 'var(--text-muted)', margin: '10px 0 0' }}>
                  GGNetwork is a third-party mirror, not run by Steam or TruongPaper. Downloading with Steam needs your own Steam account.
                </p>
              )}
              {error && <p role="alert" className="workshop-error">{error}</p>}
              {progress?.state === 'downloading' && (
                <>
                  <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 6 }}>
                    Downloading chunks... {Math.round(progress.progressPercent)}% ({formatBytes(downloadedBytes)} /{' '}
                    {formatBytes(preview.fileSizeBytes)})
                  </div>
                  <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 6 }}>
                    <button type="button" className="button" disabled={cancelling} onClick={() => {
                      setCancelling(true);
                      cancelDownload(preview.workshopId).catch((e) => { setCancelling(false); toast.error(e, 'Could not cancel the download'); });
                    }}>{cancelling ? 'Cancelling…' : 'Cancel download'}</button>
                  </div>
                  <div style={{ height: 6, borderRadius: 999, background: 'rgba(255,255,255,0.12)' }}>
                    <div
                      style={{
                        width: `${Math.min(100, progress.progressPercent)}%`,
                        height: '100%',
                        borderRadius: 999,
                        background: 'linear-gradient(90deg, var(--accent-blue), var(--accent-cyan))',
                        transition: 'width 0.3s ease',
                      }}
                    />
                  </div>
                </>
              )}
              {(installed || progress?.state === 'completed') && (
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <span style={{ fontSize: 12, color: 'var(--accent-cyan)', flex: 1 }}>Added to your library</span>
                  {installed && monitorId && (
                    <button
                      type="button"
                      className="button button-primary"
                      onClick={() => {
                        selectWallpaper(installed.wallpaperId);
                        void applyMonitorSource(monitorId, { kind: 'workshop', wallpaperId: installed.wallpaperId }).then(
                          closeWorkshopPreview,
                          (err) => toast.error(err, 'Could not set the wallpaper'),
                        );
                      }}
                    >
                      <MonitorSmartphone size={14} /> Apply Now
                    </button>
                  )}
                </div>
              )}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
};
