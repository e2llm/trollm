'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const {createHash, createPrivateKey, X509Certificate} = require('node:crypto');
const {createAdb, verifyApk, validateBinding, withSelectedAndroidDriver, identityProjection, retireResources} = require('./selected_native_launcher.cjs');
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const runId = '11111111-2222-4333-8444-555555555555';
const installationKey = 'a'.repeat(64), apkSha256 = 'b'.repeat(64);
const expected = {requestSha256: 'c'.repeat(64), installationKey, apkSha256, sourceRevision: 'd'.repeat(40), invocationKey: 'declared-only'};
function result() {
  return {schema: 'trollm.private-selected-launcher-result.v1', requestSha256: expected.requestSha256,
    apkSha256, sourceRevision: expected.sourceRevision, invocationKey: expected.invocationKey, binding: {
      schema: 'trollm.selected-worker-binding.v1', nativeRunId: runId, workerActive: false,
      privateBindingJson: JSON.stringify({nativeRunId: runId, privateInstallationSha256: installationKey, invocationKey: expected.invocationKey}),
      claimJson: JSON.stringify({nativeRunId: runId, identity: {invocationKey: expected.invocationKey}}),
      sourceAssociationJson: JSON.stringify({nativeRunId: runId})}};
}
test('closed native binding requires genuine associated records and explicit Boolean inactivity', () => {
  assert.equal(validateBinding(result(), expected).workerActive, false);
  for (const change of [r => { r.binding.workerActive = 'false'; }, r => { delete r.binding; },
    r => { r.binding.nativeRunId = 'other'; }, r => { r.apkSha256 = '0'.repeat(64); },
    r => { r.binding.claimJson = JSON.stringify({nativeRunId: runId, identity: {invocationKey: 'other'}}); },
    r => { r.binding.privateBindingJson = JSON.stringify({nativeRunId: runId, privateInstallationSha256: '0'.repeat(64)}); },
    r => { r.binding.sourceAssociationJson = '{}'; }, r => { r.error = 'REFUSED'; }]) {
    const r = result(); change(r); assert.throws(() => validateBinding(r, expected));
  }
  assert.throws(() => validateBinding(result(), {...expected, nativeRunId: 'other'}));
});

test('identity projection never converts opaque u64 tokens into JS numbers or modifies originals', () => {
  const raw = '{"attempt":18446744073709551615,"seq":9007199254740993,"text":"123\\\"456","nested":[-1,2.5e+10]}';
  const projection = identityProjection(raw);
  assert.equal(projection.attempt, '18446744073709551615');
  assert.equal(projection.seq, '9007199254740993');
  assert.equal(projection.text, '123"456');
  assert.deepEqual(projection.nested, ['-1', '2.5e+10']);
  const value = result();
  value.binding.claimJson = value.binding.claimJson.slice(0, -1) + ',"attempt":18446744073709551615}';
  assert.equal(validateBinding(value, expected).claimJson, value.binding.claimJson);
});

test('typed pre-worker native cause survives refusal without becoming a worker binding',()=>{
  const original=result();delete original.binding;
  Object.assign(original,{error:'SELECTED_LAUNCHER_REFUSED',stage:'native-start',
    nativeCause:'SELECTED_BIND_STATIC_KEY_MODE',nativeRunId:runId});
  assert.throws(()=>validateBinding(original,expected),error=>
    error.nativeCause==='SELECTED_BIND_STATIC_KEY_MODE'&&error.nativeStage==='native-start');
  original.nativeCause='private arbitrary text';
  assert.throws(()=>validateBinding(original,expected),error=>error.nativeCause===undefined);
});

