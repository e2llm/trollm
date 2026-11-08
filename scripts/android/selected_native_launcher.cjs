'use strict';
// Operator-private ADB composition, not an effect sender or agent loop.
const {spawn} = require('node:child_process');
const {createHash, randomBytes} = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const {openGuardAdapter} = require('./collection_guard_adapter.cjs');
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const packageName = 'com.trollm.app';
const hashPattern = /^[a-f0-9]{64}$/;
const uuidPattern = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
function fail(code) { const error = new Error(code); error.code = code; throw error; }

function closedFailure(error, stage) {
  const value = {stage, code: typeof error?.code === 'string' && /^[A-Z][A-Z0-9_]{0,63}$/.test(error.code) ?
    error.code : 'SELECTED_LAUNCHER_FAILURE'};
  if (value.code === 'SELECTED_LAUNCHER_FAILURE' && typeof error?.name === 'string' &&
      /^[A-Za-z][A-Za-z0-9]{0,63}$/.test(error.name)) value.type = error.name;
  // Classifications only, never raw messages/stderr, source records or input.
  if (Number.isSafeInteger(error?.childExitCode)) value.childExitCode = error.childExitCode;
  if (typeof error?.childSignal === 'string' && /^SIG[A-Z0-9]{1,16}$/.test(error.childSignal)) value.childSignal = error.childSignal;
  if (Number.isSafeInteger(error?.stderrBytes)) value.stderrBytes = error.stderrBytes;
  if (typeof error?.nativeStage === 'string' && /^[a-z-]{1,64}$/.test(error.nativeStage)) value.nativeStage = error.nativeStage;
  if (typeof error?.nativeCause === 'string' && /^SELECTED_[A-Z0-9_]{1,80}$/.test(error.nativeCause)) value.nativeCause = error.nativeCause;
  return value;
}

async function retireResources(adb, adapter, reverse) {
  const failures = [];
  try { if (reverse) await adb(['reverse', '--remove', `tcp:${adapter.port}`]); }
  catch (error) { failures.push(closedFailure(error, 'remove-reverse')); }
  try { await adapter.close(); }
  catch (error) { failures.push(closedFailure(error, 'close-adapter')); }
  return failures;
}

// Deliberately no shell on the operator. The only guest shell programs below
// are fixed templates containing validated hex identifiers, never private data.
function createAdb({executable, serverHost, serverPort, serial, timeoutMs = 15000}) {
  if (!path.isAbsolute(executable) || !/^[a-zA-Z0-9.-]+$/.test(serverHost) ||
      !Number.isInteger(serverPort) || serverPort < 1 || serverPort > 65535 ||
      !/^[a-zA-Z0-9_.:-]+$/.test(serial) || !Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 30000)
    fail('ADB_CONFIGURATION_INVALID');
  return async function adb(args, {input, maxBytes = 1048576, digestOnly = false, signal} = {}) {
    return await new Promise((resolve, reject) => {
      const child = spawn(executable, ['-H', serverHost, '-P', String(serverPort), '-s', serial, ...args],
        {stdio: ['pipe', 'pipe', 'pipe'], signal});
      let count = 0, stderrBytes = 0, overflow = false;
      const chunks = [], digest = createHash('sha256');
      let killTimer, spawnError;
      const timer = setTimeout(() => { child.kill('SIGTERM'); killTimer = setTimeout(() => child.kill('SIGKILL'), 1000); }, timeoutMs);
      child.stdout.on('data', bytes => {
        count += bytes.length;
        if (count > maxBytes) { overflow = true; child.kill('SIGTERM'); return; }
        if (digestOnly) digest.update(bytes); else chunks.push(bytes);
      });
      child.stderr.on('data', bytes => { stderrBytes += bytes.length; }); // Never expose private child output.
      child.on('error', error => {
        spawnError = error;
        // Abort emits error before close. Retain ownership until actual reap;
        // an ignored TERM still has the existing bounded KILL fallback.
        if (child.pid && !killTimer) killTimer = setTimeout(() => child.kill('SIGKILL'), 1000);
      });
      child.on('close', (code, signalName) => {
        clearTimeout(timer); clearTimeout(killTimer);
        if (spawnError) {
          const failure = Object.assign(new Error('ADB_EXEC_FAILED'), {code: spawnError.name === 'AbortError' ? 'ADB_ABORTED' : 'ADB_EXEC_FAILED', childExitCode: code, childSignal: signalName, stderrBytes});
          reject(failure); return;
        }
        if (code !== 0 || signalName || overflow) {
          const error = new Error('ADB_COMMAND_FAILED'); error.code = 'ADB_COMMAND_FAILED';
          error.childExitCode = code; error.childSignal = signalName; error.stderrBytes = stderrBytes;
          reject(error); return;
        }
        resolve(digestOnly ? {sha256: digest.digest('hex'), sizeBytes: count} : Buffer.concat(chunks));
      });
      child.stdin.on('error', () => {}); // Child exit is retained above, including EPIPE.
      child.stdin.end(input);
    });
  };
}

