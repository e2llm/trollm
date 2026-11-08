'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const https = require('node:https');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const {createPrivateKey, X509Certificate, createHash, randomBytes} = require('node:crypto');
const {openGuardAdapter, decodeRequest, canonical, schema} = require('./collection_guard_adapter.cjs');
const sha = value => createHash('sha256').update(value).digest('hex');
const selectionRef = {tenantId: 'fixture-only', artifactId: 'art_' + 'a'.repeat(32), sha256: 'b'.repeat(64)};
const rawNative = '{"runId":"fixture-only","callContext":{"epoch":"fixture-epoch","attempt":18446744073709551615,"call":4294967295}}';

function requestBody(ready) {
  return {schema, invocationKey: ready.invocationKey, selectionRef,
    nativeContext: {json: rawNative, sha256: sha(rawNative),
      callContext: {epoch: 'fixture-epoch', attempt: '18446744073709551615', call: 4294967295}}, guardRequest: {purpose: 'candidate-select'}};
}
function bytes(value) { return Buffer.from(JSON.stringify(canonical(value))); }
async function tls() {
  const text = await fs.readFile(path.join(__dirname, '../../crates/trollm-mcp/tests/common/tls_fixture.rs'), 'utf8');
  const der = name => Buffer.from(new RegExp(`pub const ${name}: &\\[u8\\] = &\\[([\\s\\S]*?)\\];`).exec(text)[1]
    .split(',').map(x => x.trim()).filter(Boolean).map(Number));
  const ca = /pub const ROOT_1_PEM: &\[u8\] = br"([\s\S]*?)";/.exec(text)[1];
  return {ca, cert: new X509Certificate(der('LEAF_1_DER')).toString(),
    key: createPrivateKey({key: der('LEAF_1_KEY_DER'), format: 'der', type: 'pkcs8'}).export({format: 'pem', type: 'pkcs8'})};
}
async function fixture(guard, readyFields = {}, clockOptions = {}) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'trollm-guard-test-'));
  const controller = new AbortController();
  const ready = {invocationKey: 'fixture-invocation', expiresAt: new Date(Date.now() + 30000).toISOString(), owner: {tenantId: 'fixture-only'}, ...readyFields};
  const trust = await tls();
  const token = Buffer.from(randomBytes(24).toString('hex'));
  const outputDirectory = path.join(directory, 'private-adapter');
  const adapter = await openGuardAdapter({ready, selectionRef, guard, signal: controller.signal, ...trust, token, outputDirectory, ...clockOptions});
  return {...adapter, controller, ready, trust, token, outputDirectory,
    async cleanup() { await adapter.close(); await fs.rm(directory, {recursive: true}); }};
}
async function send(f, value, {token = f.token, pathname = '/guard', raw, method = 'POST', ca = f.trust.ca} = {}) {
  const body = raw || bytes(value);
  return new Promise((resolve, reject) => {
    const request = https.request({hostname: 'localhost', family: 4, autoSelectFamily: false, port: f.port, path: pathname, method, ca,
      lookup(_host, _options, callback) { callback(null, '127.0.0.1', 4); },
      headers: {authorization: 'Bearer ' + token.toString(), 'content-type': 'application/json', 'content-length': method === 'POST' ? body.length : 0}},
    response => { const chunks = []; response.on('data', b => chunks.push(b)); response.on('end', () => resolve({status: response.statusCode, headers: response.headers, bytes: Buffer.concat(chunks)})); });
    request.on('error', reject); if (method === 'POST') request.write(body); request.end();
  });
}

