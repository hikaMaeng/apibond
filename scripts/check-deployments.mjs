import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, readdir, realpath } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../deploy');

/** Read a versioned artifact without allowing paths or symlinks to escape its directory. */
async function artifact(directory, path) {
  assert.equal(typeof path, 'string');
  assert.ok(path && !isAbsolute(path) && !path.includes('\\'));
  const canonical = await realpath(resolve(directory, path));
  const local = relative(directory, canonical);
  assert.ok(local && !local.startsWith(`..${sep}`) && local !== '..' && !isAbsolute(local));
  return readFile(canonical);
}

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
let verified = 0;
for (const entry of await readdir(root, { withFileTypes: true })) {
  if (!entry.isDirectory() || !/^v\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?$/.test(entry.name)) continue;
  const directory = await realpath(join(root, entry.name));
  const manifest = JSON.parse(await artifact(directory, 'manifest.json'));
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(`v${manifest.version}`, entry.name);
  assert.ok(Array.isArray(manifest.artifacts) && manifest.artifacts.length);
  const checksums = new Map();
  const sums = (await artifact(directory, 'SHA256SUMS')).toString('utf8').trim().split(/\r?\n/);
  for (const line of sums) {
    const match = /^([a-f0-9]{64})  (.+)$/.exec(line);
    assert.ok(match, `Invalid checksum line in ${entry.name}`);
    const [, digest, path] = match;
    assert.ok(!checksums.has(path), `Duplicate checksum: ${path}`);
    assert.equal(hash(await artifact(directory, path)), digest, `Checksum mismatch: ${entry.name}/${path}`);
    checksums.set(path, digest);
  }
  assert.ok(checksums.has('manifest.json'));
  assert.ok(checksums.has(manifest.exampleConfig));
  const config = JSON.parse(await artifact(directory, manifest.exampleConfig));
  assert.equal(config.schema_version, 1);
  for (const binary of manifest.artifacts) {
    assert.ok(/^[a-f0-9]{64}$/.test(binary.sha256));
    assert.equal(checksums.get(binary.path), binary.sha256);
    const bytes = await artifact(directory, binary.path);
    assert.equal(bytes.length, binary.bytes);
    assert.equal(hash(bytes), binary.sha256);
    verified++;
  }
}
assert.ok(verified > 0, 'No versioned deployment artifacts found');
console.log(`Verified ${verified} versioned deployment artifact(s)`);
