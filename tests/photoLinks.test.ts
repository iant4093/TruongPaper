// tests/photoLinks.test.ts — album names and album links shown for photos.
import { describe, expect, it } from 'vitest';
import { photoAlbumTitle, safeAlbumUrl } from '../src/lib/format';
import { CancelledError, toast, useFeedback } from '../src/store/feedback';

describe('photo album helpers', () => {
  it('prefers the real album title', () => {
    const photo = { albumSlug: 'day-6-dresden' };
    expect(photoAlbumTitle(photo, { 'day-6-dresden': 'Day 6 - Dresden' })).toBe('Day 6 - Dresden');
    expect(photoAlbumTitle(photo)).toBe('day 6 dresden');
  });

  it('only opens album links on the photography site', () => {
    const ok = { albumSlug: 'a', albumUrl: 'https://iantruongphotography.com/album/a' };
    expect(safeAlbumUrl(ok)).toBe('https://iantruongphotography.com/album/a');
    for (const albumUrl of ['https://evil.example/album/a', 'http://iantruongphotography.com/album/a', 'javascript:alert(1)', 'not a url', 'https://iantruongphotography.com.evil.example/']) {
      expect(safeAlbumUrl({ albumSlug: 'a b', albumUrl })).toBe('https://iantruongphotography.com/album/a%20b');
    }
  });

  it('does not report a declined confirmation as an error', () => {
    const before = useFeedback.getState().toasts.length;
    toast.error(new CancelledError());
    expect(useFeedback.getState().toasts.length).toBe(before);
  });
});
