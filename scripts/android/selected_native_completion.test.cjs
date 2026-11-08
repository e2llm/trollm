'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const {inspectTerminal,inspectInactiveProgress,createSelectedAndroidCompletion}=require('./selected_native_completion.cjs');
const fixture=JSON.parse(require('node:fs').readFileSync(require('node:path').join(__dirname,'fixtures/native-completion.json'),'utf8'));
const {identityProjection}=require('./selected_native_launcher.cjs');
const id=identityProjection(fixture.snapshotJson).runId;
const snapshot=(status='succeeded')=>Buffer.from(fixture.snapshotJson.replace('"status": "succeeded"','"status": "'+status+'"'));
const journal=(outcome='succeeded')=>Buffer.from(fixture.journalJsonl.replaceAll('"outcome":"succeeded"','"outcome":"'+outcome+'"'));
test('terminal native accounting remains distinct from business outcome',()=>{
  assert.equal(inspectTerminal(snapshot(),journal(),id),'COMPLETED');
  assert.equal(inspectTerminal(snapshot('failed'),journal('failed'),id),'UNKNOWN');
  assert.equal(inspectTerminal(snapshot(),journal('failed'),id),'UNKNOWN');
});
test('partial, wrong run, discontinuous journal and missing terminal refuse',()=>{
  for(const bytes of [journal().subarray(0,-1),Buffer.from(journal().toString().replace(id,'other')),
    Buffer.from(journal().toString().replace('"seq":1','"seq":2')),
    Buffer.from(journal().toString().replace('run_ended','tool_call_result')),Buffer.alloc(0)])
    assert.throws(()=>inspectTerminal(snapshot(),bytes,id));
  assert.throws(()=>inspectTerminal(snapshot(),journal(),'other'));
});
test('raw u64 content is not rounded or rewritten',()=>{
  const bytes=Buffer.from(journal().toString().replace('"type":"run_ended"','"opaqueCounter":18446744073709551615,"type":"run_ended"'));
  const original=Buffer.from(bytes);assert.equal(inspectTerminal(snapshot(),bytes,id),'COMPLETED');assert.deepEqual(bytes,original);
});
test('actual camelCase watermark must equal final contiguous journal u64',()=>{
  for(const raw of [fixture.snapshotJson.replace('"runId"','"run_id"'),
    fixture.snapshotJson.replace(/"eventWatermark":\s*\d+,?/,''),
    fixture.snapshotJson.replace(/"eventWatermark":\s*\d+/,'"eventWatermark": 0'),
    fixture.snapshotJson.replace(/"eventWatermark":\s*\d+/,'"eventWatermark": 18446744073709551616'),
    fixture.snapshotJson.replace(/"eventWatermark":\s*(\d+)/,'"eventWatermark": "$1"'),
    fixture.snapshotJson.replace(/"eventWatermark":\s*(\d+)/,'"eventWatermark": $1, "eventWatermark": $1'),
    fixture.snapshotJson.replace('"eventWatermark"','"event_watermark"')])
    assert.throws(()=>inspectTerminal(Buffer.from(raw),journal(),id));
});
test('completion refuses relative result custody before process invocation',()=>{
  assert.throws(()=>createSelectedAndroidCompletion({resultDirectory:'relative'}));
});

const fs=require('node:fs/promises');
const os=require('node:os');
const path=require('node:path');
const custodyFixture=require('./fixtures/native-custody.cjs');
async function writeAdbFixture(executable,snapshotBytes,journalBytes){
  const custody=custodyFixture(id);
  const encoded=Object.fromEntries([...custody.files].map(([key,bytes])=>[key,bytes.toString('base64')]));
  await fs.writeFile(executable,'#!'+process.execPath+'\n'+
    'const file=process.argv.at(-1),files='+JSON.stringify(encoded)+';\n'+
    'if(process.argv.includes("readlink")){process.stdout.write(file==="files/trollm"?"/data/user/0/com.trollm.app/files/trollm":"/data/user/0/com.trollm.app/"+file);}' +
    'else if(file.endsWith("state.snapshot.json"))process.stdout.write('+JSON.stringify(snapshotBytes.toString())+');'+
    'else if(file.endsWith("events.jsonl"))process.stdout.write('+JSON.stringify(journalBytes.toString())+');'+
    'else if(files[file])process.stdout.write(Buffer.from(files[file],"base64"));else process.exit(2);\n',{mode:0o700});
  return custody.binding;
}
test('real bounded child read retains original journal only after same-worker inactivity',async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'selected-completion-'));
  try{
    const executable=path.join(root,'adb');
    const stopped=await writeAdbFixture(executable,snapshot(),journal());
    const resultDirectory=path.join(root,'result');
    const useNative=createSelectedAndroidCompletion({adb:{executable,serverHost:'127.0.0.1',serverPort:5037,serial:'emulator-5554',timeoutMs:1000},resultDirectory});
    let reads=0;
    const handle={waitInactive:async()=>stopped,binding:async()=>{reads++;return stopped;}};
    const ready={invocationKey:'declared-only',expiresAt:new Date(Date.now()+10000).toISOString()};
    const result=await useNative(handle,ready,{signal:new AbortController().signal});
    assert.equal(result.status,'COMPLETED');assert.equal(reads,1);
    assert.deepEqual(await fs.readFile(result.intentLogFile),journal());
    assert.equal((await fs.stat(result.reportFile)).mode&0o777,0o600);
    assert.equal(JSON.parse(await fs.readFile(result.reportFile)).businessOutcome,'not-established');
    await assert.rejects(useNative(handle,ready,{signal:new AbortController().signal}),{code:'NATIVE_COMPLETION_ALREADY_USED'});
  }finally{await fs.rm(root,{recursive:true,force:true});}
});

