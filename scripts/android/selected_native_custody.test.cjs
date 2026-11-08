'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs/promises'),os=require('node:os'),path=require('node:path');
const {createHash}=require('node:crypto');
const {retainOriginalCustody}=require('./selected_native_custody.cjs');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const fixture=require('./fixtures/native-custody.cjs');
const base='/data/user/0/com.trollm.app/files/trollm';
for(const mode of ['complete','missing-item','missing-admission','changed-item','escape','changed-snapshot','wrong-source','abort']){
 test(`original inactive custody ${mode} preserves originals and never produces report`,async()=>{
  const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-custody-'));
  try{
   if(mode==='missing-item')f.files.delete(`${f.root}/artifacts/items.snapshot.jsonl`);
   if(mode==='missing-admission')f.files.delete(`${f.root}/input-admission.json`);
   if(mode==='changed-item')f.files.set(`${f.root}/artifacts/items.snapshot.jsonl`,Buffer.from('changed'));
   if(mode==='changed-snapshot')f.files.set(`${f.root}/state.snapshot.json`,Buffer.from('{}'));
   if(mode==='wrong-source')f.files.set(`${f.root}/source-association.json`,Buffer.from('{"pre-admission":true}'));
   const controller=new AbortController();if(mode==='abort')controller.abort();
   const commands=[];
   const adb=async args=>{
    commands.push(args);const name=args.at(-1);
    if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:mode==='escape'&&name.endsWith('configuration.json')?'/private/foreign':base+'/'+name.slice('files/trollm/'.length));
    if(!f.files.has(name))throw Object.assign(Error('private-path-not-printed'),{code:'ADB_COMMAND_FAILED'});
    return f.files.get(name);
   };
   const index=await retainOriginalCustody({adb,binding:f.binding,snapshot:f.snapshot,journal:f.journal,
    resultDirectory:directory,signal:controller.signal,check:()=>{if(controller.signal.aborted)throw Object.assign(Error('abort'),{code:'NATIVE_COMPLETION_EXPIRED'});}});
   assert.equal(index.complete,mode==='complete');
   assert.equal(index.meaning,'original-custody-only-not-a-report');
   const saved=JSON.parse(await fs.readFile(path.join(directory,'original-custody/index.json')));
   assert.deepEqual(saved,index);assert.equal((await fs.stat(path.join(directory,'original-custody/index.json'))).mode&0o777,0o600);
   for(const member of index.members){const bytes=await fs.readFile(path.join(directory,'original-custody',member.file));assert.equal(sha(bytes),member.sha256);}
   assert(commands.every(args=>!args.includes('am')&&!args.includes('start')&&!args.includes('POST')));
   assert(!(await fs.readdir(directory)).includes('completion.json'));
   if(mode==='missing-item')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_CURSOR_UNAVAILABLE'));
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 });
}
test('launch custody preserves immutable originals without an inactive-worker claim',async()=>{
 const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-launch-custody-'));
 try{
  const commands=[];
  const adb=async args=>{commands.push(args);const name=args.at(-1);
   if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:base+'/'+name.slice('files/trollm/'.length));
   if(!f.files.has(name))throw Object.assign(Error('unavailable'),{code:'ADB_COMMAND_FAILED'});return f.files.get(name);};
  const index=await retainOriginalCustody({adb,binding:{...f.binding,workerActive:true},resultDirectory:directory,
   phase:'launch',signal:new AbortController().signal,check:()=>{}});
  assert.equal(index.complete,true);assert.equal(index.workerInactiveAcknowledged,false);assert.equal(index.phase,'launch');
  assert(!commands.some(args=>args.at(-1).endsWith('state.snapshot.json')||args.at(-1).endsWith('events.jsonl')));
  assert(index.members.some(member=>member.logicalPath.endsWith('input-admission.json')));
 }finally{await fs.rm(directory,{recursive:true,force:true});}
});

for(const mode of ['allocated-originals','no-run-id','missing-source','escape','abort','changed-installation','foreign-installation']){
 test(`partial start custody ${mode} never constructs a binding, report or inactive proof`,async()=>{
  const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-partial-custody-'));
  const installation=Buffer.from('{"original":"private installation"}'),installationKey=sha(installation);
  const raw=Buffer.from(JSON.stringify({operation:'start',original:'request',installationKey})),requestSha256=sha(raw);
  const logical=`files/trollm/selected-launcher/${requestSha256}.request.json`;
  f.files.set(logical,raw);
  f.files.set(`files/trollm/selected-installations/${installationKey}.json`,installation);
  f.files.set(`files/trollm/selected-launcher/${requestSha256}.claim`,Buffer.from([1]));
  if(mode==='changed-installation')f.files.set(`files/trollm/selected-installations/${installationKey}.json`,Buffer.from('{}'));
  if(mode==='missing-source')f.files.delete(`${f.root}/source-association.json`);
  const signal=new AbortController();if(mode==='abort')signal.abort();
  const commands=[];
  const adb=async args=>{
   commands.push(args);const name=args.at(-1);
   if(args.includes('readlink'))return Buffer.from(name==='files/trollm'?base:
    mode==='escape'&&name===logical?'/foreign/source':base+'/'+name.slice('files/trollm/'.length));
   if(!f.files.has(name))throw Object.assign(Error('private raw path'),{code:'ADB_COMMAND_FAILED'});
   return f.files.get(name);
  };
  try{
   const {retainPartialStartCustody}=require('./selected_native_custody.cjs');
   const value=await retainPartialStartCustody({adb,nativeRunId:mode==='no-run-id'?undefined:f.binding.nativeRunId,
    requestSha256,installationKey:mode==='foreign-installation'?'f'.repeat(64):installationKey,resultDirectory:directory,signal:signal.signal,
    check:()=>{if(signal.signal.aborted)throw Object.assign(Error('aborted'),{code:'CUSTODY_ABORTED'});}});
   assert.equal(value.complete,false);assert.equal(value.workerInactiveAcknowledged,false);
   assert.equal(value.nativeRunId,mode==='no-run-id'?null:f.binding.nativeRunId);
   assert(!value.members.some(m=>m.file==='worker-binding.json'||m.file==='completion.json'));
   assert(commands.every(args=>args[0]==='exec-out'&&args.includes('run-as')));
   if(mode==='allocated-originals'){
    assert(value.members.some(m=>m.logicalPath.endsWith('/input-admission.json')));
    assert(value.members.some(m=>m.logicalPath.endsWith(`/${installationKey}.json`)));
    assert(value.members.some(m=>m.logicalPath.endsWith('.claim')));
   }
   if(mode==='missing-source')assert(value.missing.some(m=>m.logicalPath.endsWith('/source-association.json')));
   if(mode==='escape')assert(value.missing.some(m=>m.code==='NATIVE_CUSTODY_PATH_ESCAPE'));
   if(mode==='changed-installation'||mode==='foreign-installation')
    assert(value.missing.some(m=>m.code==='NATIVE_CUSTODY_HASH_MISMATCH'));
   for(const member of value.members)assert.equal(sha(await fs.readFile(path.join(directory,'partial-start-custody',member.file))),member.sha256);
   assert.deepEqual(JSON.parse(await fs.readFile(path.join(directory,'partial-start-custody/index.json'))),value);
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 });
}
