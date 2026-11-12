// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
const invokeMock = vi.fn();
const openMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args), convertFileSrc: (p: string) => p }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: (opts: unknown) => openMock(opts) }));
import { LocalImport } from '../src/components/LocalImport';
import { useAppStore } from '../src/store/useAppStore';
import type { WallpaperItem } from '../src/types/ipc';

const item = { wallpaperId: 'local:clip-1234abcd', title: 'clip', type: 'video' } as unknown as WallpaperItem;
const initial = useAppStore.getState();
beforeEach(() => {
  useAppStore.setState(initial, true);
  invokeMock.mockReset();
  openMock.mockReset();
  invokeMock.mockResolvedValue(item);
});
afterEach(cleanup);

it('imports a chosen video and selects it', async () => {
  const status = vi.fn();
  openMock.mockResolvedValue('/Users/me/clip.mp4');
  render(<LocalImport onStatus={status} />);
  fireEvent.click(screen.getByRole('button', { name: /Import Folder or Video/ }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Video file' })); });
  expect(openMock).toHaveBeenCalledWith(expect.objectContaining({ directory: false, filters: [{ name: 'Video', extensions: ['mp4', 'webm', 'mov', 'mkv'] }] }));
  expect(invokeMock).toHaveBeenCalledWith('library_import_local', { path: '/Users/me/clip.mp4' });
  const state = useAppStore.getState();
  expect(state.selectedWallpaperId).toBe('local:clip-1234abcd');
  expect(state.wallpapers.map((w) => w.wallpaperId)).toContain('local:clip-1234abcd');
  expect(status).toHaveBeenLastCalledWith('Imported “clip”.');
});

it('opens a directory picker for folders and does nothing when cancelled', async () => {
  openMock.mockResolvedValue(null);
  render(<LocalImport />);
  fireEvent.click(screen.getByRole('button', { name: /Import Folder or Video/ }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Folder with project.json' })); });
  expect(openMock).toHaveBeenCalledWith(expect.objectContaining({ directory: true }));
  expect(invokeMock).not.toHaveBeenCalled();
});

it('reports import errors', async () => {
  const status = vi.fn();
  openMock.mockResolvedValue('/bad');
  invokeMock.mockRejectedValue('That folder has no project.json.');
  render(<LocalImport onStatus={status} />);
  fireEvent.click(screen.getByRole('button', { name: /Import Folder or Video/ }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Folder with project.json' })); });
  expect(status).toHaveBeenLastCalledWith('That folder has no project.json.');
});
