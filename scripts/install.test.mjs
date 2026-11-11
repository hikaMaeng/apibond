import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, writeFile, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { assetName, verify, install } from './install.mjs';

test('platform names and checksum rejection', () => {
  assert.equal(assetName('win32', 'x64'), 'apibond-win32-x64.exe');
  assert.throws(() => assetName('win32', 'arm64'), /Unsupported/);
  assert.throws(() => verify(Buffer.from('bad'), 'a'.repeat(64)), /checksum/);
});

test('lazy installation verifies embedded hashes and reuses executable', async () => {
  const root = await mkdtemp(join(tmpdir(), 'apibond-test-'));
  try {
    const bytes = Buffer.from('native fixture bytes');
    const digest = createHash('sha256').update(bytes).digest('hex');
    await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.1.0' }));
    await writeFile(join(root, 'checksums.json'), JSON.stringify({ 'apibond-linux-x64': digest }));
    let calls = 0;
    const fetcher = async url => { calls++; assert.match(url, /\/v0\.1\.0\/apibond-linux-x64$/); return new Response(bytes); };
    const path = await install({ root, platform: 'linux', arch: 'x64', cacheRoot: join(root, 'cache'), fetcher });
    assert.deepEqual(await readFile(path), bytes);
    assert.equal(await install({ root, platform: 'linux', arch: 'x64', cacheRoot: join(root, 'cache'), fetcher }), path);
    assert.equal(calls, 1);
    await writeFile(path, 'tampered');
    await assert.rejects(install({ root, platform: 'linux', arch: 'x64', fetcher }), /checksum/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('git source path checks release digest and rejects corrupted download', async () => {
  const root = await mkdtemp(join(tmpdir(), 'apibond-test-'));
  try {
    await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.1.0' }));
    await assert.rejects(install({ root, platform: 'linux', arch: 'x64', cacheRoot: join(root, 'cache'), fetcher: async url => new Response(url.endsWith('.sha256') ? 'a'.repeat(64) : 'corrupt') }), /checksum/);
  } finally { await rm(root, { recursive: true, force: true }); }
});
