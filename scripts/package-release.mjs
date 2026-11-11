import { readdir, readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const directory = process.argv[2] ?? 'release';
const hashes = {};
for (const file of await readdir(directory)) {
  if (!/^apibond-(win32|linux|darwin)-(x64|arm64)(\.exe)?$/.test(file)) continue;
  const hash = createHash('sha256').update(await readFile(join(directory, file))).digest('hex');
  hashes[file] = hash;
  await writeFile(join(directory, `${file}.sha256`), `${hash}\n`, 'utf8');
}
if (Object.keys(hashes).length !== 5) throw new Error('Expected five native release binaries');
await writeFile('checksums.json', `${JSON.stringify(hashes, null, 2)}\n`, 'utf8');
