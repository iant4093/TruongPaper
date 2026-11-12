// src/components/SteamAuthModal.tsx
import React, { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../store/useAppStore';

export const SteamAuthModal: React.FC = () => {
  const authModalOpen = useAppStore((s) => s.authModalOpen);
  const qrMatrix = useAppStore((s) => s.qrMatrix);
  const need2Fa = useAppStore((s) => s.need2Fa);
  const steamAuthError = useAppStore((s) => s.steamAuthError);
  const steamSigningIn = useAppStore((s) => s.steamSigningIn);
  const closeSteamAuthModal = useAppStore((s) => s.closeSteamAuthModal);
  const startSteamQrLogin = useAppStore((s) => s.startSteamQrLogin);
  const submitSteamCredentials = useAppStore((s) => s.submitSteamCredentials);
  const submitSteam2FaCode = useAppStore((s) => s.submitSteam2FaCode);

  const [tab, setTab] = useState<'qr' | 'password'>('qr');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [twoFaCode, setTwoFaCode] = useState('');
  const qrCanvasRef = useRef<HTMLCanvasElement | null>(null);

  const dialogRef = useRef<HTMLDivElement | null>(null);
  const firstRef = useRef<HTMLButtonElement | null>(null);

  // Nothing starts on its own: the QR session begins when the user asks for it.
  // Focus moves into the dialog, Escape closes it, and Tab stays inside.
  useEffect(() => {
    if (!authModalOpen) return;
    const previous = document.activeElement as HTMLElement | null;
    firstRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); closeSteamAuthModal(); return; }
      if (event.key !== 'Tab' || !dialogRef.current) return;
      const focusable = [...dialogRef.current.querySelectorAll<HTMLElement>('button, input, [href], [tabindex]:not([tabindex="-1"])')].filter((el) => !(el as HTMLButtonElement).disabled);
      if (focusable.length === 0) return;
      const first = focusable[0], last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('keydown', onKey); if (previous?.isConnected) previous.focus(); };
  }, [authModalOpen, closeSteamAuthModal]);

  useEffect(() => {
    const canvas = qrCanvasRef.current;
    if (!canvas || !qrMatrix || qrMatrix.length === 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const rows = qrMatrix.length;
    const cols = qrMatrix[0].length;
    const quietZone = 4;
    const modulePx = 5;
    canvas.width = (cols + quietZone * 2) * modulePx;
    canvas.height = (rows + quietZone * 2) * modulePx;

    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    ctx.fillStyle = '#090b12';
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        if (qrMatrix[r][c]) {
          ctx.fillRect(
            (c + quietZone) * modulePx,
            (r + quietZone) * modulePx,
            modulePx,
            modulePx
          );
        }
      }
    }
  }, [qrMatrix]);

  if (!authModalOpen) return null;

  return (
    <div className="modal-backdrop" style={{ zIndex: 100 }}>
      <div className="panel" ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="steam-auth-title"
        style={{ width: 'min(420px, calc(100vw - 32px))', maxHeight: 'calc(100vh - 32px)', overflowY: 'auto', padding: 24 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
          <h3 id="steam-auth-title" style={{ margin: 0, fontSize: 18 }}>Sign in to Steam</h3>
          <button type="button" className="button" onClick={closeSteamAuthModal}>
            Close
          </button>
        </div>

        <div style={{ display: 'flex', gap: 8, marginBottom: 18 }}>
          <button
            type="button"
            ref={firstRef}
            className={`button ${tab === 'qr' ? 'button-primary' : ''}`}
            aria-pressed={tab === 'qr'}
            style={{ flex: 1 }}
            onClick={() => setTab('qr')}
          >
            Steam app (QR)
          </button>
          <button
            type="button"
            className={`button ${tab === 'password' ? 'button-primary' : ''}`}
            aria-pressed={tab === 'password'}
            style={{ flex: 1 }}
            onClick={() => setTab('password')}
          >
            Username and code
          </button>
        </div>

        {steamAuthError && (
          <div role="alert" style={{ fontSize: 12, color: 'var(--danger-text)', marginBottom: 12 }}>
            {steamAuthError}
          </div>
        )}

        {tab === 'qr' ? (
          <div style={{ textAlign: 'center' }}>
            <p style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 14 }}>
              Sign in with the Steam mobile app. Steam remembers you on this computer, so you only need to sign in once.
            </p>
            {qrMatrix ? (
              <>
                <canvas
                  ref={qrCanvasRef}
                  role="img"
                  aria-label="Steam sign-in QR code"
                  style={{
                    borderRadius: 14,
                    boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
                    maxWidth: '100%',
                  }}
                />
                <p style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 10 }}>
                  Scan it with the QR scanner in the Steam app, not the camera. The code refreshes about every 20 seconds.
                </p>
              </>
            ) : steamSigningIn ? (
              <div style={{ padding: '48px 0', color: 'var(--text-muted)', fontSize: 13 }}>
                Generating Steam QR session...
              </div>
            ) : (
              <button type="button" className="button button-primary" onClick={() => void startSteamQrLogin()}>
                {steamAuthError ? 'New QR code' : 'Show QR code'}
              </button>
            )}
          </div>
        ) : (
          <div>
            {!need2Fa ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void submitSteamCredentials(username, password);
                }}
                style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
              >
                <input
                  type="text"
                  placeholder="Steam Username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  className="button"
                  style={{ width: '100%', textAlign: 'left', cursor: 'text' }}
                  required
                />
                <input
                  type="password"
                  placeholder="Steam Password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="button"
                  style={{ width: '100%', textAlign: 'left', cursor: 'text' }}
                  required
                />
                <button type="submit" className="button button-primary">
                  Sign In
                </button>
              </form>
            ) : (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void submitSteam2FaCode(twoFaCode);
                }}
                style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
              >
                <div style={{ fontSize: 13, color: 'var(--info-text)' }}>{need2Fa.prompt}</div>
                <input
                  type="text"
                  placeholder={need2Fa.kind === 'email' ? 'Email Steam Guard Code' : 'Authenticator 2FA Code'}
                  value={twoFaCode}
                  onChange={(e) => setTwoFaCode(e.target.value)}
                  className="button"
                  style={{ width: '100%', textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 16 }}
                  autoFocus
                  required
                />
                <button type="submit" className="button button-primary">
                  Verify code
                </button>
              </form>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
