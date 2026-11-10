// @vitest-environment jsdom
// tests/steamAuthModal.test.tsx — QR matrix rendering & 2FA flow (§4.4)
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

const invokeMock = vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null);
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

import { SteamAuthModal } from '../src/components/SteamAuthModal';
import { useAppStore } from '../src/store/useAppStore';
import { buildDemoQrMatrix } from '../src/dev/mockTauri';

interface FakeCtx {
  fillStyle: string;
  rects: Array<{ style: string; x: number; y: number; w: number; h: number }>;
  fillRect: (x: number, y: number, w: number, h: number) => void;
}

let ctx: FakeCtx;
const initial = useAppStore.getState();

beforeEach(() => {
  invokeMock.mockClear();
  useAppStore.setState(initial, true);
  ctx = {
    fillStyle: '',
    rects: [],
    fillRect(x, y, w, h) {
      this.rects.push({ style: this.fillStyle, x, y, w, h });
    },
  };
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () => ctx as unknown as CanvasRenderingContext2D
  );
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('<SteamAuthModal />', () => {
  it('renders nothing while closed', () => {
    const { container } = render(<SteamAuthModal />);
    expect(container.innerHTML).toBe('');
  });

  it('starts a QR login session when opened without a matrix', async () => {
    await act(async () => {
      useAppStore.setState({ authModalOpen: true });
      render(<SteamAuthModal />);
    });
    expect(invokeMock).toHaveBeenCalledWith('steam_login_qr', undefined);
    expect(screen.getByText('Generating Steam QR session...')).toBeTruthy();
  });

  it('paints every dark module of the bool matrix with a 4-module quiet zone', async () => {
    const matrix = [
      [true, false, true],
      [false, true, false],
      [true, true, false],
    ];
    await act(async () => {
      useAppStore.setState({ authModalOpen: true, qrMatrix: matrix });
      render(<SteamAuthModal />);
    });
    const canvas = document.querySelector('canvas')!;
    // (cols + 2 * quietZone) * modulePx = (3 + 8) * 5
    expect(canvas.width).toBe(55);
    expect(canvas.height).toBe(55);

    const [background, ...modules] = ctx.rects;
    expect(background).toEqual({ style: '#ffffff', x: 0, y: 0, w: 55, h: 55 });
    const expected = matrix.flatMap((row, r) =>
      row.flatMap((on, c) => (on ? [{ style: '#090b12', x: (c + 4) * 5, y: (r + 4) * 5, w: 5, h: 5 }] : []))
    );
    expect(modules).toEqual(expected);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('handles a full-size QR matrix (finder patterns included)', async () => {
    const matrix = buildDemoQrMatrix(29);
    await act(async () => {
      useAppStore.setState({ authModalOpen: true, qrMatrix: matrix });
      render(<SteamAuthModal />);
    });
    const dark = matrix.flat().filter(Boolean).length;
    expect(ctx.rects.length).toBe(dark + 1);
    expect(document.querySelector('canvas')!.width).toBe((29 + 8) * 5);
    // Top-left finder pattern: 7×7 ring is dark on its border.
    expect(matrix[0].slice(0, 7).every(Boolean)).toBe(true);
    expect(matrix[1][1]).toBe(false);
  });

  it('submits credentials, then the Steam Guard code', async () => {
    await act(async () => {
      useAppStore.setState({ authModalOpen: true, qrMatrix: [[true]] });
      render(<SteamAuthModal />);
    });
    fireEvent.click(screen.getByText('Username & 2FA'));
    fireEvent.change(screen.getByPlaceholderText('Steam Username'), { target: { value: 'ian' } });
    fireEvent.change(screen.getByPlaceholderText('Steam Password'), { target: { value: 'pw' } });
    await act(async () => {
      fireEvent.click(screen.getByText('Sign In'));
    });
    expect(invokeMock).toHaveBeenCalledWith('steam_login_credentials', { username: 'ian', password: 'pw' });

    await act(async () => {
      useAppStore.setState({ need2Fa: { kind: 'email', prompt: 'Check your email' } });
    });
    expect(screen.getByText('Check your email')).toBeTruthy();
    fireEvent.change(screen.getByPlaceholderText('Email Steam Guard Code'), { target: { value: 'AB12C' } });
    await act(async () => {
      fireEvent.click(screen.getByText('Verify Steam Guard Code'));
    });
    expect(invokeMock).toHaveBeenCalledWith('steam_submit_2fa_code', { code: 'AB12C' });
    expect(useAppStore.getState().need2Fa).toBeNull();
  });

  it('Close resets QR/2FA state', async () => {
    await act(async () => {
      useAppStore.setState({ authModalOpen: true, qrMatrix: [[true]] });
      render(<SteamAuthModal />);
    });
    fireEvent.click(screen.getByText('Close'));
    expect(useAppStore.getState()).toMatchObject({ authModalOpen: false, qrMatrix: null, need2Fa: null });
  });
});