async function fixture(mode = 'normal', apkPath = '/data/app/~~declared_random==/com.trollm.app-declared_random==/base.apk') {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'declared-launcher-'));
  const executable = path.join(root, 'adb');
  const custody = require('./fixtures/native-custody.cjs')(runId);
  for (const [name, bytes] of custody.files) {
    const target = path.join(root, 'remote', name);
    await fs.mkdir(path.dirname(target), {recursive: true}); await fs.writeFile(target, bytes);
  }
  // Real child-process/wrapper fixture, not an Android/native-provider double
  // credited as runtime evidence. No native model or effect is executed here.
  const script = `#!/usr/bin/env node
const fs = require('node:fs'), path = require('node:path');
const root = ${JSON.stringify(root)}, mode = ${JSON.stringify(mode)};
const apkPath = ${JSON.stringify(apkPath)};
const args = process.argv.slice(8);
const log = path.join(root,'commands.jsonl');
fs.appendFileSync(log,JSON.stringify(args)+'\\n');
const out = x => process.stdout.write(x);
const remote = p => path.join(root,'remote',p);
if(args[0]==='version') {
 out('Android Debug Bridge version declared\\n');
 if(mode==='custody-version-warning')process.stderr.write('declared version warning');
 if(mode==='custody-version-nonzero'){process.stderr.write('declared version refusal');process.exitCode=6;}
}
else if(args[0]==='features') {
 if(mode==='custody-features-timeout-zero'){process.on('SIGTERM',()=>{out('shell_v2');process.exit(0);});setInterval(()=>{},1000);return;}
 if(mode==='custody-no-shell-v2')out('stat_v2');
 else if(mode==='custody-shell-substring')out('stat_v2,not_shell_v2,shell_v20');
 else if(mode==='custody-features-invalid')out('shell_v2'+String.fromCharCode(0));
 else if(mode==='custody-features-overflow')out('shell_v2\\n'+'a'.repeat(10000));
 else if(mode==='custody-lines')out('stat_v2\\nshell_v2\\n');
 else if(mode==='custody-crlf')out('stat_v2\\r\\nshell_v2\\r\\n');
 else if(mode==='custody-mixed')out('stat_v2, shell_v2\\tcmd\\n');
 else out('shell_v2,stat_v2');
 if(mode==='custody-features-warning')process.stderr.write('declared feature warning');
 if(mode==='custody-features-nonzero'){process.stderr.write('declared features refusal');process.exitCode=7;}
}
else if(args[0]==='get-serialno') out('emulator-declared\\n');
else if(args.join(' ')==='exec-out cat /proc/sys/kernel/random/boot_id') out('${runId}\\n');
else if(args.join(' ')==='shell su 0 cat /sys/devices/system/clocksource/clocksource0/current_clocksource') out('kvm-clock\\n');
else if(args.join(' ')==='shell pm path com.trollm.app') out('package:'+apkPath+'\\n');
else if(args.join(' ')==='exec-out cat '+apkPath) out('declared-apk-bytes');
else if(args[0]==='reverse') {if(mode.includes('reverse-cleanup-failure') && args[1]==='--remove') process.exitCode=5;}
else if(args[0]==='shell'&&args[1]==='-T'&&args[2]==='run-as'&&args[4]==='ls') {
 try{out(fs.readdirSync(remote(args.at(-1))).join('\\n'));}catch(error){process.stderr.write('declared directory missing');process.exitCode=2;}
}
else if(args[0]==='exec-out' && args[1]==='run-as' && args[3]==='readlink') out('/data/user/0/com.trollm.app/'+args.at(-1));
else if(args[0]==='shell' && args[1]==='-T' && args[2]==='run-as' && args[4]==='tar') {
 const archive=require('node:child_process').spawnSync('tar',['--format='+ (mode.startsWith('custody-longlink')?'gnu':'ustar'),'-C',path.join(root,'remote'),'-cf','-',args.at(-1)]);
 let raw=archive.stdout;
 if(args.at(-1).endsWith('.started.json')&&mode.startsWith('custody-frame-')){
  raw=Buffer.from(raw);
  if(mode==='custody-frame-checksum')raw[148]^=1;
  else if(mode==='custody-frame-truncated')raw=raw.subarray(0,511);
  else if(mode==='custody-frame-tail')raw=Buffer.concat([raw,Buffer.from([1])]);
  else{
   if(mode==='custody-frame-path')raw.write('wrong-logical-path',0,'utf8');
   if(mode==='custody-frame-type')raw[156]=50;
   raw.fill(32,148,156);let sum=0;for(let i=0;i<512;i++)sum+=raw[i];
   raw.write(sum.toString(8).padStart(6,'0')+'\\0 ',148,'ascii');
  }
 }
 if(args.at(-1).endsWith('.started.json')&&mode.startsWith('custody-longlink-')){
  raw=Buffer.from(raw);
  const sizeAt=o=>Number.parseInt(raw.subarray(o+124,o+136).toString('ascii'),8);
  const extensionSize=sizeAt(0),memberOffset=512+Math.ceil(extensionSize/512)*512;
  const bodySize=sizeAt(memberOffset),bodyEnd=memberOffset+512+Math.ceil(bodySize/512)*512;
  const checksum=o=>{raw.fill(32,o+148,o+156);let sum=0;for(let i=o;i<o+512;i++)sum+=raw[i];raw.write(sum.toString(8).padStart(6,'0')+String.fromCharCode(0)+' ',o+148,'ascii');};
  const size=(o,value)=>{raw.write(value.toString(8).padStart(11,'0')+String.fromCharCode(0),o+124,'ascii');checksum(o);};
  if(mode==='custody-longlink-checksum')raw[148]^=1;
  else if(mode==='custody-longlink-regular-checksum')raw[memberOffset+148]^=1;
  else if(mode==='custody-longlink-foreign-name')raw[512]^=1;
  else if(mode==='custody-longlink-foreign-header'){raw[memberOffset]^=1;checksum(memberOffset);}
  else if(mode==='custody-longlink-metadata-name'){raw[0]^=1;checksum(0);}
  else if(mode==='custody-longlink-link-target'){raw[156]=75;checksum(0);}
  else if(mode==='custody-longlink-double')raw=Buffer.concat([raw.subarray(0,memberOffset),raw]);
  else if(mode==='custody-longlink-extra-member')raw=Buffer.concat([raw.subarray(0,bodyEnd),raw.subarray(memberOffset)]);
  else if(mode==='custody-longlink-size')size(0,extensionSize-1);
  else if(mode==='custody-longlink-oversize')size(0,4097);
  else if(mode==='custody-longlink-body-oversize')size(memberOffset,33554433);
  else if(mode==='custody-longlink-unterminated')raw[512+extensionSize-1]=97;
  else if(mode==='custody-longlink-padding')raw[512+extensionSize]=1;
  else if(mode==='custody-longlink-body-padding')raw[memberOffset+512+bodySize]=1;
  else if(mode==='custody-longlink-truncated')raw=raw.subarray(0,memberOffset+511);
  else if(mode==='custody-longlink-missing-trailer')raw=raw.subarray(0,bodyEnd+512);
  else if(mode==='custody-longlink-trailer')raw[raw.length-1]=1;
  else if(mode==='custody-longlink-prefix'){raw.write('ustar'+String.fromCharCode(0)+'00',memberOffset+257,'ascii');raw.write('foreign',memberOffset+345,'ascii');checksum(memberOffset);}
  else if(mode==='custody-longlink-regular-link'){raw.write('foreign',memberOffset+157,'ascii');checksum(memberOffset);}
 }
 out(raw);
 if(mode==='custody-unreadable'&&args.at(-1).endsWith('.started.json')){process.stderr.write('declared remote permission refusal');process.exitCode=13;}
 else if(archive.status!==0){process.stderr.write(archive.stderr);process.exitCode=archive.status;}
}
else if(args[0]==='exec-out' && args[1]==='run-as') out(fs.readFileSync(remote(args[4])));
else if(args[0]==='shell' && args[1]==='run-as') {
 const script=args[5];
 if(script.includes('umask')) {
  if(mode==='provision-failure') {process.stderr.write('private fixture failure detail');process.exit(3);}
  const p=script.match(/cat > ([a-z0-9./-]+)'$/)[1];
  const chunks=[];process.stdin.on('data',x=>chunks.push(x));process.stdin.on('end',()=>{
   fs.mkdirSync(path.dirname(remote(p)),{recursive:true});fs.writeFileSync(remote(p),Buffer.concat(chunks),{flag:'wx',mode:0o600});
  });
 } else {
  const p=script.match(/cat ([a-z0-9./-]+)'$/)[1];if(fs.existsSync(remote(p))) out(fs.readFileSync(remote(p)));
  else if(mode.includes('lost-start')&&p.endsWith('.result.json'))fs.writeFileSync(path.join(root,'empty-reply-observed'),p);
 }
} else if(args[0]==='shell' && args[1]==='am') {
 const hash=args.at(-1), dir='files/trollm/selected-launcher/';
 const request=JSON.parse(fs.readFileSync(remote(dir+hash+'.request.json')));
 if(request.operation==='start')fs.writeFileSync(remote(dir+'invocation-'+require('node:crypto').createHash('sha256').update(request.invocationKey).digest('hex')+'.claim'),hash);
 if(mode==='pre-bind-refusal') {
  fs.writeFileSync(remote(dir+hash+'.claim'),Buffer.from([1]));
  fs.writeFileSync(remote(dir+hash+'.result.json'),JSON.stringify({schema:'trollm.private-selected-launcher-result.v1',
   requestSha256:hash,apkSha256:request.apkSha256,invocationKey:request.invocationKey,sourceRevision:request.sourceRevision,
   error:'SELECTED_LAUNCHER_REFUSED',stage:'native-start',nativeCause:'SELECTED_BIND_STATIC_KEY_MODE'}));
  fs.writeFileSync(remote(dir+hash+'.done'),Buffer.from([1]));process.exit(0);
 }
 if(mode==='cancel-failure' && request.operation==='cancel') process.exit(6);
 if(mode==='live-until-cancel' && request.operation==='cancel')fs.writeFileSync(remote('cancel-observed'),Buffer.from('1'));
 const id='${runId}';
 let binding={schema:'trollm.selected-worker-binding.v1',nativeRunId:id,workerActive:request.operation==='start'||(mode==='live-until-cancel'&&!fs.existsSync(remote('cancel-observed'))),
  privateBindingJson:JSON.stringify({nativeRunId:id,privateInstallationSha256:request.installationKey,invocationKey:request.invocationKey}),
  claimJson:JSON.stringify({nativeRunId:id,identity:{invocationKey:request.invocationKey}}),sourceAssociationJson:${JSON.stringify(custody.binding.sourceAssociationJson)}};
 for(const [name,value]of [['selected-invocation.json',binding.claimJson],['selected-private-installation.json',binding.privateBindingJson],['source-association.json',binding.sourceAssociationJson]])fs.writeFileSync(remote('files/trollm/runs/'+id+'/'+name),value);
 const result={schema:'trollm.private-selected-launcher-result.v1',requestSha256:hash,apkSha256:request.apkSha256,invocationKey:request.invocationKey,sourceRevision:request.sourceRevision,binding};
 if(mode==='drain-refusal' && request.operation==='binding')binding.workerActive='unavailable';
 if(mode==='binding-failure' && request.operation==='start') {delete result.binding;result.nativeRunId=id;result.error='SELECTED_LAUNCHER_REFUSED';result.stage='native-binding';}
 if(!mode.includes('lost-start') || request.operation!=='start') fs.writeFileSync(remote(dir+hash+'.result.json'),JSON.stringify(result));
} else {process.stderr.write('fixture unknown command');process.exitCode=3;}
`;
  await fs.writeFile(executable, script, {mode: 0o700});
  const adbConfig = {executable, serverHost: 'declared.invalid', serverPort: 5037, serial: 'emulator-declared', timeoutMs: 2000};
  return {root, adbConfig, adb: createAdb(adbConfig), async cleanup() { await fs.rm(root, {recursive: true}); }};
}
async function tls() {
  const text = await fs.readFile(path.join(__dirname, '../../crates/trollm-mcp/tests/common/tls_fixture.rs'), 'utf8');
  const der = name => Buffer.from(new RegExp(`pub const ${name}: &\\[u8\\] = &\\[([\\s\\S]*?)\\];`).exec(text)[1]
    .split(',').map(x => x.trim()).filter(Boolean).map(Number));
  return {rootsPem: /pub const ROOT_1_PEM: &\[u8\] = br"([\s\S]*?)";/.exec(text)[1],
    cert: new X509Certificate(der('LEAF_1_DER')).toString(),
    key: createPrivateKey({key: der('LEAF_1_KEY_DER'), format: 'der', type: 'pkcs8'}).export({format: 'pem', type: 'pkcs8'})};
}
async function options(f, duration = 10000) {
  return {...await tls(), adb: f.adbConfig, apk: {sha256: sha('declared-apk-bytes'), sizeBytes: 18, sourceRevision: 'd'.repeat(40)},
    ready: {invocationKey: 'declared-only', expiresAt: new Date(Date.now() + duration).toISOString(), owner: {tenantId: 'declared'}},
    selectionRef: {tenantId: 'declared', artifactId: 'art_' + 'a'.repeat(32), sha256: 'b'.repeat(64)},
    signal: new AbortController().signal, guard: async () => { throw Error('No native provider in this fixture'); },
    outputDirectory: path.join(f.root, 'private-output'), flowSha256: 'd'.repeat(64), installationSha256: 'e'.repeat(64),
    input: '{}', configurationJson: '{}', associationJson: '{}', selectedDeviceMaxErrorUs: 100,
    expectedStaticKeySha256: 'f'.repeat(64)};
}
test('real child boundary streams installed APK and refuses mismatch before provisioning', async () => {
  const f = await fixture();
  try {
    assert.deepEqual(await verifyApk(f.adb, {sha256: sha('declared-apk-bytes'), sizeBytes: 18, sourceRevision: 'd'.repeat(40)}), {sha256: sha('declared-apk-bytes'), sizeBytes: 18});
    await assert.rejects(withSelectedAndroidDriver({...await options(f), apk: {sha256: '0'.repeat(64), sizeBytes: 18, sourceRevision: 'd'.repeat(40)}}, async () => {}), /INSTALLED_APK_MISMATCH/);
    const commands = await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8');
    assert(!commands.includes('run-as')); assert(!commands.includes('reverse'));
  } finally { await f.cleanup(); }
});

