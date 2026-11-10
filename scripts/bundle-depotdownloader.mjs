// scripts/bundle-depotdownloader.mjs
// Pinned, checksum-verified DepotDownloader v3.4.0 sidecar bundler for Steam Workshop downloads.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host;

const ASSETS_BY_TARGET = {
  'aarch64-apple-darwin': {
    zipName: 'DepotDownloader-macos-arm64.zip',
    sha256: '60e80c7c496f3f9a079cd3c62036b35d088c27bc0149baf38f009eb57a52f6a5',
    exeName: 'DepotDownloader',
  },
  'x86_64-apple-darwin': {
    zipName: 'DepotDownloader-macos-x64.zip',
    sha256: '3214b689564d73e9342a8a4aef693de6ad3d293801b0f300a4466f60ec75befb',
    exeName: 'DepotDownloader',
  },
  'x86_64-pc-windows-msvc': {
    zipName: 'DepotDownloader-windows-x64.zip',
    sha256: '41c9e9f0df54b3ad02e67a11726756e5c73283bd7c2e1b04acfa5ae4c2ed3767',
    exeName: 'DepotDownloader.exe',
  },
  'aarch64-pc-windows-msvc': {
    zipName: 'DepotDownloader-windows-arm64.zip',
    sha256: '1449ba47775e9974036e615bedf00d72bef747cc5b93a78048ed4e0b2c63b2b3',
    exeName: 'DepotDownloader.exe',
  },
};

if (!target || !ASSETS_BY_TARGET[target]) {
  throw new Error(`DepotDownloader sidecar is not configured for target: ${target}`);
}

const version = '3.4.0';
const windows = target.includes('windows');
const { zipName, sha256, exeName } = ASSETS_BY_TARGET[target];

const outDir = join(root, 'src-tauri', 'bin');
const downloadsDir = join(outDir, 'downloads');
mkdirSync(downloadsDir, { recursive: true });

const destBinary = join(outDir, `DepotDownloader-${target}${windows ? '.exe' : ''}`);
if (existsSync(destBinary)) {
  console.log(`[depotdownloader] Already bundled: ${destBinary}`);
  process.exit(0);
}

async function downloadFile(url, destPath, expectedSha256) {
  if (existsSync(destPath)) {
    if (!expectedSha256) return;
    const existingDigest = createHash('sha256').update(readFileSync(destPath)).digest('hex');
    if (existingDigest === expectedSha256) return;
  }
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Download failed (${response.status}): ${url}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  if (expectedSha256) {
    const digest = createHash('sha256').update(bytes).digest('hex');
    if (digest !== expectedSha256) {
      throw new Error(`SHA-256 mismatch for ${url}: expected ${expectedSha256}, got ${digest}`);
    }
  }
  writeFileSync(destPath, bytes);
}

const zipPath = join(downloadsDir, `DepotDownloader_${version}_${zipName}`);
const releaseUrl = `https://github.com/SteamRE/DepotDownloader/releases/download/DepotDownloader_${version}/${zipName}`;
await downloadFile(releaseUrl, zipPath, sha256);

if (windows) {
  const sevenZip = 'C:\\Program Files\\7-Zip\\7z.exe';
  execFileSync(existsSync(sevenZip) ? sevenZip : '7z', ['e', zipPath, `-o${downloadsDir}`, exeName, '-y'], {
    stdio: 'ignore',
  });
} else {
  execFileSync('unzip', ['-j', '-o', zipPath, exeName, '-d', downloadsDir], { stdio: 'ignore' });
}

const extractedExe = join(downloadsDir, exeName);
copyFileSync(extractedExe, destBinary);
chmodSync(destBinary, 0o755);

const licensesDir = join(outDir, 'licenses');
mkdirSync(licensesDir, { recursive: true });
await downloadFile(
  `https://raw.githubusercontent.com/SteamRE/DepotDownloader/DepotDownloader_${version}/LICENSE`,
  join(licensesDir, 'DepotDownloader-LICENSE.txt')
);

const digest = createHash('sha256').update(readFileSync(destBinary)).digest('hex');
console.log(`[depotdownloader] Bundled DepotDownloader ${version} for ${target} (sha256: ${digest})`);
