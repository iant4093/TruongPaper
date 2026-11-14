// @vitest-environment jsdom
// tests/round12.test.tsx — download cancel, single-flight migration, light tray load, Escape layering.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/react';
import React, { useRef } from 'react';

const invokeMock = vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null);
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => undefined) }));

import { FIRST_RENDER_CAP, VirtualGrid } from '../src/components/VirtualGrid';
import { useModalFocus } from '../src/lib/useModalFocus';
import {
  isCancelMessage,
  isCancelledDownload,
  pruneDownloads,
  replaceWallpaper,
  useAppStore,
} from '../src/store/useAppStore';
import type { WallpaperItem } from '../src/types/ipc';

const initial = useAppStore.getState();
const item = (id: string, title = id) => ({ wallpaperId: id, title }) as unknown as WallpaperItem;

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async () => null);
  localStorage.clear();
  useAppStore.setState(initial, true);
});
afterEach(() => cleanup());

describe('download cancel', () => {
  it('treats a cancelled download as a cancel, not a failure', () => {
    expect(isCancelledDownload({ state: 'cancelled' })).toBe(true);
    expect(isCancelledDownload({ state: 'error', error: 'Download cancelled.' })).toBe(true);
    expect(isCancelledDownload({ state: 'error', error: 'disk full' })).toBe(false);
    expect(isCancelledDownload({ state: 'downloading' })).toBe(false);
    expect(isCancelMessage(new Error('Download cancelled.'))).toBe(true);
    expect(isCancelMessage('disk full')).toBe(false);
  });

  it('keeps unread failures when a new download starts', () => {
    const kept = pruneDownloads({ a: { state: 'error' }, b: { state: 'completed' }, c: { state: 'downloading' } });
    expect(Object.keys(kept).sort()).toEqual(['a', 'c']);
  });
});

describe('replaceWallpaper', () => {
  it('swaps only the named item', () => {
    const current = [item('a', 'edited'), item('b', 'old')];
    const out = replaceWallpaper(current, [item('a', 'server'), item('b', 'new')], 'b');
    expect(out.map((w) => w.title)).toEqual(['edited', 'new']);
    expect(replaceWallpaper(current, [], 'b')).toBe(current);
  });
});

describe('load', () => {
  it('light load skips playlists, albums, presets and the migration', async () => {
    invokeMock.mockImplementation(async (cmd) => (cmd === 'monitors_list' || cmd === 'library_list_wallpapers' ? [] : {}));
    await useAppStore.getState().refreshAll({ light: true });
    const cmds = invokeMock.mock.calls.map((c) => c[0]);
    expect(cmds).not.toContain('playlist_list');
    expect(cmds).not.toContain('photo_list_albums');
    expect(cmds).not.toContain('wallpaper_list_presets');
    expect(useAppStore.getState().loaded).toBe(true);
  });

  it('flags a failed load and clears it on the next success', async () => {
    invokeMock.mockImplementation(async (cmd) => { if (cmd === 'settings_get') throw new Error('boom'); return []; });
    await expect(useAppStore.getState().refreshAll()).rejects.toThrow('boom');
    expect(useAppStore.getState().loadFailed).toBe(true);
    invokeMock.mockImplementation(async () => []);
    await useAppStore.getState().refreshAll();
    expect(useAppStore.getState().loadFailed).toBe(false);
  });

  it('runs one album migration at a time and counts "not found" albums as done', async () => {
    localStorage.setItem('truongpaper.savedAlbums', JSON.stringify([{ albumSlug: 'gone' }, { albumSlug: 'ok' }]));
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => { release = r; });
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === 'photo_list_albums') return [];
      if (cmd === 'photo_add_album') {
        await gate;
        if ((args as { urlOrSlug: string }).urlOrSlug === 'gone') throw new Error('Album not found');
        return {};
      }
      return [];
    });
    const first = useAppStore.getState().refreshAll();
    const second = useAppStore.getState().refreshAll();
    await Promise.all([first, second]);
    release();
    await new Promise((r) => setTimeout(r, 20));
    const adds = invokeMock.mock.calls.filter((c) => c[0] === 'photo_add_album');
    expect(adds).toHaveLength(2); // not 4
    expect(localStorage.getItem('truongpaper.backendListsMigrated')).toBe('true');
  });
});

describe('VirtualGrid first render', () => {
  it('mounts only a first screenful before it is measured', () => {
    const items = Array.from({ length: 300 }, (_, i) => ({ id: `i${i}` }));
    const { container } = render(
      <VirtualGrid items={items} getKey={(i) => i.id} label="Big" renderItem={(i) => <div>{i.id}</div>} />,
    );
    expect(container.querySelectorAll('[data-grid-index]').length).toBeLessThanOrEqual(FIRST_RENDER_CAP);
  });
});

describe('Escape layering', () => {
  const Modal: React.FC<{ onClose: () => void }> = ({ onClose }) => {
    const ref = useRef<HTMLDivElement>(null);
    useModalFocus(true, ref, onClose);
    return <div ref={ref} role="dialog"><button type="button">ok</button></div>;
  };

  it('does not reach a document bubble listener (the drawer) when a modal handles Escape', () => {
    const onClose = vi.fn();
    const drawer = vi.fn();
    const listener = (e: KeyboardEvent) => { if (e.key === 'Escape' && !e.defaultPrevented) drawer(); };
    document.addEventListener('keydown', listener);
    const { getByText } = render(<Modal onClose={onClose} />);
    fireEvent.keyDown(getByText('ok'), { key: 'Escape' });
    document.removeEventListener('keydown', listener);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(drawer).not.toHaveBeenCalled();
  });

  it('keeps focus when the caller passes a new onClose each render', () => {
    const { getByText, rerender } = render(<Modal onClose={() => {}} />);
    const button = getByText('ok');
    expect(document.activeElement).toBe(button);
    button.blur();
    rerender(<Modal onClose={() => {}} />);
    expect(document.activeElement).not.toBe(button);
  });
});