test('actual TLS retains once-generated guard bytes and opaque max-u64 native context', async () => {
  let calls = 0;
  const result = {z: 'adapter serialization, not Relay evidence', invocationKey: 'fixture-invocation', args: {target: 'fixture-selector'}, a: 1};
  const f = await fixture(async request => { calls++; assert.deepEqual(request, {purpose: 'candidate-select'}); return result; });
  try {
    const response = await send(f, requestBody(f.ready));
    assert.equal(response.status, 200); assert.equal(calls, 1);
    assert.equal(response.bytes.toString(), JSON.stringify(result));
    assert.equal(response.headers['x-response-sha256'], sha(response.bytes));
    const retained = JSON.parse(await fs.readFile(path.join(f.outputDirectory, '000001.request.json'), 'utf8'));
    assert.equal(retained.nativeContext.json, rawNative);
    assert.equal(retained.nativeContext.callContext.attempt, '18446744073709551615');
    assert.deepEqual(await fs.readFile(path.join(f.outputDirectory, '000001.response.json')), response.bytes);
    assert.equal((await fs.stat(path.join(f.outputDirectory, '000001.response.json'))).mode & 0o777, 0o600);
    const control = await send(f, {}, {pathname: '/control', method: 'GET'});
    assert.equal(JSON.parse(control.bytes).aborted, false);
    await assert.rejects(send(f, requestBody(f.ready), {ca: []}));
    assert.equal(calls, 1);
  } finally { await f.cleanup(); }
});

test('auth, closed request, raw context, selection and duplicate serialization fail before guard', async () => {
  let calls = 0;
  const f = await fixture(async () => { calls++; throw Error('must not reach source guard'); });
  try {
    assert.equal((await send(f, requestBody(f.ready), {token: Buffer.from('not-authorized')})).status, 401);
    const variants = [
      {...requestBody(f.ready), unknown: true},
      {...requestBody(f.ready), invocationKey: 'wrong'},
      {...requestBody(f.ready), selectionRef: {...selectionRef, tenantId: 'other'}},
      {...requestBody(f.ready), nativeContext: {json: rawNative, sha256: '0'.repeat(64)}},
      {...requestBody(f.ready), nativeContext: {...requestBody(f.ready).nativeContext, callContext: {epoch: 'fixture', attempt: 9007199254740992, call: 1}}},
      {...requestBody(f.ready), nativeContext: {...requestBody(f.ready).nativeContext, callContext: {epoch: 'fixture', attempt: '18446744073709551616', call: 1}}},
    ];
    for (const value of variants) assert.equal((await send(f, value)).status, 409);
    const duplicate = Buffer.from(bytes(requestBody(f.ready)).toString().replace('"schema":', '"schema":"duplicate","schema":'));
    assert.equal((await send(f, {}, {raw: duplicate})).status, 409);
    assert.equal(calls, 0);
    assert.throws(() => decodeRequest(bytes(requestBody(f.ready)), f.ready, {...selectionRef, sha256: '0'.repeat(64)}));
  } finally { await f.cleanup(); }
});

test('post-await abort withholds guard args and concurrent request does not replace pending guard', async () => {
  let enter, release;
  const entered = new Promise(resolve => { enter = resolve; });
  const pending = new Promise(resolve => { release = resolve; });
  const f = await fixture(async () => { enter(); return pending; });
  try {
    const first = send(f, requestBody(f.ready)); await entered;
    assert.equal((await send(f, requestBody(f.ready))).status, 409);
    f.controller.abort();
    release({invocationKey: f.ready.invocationKey, args: {target: 'never-sent'}});
    const result = await first;
    assert.equal(result.status, 409); assert.equal(JSON.parse(result.bytes).error, 'ADAPTER_EXPIRED');
    const record = JSON.parse(await fs.readFile(path.join(f.outputDirectory, '000001.failure.json'), 'utf8'));
    assert.equal(record.status, 'GUARD_REFUSED');
    assert.equal(JSON.parse((await send(f, {}, {pathname: '/control', method: 'GET'})).bytes).aborted, true);
  } finally { release?.({invocationKey: f.ready.invocationKey}); await f.cleanup(); }
});

