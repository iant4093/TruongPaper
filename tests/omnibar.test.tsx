// @vitest-environment jsdom
// Import link classification and Workshop preview dispatch.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

import { WorkshopImport } from '../src/components/WorkshopImport';
import { parseImportLink } from '../src/lib/importLinks';
import { useAppStore } from '../src/store/useAppStore';

describe('parseImportLink classification', () => {
  it.each([
    ['https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371', '2841928371'],
    ['https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371&searchtext=rain', '2841928371'],
    ['https://steamcommunity.com/workshop/filedetails/?id=1100550912', '1100550912'],
    ['steamcommunity.com/sharedfiles/filedetails?l=english&id=1966389027', '1966389027'],
    ['steam://url/CommunityFilePage/2841928371', '2841928371'],
    ['  2841928371  ', '2841928371'],
    ['1234567', '1234567'],
  ])('Steam Workshop: %s', (input, workshopId) => {
    expect(parseImportLink(input)).toEqual({ kind: 'steam_workshop', workshopId });
  });

  it.each([
    ['https://iantruongphotography.com/album/bald-mountain-11-1', 'bald-mountain-11-1'],
    ['https://www.iantruongphotography.com/album/Cape-Horn-11-7/', 'cape-horn-11-7'],
    ['iantruongphotography.com/album/mt-rainier-astro', 'mt-rainier-astro'],
    ['cape-horn-11-7', 'cape-horn-11-7'],
    ['Seattle-Street-Night', 'seattle-street-night'],
  ])('Photo album: %s', (input, albumSlug) => {
    expect(parseImportLink(input)).toEqual({ kind: 'photo_album', albumSlug });
  });

  // Includes a non-iantruongphotography host, which must never be treated as an album URL.
  it.each(['', '   ', 'hello world', '12345', 'singleword', 'https://example.com/album/foo-bar?x', 'steam://open/games'])(
    'rejects %j',
    (input) => {
      expect(parseImportLink(input)).toBeNull();
    }
  );
});

describe('<WorkshopImport /> dispatch', () => {
  const initial = useAppStore.getState();

  beforeEach(() => {
    invokeMock.mockReset();
    localStorage.clear();
    useAppStore.setState(initial, true);
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'steam_inspect_url') return { workshopId: '2841928371', title: 'Neon Rain', description: '', previewUrl: '', fileSizeBytes: 1, tags: [], timeUpdated: 0 };
      return null;
    });
  });
  afterEach(cleanup);

  const submit = async (value: string) => {
    render(<WorkshopImport />);
    const input = screen.getByRole('textbox', { name: 'Steam Workshop link or item ID' });
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

  it('keeps photo album imports out of Workshop', async () => {
    await submit('bald-mountain-11-1');
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByRole('status').textContent).toBe('Enter a Steam Workshop link or item ID.');
  });

  it('shows a hint for unrecognised input and never invokes', async () => {
    await submit('not a link');
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByText('Enter a Steam Workshop link or item ID.')).toBeTruthy();
  });

  it('opens the Steam sign-in sheet when the backend reports an auth error', async () => {
    invokeMock.mockImplementation(async () => {
      throw new Error('Steam login required');
    });
    await submit('2841928371');
    expect(useAppStore.getState().authModalOpen).toBe(true);
  });

  it('handles Cmd/Ctrl+V in Workshop when no input is focused', async () => {
    render(<WorkshopImport />);
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
