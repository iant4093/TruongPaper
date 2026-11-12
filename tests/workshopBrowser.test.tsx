// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args), convertFileSrc: (p: string) => p }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
import { WorkshopBrowser } from '../src/components/WorkshopBrowser';
import { WorkshopPreviewModal } from '../src/components/WorkshopPreviewModal';
import { ResourceUsage } from '../src/components/ResourceUsage';
import { useAppStore } from '../src/store/useAppStore';
import type { WorkshopBrowsePage } from '../src/types/ipc';
const item = { workshopId: '1234567890', title: 'Test mountain', author: 'Artist', subscribers: 2000, rating: 4, tags: ['Scene'], previewUrl: '', description: '', fileSizeBytes: 1, timeUpdated: 0 };
const page: WorkshopBrowsePage = { items: [item], page: 1, totalPages: 3, totalItems: 90, tagGroups: [{name:'Type',tags:[{tag:'Scene',label:'Scene'}]}] };
const initial = useAppStore.getState();
const expectLastBrowse = (args: unknown) => {
  const browses = invokeMock.mock.calls.filter(([cmd]) => cmd === 'steam_browse_workshop');
  expect(browses[browses.length - 1]).toEqual(['steam_browse_workshop', args]);
};
beforeEach(() => {
  useAppStore.setState(initial, true);
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: {query?: {page:number}}) => cmd === 'steam_browse_workshop' ? { ...page, page: args?.query?.page ?? 1 } : cmd === 'steam_inspect_url' ? item : {residentBytes: 32 * 1024 * 1024});
});
afterEach(() => { cleanup(); vi.useRealTimers(); });
it('browses, filters, and pages without a login or download', async () => {
  render(<WorkshopBrowser />);
  await screen.findByRole('button', {name:'Preview Test mountain'});
  expect(invokeMock.mock.calls.every(([cmd]) => cmd === 'steam_browse_workshop' || cmd === 'steam_guest_availability')).toBe(true);
  fireEvent.click(screen.getByRole('combobox', { name: 'Workshop type' }));
  await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'Scene' })); });
  expectLastBrowse({query:expect.objectContaining({page:1,tags:['Scene']})});
  await act(async () => { fireEvent.click(screen.getByLabelText('Next Workshop page')); });
  expectLastBrowse({query:expect.objectContaining({page:2})});
  fireEvent.click(screen.getByRole('combobox', { name: 'Workshop sort' }));
  await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'Newest' })); });
  expectLastBrowse({query:expect.objectContaining({page:1,sort:'newest'})});
});
it('previews a result and downloads only after the explicit button', async () => {
  render(<><WorkshopBrowser /><WorkshopPreviewModal /></>);
  const result = await screen.findByRole('button', {name:'Preview Test mountain'});
  await act(async () => { fireEvent.click(result); });
  expect(screen.getByRole('dialog', {name:'Workshop Preview'})).toBeTruthy();
  expect(invokeMock.mock.calls.some(([cmd]) => cmd === 'steam_download_workshop_item')).toBe(false);
  invokeMock.mockImplementation(async (cmd) => { if (cmd === 'steam_download_workshop_item') throw new Error('Steam login required'); return item; });
  await act(async () => { fireEvent.click(screen.getByRole('button', {name:'Download with Steam'})); });
  expect(useAppStore.getState().authModalOpen).toBe(true);
});
it('keeps a GGNetwork availability error in the preview', async () => {
  useAppStore.setState({ inspectedWorkshop: item });
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'steam_download_workshop_item') throw new Error('GGNetwork requires its own account for this wallpaper.');
    return {};
  });
  render(<WorkshopPreviewModal />);
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Download via GGNetwork' }));
  });
  expect(invokeMock).toHaveBeenCalledWith('steam_download_workshop_item', {
    urlOrId: item.workshopId, source: 'ggnetwork',
  });
  expect(useAppStore.getState().authModalOpen).toBe(false);
  expect(screen.getByRole('alert').textContent).toContain('GGNetwork requires its own account');
});
it('supports a direct Workshop link inside Discover and keyword search', async () => {
  render(<WorkshopBrowser />);
  const input = screen.getByLabelText('Search Steam Workshop');
  await screen.findByRole('button', {name:'Preview Test mountain'});
  fireEvent.change(input,{target:{value:'mountain'}});
  await act(async () => {fireEvent.submit(input.closest('form')!);});
  expectLastBrowse({query:expect.objectContaining({search:'mountain',page:1})});
  fireEvent.change(input,{target:{value:'https://steamcommunity.com/sharedfiles/filedetails/?id=1234567890'}});
  await act(async () => {fireEvent.submit(input.closest('form')!);});
  expect(invokeMock).toHaveBeenLastCalledWith('steam_inspect_url',{urlOrId:'1234567890'});
});
it('keeps newer catalog results when an older request resolves late', async () => {
  let resolveOld!: (value: WorkshopBrowsePage) => void;
  invokeMock.mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; }));
  render(<WorkshopBrowser />);
  const sort = screen.getByRole('combobox', { name: 'Workshop sort' });
  fireEvent.keyDown(sort, { key: 'n' });
  await act(async () => { fireEvent.keyDown(sort, { key: 'Enter' }); });
  await screen.findByRole('button',{name:'Preview Test mountain'});
  await act(async () => { resolveOld({...page,items:[{...item,title:'Stale item'}]}); });
  expect(screen.queryByRole('button',{name:'Preview Stale item'})).toBeNull();
});
it('does not reopen a preview closed while metadata is still loading', async () => {
  let resolve!: (value: typeof item) => void;
  invokeMock.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const pending = useAppStore.getState().inspectWorkshopUrl('1234567890');
  useAppStore.getState().closeWorkshopPreview();
  resolve(item); await pending;
  expect(useAppStore.getState().inspectedWorkshop).toBeNull();
});
it('reports offline errors and retries without showing fake catalog entries', async () => {
  invokeMock.mockRejectedValueOnce(new Error('Cannot reach Steam Workshop'));
  render(<WorkshopBrowser />);
  expect(await screen.findByRole('alert')).toBeTruthy();
  expect(screen.queryByRole('button',{name:'Preview Test mountain'})).toBeNull();
  await act(async () => { fireEvent.click(screen.getByRole('button',{name:'Retry'})); });
  expect(await screen.findByRole('button',{name:'Preview Test mountain'})).toBeTruthy();
});
it('shows actual process RAM separately from disk cache and stops sampling after unmount', async () => {
  vi.useFakeTimers();
  const view = render(<ResourceUsage cachedBytes={1024 * 1024} />);
  await act(async () => {});
  expect(screen.getByText('32.0 MB')).toBeTruthy();
  expect(screen.getByText('1.0 MB')).toBeTruthy();
  expect(screen.getByText('RAM').closest('[title]')?.getAttribute('title')).toContain('main app process');
  await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
  expect(invokeMock).toHaveBeenCalledTimes(2);
  view.unmount();
  await vi.advanceTimersByTimeAsync(6000);
  expect(invokeMock).toHaveBeenCalledTimes(2);
});

