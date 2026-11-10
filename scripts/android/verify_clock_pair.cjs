'use strict';
// Component-only original RPA/native pair. Uses the supplier's EXPLICIT C
// adjtimex double, never qualifies this host or any Android clock.
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const https = require('node:https');
const assert = require('node:assert/strict');
const {createHash, createPrivateKey, X509Certificate} = require('node:crypto');
const {openGuardAdapter} = require('./collection_guard_adapter.cjs');
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const SOURCE = '9c97f57a22e9b84256aecc9f6b3bf70b4bd97113';
const MODULE_SHA = 'a93523bbf4db75230a44f01b923f3e6f23e8f48fe27c6bdce15c6b5ee4d853f3';
const canonical = value => value && typeof value === 'object' && !Array.isArray(value) ?
  Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
async function verifyClockPair(sourceRoot, workload600 = false) {
  const source = workload600 ? "0dbc6a821c5de10136e4a9655ef3788ce9b89ea7" : SOURCE;
  const moduleSha = workload600 ? "bc64099bfa6b97de1de1a7ec4612c07a469a094a6e4744f23f406501be9a5f5a" : MODULE_SHA;
  assert(path.isAbsolute(sourceRoot), 'Usage: node scripts/android/verify_clock_pair.cjs /absolute/exact-rpa-source');
  const moduleFile = path.join(sourceRoot, workload600 ? 'tests/integration/collection-clock-assurance-600.cjs' : 'tests/integration/collection-clock-assurance.cjs');
  assert.equal(sha(await fs.readFile(moduleFile)), moduleSha);
  const installerFile = path.join(sourceRoot, "tests/integration/selected-installed-clock-600.cjs");
  if (workload600) assert.equal(sha(await fs.readFile(installerFile)), "366512be79dc86a9bb13fbb7c6e2d8af11e9327a432afa8078891efd6b70499e");
  const {fixtureClock} = require(path.join(sourceRoot, 'tests/helpers/kernel-clock-fixture.cjs'));
  const {installationBytes, createCollectionClockAssurance} = require(moduleFile);
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'native-rpa-clock-pair-'));
  let lifetime, adapter;
  const controller = new AbortController();
  try {
    const probe = fixtureClock(directory, path.join(sourceRoot, 'ci/kernel-clock-probe.c'));
    const rawBytes = installationBytes(probe.config);
    const owner = {registrationId: 'declared', tenantId: 'declared', subject: 'declared', workerId: 'declared', runId: 'declared', epoch: 1};
    const selectionRef = {tenantId: 'declared', artifactId: 'art_' + 'b'.repeat(32), sha256: 'b'.repeat(64)};
    const assuranceRef = {tenantId: 'declared', artifactId: 'art_' + 'a'.repeat(32), sha256: sha(rawBytes)};
    const expiresAt = new Date(Date.now() + 30000).toISOString();
    lifetime = (workload600 ? require(installerFile).createInstalledClock : createCollectionClockAssurance)({assuranceRef, rawBytes,
      policy: {basis: 'bounded-utc', maxClockErrorMs: 300, assuranceRef}, owner, selectionRef,
      invocationKey: 'declared-pair', expiresAt});
    const guestBootId = '11111111-2222-4333-8444-555555555555';
    const acquireClock = (workload600 ? lifetime.clockInstaller : lifetime).bindAcquireClock({guestBootId, signal: controller.signal});
    const fixture = await fs.readFile(path.join(__dirname, '../../crates/trollm-mcp/tests/common/tls_fixture.rs'), 'utf8');
    const der = name => Buffer.from(new RegExp(`pub const ${name}: &\\[u8\\] = &\\[([\\s\\S]*?)\\];`).exec(fixture)[1]
      .split(',').map(value => value.trim()).filter(Boolean).map(Number));
    const ca = /pub const ROOT_1_PEM: &\[u8\] = br"([\s\S]*?)";/.exec(fixture)[1];
    const token = Buffer.from('declared-clock-pair-private-token');
    adapter = await openGuardAdapter({ready: {invocationKey: 'declared-pair', owner, expiresAt},
      selectionRef, guard: async () => { throw Error('No associated worker in this test'); },
      signal: controller.signal, cert: new X509Certificate(der('LEAF_1_DER')).toString(),
      key: createPrivateKey({key: der('LEAF_1_KEY_DER'), format: 'der', type: 'pkcs8'}).export({format: 'pem', type: 'pkcs8'}),
      token, outputDirectory: path.join(directory, 'adapter'), guestBootId, acquireClock});
    const send = challenge => new Promise((resolve, reject) => {
      const bytes = Buffer.from(JSON.stringify(canonical({schema: 'trollm.private-guard-clock-request.v1-proposal',
        invocationKey: 'declared-pair', guestBootId, challenge})));
      const request = https.request({hostname: 'localhost', family: 4, port: adapter.port, path: '/control', method: 'POST', ca,
        lookup(_host, _options, callback) { callback(null, '127.0.0.1', 4); },
        headers: {authorization: 'Bearer ' + token, 'content-type': 'application/json', 'content-length': bytes.length}}, response => {
        const chunks = []; response.on('data', chunk => chunks.push(chunk));
        response.on('end', () => resolve({status: response.statusCode, bytes: Buffer.concat(chunks)}));
      });
      request.on('error', reject); request.end(bytes);
    });
    for (const challenge of ['1'.repeat(64), '2'.repeat(64)]) {
      const before = Date.now(), response = await send(challenge), after = Date.now();
      assert.equal(response.status, 200);
      const value = JSON.parse(response.bytes);
      assert.equal(value.challenge, challenge); assert.equal(value.guestBootId, guestBootId);
      assert(value.sample.sampleUtcMs >= before && value.sample.sampleUtcMs <= after);
      assert.equal(value.sample.originalCutoff, expiresAt);
      assert.deepEqual(value.sample.source.assuranceRef, assuranceRef);
      assert.equal(value.sample.source.rawSha256, sha(rawBytes));
      assert.equal(Object.hasOwn(value, 'clockVerification'), false);
      assert.deepEqual(await fs.readFile(path.join(directory, 'adapter', 'clock-' + challenge + '.response.json')), response.bytes);
    }
    probe.state('5 64 16000000 16000000 0'); // Explicit test double, not a real clock write.
    const refused = await send('3'.repeat(64)); assert.equal(refused.status, 409);
    probe.state();
    assert.equal((await send('4'.repeat(64))).status, 409); // Permanent loss, no fallback.
    return {source, moduleSha256: moduleSha,
      scope: 'controlled-kernel-original-source-private-TLS-only', freshRequests: 2,
      sourceLossRefused: true, restorationRemainsFenced: true};
  } finally {
    controller.abort(); lifetime?.close(); await adapter?.close();
    await fs.rm(directory, {recursive: true, force: true});
  }
}
module.exports = {verifyClockPair};
if (require.main === module) verifyClockPair(process.argv[2], process.argv[3] === "--workload-600").then(result => process.stdout.write(JSON.stringify(result) + '\n'))
  .catch(error => { process.stderr.write(String(error.message) + '\nUsage: node scripts/android/verify_clock_pair.cjs /absolute/exact-rpa-source\n'); process.exitCode = 1; });