test('outcome HTTP recovery returns exact retained ACK without calling source twice and fences conflicts', async () => {
  let calls = 0;
  const ack = {recorded: true, source: 'native-adapter-reported-not-independent-settlement'};
  const f = await fixture(async request => { calls++; assert.equal(request.kind, 'outcome'); return ack; });
  try {
    const request = requestBody(f.ready);
    const journal = Buffer.from('fixture-only native record bytes; not authentic execution');
    request.guardRequest = {kind: 'outcome', selectionRef, selectionRawSha256: selectionRef.sha256,
      owner: f.ready.owner, selectionId: 'fixture-selection', caseId: 'fixture-case', invocationKey: f.ready.invocationKey,
      contextSha256: request.nativeContext.sha256, outcome: 'OBSERVATION_RETURNED',
      journalEvidence: {bytesBase64: journal.toString('base64'), sha256: sha(journal)}};
    const first = await send(f, request);
    const recovered = await send(f, request);
    assert.equal(first.status, 200); assert.equal(recovered.status, 200);
    assert.deepEqual(first.bytes, recovered.bytes); assert.equal(calls, 1);
    assert.deepEqual(await fs.readFile(path.join(f.outputDirectory, '000001.request.json')), bytes(request));
    assert.deepEqual(await fs.readFile(path.join(f.outputDirectory, '000001.response.json')), first.bytes);
    const recovery = JSON.parse(await fs.readFile(path.join(f.outputDirectory, 'outcome-recovery.jsonl'), 'utf8'));
    assert.equal(recovery.encoding, 'original-http-utf8'); assert.equal(recovery.requestSha256, sha(bytes(request)));
    const changed = structuredClone(request); changed.guardRequest.caseId = 'changed-case';
    assert.equal((await send(f, changed)).status, 409);
    assert.equal((await send(f, request)).status, 409); assert.equal(calls, 1);
  } finally { await f.cleanup(); }
});

test('oversized opaque outcome evidence and unknown outcomes never reach source callback', async () => {
  let calls = 0;
  const f = await fixture(async () => { calls++; throw Error('must refuse before source'); });
  try {
    const request = requestBody(f.ready), raw = Buffer.alloc(16385, 120);
    request.guardRequest = {kind: 'outcome', selectionRef, selectionRawSha256: selectionRef.sha256,
      owner: f.ready.owner, selectionId: 'fixture-selection', caseId: 'fixture-case', invocationKey: f.ready.invocationKey,
      contextSha256: request.nativeContext.sha256, outcome: 'OBSERVATION_RETURNED',
      journalEvidence: {bytesBase64: raw.toString('base64'), sha256: sha(raw)}};
    assert.equal((await send(f, request)).status, 409);
    request.guardRequest.journalEvidence = {bytesBase64: 'eA==', sha256: sha(Buffer.from('x'))};
    request.guardRequest.outcome = 'UNKNOWN';
    assert.equal((await send(f, request)).status, 409); assert.equal(calls, 0);
  } finally { await f.cleanup(); }
});


for (const version of [2, 3]) test(`selected v${version} preserves exact opaque native context and refuses missing or mismatched selection before guard`, async () => {
  let calls = 0;
  const f = await fixture(async request => {
    calls++; assert.equal(request.kind, 'prepare');
    assert.equal(Buffer.from(request.callerContext.bytesBase64, 'base64').toString(), rawNative);
    return {invocationKey: 'fixture-invocation', args: {target: 'fixture-selector'}};
  }, {schema: `controlled-collection-selected-ready.v${version}`,
    ...(version === 3 ? {nativeProfileId: 'controlled-collection-native-selected.v3-proposal'} : {}), selectionRawSha256: selectionRef.sha256,
    selection: {selectionId: 'fixture-selection', caseId: 'fixture-case'}});
  const association = Buffer.from('fixture-only generated native association bytes');
  const value = requestBody(f.ready);
  value.guardRequest = {kind: 'prepare', selectionRef, selectionRawSha256: selectionRef.sha256, owner: f.ready.owner,
    selectionId: f.ready.selection.selectionId, caseId: f.ready.selection.caseId, invocationKey: f.ready.invocationKey,
    originalItemId: 'fixture-item', inputPosition: 0, admittedOrder: 0, quantity: 3, quantityText: '3', purpose: 'quantity-editor',
    callerContext: {bytesBase64: Buffer.from(rawNative).toString('base64'), sha256: sha(rawNative)},
    callerAssociation: {bytesBase64: association.toString('base64'), sha256: sha(association)}};
  try {
    for (const change of [
      request => { request.guardRequest = {purpose: 'candidate-select'}; },
      request => { request.guardRequest.caseId = 'other-case'; },
      request => { request.guardRequest.selectionRawSha256 = '0'.repeat(64); },
      request => { request.guardRequest.extra = true; },
      request => { const raw = Buffer.from('different opaque context'); request.guardRequest.callerContext = {bytesBase64: raw.toString('base64'), sha256: sha(raw)}; },
      request => { const raw = Buffer.alloc(16385, 120); request.guardRequest.callerAssociation = {bytesBase64: raw.toString('base64'), sha256: sha(raw)}; },
    ]) { const bad = structuredClone(value); change(bad); assert.equal((await send(f, bad)).status, 409); }
    assert.equal(calls, 0);
    if (version === 3) assert.throws(() => decodeRequest(bytes(value), {...f.ready, nativeProfileId: 'controlled-collection-native-selected.v2-proposal'}, selectionRef), /SELECTED_READY_MISMATCH/);
    assert.equal((await send(f, value)).status, 200); assert.equal(calls, 1);
    const retained = JSON.parse(await fs.readFile(path.join(f.outputDirectory, '000001.request.json'), 'utf8'));
    assert.equal(retained.nativeContext.callContext.attempt, '18446744073709551615');
    assert.deepEqual(retained.guardRequest, value.guardRequest);
  } finally { await f.cleanup(); }
});

