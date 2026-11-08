'use strict';

// Private embedding transport only. The supplied guard performs observations;
// this module has no Relay client or effect dispatcher.
const https = require('node:https');
const fs = require('node:fs/promises');
const path = require('node:path');
const {createHash, timingSafeEqual} = require('node:crypto');

const MAX_REQUEST_BYTES = 262144;
const MAX_GUARD_BYTES = 65536;
const schema = 'trollm.private-collection-guard.v1';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const fail = code => { throw Object.assign(new Error(code), {code}); };
const exact = (value, keys) => {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join(',') !== [...keys].sort().join(',')) fail('CLOSED_REQUEST_REQUIRED');
};
const canonical = value => Array.isArray(value) ? value.map(canonical) :
  value && typeof value === 'object' ? Object.fromEntries(Object.keys(value).sort().map(k => [k, canonical(value[k])])) : value;

function decodeRequest(bytes, ready, selectionRef) {
  if (bytes.length > MAX_REQUEST_BYTES) fail('REQUEST_TOO_LARGE');
  const value = JSON.parse(bytes.toString('utf8'));
  const safe = item => typeof item === 'number' ? Number.isSafeInteger(item) :
    Array.isArray(item) ? item.every(safe) : item && typeof item === 'object' ? Object.values(item).every(safe) : true;
  if (!safe(value)) fail('UNSAFE_INTEGER_REQUIRES_LEXICAL');
  exact(value, ['schema', 'invocationKey', 'selectionRef', 'nativeContext', 'guardRequest']);
  // Also rejects duplicate fields and noncanonical outer serialization. The
  // opaque nativeContext.json string is never parsed into JavaScript numbers.
  if (!Buffer.from(JSON.stringify(canonical(value))).equals(bytes)) fail('CANONICAL_REQUEST_REQUIRED');
  if (value.schema !== schema || value.invocationKey !== ready.invocationKey) fail('INVOCATION_MISMATCH');
  if (JSON.stringify(canonical(value.selectionRef)) !== JSON.stringify(canonical(selectionRef))) fail('SELECTION_MISMATCH');
  exact(value.nativeContext, ['json', 'sha256', 'callContext']);
  const context = value.nativeContext;
  if (typeof context.json !== 'string' || Buffer.byteLength(context.json) > MAX_GUARD_BYTES ||
      !/^[a-f0-9]{64}$/.test(context.sha256) || sha(Buffer.from(context.json)) !== context.sha256) fail('NATIVE_CONTEXT_MISMATCH');
  exact(context.callContext, ['epoch', 'attempt', 'call']);
  const call = context.callContext;
  if (typeof call.epoch !== 'string' || !call.epoch || Buffer.byteLength(call.epoch) > 256 ||
      typeof call.attempt !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(call.attempt) ||
      BigInt(call.attempt) > 18446744073709551615n || !Number.isInteger(call.call) || call.call < 0 || call.call > 4294967295) fail('NATIVE_CONTEXT_MISMATCH');
  if (!value.guardRequest || typeof value.guardRequest !== 'object' || Array.isArray(value.guardRequest) ||
      Buffer.byteLength(JSON.stringify(value.guardRequest)) > MAX_GUARD_BYTES) fail('GUARD_REQUEST_INVALID');
  if (ready.schema === 'controlled-collection-selected-ready.v3' &&
      ready.nativeProfileId !== 'controlled-collection-native-selected.v3-proposal') fail('SELECTED_READY_MISMATCH');
  const selected = ready.schema === 'controlled-collection-selected-ready.v2' || ready.schema === 'controlled-collection-selected-ready.v3';
  if (value.guardRequest.kind === 'prepare') {
    if (!selected) fail('SELECTED_READY_REQUIRED');
    const prepare = value.guardRequest;
    exact(prepare, ['kind', 'selectionRef', 'selectionRawSha256', 'owner', 'selectionId', 'caseId',
      'invocationKey', 'originalItemId', 'inputPosition', 'admittedOrder', 'quantity', 'quantityText',
      'purpose', 'callerContext', 'callerAssociation']);
    if (prepare.invocationKey !== ready.invocationKey || prepare.selectionRawSha256 !== ready.selectionRawSha256 ||
        prepare.selectionId !== ready.selection?.selectionId || prepare.caseId !== ready.selection?.caseId ||
        JSON.stringify(canonical(prepare.selectionRef)) !== JSON.stringify(canonical(selectionRef)) ||
        JSON.stringify(canonical(prepare.owner)) !== JSON.stringify(canonical(ready.owner))) fail('PREPARE_ASSOCIATION_MISMATCH');
    for (const name of ['callerContext', 'callerAssociation']) {
      const evidence = prepare[name]; exact(evidence, ['bytesBase64', 'sha256']);
      if (typeof evidence.bytesBase64 !== 'string' || !/^[A-Za-z0-9+/]*={0,2}$/.test(evidence.bytesBase64)) fail('PREPARE_EVIDENCE_INVALID');
      const raw = Buffer.from(evidence.bytesBase64, 'base64');
      if (!raw.length || raw.length > 16384 || raw.toString('base64') !== evidence.bytesBase64 || sha(raw) !== evidence.sha256)
        fail('PREPARE_EVIDENCE_INVALID');
      if (name === 'callerContext' && (!raw.equals(Buffer.from(context.json)) || evidence.sha256 !== context.sha256))
        fail('PREPARE_NATIVE_CONTEXT_MISMATCH');
    }
  } else if (selected && value.guardRequest.kind !== 'outcome') fail('SELECTED_PREPARE_REQUIRED');
  else if (value.guardRequest.kind !== undefined && value.guardRequest.kind !== 'outcome') fail('CLOSED_REQUEST_REQUIRED');
  if (value.guardRequest.kind === 'outcome') {
    const outcome = value.guardRequest;
    exact(outcome, ['kind', 'selectionRef', 'selectionRawSha256', 'owner', 'selectionId', 'caseId',
      'invocationKey', 'contextSha256', 'outcome', 'journalEvidence']);
    if (outcome.invocationKey !== ready.invocationKey || outcome.contextSha256 !== context.sha256 ||
        JSON.stringify(canonical(outcome.selectionRef)) !== JSON.stringify(canonical(selectionRef)) ||
        JSON.stringify(canonical(outcome.owner)) !== JSON.stringify(canonical(ready.owner))) fail('OUTCOME_ASSOCIATION_MISMATCH');
    if (selected && (outcome.selectionRawSha256 !== ready.selectionRawSha256 ||
        outcome.selectionId !== ready.selection?.selectionId || outcome.caseId !== ready.selection?.caseId)) fail('OUTCOME_ASSOCIATION_MISMATCH');
    exact(outcome.journalEvidence, ['bytesBase64', 'sha256']);
    const evidence = outcome.journalEvidence;
    if (typeof evidence.bytesBase64 !== 'string' || !/^[A-Za-z0-9+/]*={0,2}$/.test(evidence.bytesBase64)) fail('OUTCOME_EVIDENCE_INVALID');
    const raw = Buffer.from(evidence.bytesBase64, 'base64');
    if (!raw.length || raw.length > 16384 || raw.toString('base64') !== evidence.bytesBase64 || sha(raw) !== evidence.sha256)
      fail('OUTCOME_EVIDENCE_INVALID');
  }
  return value;
}

