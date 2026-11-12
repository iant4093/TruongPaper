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

describe('<WorkshopImport />', () => {
  const initial = useAppStore.getState();
  beforeEach(() => {
    invokeMock.mockReset();
    localStorage.clear();
    useAppStore.setState(initial, true);
    invokeMock.mockResolvedValue(null);
  });
  afterEach(cleanup);

  it('is only the import menu: no link field, no second search box', () => {
    render(<WorkshopImport />);
    expect(screen.getByRole('button', { name: /Import from computer/ })).toBeTruthy();
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('ignores pasted Workshop links and never opens the sign-in sheet on its own', async () => {
    render(<WorkshopImport />);
    const evt = new Event('paste', { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    evt.clipboardData = { getData: () => 'steam://url/CommunityFilePage/2841928371' };
    await act(async () => { window.dispatchEvent(evt); });
    expect(evt.defaultPrevented).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(useAppStore.getState().authModalOpen).toBe(false);
  });
});