test('pm path accepts legacy and Android randomized directories but refuses split and traversal', async () => {
  const apk = {sha256: sha('declared-apk-bytes'), sizeBytes: 18, sourceRevision: 'd'.repeat(40)};
  for (const apkPath of ['/data/app/declared/base.apk',
    '/data/app/~~declared_random==/com.trollm.app-declared_random==/base.apk']) {
    const f = await fixture('normal', apkPath);
    try {
      assert.deepEqual(await verifyApk(f.adb, apk), {sha256: apk.sha256, sizeBytes: apk.sizeBytes});
      const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
      assert.deepEqual(commands.at(-1), ['exec-out', 'cat', apkPath]);
    } finally { await f.cleanup(); }
  }
  for (const apkPath of ['/data/app/declared/base.apk\npackage:/data/app/declared/split_config.apk',
    '/data/app/../elsewhere/base.apk', '/data/app/declared/split_config.apk', '/outside/declared/base.apk']) {
    const f = await fixture('normal', apkPath);
    try {
      await assert.rejects(verifyApk(f.adb, apk), /SINGLE_INSTALLED_APK_REQUIRED/);
      const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
      assert.equal(commands.length, 1); // Refuse before reading any candidate APK.
    } finally { await f.cleanup(); }
  }
});
test('composition provisions via stdin, starts once, binds same run and requires inactive readback', async () => {
  const f = await fixture();
  try {
    const o = await options(f);
    assert.equal(await withSelectedAndroidDriver(o, async handle => {
      assert.equal((await handle.binding()).nativeRunId, runId);
      return 'component-only';
    }), 'component-only');
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.workerInactiveAcknowledged, true); assert.equal(closure.nativeRunId, runId);
    const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    const requests = [];
    for (const c of commands.filter(c => c[1] === 'am')) requests.push(JSON.parse(await fs.readFile(path.join(f.root,
      'remote/files/trollm/selected-launcher', c.at(-1) + '.request.json'))));
    assert.equal(requests.filter(r => r.operation === 'start').length, 1);
    const installation = JSON.parse(await fs.readFile(path.join(f.root, 'remote/files/trollm/selected-installations', closure.installationKey + '.json')));
    assert.equal(installation.guardUrl.includes('https://localhost:'), true);
    assert.equal(commands.some(c => JSON.stringify(c).includes(installation.guardToken)), false);
  } finally { await f.cleanup(); }
});
test('post-start binding failure preserves genuine run ID for cancel and never repeats start', async () => {
  const f = await fixture('binding-failure');
  try {
    const o = await options(f);
    await assert.rejects(withSelectedAndroidDriver(o, async () => assert.fail('must not release caller')), error => {
      assert.equal(error.code, 'LAUNCHER_RESULT_REFUSED');
      assert.deepEqual(error.primaryFailure, {stage: 'native-start', code: 'LAUNCHER_RESULT_REFUSED', nativeStage: 'native-binding'});
      return true;
    });
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.nativeRunId, runId); assert.equal(closure.workerInactiveAcknowledged, true);
  } finally { await f.cleanup(); }
});