it('badges items GGNetwork has and disables its button when the mirror lacks the item', async () => {
  invokeMock.mockImplementation(async (cmd: string) => cmd === 'steam_browse_workshop' ? page
    : cmd === 'steam_guest_availability' ? { '1234567890': 'unavailable' } : cmd === 'steam_inspect_url' ? item : {});
  render(<><WorkshopBrowser /><WorkshopPreviewModal /></>);
  const result = await screen.findByRole('button', {name:'Preview Test mountain'});
  expect(invokeMock).toHaveBeenCalledWith('steam_guest_availability', { workshopIds: ['1234567890'] });
  expect(screen.queryByText('GGNetwork')).toBeNull();
  await act(async () => { fireEvent.click(result); });
  const button = await screen.findByRole('button', {name:'Not on GGNetwork'});
  expect((button as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText(/third-party mirror/)).toBeTruthy();
});

it('shows a GGNetwork badge on available results', async () => {
  invokeMock.mockImplementation(async (cmd: string) => cmd === 'steam_browse_workshop' ? page
    : cmd === 'steam_guest_availability' ? { '1234567890': 'available' } : {});
  render(<WorkshopBrowser />);
  await screen.findByRole('button', {name:'Preview Test mountain'});
  expect(await screen.findByText('GGNetwork')).toBeTruthy();
});
