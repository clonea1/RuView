// ADR-375: MCP Apps / ChatGPT widget, structured results, protocol
// negotiation, and the authenticated Streamable HTTP transport.
import test from 'node:test';
import assert from 'node:assert/strict';
import { handleRpc, negotiateProtocol, SUPPORTED_PROTOCOLS, toCallToolResult } from '../src/mcp-server.js';
import { generateToken, httpGrants, startMcpHttp } from '../src/mcp-http.js';
import { CONSOLE_HTML, CONSOLE_URI, UI_MIME, UI_TOOLS } from '../src/ui/console-widget.js';

const rpc = (method, params, id = 1) => handleRpc({ jsonrpc: '2.0', id, method, params });

test('initialize negotiates the protocol and advertises resources', async () => {
  for (const v of SUPPORTED_PROTOCOLS) assert.equal((await rpc('initialize', { protocolVersion: v })).result.protocolVersion, v);
  assert.equal(negotiateProtocol('1999-01-01'), SUPPORTED_PROTOCOLS[0]);
  const init = (await rpc('initialize', {})).result;
  assert.equal(init.protocolVersion, '2025-06-18');
  assert.deepEqual(Object.keys(init.capabilities).sort(), ['resources', 'tools']);
  assert.equal(init.serverInfo.title, 'RuView');
});

test('UI tools point at the console widget with MCP Apps and ChatGPT keys', async () => {
  const tools = (await rpc('tools/list')).result.tools;
  for (const name of UI_TOOLS) {
    const t = tools.find((x) => x.name === name);
    assert.ok(t, name);
    assert.equal(t._meta.ui.resourceUri, CONSOLE_URI);
    assert.equal(t._meta['openai/outputTemplate'], CONSOLE_URI);
    assert.equal(t._meta['openai/widgetAccessible'], true);
    assert.ok(t.title, `${name} has a title`);
  }
  const plain = tools.find((x) => x.name === 'ruview_claim_check');
  assert.equal(plain._meta, undefined, 'non-UI tools carry no widget metadata');
  assert.equal(tools.filter((t) => t._meta).length, UI_TOOLS.length);
});

test('resources list and read the self-contained widget; unknown URIs fail', async () => {
  const [res] = (await rpc('resources/list')).result.resources;
  assert.deepEqual([res.uri, res.mimeType], [CONSOLE_URI, UI_MIME]);
  assert.deepEqual(res._meta.ui.csp, { connectDomains: [], resourceDomains: [] });
  const read = (await rpc('resources/read', { uri: CONSOLE_URI })).result.contents[0];
  assert.equal(read.text, CONSOLE_HTML);
  assert.equal(read.mimeType, UI_MIME);
  const missing = await rpc('resources/read', { uri: 'ui://ruview/nope.html' });
  assert.equal(missing.error.code, -32002);
  assert.deepEqual((await rpc('resources/templates/list')).result, { resourceTemplates: [] });
});