test('lost start response at original phase cutoff is UNKNOWN with no repeated start or false stop', async () => {
  const f = await fixture('lost-start');
  try {
    const o = await options(f, 2000);
    await assert.rejects(withSelectedAndroidDriver(o, async () => assert.fail('must not release caller')), error => {
      assert.equal(error.code, 'NATIVE_WORKER_STOP_UNCONFIRMED');
      assert.deepEqual(error.primaryFailure, {stage: 'native-start', code: 'NATIVE_POLL_PHASE_EXPIRED'});
      return true;
    });
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.startMayHaveOccurred, true);
    assert.equal(closure.nativeRunId, null);
    assert.equal(closure.workerInactiveAcknowledged, false);
    assert.equal(closure.primaryFailure.code, 'NATIVE_POLL_PHASE_EXPIRED');
    const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    assert.equal(commands.filter(c => c[1] === 'am').length, 1);
    assert.equal(commands.filter(c => c[0] === 'reverse' && c[1] === '--remove').length, 1);
  } finally { await f.cleanup(); }
});

for (const reverseFailure of [false, true]) {
  test(`lost start reply-only window is reached before authority expiry (reverse failure=${reverseFailure})`, async t => {
    const f = await fixture('lost-start' + (reverseFailure ? '+reverse-cleanup-failure' : ''));
    try {
      // Advance only after the real child has read an absent result. This
      // exercises the shorter reply window inside a predeclared 60s phase,
      // without waiting 30s or changing a production timeout.
      const o = await options(f, 60000), originalNow = Date.now;
      const marker = path.join(f.root, 'empty-reply-observed');
      t.mock.method(Date, 'now', () => originalNow() + (require('node:fs').existsSync(marker) ? 31000 : 0));
      await assert.rejects(withSelectedAndroidDriver(o, async () => assert.fail('must not release caller')), error => {
        assert.equal(error.code, 'NATIVE_WORKER_STOP_UNCONFIRMED');
        assert.deepEqual(error.primaryFailure, {stage: 'native-start', code: 'NATIVE_LAUNCHER_REPLY_UNKNOWN'});
        assert.equal(error.workerInactiveAcknowledged, false);
        assert.deepEqual(error.cleanupFailures.map(e => [e.stage, e.childExitCode]),
          reverseFailure ? [['remove-reverse', 5]] : []);
        return true;
      });
      assert(Date.now() < Date.parse(o.ready.expiresAt), 'original authority is still live');
      const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
      assert.equal(commands.filter(c => c[1] === 'am').length, 1);
      const resultPath = await fs.readFile(marker, 'utf8');
      await assert.rejects(fs.stat(path.join(f.root, 'remote', resultPath)), {code: 'ENOENT'});
      const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
      assert.equal(closure.startMayHaveOccurred, true);
      assert.equal(closure.nativeRunId, null);
      assert.equal(closure.workerInactiveAcknowledged, false);
      assert.equal(closure.primaryFailure.code, 'NATIVE_LAUNCHER_REPLY_UNKNOWN');
    } finally { t.mock.restoreAll(); await f.cleanup(); }
  });
}

