import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import { mkdtemp, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, isAbsolute, join, relative, resolve, sep } from 'node:path';

const executable = resolve(process.argv[2]);
const version = execFileSync(executable, ['--version'], { encoding: 'utf8' }).trim();
assert.match(version, /^apibond \d+\.\d+\.\d+/);
const temporaryRoot = await realpath(tmpdir());
const directory = await mkdtemp(join(temporaryRoot, 'apibond-deploy-smoke-'));
const calls = [];
const upstream = createServer(async (req, res) => {
  try {
    let body = '';
    for await (const bytes of req) body += bytes.toString('utf8');
    const input = JSON.parse(body);
    assert.ok(['actual-a', 'actual-b'].includes(input.model));
    calls.push({ path: req.url, model: input.model });
    if (req.url === '/v1/chat/completions') {
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.end(`data: ${JSON.stringify({ id: 'chat_fixture', model: input.model, choices: [{ delta: { content: 'TypeScript adds static type checking to JavaScript.' } }] })}\n\ndata: [DONE]\n\n`);
    } else if (req.url === '/v1/messages') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ id: 'msg_fixture', type: 'message', role: 'assistant', model: input.model, content: [{ type: 'text', text: 'TypeScript adds static type checking.' }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 8 } }));
    } else if (req.url === '/v1/responses') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ id: input.previous_response_id ? 'resp_followup' : 'resp_fixture', object: 'response', status: 'completed', model: input.model, output: [] }));
    } else { res.writeHead(404); res.end(); }
  } catch (error) { res.writeHead(500); res.end(error.message); }
});
let child;
let logs = '';
try {
  upstream.listen(0, '127.0.0.1');
  await once(upstream, 'listening');
  const baseUrl = `http://127.0.0.1:${upstream.address().port}/v1`;
  const config = {
    schema_version: 1, listen: '127.0.0.1:0', inbound_auth: { mode: 'none' }, runtime: { worker_threads: 2 },
    backends: { a: { base_url: baseUrl, apis: ['openai', 'anthropic', 'responses'], capacity: 4, auth: { scheme: 'none' } }, b: { base_url: baseUrl, apis: ['openai', 'anthropic', 'responses'], capacity: 4, auth: { scheme: 'none' } } },
    models: { bonded: { backends: [{ backend: 'a', model: 'actual-a' }, { backend: 'b', model: 'actual-b' }] } },
  };
  const configPath = join(directory, 'config.json');
  await writeFile(configPath, JSON.stringify(config), 'utf8');
  assert.match(execFileSync(executable, ['--config', configPath, '--check'], { encoding: 'utf8' }), /Configuration valid/);
  child = spawn(executable, ['--config', configPath], { env: { ...process.env, RUST_LOG: 'apibond=info' }, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', bytes => { logs += bytes.toString('utf8'); });
  child.stderr.on('data', bytes => { logs += bytes.toString('utf8'); });
  const deadline = Date.now() + 10000;
  let match;
  while (!(match = /APIBond listening address=127\.0\.0\.1:(\d+)/.exec(logs.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')))) {
    assert.equal(child.exitCode, null, `Native child exited: ${logs}`);
    assert.ok(Date.now() < deadline, `Native startup deadline: ${logs}`);
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  const origin = `http://127.0.0.1:${match[1]}`;
  const health = await (await fetch(`${origin}/healthz`)).json();
  assert.equal(health.status, 'ok');
  assert.ok((await (await fetch(`${origin}/v1/models`)).json()).data.some(x => x.id === 'bonded'));
  const request = async (path, body, session) => {
    const headers = { 'content-type': 'application/json' };
    if (session) headers['x-session-id'] = session;
    const response = await fetch(`${origin}${path}`, { method: 'POST', headers, body: JSON.stringify({ model: 'bonded', ...body }), signal: AbortSignal.timeout(10000) });
    const content = await response.text();
    assert.equal(response.status, 200, content);
    return { backend: response.headers.get('x-apibond-backend'), content };
  };
  const chat = { messages: [{ role: 'user', content: '타입스크립트의 특징을 설명하라' }], stream: true };
  const first = await request('/v1/chat/completions', chat, 'same');
  const repeated = await request('/v1/chat/completions', chat, 'same');
  const other = await request('/v1/chat/completions', chat, 'other');
  assert.equal(first.backend, repeated.backend);
  assert.notEqual(first.backend, other.backend);
  assert.ok(first.content.includes('[DONE]') && first.content.includes('static type checking'));
  const messages = await request('/v1/messages', { messages: chat.messages, max_tokens: 128 }, 'same');
  assert.equal(messages.backend, first.backend);
  assert.equal(JSON.parse(messages.content).type, 'message');
  const created = await request('/v1/responses', { input: '타입스크립트의 특징을 설명하라' }, 'same');
  const followup = await request('/v1/responses', { input: '러스트와 비교하라', previous_response_id: JSON.parse(created.content).id });
  assert.equal(followup.backend, created.backend);
  const status = await (await fetch(`${origin}/status`)).json();
  assert.ok(status.backends.every(x => x.active === 0));
  assert.equal(status.buffer_bytes_reserved, 0);
  assert.equal(calls.length, 6);
  console.log(JSON.stringify({ success: true, version, platform: process.platform, arch: process.arch, fixtureRequests: calls.length, chatStream: true, anthropicMessages: true, responsesOwnership: true, sessionAffinity: true, released: true }));
} finally {
  if (child && child.exitCode === null) {
    child.kill('SIGTERM');
    let timer;
    await Promise.race([once(child, 'exit'), new Promise(resolve => { timer = setTimeout(resolve, 3000); })]);
    clearTimeout(timer);
    if (child.exitCode === null) child.kill('SIGKILL');
  }
  upstream.closeAllConnections();
  await new Promise(resolve => upstream.close(resolve));
  const local = relative(temporaryRoot, directory);
  assert.ok(local && local !== '..' && !local.startsWith(`..${sep}`) && !isAbsolute(local) && basename(directory).startsWith('apibond-deploy-smoke-'));
  await rm(directory, { recursive: true, force: true });
}