async function openGuardAdapter({ready, selectionRef, guard, signal, key, cert, token, outputDirectory, host = '127.0.0.1',
  acquireClock, guestBootId, clockAcquisitionTimeoutMs = 30000}) {
  if (typeof guard !== 'function' || !signal || !ready?.invocationKey || !Number.isFinite(Date.parse(ready.expiresAt)) ||
      !path.isAbsolute(outputDirectory) || !Buffer.isBuffer(token) || token.length < 16 || token.length > 4096 || token.includes(10) || token.includes(13)) fail('ADAPTER_INPUT_INVALID');
  exact(selectionRef, ['tenantId', 'artifactId', 'sha256']);
  if (selectionRef.tenantId !== ready.owner?.tenantId || !/^art_[a-f0-9]{32}$/.test(selectionRef.artifactId) ||
      !/^[a-f0-9]{64}$/.test(selectionRef.sha256)) fail('SELECTION_REFERENCE_INVALID');
  if (signal.aborted || Date.now() >= Date.parse(ready.expiresAt)) fail('ADAPTER_EXPIRED');
  // A new private directory prevents recovery by filename or overwriting prior
  // observations. This directory is not native artifact admission/custody.
  await fs.mkdir(outputDirectory, {mode: 0o700});
  let busy = false, sequence = 0, closed = false, outcomeFenced = false;
  const outcomeAcks = new Map();
  const clockChallenges = new Set();
  const clockAbort = new AbortController();
  let clockBusy = false, clockFenced = false, clockSource;
  if ((acquireClock !== undefined && (typeof acquireClock !== 'function' ||
      !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(guestBootId))) ||
      !Number.isSafeInteger(clockAcquisitionTimeoutMs) || clockAcquisitionTimeoutMs < 1 || clockAcquisitionTimeoutMs > 30000)
    fail('CLOCK_INSTALLATION_INVALID');
  const fenceClock = () => { clockFenced = true; clockAbort.abort(); };
  signal.addEventListener('abort', fenceClock, {once: true});
  if (signal.aborted) fenceClock();
  const checkClock = () => { current(); if (clockFenced) fail('CLOCK_SESSION_FENCED'); };
  const validateSample = sample => {
    exact(sample, ['sampleUtcMs', 'maxErrorUs', 'hostBootId', 'originalCutoff', 'source', 'association']);
    exact(sample.source, ['assuranceRef', 'rawSha256', 'probeSha256', 'sourcePins']);
    exact(sample.association, ['owner', 'selectionRef', 'invocationKey']);
    exact(sample.source.assuranceRef, ['tenantId', 'artifactId', 'sha256']);
    if (!Number.isSafeInteger(sample.sampleUtcMs) || sample.sampleUtcMs < 1 ||
        !Number.isSafeInteger(sample.maxErrorUs) || sample.maxErrorUs < 0 ||
        !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(sample.hostBootId) ||
        !Number.isFinite(Date.parse(sample.originalCutoff)) || sample.sampleUtcMs >= Date.parse(sample.originalCutoff) ||
        sample.association.invocationKey !== ready.invocationKey ||
        JSON.stringify(canonical(sample.association.owner)) !== JSON.stringify(canonical(ready.owner)) ||
        JSON.stringify(canonical(sample.association.selectionRef)) !== JSON.stringify(canonical(selectionRef)) ||
        ![sample.source.rawSha256, sample.source.probeSha256, sample.source.assuranceRef.sha256]
          .every(hash => typeof hash === 'string' && /^[a-f0-9]{64}$/.test(hash)) ||
        sample.source.rawSha256 !== sample.source.assuranceRef.sha256 ||
        sample.source.assuranceRef.tenantId !== ready.owner.tenantId ||
        !/^art_[a-f0-9]{32}$/.test(sample.source.assuranceRef.artifactId) ||
        !sample.source.sourcePins || Array.isArray(sample.source.sourcePins) ||
        !Object.keys(sample.source.sourcePins).length ||
        !Object.values(sample.source.sourcePins).every(hash => typeof hash === 'string' && /^[a-f0-9]{64}$/.test(hash)))
      fail('CLOCK_SAMPLE_INVALID');
    const identity = JSON.stringify(canonical({hostBootId: sample.hostBootId,
      originalCutoff: sample.originalCutoff, source: sample.source, association: sample.association}));
    if (clockSource !== undefined && clockSource !== identity) fail('CLOCK_SOURCE_CHANGED');
    clockSource = identity;
  };
  const current = () => {
    if (closed || signal.aborted || Date.now() >= Date.parse(ready.expiresAt)) fail('ADAPTER_EXPIRED');
  };
  const authorized = request => {
    const header = request.headers.authorization;
    if (typeof header !== 'string' || !header.startsWith('Bearer ')) return false;
    const offered = Buffer.from(header.slice(7));
    return offered.length === token.length && timingSafeEqual(offered, token);
  };
  const server = https.createServer({key, cert, minVersion: 'TLSv1.2'}, async (request, response) => {
    let record, outcomeRequest = false;
    try {
      if (!authorized(request)) { response.writeHead(401).end(); return; }
      if (request.method === 'GET' && request.url === '/control') {
        const bytes = Buffer.from(JSON.stringify({schema: 'trollm.private-guard-control.v1', invocationKey: ready.invocationKey,
          aborted: closed || signal.aborted || Date.now() >= Date.parse(ready.expiresAt), expiresAt: ready.expiresAt}));
        response.writeHead(200, {'content-type': 'application/json', 'cache-control': 'no-store', 'content-length': bytes.length}).end(bytes);
        return;
      }
      if (request.method === 'POST' && request.url === '/control') {
        let timer, onAbort, onClose;
        try {
          checkClock();
          if (!acquireClock) fail('CLOCK_PROVIDER_UNAVAILABLE');
          if (clockBusy) fail('CLOCK_CONCURRENT');
          clockBusy = true;
          const budget = Math.min(clockAcquisitionTimeoutMs, Date.parse(ready.expiresAt) - Date.now());
          const operation = new AbortController();
          const deadlineTick = process.hrtime.bigint() + BigInt(Math.max(0, budget)) * 1000000n;
          const withinClockBudget = () => {
            checkClock();
            if (process.hrtime.bigint() >= deadlineTick) fail('CLOCK_ACQUISITION_TIMEOUT');
          };
          const interrupted = new Promise((_, reject) => {
            const stop = code => {
              reject(Object.assign(new Error(code), {code}));
              fenceClock(); operation.abort();
              if (!request.complete) request.destroy();
            };
            timer = setTimeout(() => stop('CLOCK_ACQUISITION_TIMEOUT'), budget);
            onAbort = () => stop('CLOCK_ABORTED');
            onClose = () => { if (!response.writableEnded) stop('CLOCK_CLIENT_LOST'); };
            clockAbort.signal.addEventListener('abort', onAbort, {once: true});
            response.once('close', onClose);
          });
          // Includes request-body acquisition, custody I/O and the original
          // installed sampler wait. A continuing stream cannot renew this budget.
          const work = (async () => {
            const chunks = []; let length = 0;
            for await (const chunk of request) {
              withinClockBudget(); length += chunk.length;
              if (length > 4096) fail('CLOCK_REQUEST_TOO_LARGE');
              chunks.push(chunk);
            }
            const original = Buffer.concat(chunks);
            const text = new TextDecoder('utf-8', {fatal: true}).decode(original);
            const value = JSON.parse(text);
            exact(value, ['schema', 'invocationKey', 'challenge', 'guestBootId']);
            if (!Buffer.from(JSON.stringify(canonical(value))).equals(original)) fail('CANONICAL_REQUEST_REQUIRED');
            if (value.schema !== 'trollm.private-guard-clock-request.v1-proposal' ||
                value.invocationKey !== ready.invocationKey || value.guestBootId !== guestBootId ||
                typeof value.challenge !== 'string' || !/^[a-f0-9]{64}$/.test(value.challenge)) fail('CLOCK_REQUEST_MISMATCH');
            withinClockBudget();
            if (clockChallenges.has(value.challenge)) fail('CLOCK_CHALLENGE_REUSED');
            if (clockChallenges.size >= 256) fail('CLOCK_ACQUISITION_LIMIT');
            clockChallenges.add(value.challenge); // Consumed even on loss/refusal.
            const prefix = path.join(outputDirectory, 'clock-' + value.challenge);
            await fs.writeFile(prefix + '.request.json', original, {flag: 'wx', mode: 0o600});
            withinClockBudget();
            const sample = await acquireClock({challenge: value.challenge, guestBootId,
              invocationKey: ready.invocationKey, signal: operation.signal});
            withinClockBudget(); validateSample(sample);
            const bytes = Buffer.from(JSON.stringify({schema: 'trollm.private-guard-clock.v1-proposal',
              invocationKey: ready.invocationKey, challenge: value.challenge, guestBootId,
              aborted: false, expiresAt: ready.expiresAt, sample}));
            if (bytes.length > 32768) fail('CLOCK_RESPONSE_TOO_LARGE');
            await fs.writeFile(prefix + '.response.json', bytes, {flag: 'wx', mode: 0o600});
            withinClockBudget();
            return bytes;
          })();
          const bytes = await Promise.race([work, interrupted]);
          withinClockBudget();
          response.writeHead(200, {'content-type': 'application/json', 'cache-control': 'no-store',
            'content-length': bytes.length, 'x-response-sha256': sha(bytes)}).end(bytes);
        } catch (error) { fenceClock(); throw error; }
        finally {
          clearTimeout(timer);
          if (onAbort) clockAbort.signal.removeEventListener('abort', onAbort);
          if (onClose) response.removeListener('close', onClose);
          clockBusy = false;
        }
        return;
      }
      if (request.method !== 'POST' || request.url !== '/guard') { response.writeHead(404).end(); return; }
      current();
      if (busy) fail('GUARD_CONCURRENT');
      busy = true;
      try {
        const chunks = []; let length = 0;
        for await (const chunk of request) {
          length += chunk.length;
          if (length > MAX_REQUEST_BYTES) fail('REQUEST_TOO_LARGE');
          chunks.push(chunk);
        }
        const bytes = Buffer.concat(chunks);
        const decoded = decodeRequest(bytes, ready, selectionRef);
        current();
        const isOutcome = decoded.guardRequest.kind === 'outcome';
        outcomeRequest = isOutcome;
        if (isOutcome && decoded.guardRequest.outcome !== 'OBSERVATION_RETURNED') fail('OUTCOME_UNKNOWN');
        if (outcomeFenced) fail('OUTCOME_SESSION_FENCED');
        const previous = isOutcome && outcomeAcks.get(decoded.nativeContext.sha256);
        if (previous) {
          if (!previous.requestBytes.equals(bytes)) { outcomeFenced = true; fail('OUTCOME_CONFLICT'); }
          await fs.appendFile(path.join(outputDirectory, 'outcome-recovery.jsonl'), JSON.stringify({
            encoding: 'original-http-utf8', requestSha256: sha(bytes), responseSha256: sha(previous.responseBytes),
            nativeContextSha256: decoded.nativeContext.sha256}) + '\n', {mode: 0o600});
          current();
          response.writeHead(200, {'content-type': 'application/json', 'content-length': previous.responseBytes.length,
            'x-native-context-sha256': decoded.nativeContext.sha256, 'x-response-sha256': sha(previous.responseBytes)}).end(previous.responseBytes);
          return;
        }
        if (isOutcome && outcomeAcks.size >= 32) fail('OUTCOME_RECOVERY_LIMIT');
        const ordinal = ++sequence; // Adapter record locator, not a native attempt identity.
        const prefix = path.join(outputDirectory, String(ordinal).padStart(6, '0'));
        await fs.writeFile(prefix + '.request.json', bytes, {flag: 'wx', mode: 0o600});
        record = {invocationKey: ready.invocationKey, nativeContextSha256: decoded.nativeContext.sha256,
          requestSha256: sha(bytes), selectionRef, status: 'GUARD_PENDING'};
        const returned = await guard(decoded.guardRequest);
        // Serialize the untouched JS object ONCE. These are newly generated
        // adapter response bytes, not original Relay serialization.
        const resultBytes = Buffer.from(JSON.stringify(returned));
        if (resultBytes.length > MAX_GUARD_BYTES) fail('GUARD_RESPONSE_TOO_LARGE');
        await fs.writeFile(prefix + '.response.json', resultBytes, {flag: 'wx', mode: 0o600});
        record.responseSha256 = sha(resultBytes);
        record.responseBytes = resultBytes.length;
        if (!returned || Array.isArray(returned)) fail('GUARD_ASSOCIATION_MISMATCH');
        if (isOutcome) {
          exact(returned, ['recorded', 'source']);
          if (returned.recorded !== true || returned.source !== 'native-adapter-reported-not-independent-settlement') fail('OUTCOME_ACK_INVALID');
        } else if (returned.invocationKey !== ready.invocationKey) fail('GUARD_ASSOCIATION_MISMATCH');
        current();
        record.status = 'GUARD_RETURNED';
        await fs.writeFile(prefix + '.record.json', JSON.stringify(record), {flag: 'wx', mode: 0o600});
        if (isOutcome) outcomeAcks.set(decoded.nativeContext.sha256, {requestBytes: Buffer.from(bytes), responseBytes: Buffer.from(resultBytes)});
        response.writeHead(200, {'content-type': 'application/json', 'content-length': resultBytes.length,
          'x-native-context-sha256': decoded.nativeContext.sha256, 'x-response-sha256': record.responseSha256}).end(resultBytes);
      } finally { busy = false; }
    } catch (error) {
      if (outcomeRequest) outcomeFenced = true;
      // Closed diagnostics contain no token, model proposal or raw tool payload.
      const code = typeof error.code === 'string' && /^[A-Z][A-Z0-9_]{0,63}$/.test(error.code) ? error.code :
        error instanceof SyntaxError ? 'REQUEST_JSON_INVALID' : 'ADAPTER_FAILURE';
      await fs.appendFile(path.join(outputDirectory, 'refusals.jsonl'), JSON.stringify({code, invocationKey: ready.invocationKey}) + '\n',
        {mode: 0o600}).catch(() => {});
      if (record) await fs.writeFile(path.join(outputDirectory, `${String(sequence).padStart(6, '0')}.failure.json`),
        JSON.stringify({...record, status: 'GUARD_REFUSED', code}),
        {flag: 'wx', mode: 0o600}).catch(() => {});
      if (!response.headersSent) response.writeHead(409, {'content-type': 'application/json'}).end(JSON.stringify({error: code}));
      else response.destroy();
    }
  });
  server.requestTimeout = 30000;
  server.headersTimeout = 10000;
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, host, resolve); });
  const port = server.address().port;
  return {port, async close() {
    if (closed) return;
    closed = true;
    fenceClock();
    signal.removeEventListener('abort', fenceClock);
    server.closeIdleConnections();
    // A held native request cannot keep launcher retirement waiting on its
    // socket. This retracts no accepted native effect or retained observation.
    server.closeAllConnections();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }};
}

async function withGuardAdapter(options, useNative) {
  if (typeof useNative !== 'function') fail('NATIVE_DRIVER_REQUIRED');
  const adapter = await openGuardAdapter(options);
  try { return await useNative(adapter); }
  finally { await adapter.close(); }
}

module.exports = {openGuardAdapter, withGuardAdapter, decodeRequest, canonical, schema, MAX_REQUEST_BYTES};