test('real child pre-bind refusal retains exact request installation and reservation without a guessed run',async()=>{
 const f=await fixture('pre-bind-refusal');
 try{
  const o=await options(f);
  await assert.rejects(withSelectedAndroidDriver(o,async()=>assert.fail('must not release caller')),error=>{
   assert.equal(error.code,'NATIVE_WORKER_STOP_UNCONFIRMED');
   assert.equal(error.primaryFailure.nativeCause,'SELECTED_BIND_STATIC_KEY_MODE');
   assert.equal(error.primaryFailure.nativeStage,'native-start');return true;
  });
  const index=JSON.parse(await fs.readFile(path.join(o.outputDirectory,'partial-start-custody/index.json')));
  assert.equal(index.nativeRunId,null);assert.equal(index.workerInactiveAcknowledged,false);assert.equal(index.complete,false);
  const read=async suffix=>{const member=index.members.find(m=>m.logicalPath.endsWith(suffix));assert(member);
   const bytes=await fs.readFile(path.join(o.outputDirectory,'partial-start-custody',member.file));
   assert.equal(sha(bytes),member.sha256);return bytes;};
  const request=JSON.parse(await read('.request.json'));
  assert.equal(request.installationKey,index.installationKey);
  assert.equal(sha(await read(`/selected-installations/${index.installationKey}.json`)),index.installationKey);
  await read('.claim');await read('.done');await read('.result.json');
  const commands=(await fs.readFile(path.join(f.root,'commands.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);
  assert.equal(commands.filter(c=>c[1]==='am').length,1);
  assert(!index.members.some(m=>m.logicalPath.includes('/runs/')));
 }finally{await f.cleanup();}
});

test('primary private provisioning error survives resource retirement with no start', async () => {
  const f = await fixture('provision-failure');
  try {
    const o = await options(f);
    await assert.rejects(withSelectedAndroidDriver(o, async () => assert.fail('no caller release')), error => {
      assert.equal(error.code, 'ADB_COMMAND_FAILED');
      assert.equal(error.primaryFailure.stage, 'provision-installation');
      assert.equal(error.primaryFailure.childExitCode, 3);
      assert.equal(error.startMayHaveOccurred, false);
      assert(!JSON.stringify(error).includes('private fixture failure detail'));
      return true;
    });
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.primaryFailure.childExitCode, 3);
    assert.equal(closure.startMayHaveOccurred, false);
  } finally { await f.cleanup(); }
});

test('useNative primary error and reverse cleanup failure are both retained after real inactive readback', async () => {
  const f = await fixture('reverse-cleanup-failure');
  try {
    const o = await options(f);
    await assert.rejects(withSelectedAndroidDriver(o, async () => {
      throw Object.assign(new Error('private callback details must stay private'), {code: 'CONTROLLED_NATIVE_FAILURE'});
    }), error => {
      assert.equal(error.code, 'SELECTED_LAUNCHER_CLEANUP_FAILED');
      assert.deepEqual(error.primaryFailure, {stage: 'use-native', code: 'CONTROLLED_NATIVE_FAILURE'});
      assert.equal(error.workerInactiveAcknowledged, true);
      assert.deepEqual(error.cleanupFailures.map(e => [e.stage, e.childExitCode]), [['remove-reverse', 5]]);
      assert(!JSON.stringify(error).includes('private callback details'));
      return true;
    });
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.primaryFailure.code, 'CONTROLLED_NATIVE_FAILURE');
    assert.equal(closure.cleanupFailures[0].childExitCode, 5);
  } finally { await f.cleanup(); }
});

test('adapter close failure is collected even after reverse failure, without escaping retirement', async () => {
  const failures = await retireResources(async () => { throw Object.assign(new Error('private child text'), {code: 'CONTROLLED_REVERSE_FAILURE'}); },
    {port: 12345, async close() { throw Object.assign(new Error('private socket text'), {code: 'CONTROLLED_CLOSE_FAILURE'}); }}, true);
  assert.deepEqual(failures, [{stage: 'remove-reverse', code: 'CONTROLLED_REVERSE_FAILURE'},
    {stage: 'close-adapter', code: 'CONTROLLED_CLOSE_FAILURE'}]);
  assert(!JSON.stringify(failures).includes('private'));
});

test('UNKNOWN stop keeps original phase expiry and cleanup failure together', async () => {
  const f = await fixture('lost-start+reverse-cleanup-failure');
  try {
    const o = await options(f, 2000);
    await assert.rejects(withSelectedAndroidDriver(o, async () => assert.fail('no native caller release')), error => {
      assert.equal(error.code, 'NATIVE_WORKER_STOP_UNCONFIRMED');
      assert.equal(error.workerInactiveAcknowledged, false);
      assert.equal(error.primaryFailure.code, 'NATIVE_POLL_PHASE_EXPIRED');
      assert.deepEqual(error.cleanupFailures.map(e => [e.stage, e.childExitCode]), [['remove-reverse', 5]]);
      return true;
    });
    const closure = JSON.parse(await fs.readFile(path.join(o.outputDirectory, 'launcher-closure.json')));
    assert.equal(closure.primaryFailure.code, 'NATIVE_POLL_PHASE_EXPIRED');
    assert.equal(closure.cleanupFailures[0].childExitCode, 5);
  } finally { await f.cleanup(); }
});

test('failed cancel command still performs independent same-worker inactivity readback', async () => {
  const f = await fixture('cancel-failure');
  try {
    const o = await options(f);
    await assert.rejects(withSelectedAndroidDriver(o, async () => {
      throw Object.assign(new Error('private caller text'), {code: 'CONTROLLED_NATIVE_FAILURE'});
    }), error => {
      assert.equal(error.code, 'SELECTED_LAUNCHER_CLEANUP_FAILED');
      assert.equal(error.primaryFailure.code, 'CONTROLLED_NATIVE_FAILURE');
      assert.equal(error.workerInactiveAcknowledged, true);
      assert.deepEqual(error.cleanupFailures.map(e => [e.stage, e.childExitCode]), [['cancel-native', 6]]);
      return true;
    });
    const commands = (await fs.readFile(path.join(f.root, 'commands.jsonl'), 'utf8')).trim().split('\n').map(JSON.parse);
    const operations = [];
    for (const c of commands.filter(c => c[1] === 'am')) operations.push(JSON.parse(await fs.readFile(path.join(f.root,
      'remote/files/trollm/selected-launcher', c.at(-1) + '.request.json'))).operation);
    assert.deepEqual(operations, ['start', 'cancel', 'binding']);
  } finally { await f.cleanup(); }
});

test('private child failures expose exit classification without stderr or private input', async () => {
  const f = await fixture();
  try {
    await assert.rejects(f.adb(['unknown-command'], {input: Buffer.from('private-input-never-argv')}), error => {
      assert.equal(error.code, 'ADB_COMMAND_FAILED');
      assert.equal(error.childExitCode, 3); assert(error.stderrBytes > 0);
      assert(!String(error).includes('private-input')); assert(!String(error).includes('fixture unknown'));
      return true;
    });
    await assert.rejects(verifyApk(f.adb, {sha256: sha('declared-apk-bytes'), sizeBytes: 1, sourceRevision: 'd'.repeat(40)}),
      /ADB_COMMAND_FAILED/);
    assert.throws(() => createAdb({...f.adbConfig, serial: 'shell;injection'}), /ADB_CONFIGURATION_INVALID/);
  } finally { await f.cleanup(); }
});


test('calibrated installation binds actual original guest boot before sole start outside public options', async () => {
  const f = await fixture(); let bound = 0;
  try {
    const input = await options(f);
    delete input.selectedDeviceMaxErrorUs;
    input.clockPolicy = {profileId: 'trollm.android-selected-emulator-calibrated-clock.v1-proposal',
      minimumRateNumerator: 1, minimumRateDenominator: 2, maxCalibrationAgeMs: 10000}; // Declared control only.
    const installer = {sourceRevision: '9c97f57a22e9b84256aecc9f6b3bf70b4bd97113',
      moduleSha256: 'a93523bbf4db75230a44f01b923f3e6f23e8f48fe27c6bdce15c6b5ee4d853f3',
      bindAcquireClock({guestBootId, signal}) {
        bound++; assert.equal(guestBootId, runId); assert.equal(signal.aborted, false);
        return async () => { throw Error('No native calibration in declared launcher fixture'); };
      }};
    await withSelectedAndroidDriver(input, async () => {
      assert.equal(bound, 1);
      const directory = path.join(f.root, 'remote/files/trollm/selected-installations');
      const files = await fs.readdir(directory); assert.equal(files.length, 1);
      const original = JSON.parse(await fs.readFile(path.join(directory, files[0]), 'utf8'));
      assert.equal(original.schema, 'trollm.private-selected-installation.v2-proposal');
      assert.equal(original.clockPolicy.guestBootId, runId);
      assert.equal(Object.hasOwn(original, 'selectedDeviceMaxErrorUs'), false);
    }, installer);
    assert.equal(bound, 1);
  } finally { await f.cleanup(); }
});
test('calibrated policy refuses missing installer or public callback override before ADB', async () => {
  const f = await fixture();
  try {
    const input = await options(f); delete input.selectedDeviceMaxErrorUs;
    input.clockPolicy = {profileId: 'trollm.android-selected-emulator-calibrated-clock.v1-proposal',
      minimumRateNumerator: 1, minimumRateDenominator: 2, maxCalibrationAgeMs: 10000};
    await assert.rejects(withSelectedAndroidDriver(input, async () => {}));
    await assert.rejects(withSelectedAndroidDriver({...input, bindAcquireClock() {}}, async () => {}), /CLOCK_PRIVATE_INSTALLER_REQUIRED/);
    assert.equal(await fs.stat(path.join(f.root, 'commands.jsonl')).then(() => true, () => false), false);
  } finally { await f.cleanup(); }
});

test('aborted ADB child is actually reaped even when it ignores TERM', async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'owned-adb-abort-'));
  const controller = new AbortController(); let pending;
  try {
    const executable = path.join(root, 'adb'), pidFile = path.join(root, 'pid');
    await fs.writeFile(executable, '#!'+process.execPath+'\n'+
      'process.on("SIGTERM",()=>{});require("node:fs").writeFileSync('+JSON.stringify(pidFile)+',String(process.pid));setInterval(()=>{},1000);\n', {mode:0o700});
    const adb = createAdb({executable,serverHost:'127.0.0.1',serverPort:5037,serial:'declared',timeoutMs:3000});
    pending = adb(['declared-no-device'], {signal:controller.signal});
    // Attach rejection handling before any asynchronous readiness wait.
    const settled = pending.then(()=>({code:'UNEXPECTED_SUCCESS'}), error=>error);
    const cutoff = Date.now()+2000;
    while(!(await fs.stat(pidFile).catch(()=>null))) {
      assert(Date.now()<cutoff); await new Promise(resolve=>setTimeout(resolve,5));
    }
    const pid = Number(await fs.readFile(pidFile,'utf8'));
    controller.abort();
    const failure = await settled;
    assert.equal(failure.code,'ADB_ABORTED');assert.equal(failure.childSignal,'SIGKILL');
    assert.throws(()=>process.kill(pid,0),error=>error.code==='ESRCH');
  } finally {
    controller.abort(); if(pending)await pending.catch(()=>{});
    await fs.rm(root,{recursive:true,force:true});
  }
});

