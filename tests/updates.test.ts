// tests/updates.test.ts — signed auto-updater service (docs/06 §5) with a mocked plugin-updater.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const checkMock = vi.fn();
const relaunchMock = vi.fn(async () => {});
const invokeMock = vi.fn();
const getVersionMock = vi.fn(async () => '1.0.0');

vi.mock('@tauri-apps/plugin-updater', () => ({ check: (...a: unknown[]) => checkMock(...a) }));
vi.mock('@tauri-apps/plugin-process', () => ({ relaunch: () => relaunchMock() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: () => getVersionMock() }));

type UpdatesModule = typeof import('../src/services/updates');

async function freshModule(): Promise<UpdatesModule> {
  vi.resetModules();
  return import('../src/services/updates');
}

function fakeUpdate(events: Array<{ event: string; data: Record<string, number> }> = []) {
  return {
    version: '1.1.0',
    currentVersion: '1.0.0',
    date: '2025-01-02T00:00:00Z',
    body: 'Faster Liquid Glass transitions',
    downloadAndInstall: vi.fn(async (cb: (e: unknown) => void) => {
      for (const e of events) cb(e);
    }),
  };
}

beforeEach(() => {
  checkMock.mockReset();
  relaunchMock.mockClear();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});
afterEach(() => {
  vi.useRealTimers();
});

describe('services/updates', () => {
  it('appVersion() proxies getVersion()', async () => {
    const m = await freshModule();
    await expect(m.appVersion()).resolves.toBe('1.0.0');
  });

  it('returns null and notifies listeners when no update is available', async () => {
    const m = await freshModule();
    checkMock.mockResolvedValue(null);
    const seen: unknown[] = [];
    m.onUpdateChange((u) => seen.push(u));
    await expect(m.checkForUpdate()).resolves.toBeNull();
    expect(checkMock).toHaveBeenCalledWith({ timeout: 20000 });
    expect(seen).toEqual([null]);
    expect(m.availableUpdate()).toBeNull();
  });

  it('summarizes an available update and lets listeners unsubscribe', async () => {
    const m = await freshModule();
    checkMock.mockResolvedValue(fakeUpdate());
    const listener = vi.fn();
    const off = m.onUpdateChange(listener);
    const info = await m.checkForUpdate();
    expect(info).toEqual({
      version: '1.1.0',
      currentVersion: '1.0.0',
      date: '2025-01-02T00:00:00Z',
      notes: 'Faster Liquid Glass transitions',
    });
    expect(listener).toHaveBeenCalledWith(info);
    off();
    await m.checkForUpdate();
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it('retries once after a transient failure', async () => {
    vi.useFakeTimers();
    const m = await freshModule();
    checkMock.mockRejectedValueOnce(new Error('network')).mockResolvedValueOnce(fakeUpdate());
    const p = m.checkForUpdate();
    await vi.advanceTimersByTimeAsync(1500);
    await expect(p).resolves.toMatchObject({ version: '1.1.0' });
    expect(checkMock).toHaveBeenCalledTimes(2);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('explains that a missing feed requires a release', async () => {
    vi.useFakeTimers();
    const m = await freshModule();
    checkMock.mockRejectedValue(new Error('boom'));
    invokeMock.mockResolvedValue('feed_not_found');
    const p = m.checkForUpdate();
    const assertion = expect(p).rejects.toThrow('No update feed is available yet. In-app updates start when a release is published.');
    await vi.advanceTimersByTimeAsync(1500);
    await assertion;
    expect(invokeMock).toHaveBeenCalledWith('update_feed_status');
  });

  it('rethrows the original error when the feed is healthy', async () => {
    vi.useFakeTimers();
    const m = await freshModule();
    checkMock.mockRejectedValue(new Error('signature mismatch'));
    invokeMock.mockResolvedValue('ok');
    const p = m.checkForUpdate();
    const assertion = expect(p).rejects.toThrow('signature mismatch');
    await vi.advanceTimersByTimeAsync(1500);
    await assertion;
  });

  it('installUpdate() refuses when nothing is pending', async () => {
    const m = await freshModule();
    await expect(m.installUpdate(() => {})).rejects.toThrow('No update available');
  });

  it('shuts the runtime down, streams progress, installs and relaunches', async () => {
    const m = await freshModule();
    const update = fakeUpdate([
      { event: 'Started', data: { contentLength: 300 } },
      { event: 'Progress', data: { chunkLength: 100 } },
      { event: 'Progress', data: { chunkLength: 200 } },
      { event: 'Finished', data: {} },
    ]);
    checkMock.mockResolvedValue(update);
    await m.checkForUpdate();

    const order: string[] = [];
    invokeMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
    });
    update.downloadAndInstall.mockImplementationOnce(async (cb: (e: unknown) => void) => {
      order.push('downloadAndInstall');
      cb({ event: 'Started', data: { contentLength: 300 } });
      cb({ event: 'Progress', data: { chunkLength: 100 } });
      cb({ event: 'Progress', data: { chunkLength: 200 } });
    });
    relaunchMock.mockImplementationOnce(async () => {
      order.push('relaunch');
    });

    const progress: Array<[number, number | null]> = [];
    await m.installUpdate((d, t) => progress.push([d, t]));
    expect(order).toEqual(['runtime_shutdown_for_update', 'downloadAndInstall', 'relaunch']);
    expect(progress).toEqual([
      [0, 300],
      [100, 300],
      [300, 300],
    ]);
  });

  it('still installs if runtime_shutdown_for_update fails', async () => {
    const m = await freshModule();
    const update = fakeUpdate();
    checkMock.mockResolvedValue(update);
    await m.checkForUpdate();
    invokeMock.mockRejectedValue(new Error('not running'));
    await m.installUpdate(() => {});
    expect(update.downloadAndInstall).toHaveBeenCalled();
    expect(relaunchMock).toHaveBeenCalled();
  });
});
