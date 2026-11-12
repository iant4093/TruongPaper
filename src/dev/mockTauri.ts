// src/dev/mockTauri.ts
// Browser-preview mock (docs/06 §7.2). Activates automatically when the UI is
// opened at http://localhost:1420 without the Tauri shell and implements every
// §5.1 command with in-memory sample data so the whole studio is interactive.
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import type {
  AppSettings,
  FilterMode,
  IpcCommand,
  MonitorInfo,
  PhotoItem,
  PhotoStatus,
  Playlist,
  ProjectJsonSchema,
  PublicCatalogAlbum,
  SavedAlbum,
  UniversalAdjustments,
  WallpaperItem,
  WallpaperPreset,
  WallpaperSource,
  WorkshopPreview,
  WorkshopBrowseQuery,
} from '../types/ipc';

type Args = Record<string, unknown>;
type Emitter = (event: string, payload: unknown) => void;

const MB = 1024 * 1024;

/** Inline SVG gradient thumbnails so the preview works fully offline. */
function svgThumb(a: string, b: string, label: string, w = 640, h = 400): string {
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/>` +
    `<stop offset="1" stop-color="${b}"/></linearGradient></defs>` +
    `<rect width="100%" height="100%" fill="url(#g)"/>` +
    `<circle cx="${w * 0.72}" cy="${h * 0.3}" r="${h * 0.16}" fill="rgba(255,255,255,0.35)"/>` +
    `<path d="M0 ${h * 0.78} L${w * 0.3} ${h * 0.45} L${w * 0.55} ${h * 0.7} L${w * 0.78} ${h * 0.5} L${w} ${h * 0.75} L${w} ${h} L0 ${h} Z" fill="rgba(9,11,18,0.55)"/>` +
    `<text x="24" y="${h - 24}" font-family="sans-serif" font-size="22" fill="rgba(255,255,255,0.85)">${label}</text>` +
    `</svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

const DEFAULT_ADJUSTMENTS: UniversalAdjustments = {
  volume: 0.65,
  muted: false,
  audioSensitivity: 1.0,
  playbackSpeed: 1.0,
  fitMode: 'fill',
  zoom: 1.0,
  offsetX: 0,
  offsetY: 0,
  brightness: 0,
  contrast: 1,
  saturation: 1,
  hueShift: 0,
};

const PHOTO_PALETTE: Array<[string, string, string]> = [
  ['#f97316', '#7c3aed', 'bald-mountain-11-1'],
  ['#0ea5e9', '#1e3a8a', 'cape-horn-11-7'],
  ['#22c55e', '#0f172a', 'mt-rainier-astro'],
  ['#eab308', '#be123c', 'seattle-street-night'],
  ['#38bdf8', '#312e81', 'bald-mountain-11-1'],
  ['#f43f5e', '#1e1b4b', 'husky-stadium-sports'],
];

/** Builds a pseudo QR-code matrix with the three finder patterns. */
export function buildDemoQrMatrix(size = 29, seed = 7): boolean[][] {
  let s = seed;
  const rand = () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return s / 0x7fffffff;
  };
  const m = Array.from({ length: size }, () => Array.from({ length: size }, () => rand() > 0.55));
  const finder = (r0: number, c0: number) => {
    for (let r = -1; r <= 7; r++) {
      for (let c = -1; c <= 7; c++) {
        const rr = r0 + r;
        const cc = c0 + c;
        if (rr < 0 || cc < 0 || rr >= size || cc >= size) continue;
        const ring = Math.max(Math.abs(r - 3), Math.abs(c - 3));
        m[rr][cc] = r >= 0 && r <= 6 && c >= 0 && c <= 6 && ring !== 2 && ring !== 4;
      }
    }
  };
  finder(0, 0);
  finder(0, size - 7);
  finder(size - 7, 0);
  return m;
}

