// @vitest-environment jsdom
// tests/libraryDownloads.test.tsx — Workshop downloads appear in the Library, grayed out with a
// progress bar, and can be cancelled, retried or dismissed there (issue #5).
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { installBrowserPreviewMocksIfNeeded } from '../src/dev/mockTauri';
import { App } from '../src/App';
import { DownloadCard } from '../src/components/WallpaperCard';
import { useAppStore } from '../src/store/useAppStore';

const initial = useAppStore.getState();

beforeAll(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    () => new Proxy({}, { get: (_t, prop) => (prop === 'createRadialGradient' ? () => ({ addColorStop() {} }) : () => {}) }) as never
  );
  installBrowserPreviewMocksIfNeeded();
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  useAppStore.setState(initial, true);
});

const openApp = async () => {
  await act(async () => { render(<App />); });
  await screen.findByRole('button', { name: 'Details for Neon Rain City' });
};

describe('Library downloads', () => {
  it('shows a running download as a grayed-out card with its progress', async () => {
    await openApp();
    act(() => {
      useAppStore.setState({
        downloadsById: {
          '123': { workshopId: '123', title: 'Aurora Peaks', progressPercent: 42.6, state: 'downloading', previewUrl: 'https://example.com/p.jpg' },
          '456': { workshopId: '456', title: 'Done Already', progressPercent: 100, state: 'completed' },
        },
      });
    });
    const bar = await screen.findByRole('progressbar', { name: 'Downloading Aurora Peaks' });
    expect(bar.getAttribute('aria-valuenow')).toBe('43');
    const card = bar.closest('article')!;
    expect(card.className).toContain('is-downloading');
    expect(within(card).getByText('Downloading 43%')).toBeTruthy();
    expect(card.querySelector('img')?.getAttribute('src')).toBe('https://example.com/p.jpg');
    // Finished downloads are already library items, not download cards.
    expect(screen.queryByText('Done Already')).toBeNull();
    // A download card cannot be set as the wallpaper.
    expect(within(card).queryByRole('button', { name: /Set as wallpaper/ })).toBeNull();
  });

  it('hides download cards outside the All filter and when the search does not match', async () => {
    await openApp();
    act(() => {
      useAppStore.setState({ downloadsById: { '123': { workshopId: '123', title: 'Aurora Peaks', progressPercent: 5, state: 'downloading' } } });
    });
    expect(await screen.findByText('Aurora Peaks')).toBeTruthy();
    await act(async () => { fireEvent.change(screen.getByRole('textbox', { name: 'Search library' }), { target: { value: 'neon' } }); });
    await vi.waitFor(() => expect(screen.queryByText('Aurora Peaks')).toBeNull());
    await act(async () => { fireEvent.change(screen.getByRole('textbox', { name: 'Search library' }), { target: { value: 'auro' } }); });
    expect(await screen.findByText('Aurora Peaks')).toBeTruthy();
  });

  it('a failed download offers Retry and Dismiss', async () => {
    await openApp();
    act(() => {
      useAppStore.setState({ downloadsById: { '123': { workshopId: '123', title: 'Aurora Peaks', progressPercent: 30, state: 'error', error: 'No space left' } } });
    });
    const card = (await screen.findByText('Aurora Peaks')).closest('article')!;
    expect(card.className).toContain('is-failed');
    expect(within(card).getByText('No space left')).toBeTruthy();
    await act(async () => { fireEvent.click(within(card).getByRole('button', { name: 'Dismiss' })); });
    expect(screen.queryByText('Aurora Peaks')).toBeNull();
    expect(useAppStore.getState().downloadsById['123']).toBeUndefined();
  });
});

describe('DownloadCard', () => {
  it('cancels once and shows Cancelling…', () => {
    const onCancel = vi.fn();
    const { rerender } = render(<DownloadCard workshopId="9" title="T" preview={null} percent={10}
      onCancel={onCancel} onRetry={vi.fn()} onDismiss={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel download: T' }));
    expect(onCancel).toHaveBeenCalledWith('9');
    rerender(<DownloadCard workshopId="9" title="T" preview={null} percent={10} cancelling
      onCancel={onCancel} onRetry={vi.fn()} onDismiss={vi.fn()} />);
    const button = screen.getByRole('button', { name: 'Cancel download: T' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.textContent).toBe('Cancelling…');
  });
});

describe('download previews', () => {
  it('keeps the preview image of the item opened in the preview dialog', async () => {
    await openApp();
    let previewUrl = '';
    await act(async () => { previewUrl = (await useAppStore.getState().inspectWorkshopUrl('3800000777')).previewUrl; });
    expect(previewUrl).toBeTruthy();
    await act(async () => {
      await useAppStore.getState().downloadWorkshopItem('3800000777');
      useAppStore.getState().closeWorkshopPreview();
    });
    // The browser-preview mock emits progress events for the download.
    await vi.waitFor(() => expect(useAppStore.getState().downloadsById['3800000777']?.previewUrl).toBe(previewUrl));
    const bar = await screen.findByRole('progressbar', { name: /^Downloading / });
    expect(bar.closest('article')!.querySelector('img')).toBeTruthy();
  });
});