for(const mode of ['custody-binary','custody-lines','custody-crlf','custody-mixed','custody-version-warning','custody-version-nonzero','custody-features-warning','custody-missing','custody-unreadable','custody-no-shell-v2','custody-shell-substring','custody-features-invalid','custody-features-nonzero','custody-features-overflow','custody-features-timeout-zero','custody-frame-checksum','custody-frame-truncated','custody-frame-tail','custody-frame-path','custody-frame-type',
 'custody-longlink','custody-longlink-binary','custody-longlink-checksum','custody-longlink-regular-checksum',
 'custody-longlink-foreign-name','custody-longlink-foreign-header','custody-longlink-metadata-name',
 'custody-longlink-link-target','custody-longlink-double','custody-longlink-extra-member','custody-longlink-size',
 'custody-longlink-oversize','custody-longlink-body-oversize','custody-longlink-unterminated',
 'custody-longlink-padding','custody-longlink-body-padding','custody-longlink-truncated',
 'custody-longlink-missing-trailer','custody-longlink-trailer','custody-longlink-prefix','custody-longlink-regular-link']){
 test(`actual child custody ${mode} requires remote status and binary framing`,async()=>{
  const f=await fixture(mode);
  try{
   const {retainPartialStartCustody}=require('./selected_native_custody.cjs');
   const original=Buffer.from([0,255,10,...Buffer.from('cat: missing file: No such file or directory\r\n'),0,13]);
   const installation=Buffer.from('{"declared":"installation"}'),key=sha(installation);
   const request=Buffer.from(JSON.stringify({installationKey:key,invocationKey:'declared-binary-test'})),requestHash=sha(request);
   const dir=path.join(f.root,'remote/files/trollm/selected-launcher');await fs.mkdir(dir,{recursive:true});
   await fs.mkdir(path.join(f.root,'remote/files/trollm/selected-installations'),{recursive:true});
   await fs.writeFile(path.join(dir,requestHash+'.request.json'),request);
   await fs.writeFile(path.join(f.root,'remote/files/trollm/selected-installations',key+'.json'),installation);
   const logical=`files/trollm/selected-launcher/${requestHash}.started.json`;
   if(mode!=='custody-missing')await fs.writeFile(path.join(f.root,'remote',logical),original);
   const output=path.join(f.root,'custody-output');await fs.mkdir(output);
   const index=await retainPartialStartCustody({adb:f.adb,requestSha256:requestHash,installationKey:key,
    resultDirectory:output,signal:new AbortController().signal,check:()=>{}});
   const member=index.members.find(m=>m.logicalPath===logical);
   assert.equal(index.complete,false);assert.equal(index.workerInactiveAcknowledged,false);
   for(const probe of index.adbProbes){
    assert.equal(probe.command.executable,f.adbConfig.executable);
    assert.deepEqual(probe.command.argv.slice(0,6),['-H','declared.invalid','-P','5037','-s','emulator-declared']);
    for(const stream of probe.streams){
     const raw=await fs.readFile(path.join(output,'partial-start-custody',stream.file));
     assert.equal(sha(raw),stream.sha256);assert.equal(raw.length,stream.sizeBytes);
    }
   }
   assert.deepEqual(index.adbProbes.map(p=>p.probe),['version','features']);
   assert(index.members.every(m=>!m.logicalPath.startsWith('adb/')));
   const commands=(await fs.readFile(path.join(f.root,'commands.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);
   assert(commands.filter(args=>args.includes('tar')).every(args=>args[0]==='shell'&&args[1]==='-T'&&!args.includes('-x')));
   const positives=['custody-binary','custody-lines','custody-crlf','custody-mixed','custody-version-warning','custody-version-nonzero','custody-features-warning','custody-longlink','custody-longlink-binary'];
   if(positives.includes(mode)){
    assert(member);assert.deepEqual(await fs.readFile(path.join(output,'partial-start-custody',member.file)),original);
   }else{
    assert.equal(member,undefined);
    if(['custody-no-shell-v2','custody-shell-substring','custody-features-nonzero','custody-features-overflow','custody-features-timeout-zero'].includes(mode)){
     assert(index.missing.some(m=>m.code==='NATIVE_CUSTODY_REMOTE_STATUS_UNAVAILABLE'));
     assert(!commands.some(args=>args.includes('tar')));
    }else if(mode==='custody-features-invalid'){
     assert(index.missing.some(m=>m.code==='NATIVE_CUSTODY_ADB_FEATURES_INVALID'));
     assert(!commands.some(args=>args.includes('tar')));
    }else if(mode.startsWith('custody-frame-')||mode.startsWith('custody-longlink-')){
     assert(index.missing.some(m=>m.logicalPath===logical&&m.code==='NATIVE_CUSTODY_FRAME_INVALID'));
     assert(index.diagnostics.some(m=>m.logicalPath===logical&&m.meaning==='failed-read-transport-not-file-original'));
    }else{
     assert(index.missing.some(m=>m.logicalPath===logical&&m.code==='ADB_COMMAND_FAILED'));
     const stderr=index.diagnostics.find(m=>m.logicalPath===logical&&m.stream==='stderr');assert(stderr);
     assert((await fs.readFile(path.join(output,'partial-start-custody',stderr.file))).length>0);
    }
   }
   if(mode==='custody-features-nonzero'){assert.equal(index.adbProbes[1].childExitCode,7);assert.equal(index.adbProbes[1].status,'unavailable');}
   if(mode==='custody-features-overflow'){assert.equal(index.adbProbes[1].outputTruncated,true);assert(index.adbProbes[1].streams.every(s=>s.sizeBytes<=8192));}
   if(mode==='custody-features-timeout-zero'){assert.equal(index.adbProbes[1].timedOut,true);assert.equal(index.adbProbes[1].childExitCode,0);}
   if(mode==='custody-version-nonzero'){assert.equal(index.adbProbes[0].childExitCode,6);assert(index.missing.some(m=>m.logicalPath==='adb/version'));}
  }finally{await f.cleanup();}
 });
}


test('aborted original inactivity poll is joined before cleanup cancel and independent drain', async()=>{
 const f=await fixture('live-until-cancel');
 try{
  const o=await options(f),abort=new AbortController();o.signal=abort.signal;
  let originalPoll,settled=false;
  await assert.rejects(withSelectedAndroidDriver(o,async handle=>{
   originalPoll=handle.waitInactive();originalPoll.then(()=>{settled=true;},()=>{settled=true;});
   await new Promise(resolve=>setTimeout(resolve,150));
   abort.abort();
   throw Object.assign(new Error('declared original cutoff'),{code:'CONTROLLED_ORIGINAL_CUTOFF'});
  }),error=>{assert.equal(error.primaryFailure.code,'CONTROLLED_ORIGINAL_CUTOFF');assert.equal(error.workerInactiveAcknowledged,true);return true;});
  assert(settled);await assert.rejects(originalPoll);
  const phaseFile=path.join(o.outputDirectory,'launcher-retirement.private.json');
  const phases=JSON.parse(await fs.readFile(phaseFile));
  assert.equal((await fs.stat(phaseFile)).mode&0o777,0o600);
  assert.equal(phases.effectAuthorityRenewed,false);assert.equal(phases.workerInactiveAcknowledged,true);
  assert.equal(phases.originalEffectCutoffUtcMs,Date.parse(o.ready.expiresAt));
  assert(phases.events.length<=16);assert((await fs.readFile(phaseFile)).length<=16384);
  const names=phases.events.map(e=>e.event);
  assert(names.indexOf('productive-joined')<names.indexOf('cancel-requested'));
  assert(names.indexOf('drain-finished')<names.indexOf('custody-budget-start'));
  for(let i=1;i<phases.events.length;i++)assert(phases.events[i].elapsedMs>=phases.events[i-1].elapsedMs);

  const commands=(await fs.readFile(path.join(f.root,'commands.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);
  const operations=[];
  for(const c of commands.filter(c=>c[1]==='am'))operations.push(JSON.parse(await fs.readFile(path.join(f.root,'remote/files/trollm/selected-launcher',c.at(-1)+'.request.json'))).operation);
  assert.equal(operations.filter(x=>x==='start').length,1);assert.equal(operations.filter(x=>x==='cancel').length,1);
  assert.equal(operations.at(-1),'binding');
  const original=await fs.readFile(path.join(f.root,'commands.jsonl'));await new Promise(resolve=>setTimeout(resolve,150));
  assert.deepEqual(await fs.readFile(path.join(f.root,'commands.jsonl')),original);
 }finally{await f.cleanup();}
});


test('missing authentic drain reply retains unconfirmed flags and private phases without another start',async()=>{
 const f=await fixture('drain-refusal');
 try{
  const o=await options(f);
  await assert.rejects(withSelectedAndroidDriver(o,async()=>{throw Object.assign(Error('declared cutoff'),{code:'CONTROLLED_ORIGINAL_CUTOFF'});}),error=>{
   assert.equal(error.code,'NATIVE_WORKER_STOP_UNCONFIRMED');assert.equal(error.startMayHaveOccurred,true);
   assert.equal(error.workerInactiveAcknowledged,false);assert.equal(error.primaryFailure.code,'CONTROLLED_ORIGINAL_CUTOFF');
   assert(error.cleanupFailures.some(e=>e.stage==='wait-inactive-cleanup'));return true;
  });
  const closure=JSON.parse(await fs.readFile(path.join(o.outputDirectory,'launcher-closure.json')));
  const phases=JSON.parse(await fs.readFile(path.join(o.outputDirectory,'launcher-retirement.private.json')));
  assert.equal(closure.workerInactiveAcknowledged,false);assert.equal(phases.workerInactiveAcknowledged,false);
  assert.equal(phases.events.find(e=>e.event==='drain-finished').workerInactiveAcknowledged,false);
  assert.equal(phases.effectAuthorityRenewed,false);
  const commands=(await fs.readFile(path.join(f.root,'commands.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);
  const operations=[];for(const c of commands.filter(c=>c[1]==='am'))operations.push(JSON.parse(await fs.readFile(path.join(f.root,'remote/files/trollm/selected-launcher',c.at(-1)+'.request.json'))).operation);
  assert.deepEqual(operations,['start','cancel','binding']);
 }finally{await f.cleanup();}
});


test('retirement event overflow is one closed failure and never throws out of cleanup',()=>{
 const failures=[];
 const {events,mark}=require('./selected_native_launcher.cjs').createRetirementEvents(failures);
 for(let i=0;i<40;i++)assert.doesNotThrow(()=>mark('declared-phase',{ordinal:i}));
 assert.equal(events.length,16);
 assert.deepEqual(failures,[{stage:'retain-retirement-phases',code:'NATIVE_RETIREMENT_DIAGNOSTIC_LIMIT'}]);
 assert.equal(events[0].ordinal,0);assert.equal(events.at(-1).ordinal,15);
});

test('occupied private phase destination cannot skip cancel or replace primary failure',async()=>{
 const f=await fixture('live-until-cancel');
 try{
  const o=await options(f), original=Buffer.from('original occupied private diagnostic');
  const target=path.join(o.outputDirectory,'launcher-retirement.private.json');
  await assert.rejects(withSelectedAndroidDriver(o,async()=>{
   await fs.writeFile(target,original,{flag:'wx',mode:0o600});
   throw Object.assign(Error('original cutoff'),{code:'CONTROLLED_ORIGINAL_CUTOFF'});
  }),error=>{
   assert.equal(error.primaryFailure.code,'CONTROLLED_ORIGINAL_CUTOFF');
   assert.equal(error.workerInactiveAcknowledged,true);
   assert(error.cleanupFailures.some(e=>e.stage==='retain-retirement-phases'));
   return true;
  });
  assert.deepEqual(await fs.readFile(target),original);
  const closure=JSON.parse(await fs.readFile(path.join(o.outputDirectory,'launcher-closure.json')));
  assert.equal(closure.primaryFailure.code,'CONTROLLED_ORIGINAL_CUTOFF');
  assert.equal(closure.workerInactiveAcknowledged,true);
  const commands=(await fs.readFile(path.join(f.root,'commands.jsonl'),'utf8')).trim().split('\n').map(JSON.parse);
  const operations=[];for(const c of commands.filter(c=>c[1]==='am'))operations.push(JSON.parse(await fs.readFile(path.join(f.root,'remote/files/trollm/selected-launcher',c.at(-1)+'.request.json'))).operation);
  assert.deepEqual(operations,['start','cancel','binding']);
 }finally{await f.cleanup();}
});


test('shutdown phase deadline bounds a real blocked recovery child and refuses already-expired calls',async()=>{
 const f=await fixture('custody-features-timeout-zero');
 try{
  const {phaseAdb}=require('./selected_native_launcher.cjs');
  const phase={signal:new AbortController().signal,cutoff:Date.now()+150,elapsedDeadline:performance.now()+150};
  await assert.rejects(phaseAdb(f.adb,['features'],phase));
  let calls=0;
  await assert.rejects(phaseAdb(async()=>{calls++;return Buffer.alloc(0);},['unused'],phase),/NATIVE_POLL_PHASE_EXPIRED/);
  assert.equal(calls,0);
  const before=await fs.readFile(path.join(f.root,'commands.jsonl'));
  await new Promise(resolve=>setTimeout(resolve,200));
  assert.deepEqual(await fs.readFile(path.join(f.root,'commands.jsonl')),before);
 }finally{await f.cleanup();}
});


test('v4 private clock role is selected exactly and crossed v3 roles refuse before ADB',async()=>{
 for(const nativeVersion of [3,4])for(const installerVersion of [3,4]){
  const f=await fixture();
  try{
   const o=await options(f);delete o.selectedDeviceMaxErrorUs;
   o.ready.nativeProfileId=`controlled-collection-native-selected.v${nativeVersion}-proposal`;
   o.clockPolicy={profileId:'trollm.android-selected-emulator-calibrated-clock.v1-proposal',minimumRateNumerator:9,minimumRateDenominator:10,maxCalibrationAgeMs:30000};
   let bindings=0;
   const installer={sourceRevision:installerVersion===4?'0dbc6a821c5de10136e4a9655ef3788ce9b89ea7':'9c97f57a22e9b84256aecc9f6b3bf70b4bd97113',
    moduleSha256:installerVersion===4?'bc64099bfa6b97de1de1a7ec4612c07a469a094a6e4744f23f406501be9a5f5a':'a93523bbf4db75230a44f01b923f3e6f23e8f48fe27c6bdce15c6b5ee4d853f3',
    bindAcquireClock(){bindings++;return async()=>{throw Error('declared no native calibration');};}};
   if(nativeVersion===installerVersion){await withSelectedAndroidDriver(o,async()=>{},installer);assert.equal(bindings,1);}
   else{
    await assert.rejects(withSelectedAndroidDriver(o,async()=>assert.fail('crossed role released'),installer),/CLOCK_PRIVATE_INSTALLER_REQUIRED/);
    assert.equal(bindings,0);await assert.rejects(fs.readFile(path.join(f.root,'commands.jsonl')),e=>e.code==='ENOENT');
   }
  }finally{await f.cleanup();}
 }
});
