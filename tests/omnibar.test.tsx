// @vitest-environment jsdom
// tests/omnibar.test.tsx — Universal Omnibar paste classification & dispatch (§2.1 / §4.3)
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

import { UniversalOmnibar, parseOmnibarInput } from '../src/components/UniversalOmnibar';
import { useAppStore } from '../src/store/useAppStore';

describe('parseOmnibarInput classification', () => {
  it.each([
    ['https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371', '2841928371'],
    ['https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371&searchtext=rain', '2841928371'],
    ['https://steamcommunity.com/workshop/filedetails/?id=1100550912', '1100550912'],
    ['steamcommunity.com/sharedfiles/filedetails?l=english&id=1966389027', '1966389027'],
    ['steam://url/CommunityFilePage/2841928371', '2841928371'],
    ['  2841928371  ', '2841928371'],
    ['1234567', '1234567'],
  ])('Steam Workshop: %s', (input, workshopId) => {
    expect(parseOmnibarInput(input)).toEqual({ kind: 'steam_workshop', workshopId });
  });

  it.each([
    ['https://iantruongphotography.com/album/bald-mountain-11-1', 'bald-mountain-11-1'],
    ['https://www.iantruongphotography.com/album/Cape-Horn-11-7/', 'cape-horn-11-7'],
    ['iantruongphotography.com/album/mt-rainier-astro', 'mt-rainier-astro'],
    ['cape-horn-11-7', 'cape-horn-11-7'],
    ['Seattle-Street-Night', 'seattle-street-night'],
  ])('Photo album: %s', (input, albumSlug) => {
    expect(parseOmnibarInput(input)).toEqual({ kind: 'photo_album', albumSlug });
  });

  // Includes a non-iantruongphotography host, which must never be treated as an album URL.
  it.each(['', '   ', 'hello world', '12345', 'singleword', 'https://example.com/album/foo-bar?x', 'steam://open/games'])(
    'rejects %j',
    (input) => {
      expect(parseOmnibarInput(input)).toBeNull();
    }
  );
});

describe('<UniversalOmnibar /> dispatch', () => {
  const initial = useAppStore.getState();

  beforeEach(() => {
    invokeMock.mockReset();
    localStorage.clear();
    useAppStore.setState(initial, true);
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'steam_inspect_url') return { workshopId: '2841928371', title: 'Neon Rain', description: '', previewUrl: '', fileSizeBytes: 1, tags: [], timeUpdated: 0 };
      if (cmd === 'steam_download_workshop_item') return 'workshop:2841928371';
      if (cmd === 'photo_add_album') return { albumSlug: args?.urlOrSlug, displayName: 'Bald Mountain', albumUrl: '', isEnabled: true, addedAt: 0 };
      return null;
    });
  });
  afterEach(cleanup);

  const submit = async (value: string) => {
    render(<UniversalOmnibar />);
    const input = screen.getByPlaceholderText(/Paste Steam Workshop URL/);
    fireEvent.change(input, { target: { value } });
    await act(async () => {
      fireEvent.submit(input.closest('form')!);
    });
  };

  it('opens a preview from a direct Workshop URL so the user can choose the source', async () => {
    await submit('https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371');
    expect(invokeMock.mock.calls).toEqual([
      ['steam_inspect_url', { urlOrId: '2841928371' }],
    ]);
    expect(useAppStore.getState().inspectedWorkshop?.title).toBe('Neon Rain');
  });

  it('adds a photo album slug', async () => {
    await submit('bald-mountain-11-1');
    expect(invokeMock).toHaveBeenCalledWith('photo_add_album', { urlOrSlug: 'bald-mountain-11-1', displayName: undefined });
    expect(await screen.findByText('Added album "Bald Mountain" to Saved Albums')).toBeTruthy();
  });

  it('shows a hint for unrecognised input and never invokes', async () => {
    await submit('not a link');
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByText(/Paste a valid Steam Workshop URL/)).toBeTruthy();
  });

  it('opens the Steam sign-in sheet when the backend reports an auth error', async () => {
    invokeMock.mockImplementation(async () => {
      throw new Error('Steam login required');
    });
    await submit('2841928371');
    expect(useAppStore.getState().authModalOpen).toBe(true);
  });

  it('handles Cmd/Ctrl+V anywhere when no input is focused', async () => {
    render(<UniversalOmnibar />);
    (document.activeElement as HTMLElement | null)?.blur();
    const evt = new Event('paste', { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    evt.clipboardData = { getData: () => 'steam://url/CommunityFilePage/2841928371' };
    await act(async () => {
      window.dispatchEvent(evt);
    });
    expect(evt.defaultPrevented).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith('steam_inspect_url', { urlOrId: '2841928371' });
  });
});
