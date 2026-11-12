// src/services/updates.ts
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';

export interface AvailableUpdate {
  version: string;
  currentVersion: string;
  date: string | null;
  notes: string | null;
}

let pending: Update | null = null;
const listeners = new Set<(u: AvailableUpdate | null) => void>();

const summarize = (u: Update): AvailableUpdate => ({
  version: u.version,
  currentVersion: u.currentVersion,
  date: u.date ?? null,
  notes: u.body ?? null,
});

export const appVersion = () => getVersion();

export function availableUpdate(): AvailableUpdate | null {
  return pending ? summarize(pending) : null;
}

export function onUpdateChange(fn: (u: AvailableUpdate | null) => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

async function checkWithDiagnosis(): Promise<Update | null> {
  try {
    return await check({ timeout: 20000 });
  } catch (first) {
    await new Promise((r) => setTimeout(r, 1500));
    try {
      return await check({ timeout: 20000 });
    } catch (e) {
      const status = await invoke<string>('update_feed_status').catch(() => '');
      if (status === 'feed_not_found') {
        throw new Error('No update feed is available yet. In-app updates start when a release is published.');
      }
      if (status && status !== 'ok') throw new Error(`${status}. Try again in a few minutes.`);
      throw e ?? first;
    }
  }
}

export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  pending = await checkWithDiagnosis();
  const info = availableUpdate();
  listeners.forEach((fn) => fn(info));
  return info;
}

export async function installUpdate(onProgress: (downloaded: number, total: number | null) => void) {
  if (!pending) throw new Error('No update available');
  // Cleanly detach desktop wallpaper hosts / JobObjects before replacing executable
  await invoke('runtime_shutdown_for_update').catch(() => {});
  let downloaded = 0;
  let total: number | null = null;
  await pending.downloadAndInstall((event) => {
    if (event.event === 'Started') total = event.data.contentLength ?? null;
    else if (event.event === 'Progress') downloaded += event.data.chunkLength;
    onProgress(downloaded, total);
  });
  await relaunch();
}