function sceneProject(): ProjectJsonSchema {
  return {
    title: 'Neon Rain City',
    type: 'scene',
    file: 'scene.json',
    preview: 'preview.gif',
    description: 'Rainy cyberpunk skyline with audio-reactive neon signs.',
    tags: ['Scene', 'Cyberpunk', 'Audio Responsive'],
    workshopid: '2841928371',
    general: {
      supportsaudioprocessing: true,
      localization: {
        'en-us': {
          ui_browse_properties_scheme_color: 'Scheme Color',
          ui_browse_properties_weather: 'Weather',
          ui_browse_properties_rain_speed: 'Rain Speed',
          ui_weather_clear: 'Clear',
          ui_weather_rain: 'Rain',
          ui_weather_storm: 'Storm',
        },
      },
      properties: {
        group_weather: { type: 'group', text: '<b>Weather</b>', order: 0 },
        weather_mode: {
          type: 'combo',
          text: 'ui_browse_properties_weather',
          order: 1,
          value: 1,
          options: [
            { label: 'ui_weather_clear', value: 0 },
            { label: 'ui_weather_rain', value: 1 },
            { label: 'ui_weather_storm', value: 2 },
          ],
        },
        enable_rain: { type: 'bool', text: 'Enable Rain', order: 2, value: true },
        rain_speed: {
          type: 'slider',
          text: 'ui_browse_properties_rain_speed',
          order: 3,
          min: 0,
          max: 2,
          fraction: true,
          value: 1.2,
          condition: 'enable_rain.value == true && weather_mode.value != 0',
        },
        group_look: { type: 'group', text: 'Look', order: 10 },
        schemecolor: { type: 'color', text: 'ui_browse_properties_scheme_color', order: 11, value: '0.14902 0.45098 0.85098' },
        show_clock: { type: 'bool', text: 'Show Clock', order: 12, value: true },
        clock_label: { type: 'text', text: 'Clock Caption', order: 13, value: 'Seattle', condition: 'show_clock.value' },
        custom_logo: { type: 'file', text: 'Custom Logo', order: 14, value: '' },
      },
    },
  };
}

function initialWallpapers(): WallpaperItem[] {
  const scene = sceneProject();
  return [
    {
      wallpaperId: 'workshop:2841928371',
      workshopId: '2841928371',
      title: scene.title,
      type: 'scene',
      previewPath: svgThumb('#7c3aed', '#0ea5e9', 'Neon Rain City'),
      installDir: '/Users/demo/Library/Application Support/Steam/steamapps/workshop/content/431960/2841928371',
      bytes: 160 * MB,
      audioReactive: true,
      windowsOnly: false,
      projectJson: scene,
      userProperties: { weather_mode: 1, enable_rain: true, rain_speed: 1.2, schemecolor: '0.14902 0.45098 0.85098', show_clock: true, clock_label: 'Seattle' },
      adjustments: { ...DEFAULT_ADJUSTMENTS },
    },
    {
      wallpaperId: 'workshop:1966389027',
      workshopId: '1966389027',
      title: 'Lo-fi Train Window',
      type: 'video',
      previewPath: svgThumb('#f59e0b', '#9d174d', 'Lo-fi Train Window'),
      installDir: '/mock/TruongPaper/workshop/1966389027',
      bytes: 84 * MB,
      audioReactive: false,
      windowsOnly: false,
      projectJson: { title: 'Lo-fi Train Window', type: 'video', file: 'train.mp4', tags: ['Video', 'Relaxing'], general: { properties: {} } },
      userProperties: {},
      adjustments: { ...DEFAULT_ADJUSTMENTS, volume: 0.3 },
    },
    {
      wallpaperId: 'workshop:2504133121',
      workshopId: '2504133121',
      title: 'Audio Orb Visualizer',
      type: 'web',
      previewPath: svgThumb('#22d3ee', '#4c1d95', 'Audio Orb'),
      installDir: '/mock/TruongPaper/workshop/2504133121',
      bytes: 12 * MB,
      audioReactive: true,
      windowsOnly: false,
      projectJson: {
        title: 'Audio Orb Visualizer',
        type: 'web',
        file: 'index.html',
        tags: ['Web', 'Audio Responsive'],
        general: {
          supportsaudioprocessing: true,
          properties: {
            orb_size: { type: 'slider', text: 'Orb Size', min: 10, max: 100, value: 55, order: 1 },
            orb_color: { type: 'color', text: 'Orb Color', value: '0.2 0.8 1.0', order: 2 },
          },
        },
      },
      userProperties: { orb_size: 55, orb_color: '0.2 0.8 1.0' },
      adjustments: { ...DEFAULT_ADJUSTMENTS },
    },
    {
      wallpaperId: 'workshop:1100550912',
      workshopId: '1100550912',
      title: 'Desktop Aquarium (EXE)',
      type: 'application',
      previewPath: svgThumb('#14b8a6', '#082f49', 'Desktop Aquarium'),
      installDir: '/mock/TruongPaper/workshop/1100550912',
      bytes: 210 * MB,
      audioReactive: false,
      windowsOnly: true,
      projectJson: { title: 'Desktop Aquarium (EXE)', type: 'application', file: 'aquarium.exe', tags: ['Application'], general: { properties: {} } },
      userProperties: {},
      adjustments: { ...DEFAULT_ADJUSTMENTS },
    },
  ];
}