const guestBoot = '12345678-1234-1234-1234-123456789abc';
function clockSample(ready) {
  return {sampleUtcMs: Date.now(), maxErrorUs: 42000, hostBootId: 'abcdefab-1234-1234-1234-123456789abc',
    originalCutoff: ready.expiresAt,
    source: {assuranceRef: {...selectionRef}, rawSha256: selectionRef.sha256, probeSha256: 'd'.repeat(64),
      sourcePins: {'ci/kernel-clock-probe.c': 'e'.repeat(64)}},
    association: {owner: ready.owner, selectionRef, invocationKey: ready.invocationKey}};
}
function clockRequest(f, challenge = '1'.repeat(64)) {
  return {schema: 'trollm.private-guard-clock-request.v1-proposal', invocationKey: f.ready.invocationKey,
    challenge, guestBootId: guestBoot};
}
test('fresh authenticated clock is available before associated guard and retains original response bytes', async () => {
  let f, calls = 0;
  f = await fixture(async () => { throw Error('guard must not run'); }, {}, {guestBootId: guestBoot,
    acquireClock: async request => { calls++; assert.equal(request.guestBootId, guestBoot); return clockSample(f.ready); }});
  try {
    const request = clockRequest(f), response = await send(f, request, {pathname: '/control'});
    assert.equal(response.status, 200); assert.equal(calls, 1);
    const value = JSON.parse(response.bytes);
    assert.equal(value.challenge, request.challenge); assert.equal(value.expiresAt, f.ready.expiresAt);
    assert.equal(value.sample.association.invocationKey, f.ready.invocationKey);
    assert.deepEqual(await fs.readFile(path.join(f.outputDirectory, 'clock-' + request.challenge + '.response.json')), response.bytes);
    assert.equal(response.headers['x-response-sha256'], sha(response.bytes));
    assert.equal((await send(f, request, {pathname: '/control'})).status, 409);
    assert.equal((await send(f, clockRequest(f, '2'.repeat(64)), {pathname: '/control'})).status, 409);
    assert.equal(calls, 1);
  } finally { await f.cleanup(); }
});
test('clock request foreign boot, extra field and missing installed provider refuse before probing', async () => {
  for (const change of [r => ({...r, guestBootId: 'foreign'}), r => ({...r, suppliedUtc: 1})]) {
    let calls = 0;
    const f = await fixture(async () => ({}), {}, {guestBootId: guestBoot, acquireClock: async () => { calls++; }});
    try { assert.equal((await send(f, change(clockRequest(f)), {pathname: '/control'})).status, 409); assert.equal(calls, 0); }
    finally { await f.cleanup(); }
  }
  const f = await fixture(async () => ({}));
  try { assert.equal((await send(f, clockRequest(f), {pathname: '/control'})).status, 409); }
  finally { await f.cleanup(); }
});
test('clock continuing callback cannot renew total budget or revive late result', async () => {
  let f, release, observedSignal;
  const held = new Promise(resolve => { release = resolve; });
  f = await fixture(async () => ({}), {}, {guestBootId: guestBoot, clockAcquisitionTimeoutMs: 30,
    acquireClock: async ({signal}) => { observedSignal = signal; await held; return clockSample(f.ready); }});
  try {
    const response = await send(f, clockRequest(f), {pathname: '/control'});
    assert.equal(response.status, 409); assert.equal(JSON.parse(response.bytes).error, 'CLOCK_ACQUISITION_TIMEOUT');
    assert.equal(observedSignal.aborted, true);
    release(); await new Promise(resolve => setImmediate(resolve));
    assert.equal((await fs.readdir(f.outputDirectory)).some(p => p.endsWith('.response.json')), false);
    assert.equal((await send(f, clockRequest(f, '2'.repeat(64)), {pathname: '/control'})).status, 409);
  } finally { release(); await f.cleanup(); }
});
test('clock host source drift and aborted sampler permanently fence instead of returning cached time', async () => {
  for (const mode of ['source', 'abort']) {
    let f, calls = 0;
    f = await fixture(async () => ({}), {}, {guestBootId: guestBoot, acquireClock: async () => {
      calls++; const sample = clockSample(f.ready);
      if (calls === 2) {
        if (mode === 'source') sample.hostBootId = '00000000-1234-1234-1234-123456789abc';
        else f.controller.abort();
      }
      return sample;
    }});
    try {
      assert.equal((await send(f, clockRequest(f), {pathname: '/control'})).status, 200);
      assert.equal((await send(f, clockRequest(f, '2'.repeat(64)), {pathname: '/control'})).status, 409);
      assert.equal((await send(f, clockRequest(f, '3'.repeat(64)), {pathname: '/control'})).status, 409);
      assert.equal(calls, 2);
    } finally { await f.cleanup(); }
  }
});

