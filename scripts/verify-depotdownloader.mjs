// Verify that the packaged .NET host starts, without authenticating or contacting Steam.
// DepotDownloader 3.4.0 reports a controlled usage error when -app is omitted.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binary = process.platform === 'darwin'
  ? join(root, 'src-tauri/target/release/bundle/macos/TruongPaper.app/Contents/MacOS/DepotDownloader')
  : join(root, 'src-tauri/target/release/DepotDownloader.exe');
if (!existsSync(binary)) throw new Error(`Bundled downloader is missing: ${binary}`);
const result = spawnSync(binary, ['-help'], { encoding: 'utf8', timeout: 20000 });
const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
if (result.status !== 1 || !output.includes('Error: -app not specified!')) {
  throw new Error(`Bundled downloader failed its startup check (exit ${result.status}, signal ${result.signal}): ${result.error?.message ?? output.slice(0, 1000)}`);
}
console.log('Bundled DepotDownloader started and returned the expected usage error.');