test('passive inactive prefix stays UNKNOWN without inventing terminal record or watermark',()=>{
  const records=fixture.journalJsonl.trimEnd().split('\n');
  const prefix=Buffer.from(records.slice(0,-1).join('\n')+'\n');
  const last=JSON.parse(records.at(-2));
  for(const phase of ['running','paused']){
    const raw=snapshot(phase).toString().replace(/"eventWatermark":\s*\d+/, '"eventWatermark": '+last.seq);
    const bytes=Buffer.from(raw),before=Buffer.from(bytes),journalBefore=Buffer.from(prefix);
    assert.equal(inspectInactiveProgress(bytes,prefix,id),'UNKNOWN');
    assert.throws(()=>inspectTerminal(bytes,prefix,id));
    assert.deepEqual(bytes,before);assert.deepEqual(prefix,journalBefore);
    assert.throws(()=>inspectInactiveProgress(bytes,prefix.subarray(0,-1),id));
    assert.throws(()=>inspectInactiveProgress(snapshot(phase),prefix,id));
    assert.throws(()=>inspectInactiveProgress(bytes,prefix,'wrong'));
  }
  assert.throws(()=>inspectInactiveProgress(Buffer.from('{}'),prefix,id));
  assert.throws(()=>inspectInactiveProgress(snapshot('unknown'),journal(),id));
});

// Declared handle/ADB process control, not installed-provider authentication proof.
test('same-binding inactive callback retains interrupted UNKNOWN and fences changed binding',async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'selected-interrupted-'));
  try{
    const records=fixture.journalJsonl.trimEnd().split('\n');
    const prefix=Buffer.from(records.slice(0,-1).join('\n')+'\n');
    const last=JSON.parse(records.at(-2));
    const raw=Buffer.from(snapshot('running').toString().replace(/"eventWatermark":\s*\d+/,'"eventWatermark": '+last.seq));
    const executable=path.join(root,'adb');
    const stopped=await writeAdbFixture(executable,raw,prefix);
    const ready={invocationKey:'declared-only',expiresAt:new Date(Date.now()+10000).toISOString()};
    for(const changed of [false,true]){
      const resultDirectory=path.join(root,changed?'conflict':'retained');
      const callback=createSelectedAndroidCompletion({adb:{executable,serverHost:'127.0.0.1',serverPort:5037,serial:'emulator-5554',timeoutMs:1000},resultDirectory});
      const handle={waitInactive:async()=>stopped,binding:async()=>({...stopped,sourceAssociationJson:changed?'changed':stopped.sourceAssociationJson})};
      const pending=callback(handle,ready,{signal:new AbortController().signal});
      if(changed){
        await assert.rejects(pending,{code:'NATIVE_STOP_ASSOCIATION_CHANGED'});
        assert.deepEqual(await fs.readdir(resultDirectory),[]);
      }else{
        const result=await pending;
        assert.equal(result.status,'UNKNOWN');
        const report=JSON.parse(await fs.readFile(result.reportFile));
        assert.equal(report.progressDisposition,'interrupted');assert.equal(report.businessOutcome,'not-established');
        assert.deepEqual(await fs.readFile(result.intentLogFile),prefix);
        assert.deepEqual(await fs.readFile(path.join(resultDirectory,'state.snapshot.json')),raw);
      }
    }
  }finally{await fs.rm(root,{recursive:true,force:true});}
});