test('clock authorization cannot call sampler or poison a later genuine request', async () => {
  let f, calls = 0;
  f = await fixture(async () => ({}), {}, {guestBootId: guestBoot,
    acquireClock: async () => { calls++; return clockSample(f.ready); }});
  try {
    assert.equal((await send(f, clockRequest(f), {pathname: '/control', token: Buffer.from('wrong')})).status, 401);
    assert.equal(calls, 0);
    assert.equal((await send(f, clockRequest(f), {pathname: '/control'})).status, 200);
    assert.equal(calls, 1);
  } finally { await f.cleanup(); }
});
test('slow progress body cannot extend clock acquisition budget or reach sampler', async () => {
  let calls = 0;
  const f = await fixture(async () => ({}), {}, {guestBootId: guestBoot, clockAcquisitionTimeoutMs: 30,
    acquireClock: async () => { calls++; }});
  let interval;
  try {
    await new Promise(resolve => {
      const request = https.request({hostname: 'localhost', family: 4, port: f.port, path: '/control', method: 'POST', ca: f.trust.ca,
        lookup(_host, _options, callback) { callback(null, '127.0.0.1', 4); },
        headers: {authorization: 'Bearer ' + f.token, 'content-type': 'application/json', 'content-length': 1000}},
        response => { response.resume(); response.on('end', resolve); });
      request.on('error', resolve); request.on('close', resolve);
      request.write('{'); interval = setInterval(() => request.write(' '), 5);
    });
    clearInterval(interval); assert.equal(calls, 0);
    assert.equal((await send(f, clockRequest(f), {pathname: '/control'})).status, 409);
  } finally { clearInterval(interval); await f.cleanup(); }
});

test('synchronous probe blocking the event loop cannot win microtasks over an expired budget', async () => {
  let f, calls = 0;
  f = await fixture(async () => ({}), {}, {guestBootId: guestBoot, clockAcquisitionTimeoutMs: 20,
    acquireClock: async () => {
      calls++; Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 45);
      return clockSample(f.ready);
    }});
  try {
    const response = await send(f, clockRequest(f), {pathname: '/control'});
    assert.equal(response.status, 409); assert.equal(JSON.parse(response.bytes).error, 'CLOCK_ACQUISITION_TIMEOUT');
    assert.equal(calls, 1);
    assert.equal((await fs.readdir(f.outputDirectory)).some(p => p.endsWith('.response.json')), false);
    assert.equal((await send(f, clockRequest(f, '2'.repeat(64)), {pathname: '/control'})).status, 409);
  } finally { await f.cleanup(); }
});
