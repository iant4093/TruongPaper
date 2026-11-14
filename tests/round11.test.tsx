// @vitest-environment jsdom
// tests/round11.test.tsx — modal focus hook, Steam sign-in form, playlist export, preset errors.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import React, { useRef } from 'react';

const invokeMock = vi.fn(async (cmd: string, _args?: unknown): Promise<unknown> =>
  cmd === 'steam_login_credentials' ? new Promise(() => {}) : null);
const saveMock = vi.fn(async (_opts: unknown): Promise<string | null> => '/tmp/playlists.json');
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => undefined) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: (opts: unknown) => saveMock(opts), open: vi.fn() }));

import { useModalFocus } from '../src/lib/useModalFocus';
import { SteamAuthModal } from '../src/components/SteamAuthModal';
import { PresetBar } from '../src/components/PresetBar';
import { PlaylistsView } from '../src/components/views/PlaylistsView';
import { useAppStore } from '../src/store/useAppStore';
import { useFeedback } from '../src/store/feedback';

const initial = useAppStore.getState();

beforeEach(() => {
  invokeMock.mockClear();
  saveMock.mockClear();
  useAppStore.setState(initial, true);
  useFeedback.setState({ toasts: [] });
});
afterEach(() => cleanup());

const Harness: React.FC<{ onClose: () => void }> = ({ onClose }) => {
  const ref = useRef<HTMLDivElement>(null);
  useModalFocus(true, ref, onClose);
  return (
    <div>
      <button type="button">outside</button>
      <div ref={ref} role="dialog">
        <button type="button">first</button>
        <button type="button">last</button>
      </div>
    </div>
  );
};

describe('useModalFocus', () => {
  it('focuses the first control, wraps Tab, closes on Escape', () => {
    const onClose = vi.fn();
    render(<Harness onClose={onClose} />);
    const first = screen.getByText('first');
    const last = screen.getByText('last');
    expect(document.activeElement).toBe(first);
    last.focus();
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(document.activeElement).toBe(first);
    fireEvent.keyDown(document, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(last);
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('Steam sign-in form', () => {
  const openPasswordTab = async () => {
    await act(async () => { useAppStore.setState({ authModalOpen: true }); render(<SteamAuthModal />); });
    fireEvent.click(screen.getByRole('button', { name: 'Username and code' }));
  };

  it('labels its fields and clears the password after submit', async () => {
    await openPasswordTab();
    const user = screen.getByLabelText('Steam username') as HTMLInputElement;
    const pass = screen.getByLabelText('Steam password') as HTMLInputElement;
    expect(user.autocomplete).toBe('username');
    expect(pass.autocomplete).toBe('current-password');
    fireEvent.change(user, { target: { value: 'ian' } });
    fireEvent.change(pass, { target: { value: 'secret' } });
    await act(async () => { fireEvent.submit(pass.closest('form')!); });
    expect(invokeMock).toHaveBeenCalledWith('steam_login_credentials', { username: 'ian', password: 'secret' });
    expect(pass.value).toBe('');
  });

  it('disables the form and shows progress while signing in', async () => {
    await openPasswordTab();
    await act(async () => { useAppStore.setState({ steamSigningIn: true }); });
    expect((screen.getByLabelText('Steam username') as HTMLInputElement).disabled).toBe(true);
    const button = screen.getByRole('button', { name: 'Signing in…' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

  it('forgets typed values when the window closes', async () => {
    await openPasswordTab();
    fireEvent.change(screen.getByLabelText('Steam username'), { target: { value: 'ian' } });
    await act(async () => { useAppStore.setState({ authModalOpen: false }); });
    await act(async () => { useAppStore.setState({ authModalOpen: true }); });
    fireEvent.click(screen.getByRole('button', { name: 'Username and code' }));
    expect((screen.getByLabelText('Steam username') as HTMLInputElement).value).toBe('');
  });
});

describe('PresetBar', () => {
  it('shows a message when applying a preset fails', async () => {
    useAppStore.setState({
      presetsByWallpaper: { 'workshop:1': [{ presetId: 'p1', wallpaperId: 'workshop:1', name: 'Night', properties: {}, adjustments: {} } as never] },
    });
    invokeMock.mockImplementation(async (cmd: string) => { if (cmd === 'wallpaper_apply_preset') throw new Error('gone'); return null; });
    render(<PresetBar wallpaperId="workshop:1" />);
    fireEvent.click(screen.getByRole('combobox', { name: 'Preset' }));
    await act(async () => { fireEvent.click(await screen.findByRole('option', { name: /Night/ })); });
    await waitFor(() => expect(useFeedback.getState().toasts.some((t) => t.tone === 'error')).toBe(true));
  });
});

describe('Playlist export', () => {
  const playlist = { playlistId: 'pl1', name: 'Mix', mode: 'mixed', intervalSeconds: 60, items: [{ kind: 'photo_api_draw' }] };

  it('writes through the save dialog and a backend command', async () => {
    useAppStore.setState({ playlists: [playlist as never] });
    render(<PlaylistsView />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Save playlists to a file/ })); });
    expect(saveMock).toHaveBeenCalled();
    const call = invokeMock.mock.calls.find(([cmd]) => cmd === 'playlist_export');
    expect(call).toBeTruthy();
    const args = call![1] as { path: string; contents: string };
    expect(args.path).toBe('/tmp/playlists.json');
    expect(JSON.parse(args.contents)).toMatchObject({ format: 'truongpaper-playlists', playlists: [{ playlistId: 'pl1' }] });
  });

  it('does nothing when the dialog is cancelled', async () => {
    saveMock.mockResolvedValueOnce(null);
    render(<PlaylistsView />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Save playlists to a file/ })); });
    expect(invokeMock.mock.calls.some(([cmd]) => cmd === 'playlist_export')).toBe(false);
  });
});