export interface MockBackend {
  handle: (cmd: string, args?: Args) => unknown;
  /** Snapshot accessors for tests. */
  state: () => {
    photoStatus: PhotoStatus;
    savedAlbums: SavedAlbum[];
    wallpapers: WallpaperItem[];
    monitors: MonitorInfo[];
    playlists: Playlist[];
    settings: AppSettings;
  };
}

/**
 * Pure in-memory implementation of every §5.1 command. `emitEvent` receives the
 * Rust → frontend events the real backend would send; `schedule` controls timing
 * of simulated async work (download progress, QR codes).
 */
export function createMockBackend(
  emitEvent: Emitter = () => {},
  schedule: (fn: () => void, ms: number) => void = (fn, ms) => void setTimeout(fn, ms)
): MockBackend {
  let photoSeq = 0;
  const makePhoto = (overrides: Partial<PhotoItem> = {}): PhotoItem => {
    const [a, b, slug] = PHOTO_PALETTE[photoSeq % PHOTO_PALETTE.length];
    const portrait = photoSeq % 4 === 3;
    photoSeq += 1;
    const id = `demo-photo-${photoSeq.toString().padStart(4, '0')}`;
    return {
      id,
      albumSlug: slug,
      albumUrl: `https://iantruongphotography.com/album/${slug}`,
      filePath: svgThumb(a, b, slug, portrait ? 400 : 640, portrait ? 640 : 400),
      width: portrait ? 4000 : 6000,
      height: portrait ? 6000 : 4000,
      bytes: Math.round((12 + (photoSeq % 7) * 1.4) * MB),
      featured: photoSeq % 2 === 1,
      isFavorite: false,
      downloadedAt: Date.now() - (10 - photoSeq) * 60_000,
      lastShownAt: null,
      ...overrides,
    };
  };

  let settings: AppSettings = {
    rotationIntervalSeconds: 1800,
    transitionType: 'liquid_glass',
    transitionDurationSeconds: 1.2,
    skipMismatchedOrientation: true,
    defaultFitMode: 'fill',
    exhaustionAutoReset: true,
    maxPhotoCacheBytes: 250 * MB,
    fpsCap: 60,
    qualityPreset: 'high',
    pauseOnFullscreen: true,
    pauseOnMaximized: false,
    batteryPolicy: 'pause_animations',
    launchAtLogin: true,
    syncLockScreen: false,
  };

  const current1 = makePhoto({ isFavorite: true, lastShownAt: Date.now() - 300_000 });
  const current2 = makePhoto({ lastShownAt: Date.now() - 200_000 });
  const favorites = new Map([[current1.id, current1]]);
  let queue: PhotoItem[] = Array.from({ length: 5 }, () => makePhoto());
  let currentByMonitor: Record<string, PhotoItem | null> = { 'DISPLAY-1': current1, 'DISPLAY-2': current2 };
  let filterMode: FilterMode = 'featured';
  let featuredOnly = true;
  let historyCount = 14;
  let blacklistCount = 0;
  let isRefilling = false;
  let rateLimitRemaining = 98;
  let cachedBytes = 92 * MB;

  const catalog: PublicCatalogAlbum[] = [
    ['bald-mountain-11-1', 'Bald Mountain 11/1', 'Landscape', 41, '#f97316', '#7c3aed'],
    ['cape-horn-11-7', 'Cape Horn 11/7', 'Landscape', 1, '#0ea5e9', '#1e3a8a'],
    ['seattle-street-night', 'Seattle Street Night', 'Street', 27, '#eab308', '#be123c'],
    ['husky-stadium-sports', 'Husky Stadium', 'Sports', 64, '#f43f5e', '#1e1b4b'],
    ['mt-rainier-astro', 'Mt. Rainier Milky Way', 'Astro', 18, '#22c55e', '#0f172a'],
  ].map(([slug, title, category, count, a, b], i) => ({
    albumId: `demo-album-${i + 1}`,
    type: 'photo' as const,
    title: String(title),
    description: `${title} — public album on iantruongphotography.com`,
    category: String(category),
    imageCount: Number(count),
    coverImageUrl: svgThumb(String(a), String(b), String(title), 1280, 800),
    coverThumbnailUrl: svgThumb(String(a), String(b), String(title)),
    coverBlurhash: 'LGF5]+Yk^6#M@-5c,1J5@[or[Q6.',
    slug: String(slug),
  }));

  let savedAlbums: SavedAlbum[] = [
    {
      albumSlug: 'bald-mountain-11-1',
      displayName: 'Bald Mountain 11/1',
      albumUrl: 'https://iantruongphotography.com/album/bald-mountain-11-1',
      isEnabled: true,
      coverThumbnailUrl: catalog[0].coverThumbnailUrl,
      imageCount: 41,
      addedAt: Date.now() - 86400_000,
    },
  ];

  let wallpapers = initialWallpapers();
  let monitors: MonitorInfo[] = [
    {
      monitorId: 'DISPLAY-1',
      name: 'Monitor 1: 27" 4K',
      width: 3840,
      height: 2160,
      scaleFactor: 2.0,
      refreshRateHz: 60,
      isPrimary: true,
      isPortrait: false,
      userPaused: false,
      activeSource: { kind: 'photo_api' },
      activeWallpaperId: null,
    },
    {
      monitorId: 'DISPLAY-2',
      name: 'Monitor 2: 24" Vertical',
      width: 1440,
      height: 2560,
      scaleFactor: 1.0,
      refreshRateHz: 75,
      isPrimary: false,
      isPortrait: true,
      userPaused: false,
      activeSource: { kind: 'workshop', wallpaperId: 'workshop:2841928371' },
      activeWallpaperId: 'workshop:2841928371',
    },
  ];
  let playlists: Playlist[] = [
    {
      playlistId: 'demo-playlist-1',
      name: 'Evening Mix',
      mode: 'mixed',
      intervalSeconds: 900,
      items: [{ kind: 'photo_api_draw' }, { kind: 'workshop', wallpaperId: 'workshop:2841928371' }, { kind: 'photo_api_draw' }],
    },
  ];
  const presets: WallpaperPreset[] = [];

  const photoStatus = (): PhotoStatus => ({
    filterMode,
    featuredOnly: filterMode === 'featured' || (filterMode === 'albums' && featuredOnly),
    unusedQueue: queue,
    favoritePhotos: [...favorites.values()],
    currentPhotoByMonitor: currentByMonitor,
    cachedBytes,
    maxCacheBytes: settings.maxPhotoCacheBytes,
    historyCount,
    blacklistCount,
    isRefilling,
    rateLimitRemaining,
  });

  const updatePhoto = (id: string, patch: Partial<PhotoItem>) => {
    queue = queue.map((p) => (p.id === id ? { ...p, ...patch } : p));
    currentByMonitor = Object.fromEntries(
      Object.entries(currentByMonitor).map(([k, p]) => [k, p && p.id === id ? { ...p, ...patch } : p])
    );
  };

  const refillQueue = () => {
    while (queue.length < 5) {
      const p = makePhoto();
      queue = [...queue, p];
      cachedBytes += p.bytes;
      rateLimitRemaining = Math.max(0, rateLimitRemaining - 1);
    }
  };

  const rotate = (monitorId: string): PhotoItem => {
    if (queue.length === 0) refillQueue();
    const [next, ...rest] = queue;
    queue = rest;
    const shown = { ...next, lastShownAt: Date.now() };
    currentByMonitor = { ...currentByMonitor, [monitorId]: shown };
    historyCount += 1;
    const source = monitors.find((m) => m.monitorId === monitorId)?.activeSource ?? { kind: 'photo_api' };
    emitEvent('wallpaper://changed', { monitorId, source, currentItem: shown, transitionUsed: settings.transitionType });
    // Background refill once the queue drops to <= 2 (docs/05 §4).
    if (queue.length <= 2) {
      isRefilling = true;
      schedule(() => {
        refillQueue();
        isRefilling = false;
        emitEvent('photo-queue://updated', photoStatus());
      }, 800);
    }
    emitEvent('photo-queue://updated', photoStatus());
    return shown;
  };

  const parseWorkshopId = (urlOrId: unknown): string => {
    const raw = String(urlOrId ?? '');
    const m = raw.match(/(\d{6,12})/);
    if (!m) throw new Error(`Not a Steam Workshop URL or ID: ${raw}`);
    return m[1];
  };

  const previewFor = (workshopId: string): WorkshopPreview => {
    const existing = wallpapers.find((w) => w.workshopId === workshopId);
    return {
      workshopId,
      title: existing?.title ?? `Workshop Wallpaper #${workshopId}`,
      description: existing?.projectJson.description ?? 'A Wallpaper Engine Workshop item (browser preview data).',
      previewUrl: existing?.previewPath ?? svgThumb('#6366f1', '#0f172a', `#${workshopId}`),
      fileSizeBytes: existing?.bytes ?? 160 * MB,
      tags: existing?.projectJson.tags ?? ['Scene'],
      timeUpdated: Math.floor(Date.now() / 1000) - 86400 * 3,
    };
  };

  const findWallpaper = (wallpaperId: unknown) => {
    const w = wallpapers.find((x) => x.wallpaperId === wallpaperId);
    if (!w) throw new Error(`Unknown wallpaper: ${String(wallpaperId)}`);
    return w;
  };

  const handlers: Record<IpcCommand, (args: Args) => unknown> = {
    // --- Photo API & Cache ---
    photo_get_status: () => photoStatus(),
    photo_fetch_public_catalog: () => catalog,
    photo_rotate_next: (args) => rotate(String(args.monitorId ?? monitors[0].monitorId)),
    photo_trigger_refill: () => {
      isRefilling = true;
      emitEvent('photo-queue://updated', photoStatus());
      schedule(() => {
        refillQueue();
        isRefilling = false;
        emitEvent('photo-queue://updated', photoStatus());
      }, 600);
      return null;
    },
    photo_add_album: (args) => {
      const raw = String(args.urlOrSlug ?? '').trim();
      const slug = raw.replace(/^.*\/album\//, '').replace(/[/?#].*$/, '').toLowerCase();
      if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug)) throw new Error(`Invalid album slug: ${raw}`);
      const known = catalog.find((c) => c.slug === slug);
      const created: SavedAlbum = {
        albumSlug: slug,
        displayName: args.displayName ? String(args.displayName) : known?.title ?? slug,
        albumUrl: `https://iantruongphotography.com/album/${slug}`,
        isEnabled: true,
        coverThumbnailUrl: known?.coverThumbnailUrl,
        imageCount: known?.imageCount,
        addedAt: Date.now(),
      };
      savedAlbums = [...savedAlbums.filter((a) => a.albumSlug !== slug), created];
      return created;
    },
    photo_toggle_album: (args) => {
      savedAlbums = savedAlbums.map((a) => (a.albumSlug === args.albumSlug ? { ...a, isEnabled: Boolean(args.enabled) } : a));
      return null;
    },
    photo_remove_album: (args) => {
      savedAlbums = savedAlbums.filter((a) => a.albumSlug !== args.albumSlug);
      return null;
    },
    photo_set_filter: (args) => {
      filterMode = (args.mode as FilterMode) ?? 'all';
      featuredOnly = Boolean(args.featuredOnly);
      return null;
    },
    photo_set_favorite: (args) => {
      const id = String(args.photoId);
      const favorite = Boolean(args.favorite);
      const photo = favorites.get(id) ?? queue.find((p) => p.id === id) ??
        Object.values(currentByMonitor).find((p) => p?.id === id);
      if (photo && favorite) favorites.set(id, { ...photo, isFavorite: true });
      else favorites.delete(id);
      updatePhoto(id, { isFavorite: favorite });
      emitEvent('photo-queue://updated', photoStatus());
      return null;
    },
    photo_apply_favorite: (args) => {
      const photo = favorites.get(String(args.photoId));
      const monitorId = String(args.monitorId);
      if (!photo) throw new Error('This favorite is no longer available on disk');
      const shown = { ...photo, lastShownAt: Date.now() };
      const source: WallpaperSource = { kind: 'photo_api' };
      currentByMonitor = { ...currentByMonitor, [monitorId]: shown };
      monitors = monitors.map((m) => m.monitorId === monitorId
        ? { ...m, activeSource: source, activeWallpaperId: null }
        : m);
      queue = queue.filter((p) => p.id !== photo.id);
      favorites.set(photo.id, shown);
      emitEvent('wallpaper://changed', { monitorId, source, currentItem: shown, transitionUsed: settings.transitionType });
      emitEvent('photo-queue://updated', photoStatus());
      return null;
    },
    photo_blacklist_and_skip: (args) => {
      const id = String(args.photoId);
      blacklistCount += 1;
      favorites.delete(id);
      queue = queue.filter((p) => p.id !== id);
      for (const [monitorId, p] of Object.entries(currentByMonitor)) {
        if (p?.id === id) rotate(monitorId);
      }
      return null;
    },
    photo_set_cache_limit: (args) => {
      settings = { ...settings, maxPhotoCacheBytes: Number(args.maxBytes) };
      return null;
    },
    photo_clear_cache: () => {
      queue = queue.filter((p) => p.isFavorite);
      const kept = [...favorites.values(), ...Object.values(currentByMonitor)].filter((p): p is PhotoItem => !!p);
      cachedBytes = [...new Map(kept.map((p) => [p.id, p])).values()].reduce((sum, p) => sum + p.bytes, 0);
      return null;
    },
    photo_reset_history: () => {
      historyCount = 0;
      return null;
    },

    // --- Steam Workshop & Wallpaper Engine ---
    steam_browse_workshop: (args) => {
      const query = (args.query ?? {}) as Partial<WorkshopBrowseQuery>;
      const page = query.page ?? 1;
      const samples = [
        { ...previewFor('3800000001'), title: 'Quiet mountain lake', tags: ['Scene', 'Nature', 'Wallpaper'], author: 'Landscape Studio', subscribers: 14250, rating: 5 },
        { ...previewFor('3800000002'), title: 'City after dark', tags: ['Video', 'City', 'Wallpaper'], author: 'Nightfall', subscribers: 9600, rating: 4 },
        { ...previewFor('3800000003'), title: 'Minimal orbit', tags: ['Web', 'Abstract', 'Wallpaper'], author: 'Orbit', subscribers: 4100, rating: 4 },
      ];
      const items = samples.filter((item) => (!query.search || item.title.toLowerCase().includes(query.search.toLowerCase())) && (query.tags ?? []).every((t) => item.tags.includes(t)));
      return { items: page === 1 ? items : [...items].reverse(), page, totalPages: items.length ? 3 : 1, totalItems: items.length * 3,
        tagGroups: [{name: 'Type', tags: ['Scene','Video','Web','Application'].map((tag) => ({tag, label: tag}))}, {name:'Genre', tags:['Nature','City','Abstract'].map((tag) => ({tag,label:tag}))}, {name:'Resolution', tags:[{tag:'1920 x 1080',label:'1920 x 1080'}]}, {name:'Miscellaneous', tags:[{tag:'Audio responsive',label:'Audio responsive'}]}, {name:'Age Rating', tags:[{tag:'Everyone',label:'Everyone'}]}] };
    },
    steam_inspect_url: (args) => previewFor(parseWorkshopId(args.urlOrId)),
    steam_download_workshop_item: (args) => {
      const workshopId = parseWorkshopId(args.urlOrId);
      const preview = previewFor(workshopId);
      const wallpaperId = `workshop:${workshopId}`;
      const steps = [0, 12, 27, 42, 58, 73, 88, 100];
      steps.forEach((pct, i) => {
        schedule(() => {
          if (pct === 100 && !wallpapers.some((w) => w.workshopId === workshopId)) {
            wallpapers = [
              ...wallpapers,
              {
                wallpaperId,
                workshopId,
                title: preview.title,
                type: 'scene',
                previewPath: preview.previewUrl,
                installDir: `/mock/TruongPaper/workshop/${workshopId}`,
                bytes: preview.fileSizeBytes,
                audioReactive: false,
                windowsOnly: false,
                projectJson: { title: preview.title, type: 'scene', file: 'scene.json', tags: preview.tags, general: { properties: {} } },
                userProperties: {},
                adjustments: { ...DEFAULT_ADJUSTMENTS },
              },
            ];
          }
          emitEvent('steam-download://progress', {
            workshopId,
            title: preview.title,
            progressPercent: pct,
            state: pct === 100 ? 'completed' : 'downloading',
          });
        }, 350 * (i + 1));
      });
      return wallpaperId;
    },
    steam_login_qr: () => {
      schedule(() => emitEvent('steam-auth://qr-matrix', { matrix: buildDemoQrMatrix() }), 400);
      return null;
    },
    steam_login_credentials: (args) => {
      if (!args.username || !args.password) throw new Error('Steam username and password are required');
      schedule(
        () => emitEvent('steam-auth://need-2fa', { kind: 'totp', prompt: 'Enter the code from your Steam Mobile Authenticator' }),
        300
      );
      return null;
    },
    steam_submit_2fa_code: (args) => {
      if (!/^[A-Za-z0-9]{5}$/.test(String(args.code ?? ''))) throw new Error('Invalid Steam Guard code');
      return null;
    },
    steam_logout: () => null,
    steam_sync_local_folders: () => wallpapers,
    library_list_wallpapers: () => wallpapers,
    library_delete_wallpaper: (args) => {
      wallpapers = wallpapers.filter((w) => w.wallpaperId !== args.wallpaperId);
      return null;
    },

    // --- Monitor, Playback & Property Customization ---
    monitors_list: () => monitors,
    monitor_apply_wallpaper: (args) => {
      const source = args.source as WallpaperSource;
      const monitorId = String(args.monitorId);
      const wallpaperId = source.kind === 'workshop' ? source.wallpaperId : null;
      monitors = monitors.map((m) => m.monitorId === monitorId
        ? { ...m, activeSource: source, activeWallpaperId: wallpaperId } : m);
      const item = wallpaperId ? findWallpaper(wallpaperId) : currentByMonitor[monitorId] ?? null;
      emitEvent('wallpaper://changed', { monitorId, source, currentItem: item, transitionUsed: settings.transitionType });
      return null;
    },
    wallpaper_set_lock_screen: () => null,
    monitor_update_property: (args) => {
      const w = findWallpaper(args.wallpaperId);
      wallpapers = wallpapers.map((x) =>
        x === w ? { ...x, userProperties: { ...x.userProperties, [String(args.propKey)]: args.propValue as string | number | boolean } } : x
      );
      return null;
    },
    monitor_update_adjustments: (args) => {
      const w = findWallpaper(args.wallpaperId);
      wallpapers = wallpapers.map((x) => (x === w ? { ...x, adjustments: args.adjustments as UniversalAdjustments } : x));
      return null;
    },
    wallpaper_save_preset: (args) => {
      const w = findWallpaper(args.wallpaperId);
      const preset: WallpaperPreset = {
        presetId: `preset-${Date.now().toString(36)}-${presets.length}`,
        wallpaperId: w.wallpaperId,
        name: String(args.presetName ?? 'Preset'),
        properties: { ...w.userProperties },
        adjustments: { ...w.adjustments },
      };
      presets.push(preset);
      return preset;
    },
    wallpaper_apply_preset: (args) => {
      const preset = presets.find((p) => p.presetId === args.presetId && p.wallpaperId === args.wallpaperId);
      if (preset) {
        wallpapers = wallpapers.map((x) =>
          x.wallpaperId === preset.wallpaperId ? { ...x, userProperties: { ...preset.properties }, adjustments: { ...preset.adjustments } } : x
        );
      }
      return null;
    },
    playback_set_paused: (args) => {
      const monitorId = args.monitorId == null ? null : String(args.monitorId);
      const paused = Boolean(args.paused);
      monitors = monitors.map((m) => monitorId === null || m.monitorId === monitorId ? { ...m, userPaused: paused } : m);
      emitEvent('playback://pause-changed', { monitorId, paused });
      return null;
    },

    // --- Playlists, Auto-Updater & Global Settings ---
    playlist_list: () => playlists,
    playlist_save: (args) => {
      const playlist = args.playlist as Playlist;
      playlists = [...playlists.filter((p) => p.playlistId !== playlist.playlistId), playlist];
      return playlist;
    },
    settings_get: () => settings,
    settings_update: (args) => {
      settings = { ...settings, ...(args.settings as Partial<AppSettings>) };
      emitEvent('settings://changed', settings);
      return null;
    },
    update_feed_status: () => 'ok',
    runtime_shutdown_for_update: () => null,
    runtime_get_memory_usage: () => ({ residentBytes: 164 * MB }),
  };

  const handlePlugin = (cmd: string, args: Args): unknown => {
    switch (cmd) {
      case 'plugin:app|version':
        return '1.0.0';
      case 'plugin:app|name':
        return 'TruongPaper';
      case 'plugin:updater|check':
        return null; // No update available in browser preview
      case 'plugin:shell|open':
        if (typeof window !== 'undefined' && typeof args.path === 'string') window.open(args.path, '_blank', 'noopener');
        return null;
      case 'plugin:dialog|open':
      case 'plugin:dialog|save':
        return null;
      case 'plugin:window|get_all_windows':
        return ['main', 'tray-popover'];
      case 'plugin:webview|get_all_webviews':
        return [
          { windowLabel: 'main', label: 'main' },
          { windowLabel: 'tray-popover', label: 'tray-popover' },
        ];
      case 'plugin:autostart|is_enabled':
        return settings.launchAtLogin;
      default:
        // Window/webview/process plugin calls (hide, minimize, setFocus, …) are no-ops in the browser.
        return null;
    }
  };

  return {
    handle: (cmd, args = {}) => {
      if (cmd.startsWith('plugin:')) return handlePlugin(cmd, args);
      const handler = (handlers as Record<string, (a: Args) => unknown>)[cmd];
      if (!handler) throw new Error(`[mockTauri] Unhandled command: ${cmd}`);
      return handler(args);
    },
    state: () => ({ photoStatus: photoStatus(), savedAlbums, wallpapers, monitors, playlists, settings }),
  };
}