test('widget is self-contained and never writes HTML from data', () => {
  assert.ok(Buffer.byteLength(CONSOLE_HTML) < 16 * 1024, 'widget stays under 16 KiB');
  assert.doesNotMatch(CONSOLE_HTML, /https?:\/\//, 'no external origins');
  assert.doesNotMatch(CONSOLE_HTML, /innerHTML|outerHTML|insertAdjacentHTML|document\.write|eval\(|new Function/);
  assert.doesNotMatch(CONSOLE_HTML, /<(link|iframe|img|object|embed)\b/i);
  assert.match(CONSOLE_HTML, /e\.source!==window\.parent/, 'only the host frame may post messages');
  assert.match(CONSOLE_HTML, /ui\/initialize/);
  assert.match(CONSOLE_HTML, /openai:set_globals/);
});

test('tools/call returns structuredContent, compact text, and the canonical tool name', async () => {
  const r = (await rpc('tools/call', { name: 'ruview.claim_check', arguments: { text: 'PCK 0.9 MEASURED with baseline' } })).result;
  assert.equal(typeof r.structuredContent, 'object');
  assert.equal(r.content[0].text, JSON.stringify(r.structuredContent), 'text is the compact serialization');
  assert.equal(r._meta['ruview/tool'], 'ruview_claim_check');
  assert.equal(r.isError, r.structuredContent.ok === false);
  assert.deepEqual(toCallToolResult('x', [1, 2]).structuredContent, { value: [1, 2] }, 'structuredContent is always an object');
});

test('invalid messages and unknown methods get JSON-RPC errors; notifications get none', async () => {
  assert.equal((await handleRpc({ id: 1, method: 'ping' })).error.code, -32600, 'jsonrpc field required');
  assert.equal((await handleRpc([])).error.code, -32600);
  assert.equal((await rpc('nope/method')).error.code, -32601);
  assert.equal(await handleRpc({ jsonrpc: '2.0', method: 'notifications/initialized' }), null);
  assert.equal(await handleRpc({ jsonrpc: '2.0', method: 'notifications/whatever' }), null);
});

test('HTTP grants never include writes', () => {
  assert.deepEqual(httpGrants(['device-access', 'hardware-write', 'workspace-write', 'credential-use']), ['device-access', 'credential-use']);
  assert.ok(generateToken().length >= 32);
});

test('HTTP transport: auth, origin, method, size, protocol header, notifications, write denial', async (t) => {
  const token = generateToken();
  const logs = [];
  const srv = await startMcpHttp({ port: 0, token, grants: ['device-access', 'hardware-write'], allowOrigins: ['https://ok.example'], log: (m) => logs.push(m) });
  t.after(() => srv.close());
  const post = (path, body, headers = {}) => fetch(`${srv.url.replace(/\/mcp$/, '')}${path}`, {
    method: 'POST', headers: { 'content-type': 'application/json', ...headers }, body: typeof body === 'string' ? body : JSON.stringify(body),
  });
  const init = { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18' } };

  assert.deepEqual(srv.grants, ['device-access']);
  assert.match(logs.join('\n'), /never honoured over HTTP; ignoring: hardware-write/);
  assert.equal((await post('/mcp', init)).status, 401);
  assert.equal((await post('/mcp', init, { authorization: 'Bearer wrong-token-xxxxxxxxxxxx' })).status, 401);
  assert.equal((await post('/mcp/wrong-token-xxxxxxxxxxxx', init)).status, 401);
  const bearer = await post('/mcp', init, { authorization: `Bearer ${token}` });
  assert.equal(bearer.status, 200);
  assert.equal((await bearer.json()).result.protocolVersion, '2025-06-18');
  const secretPath = await post(`/mcp/${token}`, init);
  assert.equal(secretPath.status, 200, 'secret-path form for clients without custom headers');

  assert.equal((await post(`/mcp/${token}`, init, { origin: 'https://evil.example' })).status, 403);
  assert.equal((await post(`/mcp/${token}`, init, { origin: 'https://ok.example' })).status, 200);
  assert.equal((await fetch(`${srv.url}/${token}`)).status, 405);
  assert.equal((await post(`/mcp/${token}`, init, { 'mcp-protocol-version': '1999-01-01' })).status, 400);
  assert.equal((await post(`/mcp/${token}`, { jsonrpc: '2.0', method: 'notifications/initialized' })).status, 202);
  assert.equal((await post(`/mcp/${token}`, 'not json')).status, 400);
  assert.equal((await post(`/mcp/${token}`, [init])).status, 400, 'batches refused');
  assert.equal((await post(`/mcp/${token}`, 'x'.repeat(300 * 1024))).status, 413);
  assert.equal((await post('/elsewhere', init)).status, 404);
  assert.equal((await fetch(srv.url.replace(/\/mcp$/, '/healthz'))).status, 200);

  // Flash is a hardware-write tool: denied over HTTP even though the operator granted it.
  const flash = await post(`/mcp/${token}`, { jsonrpc: '2.0', id: 9, method: 'tools/call', params: { name: 'ruview_node_flash', arguments: { port: 'COM9', bundle: '.', variant: 'c6', confirm: true } } });
  const sc = (await flash.json()).result.structuredContent;
  assert.deepEqual([sc.ok, sc.reason, sc.requiredGrant], [false, 'authority_denied', 'hardware-write']);
  const deny = await post(`/mcp/${token}`, { jsonrpc: '2.0', id: 10, method: 'tools/call', params: { name: 'ruview_calibrate', arguments: { step: 'baseline', confirm: true } } });
  assert.equal((await deny.json()).result.structuredContent.reason, 'authority_denied');
});

test('HTTP transport refuses short tokens', async () => {
  await assert.rejects(startMcpHttp({ port: 0, token: 'short' }), /at least 16/);
});