// Read string identity fields without interpreting any original numeric token
// as a JS Number. Original native record strings are retained untouched.
function identityProjection(raw) {
  if (typeof raw !== 'string' || Buffer.byteLength(raw) > 262144) fail('NATIVE_RECORD_INVALID');
  let projected = '';
  for (let i = 0; i < raw.length;) {
    if (raw[i] === '"') {
      const start = i++;
      while (i < raw.length) {
        if (raw[i] === '\\') { i += 2; continue; }
        if (raw[i++] === '"') break;
      }
      projected += raw.slice(start, i);
    } else if (raw[i] === '-' || /[0-9]/.test(raw[i])) {
      const token = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/.exec(raw.slice(i));
      if (!token) fail('NATIVE_RECORD_INVALID');
      projected += JSON.stringify(token[0]); i += token[0].length;
    } else projected += raw[i++];
  }
  return JSON.parse(projected);
}

function validateBinding(result, expected) {
  if (result.schema !== 'trollm.private-selected-launcher-result.v1' || result.error ||
      result.requestSha256 !== expected.requestSha256 || result.apkSha256 !== expected.apkSha256 ||
      result.invocationKey !== expected.invocationKey || result.sourceRevision !== expected.sourceRevision) {
    const error = new Error('LAUNCHER_RESULT_REFUSED'); error.code = 'LAUNCHER_RESULT_REFUSED';
    error.nativeStage = result.stage;
    if(typeof result.nativeCause==='string'&&/^SELECTED_[A-Z0-9_]{1,80}$/.test(result.nativeCause))
      error.nativeCause = result.nativeCause;
    throw error;
  }
  const binding = result.binding;
  if (!binding || binding.schema !== 'trollm.selected-worker-binding.v1' ||
      typeof binding.workerActive !== 'boolean' || !uuidPattern.test(binding.nativeRunId) ||
      (expected.nativeRunId && binding.nativeRunId !== expected.nativeRunId)) fail('NATIVE_BINDING_INVALID');
  const installation = identityProjection(binding.privateBindingJson), claim = identityProjection(binding.claimJson);
  const source = identityProjection(binding.sourceAssociationJson);
  if (installation.privateInstallationSha256 !== expected.installationKey ||
      installation.invocationKey !== expected.invocationKey || installation.nativeRunId !== binding.nativeRunId ||
      claim.nativeRunId !== binding.nativeRunId || claim.identity?.invocationKey !== expected.invocationKey ||
      source.nativeRunId !== binding.nativeRunId) fail('NATIVE_BINDING_ASSOCIATION_INVALID');
  return binding;
}

async function verifyApk(adb, apk) {
  if (!hashPattern.test(apk.sha256) || !/^[a-f0-9]{40}$/.test(apk.sourceRevision) || !Number.isSafeInteger(apk.sizeBytes) || apk.sizeBytes <= 0 || apk.sizeBytes > 134217728)
    fail('APK_IDENTITY_INVALID');
  const located = (await adb(['shell', 'pm', 'path', packageName])).toString().trim();
  // PackageManager uses both legacy one-directory paths and the modern
  // /data/app/~~random/package-random/base.apk layout. No split/traversal path.
  if (!/^package:\/data\/app\/(?:[a-zA-Z0-9_+=.~\-]+\/){1,2}base\.apk$/.test(located) ||
      located.slice(8).split('/').some(part => part === '.' || part === '..')) fail('SINGLE_INSTALLED_APK_REQUIRED');
  const observed = await adb(['exec-out', 'cat', located.slice(8)], {digestOnly: true, maxBytes: apk.sizeBytes});
  if (observed.sha256 !== apk.sha256 || observed.sizeBytes !== apk.sizeBytes) fail('INSTALLED_APK_MISMATCH');
  return observed;
}

