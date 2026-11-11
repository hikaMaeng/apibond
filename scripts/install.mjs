import { createHash } from 'node:crypto';
import { readFile, mkdir, writeFile, chmod, rename, access, unlink } from 'node:fs/promises';
import { constants } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repository = 'hikaMaeng/apibond';
const supported = new Set(['win32-x64', 'linux-x64', 'linux-arm64', 'darwin-x64', 'darwin-arm64']);
const maximumAssetBytes = 128 * 1024 * 1024;

/** Resolve an exact versioned native release asset; no archive extraction is used. */
export function assetName(platform = process.platform, arch = process.arch) {
  const key = `${platform}-${arch}`;
  if (!supported.has(key)) throw new Error(`Unsupported platform ${key}; build from source with cargo instead.`);
  return `apibond-${key}${platform === 'win32' ? '.exe' : ''}`;
}

/** Verify bytes against one SHA-256 digest before making them executable. */
export function verify(bytes, expected) {
  if (!/^[a-f0-9]{64}$/i.test(expected ?? '')) throw new Error('Invalid release SHA-256 digest');
  if (createHash('sha256').update(bytes).digest('hex') !== expected.toLowerCase()) throw new Error('Native binary checksum mismatch');
}

async function download(url, maximum, fetcher) {
  const response = await fetcher(url, { signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw new Error(`Release download failed (${response.status}): ${url}`);
  const declared = Number(response.headers.get('content-length'));
  if (declared > maximum) throw new Error('Release asset exceeds download limit');
  const chunks = []; let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > maximum) throw new Error('Release asset exceeds download limit');
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

/** Install a pinned native binary into the package or a user cache if the prefix is read-only. */
export async function install(options = {}) {
  const packageRoot = options.root ?? root;
  const platform = options.platform ?? process.platform;
  const arch = options.arch ?? process.arch;
  const fetcher = options.fetcher ?? fetch;
  const asset = assetName(platform, arch);
  const pkg = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
  if (!/^\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?$/.test(pkg.version)) throw new Error('Invalid package version');
  const cacheRoot = options.cacheRoot ?? join(process.env.LOCALAPPDATA ?? process.env.XDG_CACHE_HOME ?? join(homedir(), '.cache'), 'apibond');
  const executable = platform === 'win32' ? 'apibond.exe' : 'apibond';
  const localDir = join(packageRoot, 'vendor', `${platform}-${arch}`);
  const cacheDir = join(cacheRoot, pkg.version, `${platform}-${arch}`);
  let hashes = {};
  try { hashes = JSON.parse(await readFile(join(packageRoot, 'checksums.json'), 'utf8')); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  const base = `https://github.com/${repository}/releases/download/v${pkg.version}`;
  let expected = hashes[asset];
  for (const directory of [localDir, cacheDir]) {
    const candidate = join(directory, executable);
    try {
      const bytes = await readFile(candidate);
      let digest = expected;
      if (!digest) digest = (await readFile(`${candidate}.sha256`, 'utf8')).trim();
      verify(bytes, digest);
      return candidate;
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  if (!expected) expected = (await download(`${base}/${asset}.sha256`, 4096, fetcher)).toString('utf8').trim().split(/\s+/)[0];
  if (!/^[a-f0-9]{64}$/i.test(expected)) throw new Error('Invalid release SHA-256 file');
  const bytes = await download(`${base}/${asset}`, maximumAssetBytes, fetcher);
  verify(bytes, expected);
  let directory = localDir;
  try { await mkdir(directory, { recursive: true }); await access(directory, constants.W_OK); }
  catch { directory = cacheDir; await mkdir(directory, { recursive: true }); }
  const target = join(directory, executable);
  const temporary = `${target}.${process.pid}.${Math.random().toString(16).slice(2)}.tmp`;
  try {
    await writeFile(temporary, bytes, { flag: 'wx', mode: 0o755 });
    await chmod(temporary, 0o755);
    try { await rename(temporary, target); }
    catch (error) {
      if (error.code !== 'EEXIST' && error.code !== 'EPERM') throw error;
      verify(await readFile(target), expected);
      await unlink(temporary);
    }
    await writeFile(`${target}.sha256`, `${expected}\n`, 'utf8');
    return target;
  } catch (error) { await unlink(temporary).catch(() => {}); throw error; }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  install().then(path => process.stdout.write(`APIBond installed: ${path}\n`)).catch(error => { process.stderr.write(`apibond install: ${error.message}\n`); process.exitCode = 1; });
}
