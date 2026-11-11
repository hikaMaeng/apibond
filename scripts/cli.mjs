#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { install } from './install.mjs';

try {
  const executable = await install();
  const child = spawn(executable, process.argv.slice(2), { stdio: 'inherit', windowsHide: true });
  if (process.platform === 'win32') process.on('SIGINT', () => {});
  else for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
  child.once('error', error => { process.stderr.write(`apibond: ${error.message}\n`); process.exitCode = 1; });
  child.once('exit', (code, signal) => { process.exitCode = code ?? (signal ? 1 : 0); });
} catch (error) {
  process.stderr.write(`apibond: ${error.message}\n`);
  process.exitCode = 1;
}