async function provision(adb, directory, name, bytes) {
  if (!['selected-installations', 'selected-launcher'].includes(directory) || !/^[a-f0-9]{64}(?:\.request)?\.json$/.test(name) ||
      !Buffer.isBuffer(bytes) || bytes.length > 262144) fail('PRIVATE_PROVISION_INVALID');
  const relative = `files/trollm/${directory}/${name}`;
  await adb(['shell', 'run-as', packageName, 'sh', '-c',
    `'umask 077; mkdir -p files/trollm/${directory} && chmod 700 files/trollm/${directory} && set -C && cat > ${relative}'`], {input: bytes});
  const retained = await adb(['exec-out', 'run-as', packageName, 'cat', relative]);
  if (!retained.equals(bytes)) fail('PRIVATE_PROVISION_READBACK_MISMATCH');
}

/** One selected start, exact private adapter token, actual installed APK and
 * same-process worker binding. Missing/crashed/evicted handles never mean stop.
 * This does not install APKs, acquire keys, admit a source, or send Relay acts.
 */
const exactClockFields = (value, keys) => {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      Object.keys(value).sort().join(',') !== [...keys].sort().join(',')) fail('CLOCK_CLOSED_INPUT_REQUIRED');
};
const CALIBRATED_CLOCK_SOURCE = '9c97f57a22e9b84256aecc9f6b3bf70b4bd97113';
const CALIBRATED_CLOCK_MODULE_SHA256 = 'a93523bbf4db75230a44f01b923f3e6f23e8f48fe27c6bdce15c6b5ee4d853f3';
async function withSelectedAndroidDriver(options, useNative, clockInstaller) {
  const {ready, selectionRef, guard, signal, key, cert, rootsPem, outputDirectory, apk,
    flowSha256, installationSha256, input, configurationJson, associationJson,
    selectedDeviceMaxErrorUs, expectedStaticKeySha256, clockPolicy} = options;
  const adb = createAdb(options.adb);
  if (typeof useNative !== 'function' || typeof guard !== 'function' || !signal || signal.aborted ||
      ![flowSha256, installationSha256, expectedStaticKeySha256].every(x => hashPattern.test(x)) ||
      typeof rootsPem !== 'string' || !rootsPem.length ||
      (clockPolicy === undefined && (!Number.isSafeInteger(selectedDeviceMaxErrorUs) || selectedDeviceMaxErrorUs < 0)))
    fail('SELECTED_LAUNCHER_INPUT_INVALID');
  if (Object.hasOwn(options, 'acquireClock') || Object.hasOwn(options, 'bindAcquireClock')) fail('CLOCK_PRIVATE_INSTALLER_REQUIRED');
  if (clockPolicy !== undefined) {
    exactClockFields(clockPolicy, ['profileId', 'minimumRateNumerator', 'minimumRateDenominator', 'maxCalibrationAgeMs']);
    if (selectedDeviceMaxErrorUs !== undefined ||
        clockPolicy.profileId !== 'trollm.android-selected-emulator-calibrated-clock.v1-proposal' ||
        ![clockPolicy.minimumRateNumerator, clockPolicy.minimumRateDenominator, clockPolicy.maxCalibrationAgeMs]
          .every(n => Number.isSafeInteger(n) && n > 0) ||
        clockPolicy.minimumRateNumerator > clockPolicy.minimumRateDenominator || clockPolicy.maxCalibrationAgeMs > 30000)
      fail('CLOCK_POLICY_INVALID');
    exactClockFields(clockInstaller, ['sourceRevision', 'moduleSha256', 'bindAcquireClock']);
    if (clockInstaller.sourceRevision !== CALIBRATED_CLOCK_SOURCE ||
        clockInstaller.moduleSha256 !== CALIBRATED_CLOCK_MODULE_SHA256 || typeof clockInstaller.bindAcquireClock !== 'function')
      fail('CLOCK_PRIVATE_INSTALLER_REQUIRED');
  } else if (clockInstaller !== undefined) fail('CLOCK_POLICY_REQUIRED');
  if ((await adb(['get-serialno'])).toString().trim() !== options.adb.serial) fail('ADB_SERIAL_MISMATCH');
  const bootId = (await adb(['exec-out', 'cat', '/proc/sys/kernel/random/boot_id'])).toString().trim();
  if (!uuidPattern.test(bootId)) fail('DEVICE_BOOT_ID_INVALID');
  if (clockPolicy !== undefined && (await adb(['shell', 'su', '0', 'cat',
    '/sys/devices/system/clocksource/clocksource0/current_clocksource'])).toString().trim() !== 'kvm-clock')
    fail('CLOCK_PLATFORM_UNSUPPORTED');
  await verifyApk(adb, apk);
  const token = Buffer.from(randomBytes(32).toString('hex'));
  let release, refuse;
  const associated = new Promise((resolve, reject) => { release = resolve; refuse = reject; });
  associated.catch(() => {});
  let binding, nativeRunId, startRequestSha, cleanupCutoff, started = false, reverse = false, stopped = false;
  const control = new AbortController();
  const abort = () => control.abort();
  signal.addEventListener('abort', abort, {once: true});
  if (signal.aborted) control.abort();
  let adapter;
  try {
    // This closure comes from the installed RPA lifetime, outside public options.
    // Binding uses the original actual ADB boot read and precedes native start.
    const acquireClock = clockPolicy === undefined ? undefined :
      clockInstaller.bindAcquireClock({guestBootId: bootId, signal: control.signal});
    if (clockPolicy !== undefined && typeof acquireClock !== 'function') fail('CLOCK_PRIVATE_INSTALLER_REQUIRED');
    adapter = await openGuardAdapter({ready, selectionRef, signal: control.signal, key, cert, token, outputDirectory,
      acquireClock, guestBootId: bootId,
      guard: async request => {
        await associated;
        if (control.signal.aborted) fail('SELECTED_LAUNCHER_CLOSED');
        return await guard(request);
      }});
  } catch (error) {
    signal.removeEventListener('abort', abort);
    throw error;
  }
  let installationKey, common, completion, primaryFailure;
  let stage = 'serialize-installation';
  const cleanupFailures = [];
  async function command(operation) {
    if ((await adb(['exec-out', 'cat', '/proc/sys/kernel/random/boot_id'])).toString().trim() !== bootId)
      fail('DEVICE_BOOT_CHANGED');
    const request = {...common, operation, requestId: randomBytes(16).toString('hex'),
      ...(operation === 'start' ? {flowSha256, installationSha256, input, configurationJson, associationJson} :
        {nativeRunId})};
    const bytes = Buffer.from(JSON.stringify(request)), requestSha256 = sha(bytes);
    await provision(adb, 'selected-launcher', `${requestSha256}.request.json`, bytes);
    if (operation === 'start') {
      if (signal.aborted || Date.now() >= Date.parse(ready.expiresAt)) fail('LAUNCHER_ABORTED_BEFORE_START');
      startRequestSha = requestSha256;
      started = true; // Intent may have been accepted even if adb later loses its reply.
    }
    await adb(['shell', 'am', 'start', '-n', `${packageName}/.MainActivity`, '-a',
      'com.trollm.app.PRIVATE_SELECTED_REQUEST', '-f', '0x20000000', '--es', 'requestSha256', requestSha256]);
    const cutoff = Math.min(cleanupCutoff ?? Date.parse(ready.expiresAt), Date.now() + 30000);
    while (Date.now() < cutoff) {
      const available = await adb(['shell', 'run-as', packageName, 'sh', '-c',
        `'test ! -f files/trollm/selected-launcher/${requestSha256}.done || cat files/trollm/selected-launcher/${requestSha256}.result.json'`]);
      if (available.length) {
        await fs.writeFile(path.join(outputDirectory, `${requestSha256}.launcher-result.json`), available, {flag: 'wx', mode: 0o600});
        const result = JSON.parse(available);
        if (operation === 'start' && result.requestSha256 === requestSha256 &&
            uuidPattern.test(result.nativeRunId ?? result.binding?.nativeRunId))
          nativeRunId = result.nativeRunId ?? result.binding.nativeRunId;
        binding = validateBinding(result, {...common, requestSha256, nativeRunId});
        nativeRunId = binding.nativeRunId;
        return binding;
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    fail('NATIVE_LAUNCHER_REPLY_UNKNOWN');
  }
  const handle = {async binding() { return await command('binding'); },
    async cancel() { return await command('cancel'); },
    async waitInactive() {
      while (Date.now() < (cleanupCutoff ?? Date.parse(ready.expiresAt))) {
        const observed = await command('binding');
        if (!observed.workerActive) { stopped = true; return observed; }
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      fail('NATIVE_WORKER_STOP_UNCONFIRMED');
    }};
  try {
    const privateInput = {schema: clockPolicy === undefined ? 'trollm.private-selected-installation.v1' :
      'trollm.private-selected-installation.v2-proposal', installationSha256, ready,
      guardUrl: `https://localhost:${adapter.port}/guard`, guardToken: token.toString(), guardRootsPem: rootsPem,
      ...(clockPolicy === undefined ? {selectedDeviceMaxErrorUs} : {clockPolicy: {...clockPolicy, guestBootId: bootId}}),
      expectedStaticKeySha256};
    const privateBytes = Buffer.from(JSON.stringify(privateInput));
    installationKey = sha(privateBytes);
    common = {schema: 'trollm.private-selected-launcher-request.v1', apkSha256: apk.sha256, sourceRevision: apk.sourceRevision,
      invocationKey: ready.invocationKey, installationKey};
    stage = 'provision-installation';
    await provision(adb, 'selected-installations', `${installationKey}.json`, privateBytes);
    stage = 'create-reverse';
    await adb(['reverse', '--no-rebind', `tcp:${adapter.port}`, `tcp:${adapter.port}`]); reverse = true;
    stage = 'native-start';
    await command('start');
    release();
    stage = 'retain-launch-custody';
    const {retainOriginalCustody} = require('./selected_native_custody.cjs');
    const launchCustody = await retainOriginalCustody({adb, binding, resultDirectory: outputDirectory, signal,
      phase: 'launch', check: () => { if(signal.aborted || Date.now() >= Date.parse(ready.expiresAt)) fail('LAUNCH_CUSTODY_EXPIRED'); }});
    if (!launchCustody.complete) fail('NATIVE_LAUNCH_CUSTODY_INCOMPLETE');
    stage = 'use-native';
    completion = await useNative(handle);
    stage = 'wait-inactive';
    await handle.waitInactive();
  } catch (error) {
    primaryFailure = closedFailure(error, stage);
  } finally {
    control.abort();
    signal.removeEventListener('abort', abort);
    refuse(new Error('SELECTED_LAUNCHER_CLOSED'));
    cleanupCutoff = Date.now() + 30000; // Local cancel/stop readback only, never renewed effect authority.
    if (started && !nativeRunId && startRequestSha) {
      // Recover only the app-retained genuine ID, never repeat a start whose
      // reply was lost. Missing/partial state remains unresolved.
      try {
        if ((await adb(['exec-out', 'cat', '/proc/sys/kernel/random/boot_id'])).toString().trim() !== bootId)
          fail('DEVICE_BOOT_CHANGED');
        const retained = await adb(['shell', 'run-as', packageName, 'sh', '-c',
          `'test ! -f files/trollm/selected-launcher/${startRequestSha}.started.json || cat files/trollm/selected-launcher/${startRequestSha}.started.json'`]);
        if (retained.length) {
          await fs.writeFile(path.join(outputDirectory, `${startRequestSha}.started.json`), retained, {flag: 'wx', mode: 0o600});
          const original = JSON.parse(retained);
          if (original.invocationKey === ready.invocationKey && uuidPattern.test(original.nativeRunId))
            nativeRunId = original.nativeRunId;
        }
      } catch (error) { cleanupFailures.push(closedFailure(error, 'recover-started-id')); }
    }
    if(started&&!stopped&&startRequestSha){
      try{
        const remaining=cleanupCutoff-Date.now();
        if(remaining<=0)fail('NATIVE_CUSTODY_CLEANUP_EXPIRED');
        const custodySignal=AbortSignal.timeout(remaining);
        const check=()=>{if(custodySignal.aborted||Date.now()>=cleanupCutoff)fail('NATIVE_CUSTODY_CLEANUP_EXPIRED');};
        if((await adb(['exec-out','cat','/proc/sys/kernel/random/boot_id'],{signal:custodySignal})).toString().trim()!==bootId)
          fail('DEVICE_BOOT_CHANGED');
        await require('./selected_native_custody.cjs').retainPartialStartCustody({adb,nativeRunId,
          requestSha256:startRequestSha,installationKey,resultDirectory:outputDirectory,signal:custodySignal,check});
        if((await adb(['exec-out','cat','/proc/sys/kernel/random/boot_id'],{signal:custodySignal})).toString().trim()!==bootId)
          fail('DEVICE_BOOT_CHANGED');
      }catch(error){cleanupFailures.push(closedFailure(error,'retain-partial-start-custody'));}
    }
    if (started && nativeRunId && !stopped) {
      // Cancel once, never start/resend an effect to repair a missing response.
      try { await handle.cancel(); }
      catch (error) { cleanupFailures.push(closedFailure(error, 'cancel-native')); }
      // A lost cancel reply does not justify skipping independent readback.
      try { await handle.waitInactive(); }
      catch (error) { cleanupFailures.push(closedFailure(error, 'wait-inactive-cleanup')); }
    }
    if (stopped && binding) {
      // Passive originals are collected even if useNative/actor signal failed.
      // This uses only the existing bounded cleanup interval, never an effect
      // lease or a renewed start. No retained report is produced here.
      try {
        const remaining = cleanupCutoff - Date.now();
        if (remaining <= 0) fail('NATIVE_CUSTODY_CLEANUP_EXPIRED');
        const custodySignal = AbortSignal.timeout(remaining);
        const current = () => { if (Date.now() >= cleanupCutoff || custodySignal.aborted) fail('NATIVE_CUSTODY_CLEANUP_EXPIRED'); };
        if ((await adb(['exec-out', 'cat', '/proc/sys/kernel/random/boot_id'], {signal: custodySignal})).toString().trim() !== bootId)
          fail('DEVICE_BOOT_CHANGED');
        const root = `files/trollm/runs/${nativeRunId}`;
        const snapshot = await adb(['exec-out', 'run-as', packageName, 'cat', `${root}/state.snapshot.json`], {maxBytes: 262144, signal: custodySignal});
        const journal = await adb(['exec-out', 'run-as', packageName, 'cat', `${root}/events.jsonl`], {maxBytes: 8388608, signal: custodySignal});
        const {retainOriginalCustody} = require('./selected_native_custody.cjs');
        const custody = await retainOriginalCustody({adb, binding, snapshot, journal,
          resultDirectory: outputDirectory, signal: custodySignal, check: current});
        if (!custody.complete) fail('NATIVE_CUSTODY_INCOMPLETE');
        if ((await adb(['exec-out', 'cat', '/proc/sys/kernel/random/boot_id'], {signal: custodySignal})).toString().trim() !== bootId)
          fail('DEVICE_BOOT_CHANGED');
      } catch (error) { cleanupFailures.push(closedFailure(error, 'retain-original-custody')); }
    }
    cleanupFailures.push(...await retireResources(adb, adapter, reverse));
    try {
      await fs.writeFile(path.join(outputDirectory, 'launcher-closure.json'), JSON.stringify({
        schema: 'trollm.private-selected-launcher-closure.v1', invocationKey: ready.invocationKey,
        nativeRunId: nativeRunId ?? null, startMayHaveOccurred: started,
        workerInactiveAcknowledged: stopped, installationKey: installationKey ?? null, apk, bootId, serial: options.adb.serial,
        primaryFailure: primaryFailure ?? null, cleanupFailures}), {flag: 'wx', mode: 0o600});
    } catch (error) { cleanupFailures.push(closedFailure(error, 'write-closure')); }
  }
  const code = started && !stopped ? 'NATIVE_WORKER_STOP_UNCONFIRMED' :
    cleanupFailures.length ? 'SELECTED_LAUNCHER_CLEANUP_FAILED' : primaryFailure?.code;
  if (code) {
    const error = new Error(code);
    Object.assign(error, {code, primaryFailure: primaryFailure ?? null, cleanupFailures,
      nativeRunId: nativeRunId ?? null, startMayHaveOccurred: started, workerInactiveAcknowledged: stopped});
    throw error;
  }
  return completion;
}

module.exports = {withSelectedAndroidDriver, createAdb, validateBinding, verifyApk, identityProjection, retireResources};
