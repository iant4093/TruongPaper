// Photo caption settings: one default for every display, plus displays with their own choice.
import type { AppSettings, PhotoCaptionParts } from '../types/ipc';

/** The caption scope that edits the default every display follows. */
export const ALL_DISPLAYS = '';

type CaptionSettings = Pick<AppSettings, 'photoShowTime' | 'photoShowAlbum' | 'photoShowExif' | 'photoCaptionByMonitor'>;

export const defaultCaption = (s: CaptionSettings | null | undefined): PhotoCaptionParts => ({
  time: s?.photoShowTime ?? false,
  album: s?.photoShowAlbum ?? false,
  exif: s?.photoShowExif ?? false,
});

/** Displays with their own caption choice. */
export const captionOverrides = (s: CaptionSettings | null | undefined): Record<string, PhotoCaptionParts> =>
  s?.photoCaptionByMonitor ?? {};

export const hasOwnCaption = (s: CaptionSettings | null | undefined, monitorId: string): boolean =>
  monitorId !== ALL_DISPLAYS && monitorId in captionOverrides(s);

/** What `monitorId` shows (`ALL_DISPLAYS`: the default). */
export const captionFor = (s: CaptionSettings | null | undefined, monitorId: string): PhotoCaptionParts =>
  (monitorId !== ALL_DISPLAYS && captionOverrides(s)[monitorId]) || defaultCaption(s);

/** Settings patch that sets one part of the caption for `monitorId` (or the default). */
export const captionPatch = (
  s: CaptionSettings | null | undefined,
  monitorId: string,
  part: keyof PhotoCaptionParts,
  on: boolean,
): Partial<AppSettings> => {
  if (monitorId === ALL_DISPLAYS) {
    const key = ({ time: 'photoShowTime', album: 'photoShowAlbum', exif: 'photoShowExif' } as const)[part];
    return { [key]: on };
  }
  const parts = { ...captionFor(s, monitorId), [part]: on };
  return { photoCaptionByMonitor: { ...captionOverrides(s), [monitorId]: parts } };
};

/** Settings patch that makes `monitorId` follow the default again. */
export const followDefaultCaptionPatch = (
  s: CaptionSettings | null | undefined,
  monitorId: string,
): Partial<AppSettings> => {
  const { [monitorId]: _removed, ...rest } = captionOverrides(s);
  return { photoCaptionByMonitor: rest };
};
