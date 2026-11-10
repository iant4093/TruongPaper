// src/components/SteamAuthModal.tsx
import React, { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../store/useAppStore';

export const SteamAuthModal: React.FC = () => {
  const authModalOpen = useAppStore((s) => s.authModalOpen);
  const qrMatrix = useAppStore((s) => s.qrMatrix);
  const need2Fa = useAppStore((s) => s.need2Fa);
  const steamAuthError = useAppStore((s) => s.steamAuthError);
  const closeSteamAuthModal = useAppStore((s) => s.closeSteamAuthModal);
  const startSteamQrLogin = useAppStore((s) => s.startSteamQrLogin);
  const submitSteamCredentials = useAppStore((s) => s.submitSteamCredentials);
  const submitSteam2FaCode = useAppStore((s) => s.submitSteam2FaCode);

  const [tab, setTab] = useState<'qr' | 'password'>('qr');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [twoFaCode, setTwoFaCode] = useState('');
  const qrCanvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    if (authModalOpen && tab === 'qr' && !qrMatrix) {
      void startSteamQrLogin();
    }
  }, [authModalOpen, tab]);

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
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 100,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0, 0, 0, 0.65)',
      }}
    >
      <div className="panel" style={{ width: 420, padding: 24 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
          <h3 style={{ margin: 0, fontSize: 18 }}>Sign in to Steam Workshop</h3>
          <button type="button" className="button" onClick={closeSteamAuthModal}>
            Close
          </button>
        </div>

        <div style={{ display: 'flex', gap: 8, marginBottom: 18 }}>
          <button
            type="button"
            className={`button ${tab === 'qr' ? 'button-primary' : ''}`}
            style={{ flex: 1 }}
            onClick={() => setTab('qr')}
          >
            Steam Mobile QR
          </button>
          <button
            type="button"
            className={`button ${tab === 'password' ? 'button-primary' : ''}`}
            style={{ flex: 1 }}
            onClick={() => setTab('password')}
          >
            Username & 2FA
          </button>
        </div>

        {steamAuthError && (
          <div role="alert" style={{ fontSize: 12, color: '#fca5a5', marginBottom: 12 }}>
            {steamAuthError}
          </div>
        )}

        {tab === 'qr' ? (
          <div style={{ textAlign: 'center' }}>
            <p style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 14 }}>
              Scan with the Steam Mobile App to authenticate DepotDownloader and save a persistent refresh token in your OS Keychain.
            </p>
            {qrMatrix ? (
              <canvas
                ref={qrCanvasRef}
                style={{
                  borderRadius: 14,
                  boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
                  maxWidth: '100%',
                }}
              />
            ) : (
              <div style={{ padding: '48px 0', color: 'var(--text-muted)', fontSize: 13 }}>
                Generating Steam QR session...
              </div>
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
                <div style={{ fontSize: 13, color: 'var(--accent-cyan)' }}>{need2Fa.prompt}</div>
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
                  Verify Steam Guard Code
                </button>
              </form>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