export function installBrowserPreviewMocksIfNeeded(): void {
  if (typeof window === 'undefined' || '__TAURI_INTERNALS__' in window) {
    return;
  }

  mockWindows('main', 'tray-popover');

  // Minimal event bus (instead of mockIPC's `shouldMockEvents`, whose unlisten
  // leaves stale handler ids behind and logs "Couldn't find callback id").
  const eventHandlers = new Map<string, number[]>();
  const dispatch = (event: string, payload: unknown) => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: { runCallback: (id: number, data: unknown) => void } })
      .__TAURI_INTERNALS__;
    for (const id of [...(eventHandlers.get(event) ?? [])]) {
      internals.runCallback(id, { event, id, payload });
    }
  };

  const backend = createMockBackend(dispatch);

  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Args;
    switch (cmd) {
      case 'plugin:event|listen': {
        const id = Number(args.handler);
        eventHandlers.set(String(args.event), [...(eventHandlers.get(String(args.event)) ?? []), id]);
        return id;
      }
      case 'plugin:event|unlisten': {
        const event = String(args.event);
        eventHandlers.set(event, (eventHandlers.get(event) ?? []).filter((id) => id !== Number(args.eventId)));
        return null;
      }
      case 'plugin:event|emit':
      case 'plugin:event|emit_to':
        dispatch(String(args.event), args.payload);
        return null;
      default:
        return backend.handle(cmd, args);
    }
  });
}
