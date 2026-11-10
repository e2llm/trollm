'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs/promises'),os=require('node:os'),path=require('node:path');
const {createHash}=require('node:crypto');
const {retainOriginalCustody}=require('./selected_native_custody.cjs');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const fixture=require('./fixtures/native-custody.cjs');
const base='/data/user/0/com.trollm.app/files/trollm';
function declaredProbe(args){
 const stdout=Buffer.from(args[0]==='version'?'Android Debug Bridge version declared\n':'shell_v2,stat_v2');
 return {stdout,stderr:Buffer.alloc(0),childExitCode:0,childSignal:null,stdoutBytes:stdout.length,stderrBytes:0,
  command:{executable:'/declared/adb',argv:[...args]}};
}
function declaredTar(logical,bytes){
 const h=Buffer.alloc(512),split=logical.lastIndexOf('/');
 assert(Buffer.byteLength(logical.slice(split+1))<=100&&Buffer.byteLength(logical.slice(0,split))<=155);
 h.write(logical.slice(split+1),0,100);h.write(logical.slice(0,split),345,155);
 h.write('0000600\0',100,8);h.write('0000000\0',108,8);h.write('0000000\0',116,8);
 h.write(bytes.length.toString(8).padStart(11,'0')+'\0',124,12);h.fill(32,148,156);h[156]=48;
 h.write('ustar\0',257,6);h.write('00',263,2);
 const sum=[...h].reduce((a,b)=>a+b,0);h.write(sum.toString(8).padStart(6,'0')+'\0 ',148,8);
 return Buffer.concat([h,bytes,Buffer.alloc((512-bytes.length%512)%512),Buffer.alloc(1024)]);
}

async function frame(args,bytes,format='ustar'){
 if(!args.includes('tar'))return bytes;
 const root=await fs.mkdtemp(path.join(os.tmpdir(),'custody-tar-'));
 try{const name=args.at(-1);
  await fs.mkdir(path.dirname(path.join(root,name)),{recursive:true});
  await fs.writeFile(path.join(root,name),bytes);
  return require('node:child_process').execFileSync('tar',['--format='+format,'-C',root,'-cf','-',name]);
 }finally{await fs.rm(root,{recursive:true,force:true});}
}

for(const mode of ['complete','root-alias','selected-originals','selected-incomplete-originals','missing-item','missing-admission','changed-item','escape','changed-snapshot','wrong-source','abort','frame-cursor','frame-cursor-missing','same-commit-duplicate','wrong-size-duplicate','wrong-hash-duplicate','foreign-journal','journal-gap','missing-clock-raw','gnu-complete','http-originals','http-dirty-hash','http-foreign-name','http-missing-member']){
 test(`original inactive custody ${mode} preserves originals and never produces report`,async()=>{
  const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-custody-'));
  try{
   if(mode==='gnu-complete'){
    for(const name of ['received-bracket','response-metadata'])f.files.set(`${f.root}/clock-calibration-000001.${name}.json`,Buffer.from('{"declared":"original"}'));
    const state=JSON.parse(f.snapshot);state.stack=[{foreachCursors:state.foreachCursors}];state.foreachCursors={};
    f.snapshot=Buffer.from(JSON.stringify(state));f.files.set(`${f.root}/state.snapshot.json`,f.snapshot);
   }
   if(mode==='missing-item')f.files.delete(`${f.root}/${f.itemReference}`);
   if(mode==='missing-admission')f.files.delete(`${f.root}/input-admission.json`);
   if(mode==='changed-item')f.files.set(`${f.root}/${f.itemReference}`,Buffer.from('changed'));
   if(mode==='changed-snapshot')f.files.set(`${f.root}/state.snapshot.json`,Buffer.from('{}'));
   const httpOriginal=Buffer.from(JSON.stringify({response:{status:421,bodyPrefixBytes:[0,255,10],bodyComplete:true},scope:'declared-private-original'}));
   const httpName=`${f.root}/artifacts/mcp-http-failure-${sha(httpOriginal)}.json`;
   if(mode.startsWith('http-')){
    f.files.set(httpName,mode==='http-dirty-hash'?Buffer.from('changed-original'):httpOriginal);
    if(mode==='http-foreign-name')f.files.set(`${f.root}/artifacts/mcp-http-failure-../../foreign.json`,httpOriginal);
   }
   if(mode==='root-alias'){f.journal=Buffer.from(f.journal.toString().replace('/data/user/0/','/data/data/'));f.files.set(`${f.root}/events.jsonl`,f.journal);}
   if(mode.startsWith('selected-')){f.files.set(`${f.root}/artifacts/selected-result-${'b'.repeat(64)}.json`,Buffer.from([0,255,13,10]));f.files.set(`${f.root}/artifacts/selected-result-${'b'.repeat(64)}.json.meta.json`,Buffer.from(JSON.stringify({complete:mode==='selected-originals'})));}
   if(mode==='wrong-source')f.files.set(`${f.root}/source-association.json`,Buffer.from('{"pre-admission":true}'));
   if(mode==='frame-cursor'||mode==='frame-cursor-missing'){
    const state=JSON.parse(f.snapshot);state.stack=[{foreachCursors:state.foreachCursors}];state.foreachCursors={};
    if(mode.startsWith('selected-'))assert(index.members.filter(member=>member.logicalPath.includes('/selected-result-')).every(member=>member.custodyAssociation==='selected-result-file-original-not-a-journal-commitment-or-ACK'));
   if(mode==='root-alias')assert(index.members.some(member=>member.originalArtifactPath?.startsWith('/data/data/')));
   if(mode==='frame-cursor-missing')state.stack[0].foreachCursors.items.snapshotRef='artifacts/missing.snapshot.jsonl';
    f.snapshot=Buffer.from(JSON.stringify(state));f.files.set(`${f.root}/state.snapshot.json`,f.snapshot);
   }
   if(['same-commit-duplicate','wrong-size-duplicate','wrong-hash-duplicate','foreign-journal','journal-gap'].includes(mode)){
    const event=JSON.parse(f.journal);let events=[event];
    if(mode.endsWith('duplicate')){const second={...event,seq:2};
     if(mode==='wrong-size-duplicate')second.bytes++;
     if(mode==='wrong-hash-duplicate')second.sha256='f'.repeat(64);
     events.push(second);
    }
    if(mode==='foreign-journal')event.runId='foreign-run';
    if(mode==='journal-gap')event.seq=2;
    f.journal=Buffer.from(events.map(value=>JSON.stringify(value)+'\n').join(''));f.files.set(`${f.root}/events.jsonl`,f.journal);
   }
   const controller=new AbortController();if(mode==='abort')controller.abort();
   const commands=[];
   const adb=async(args,options)=>{
    commands.push(args);if(['version','features'].includes(args[0]))return declaredProbe(args);const name=args.at(-1);
    if(args.includes('ls')&&mode==='http-foreign-name'&&name===`${f.root}/artifacts`)
      return Buffer.from('mcp-http-failure-foreign.json');
    if(args.includes('ls'))return Buffer.from([...f.files.keys()].filter(key=>key.startsWith(name+'/')&&!key.slice(name.length+1).includes('/')).map(key=>key.slice(name.length+1)).join('\n'));
    if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:mode==='escape'&&name.endsWith('configuration.json')?'/private/foreign':base+'/'+name.slice('files/trollm/'.length));
    if(mode==='http-missing-member'&&name===httpName)throw Object.assign(Error('private missing response'),{code:'ADB_COMMAND_FAILED'});
    if(mode==='missing-clock-raw'&&name.endsWith('clock-calibration-000001.response.json'))throw Object.assign(Error('private missing clock'),{code:'ADB_COMMAND_FAILED'});
    if(!f.files.has(name))throw Object.assign(Error('private-path-not-printed'),{code:'ADB_COMMAND_FAILED'});
    return await frame(args,f.files.get(name),mode==='gnu-complete'?'gnu':'ustar');
   };
   const index=await retainOriginalCustody({adb,binding:f.binding,snapshot:f.snapshot,journal:f.journal,
    resultDirectory:directory,signal:controller.signal,check:()=>{if(controller.signal.aborted)throw Object.assign(Error('abort'),{code:'NATIVE_COMPLETION_EXPIRED'});}});
   assert.equal(index.complete,['complete','root-alias','selected-originals','selected-incomplete-originals','frame-cursor','same-commit-duplicate','gnu-complete','http-originals'].includes(mode));
   assert.equal(index.meaning,'original-custody-only-not-a-report');
   const saved=JSON.parse(await fs.readFile(path.join(directory,'original-custody/index.json')));
   assert.deepEqual(saved,index);assert.equal((await fs.stat(path.join(directory,'original-custody/index.json'))).mode&0o777,0o600);
   for(const member of index.members){const bytes=await fs.readFile(path.join(directory,'original-custody',member.file));assert.equal(sha(bytes),member.sha256);}
   assert(commands.every(args=>!args.includes('am')&&!args.includes('start')&&!args.includes('POST')));
   assert(!(await fs.readdir(directory)).includes('completion.json'));
   if(mode==='frame-cursor-missing')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_CURSOR_UNAVAILABLE'));
   if(mode==='wrong-size-duplicate'||mode==='wrong-hash-duplicate')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_COMMITMENT_CONFLICT'));
   if(mode==='foreign-journal'||mode==='journal-gap')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_JOURNAL_INVALID'));
   if(mode==='http-originals'){
    const member=index.members.find(m=>m.logicalPath===httpName);assert(member);
    assert((await fs.readFile(path.join(directory,'original-custody',member.file))).equals(httpOriginal));
   }
   if(mode==='http-dirty-hash')assert(index.missing.some(x=>x.logicalPath===httpName&&x.code==='NATIVE_CUSTODY_HASH_MISMATCH'));
   if(mode==='http-foreign-name')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_HTTP_DIAGNOSTIC_INVALID'));
   if(mode==='http-missing-member')assert(index.missing.some(x=>x.logicalPath===httpName));
   if(mode==='missing-clock-raw')assert(index.missing.some(x=>x.logicalPath.endsWith('clock-calibration-000001.response.json')));
   if(mode==='missing-item')assert(index.missing.some(x=>x.code==='NATIVE_CUSTODY_CURSOR_UNAVAILABLE'));
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 });
}
for(const mode of ['valid','missing-ack','foreign-context','foreign-selection','foreign-case','dirty-evidence','invalid-ack','early-missing-request','volume32','volume435','volume435-missing-ack','volume-context-overflow'])test(`native outcome originals ${mode} never synthesize delivery or ACK`,async(t)=>{
 const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-outcome-custody-'));
 const source=JSON.parse(f.binding.sourceAssociationJson),selectionRef={tenantId:'declared',artifactId:'art_selection',sha256:'a'.repeat(64)};
 const owner={registrationId:'declared',tenantId:'declared',subject:'declared',workerId:'declared',runId:'declared',epoch:1};
 const selection={schema:'moray.declared-collection-selection.v1-proposal',selectionId:'original-selection',caseId:'original-case',invocationKey:'original-invocation',owner};
 const selectionBytes=Buffer.from(JSON.stringify(selection));selectionRef.sha256=sha(selectionBytes);
 const oldRoot=`files/trollm/bundle-revisions/${source.installation.installationSha256}`;
 const manifest=JSON.parse(f.files.get(`${oldRoot}/manifest.json`));manifest.files.push({path:'selection.json',sha256:sha(selectionBytes)});
 const manifestBytes=Buffer.from(JSON.stringify(manifest)),newRoot=`files/trollm/bundle-revisions/${sha(manifestBytes)}`;
 for(const [name,bytes]of [...f.files])if(name.startsWith(oldRoot+'/')){f.files.delete(name);f.files.set(newRoot+name.slice(oldRoot.length),bytes);}
 f.files.set(`${newRoot}/manifest.json`,manifestBytes);f.files.set(`${newRoot}/selection.json`,selectionBytes);
 source.installation.installationSha256=sha(manifestBytes);
 source.descriptor={source:{selectionRef,owner}};f.binding.sourceAssociationJson=JSON.stringify(source);
 f.binding.claimJson=JSON.stringify({nativeRunId:f.binding.nativeRunId,identity:{invocationKey:'original-invocation'}});
 f.binding.privateBindingJson=JSON.stringify({nativeRunId:f.binding.nativeRunId,invocationKey:'original-invocation'});
 for(const [name,value]of [['source-association.json',f.binding.sourceAssociationJson],['selected-invocation.json',f.binding.claimJson],['selected-private-installation.json',f.binding.privateBindingJson]])f.files.set(`${f.root}/${name}`,Buffer.from(value));
 const json=`{"runId":"${f.binding.nativeRunId}","callContext":{"epoch":"original","attempt":18446744073709551615,"call":1}}`;
 const native={json,sha256:sha(Buffer.from(json)),callContext:{epoch:'original',attempt:'18446744073709551615',call:1}};
 f.journal=Buffer.concat([f.journal,Buffer.from(JSON.stringify({runId:f.binding.nativeRunId,seq:2,type:'tool_call_prepared',nativeContext:native})+'\n')]);
 f.files.set(`${f.root}/events.jsonl`,f.journal);
 const evidence=Buffer.from('opaque original native association bytes, not decoded by custody');
 const request={schema:'trollm.private-collection-guard.v1',invocationKey:'original-invocation',selectionRef,nativeContext:native,
  guardRequest:{kind:'outcome',outcome:'OBSERVATION_RETURNED',contextSha256:native.sha256,selectionRef,owner,selectionId:selection.selectionId,caseId:selection.caseId,selectionRawSha256:selectionRef.sha256,
   journalEvidence:{bytesBase64:evidence.toString('base64'),sha256:sha(evidence)}}};
 if(mode==='foreign-context')request.nativeContext={...native,json:'{"runId":"foreign"}'};
 if(mode==='foreign-selection')request.guardRequest.selectionRef={...selectionRef,sha256:'f'.repeat(64)};
 if(mode==='foreign-case')request.guardRequest.caseId='foreign';
 if(mode==='dirty-evidence')request.guardRequest.journalEvidence.sha256='f'.repeat(64);
 const requestPath=`${f.root}/selected-report/${native.sha256}.request.json`,ackPath=`${f.root}/selected-report/${native.sha256}.ack.json`;
 const requestBytes=Buffer.from(JSON.stringify(request));
 if(mode!=='early-missing-request')f.files.set(requestPath,requestBytes);
 if(mode!=='missing-ack')f.files.set(ackPath,Buffer.from(JSON.stringify({recorded:mode!=='invalid-ack',source:'native-adapter-reported-not-independent-settlement'})));
 const volume=mode.startsWith('volume'),contextCount=mode==='volume-context-overflow'?129:mode==='volume32'?32:volume?128:1;
 if(volume){
  // DECLARED source/transport volume, not new native/calibration execution.
  let seq=2;
  const rows=f.journal.toString().trim().split('\n').map(JSON.parse).slice(0,1);
  for(let ordinal=1;ordinal<=contextCount;ordinal++){
   const json=JSON.stringify({runId:f.binding.nativeRunId,callContext:{epoch:'declared-volume',attempt:ordinal,call:1}});
   const context={json,sha256:sha(Buffer.from(json)),callContext:{epoch:'declared-volume',attempt:String(ordinal),call:1}};
   rows.push({runId:f.binding.nativeRunId,seq:seq++,type:'tool_call_prepared',nativeContext:context});
   const payload=Buffer.from(JSON.stringify({declaredOriginal:ordinal})),artifact=`artifacts/selected-result-${context.sha256}.json`;
   f.files.set(`${f.root}/${artifact}`,payload);f.files.set(`${f.root}/${artifact}.meta.json`,Buffer.from('{"complete":true}'));
   const pathDiagnostic=Buffer.from(JSON.stringify({declaredOriginal:ordinal,scope:'path-operands-not-outcome'}));
   f.files.set(`${f.root}/artifacts/native-result-path-${sha(pathDiagnostic)}.json`,pathDiagnostic);
   rows.push({runId:f.binding.nativeRunId,seq:seq++,type:'artifact_written',path:base+`/runs/${f.binding.nativeRunId}/${artifact}`,sha256:sha(payload),bytes:payload.length});
   const body={...request,nativeContext:context,guardRequest:{...request.guardRequest,contextSha256:context.sha256}};
   f.files.set(`${f.root}/selected-report/${context.sha256}.request.json`,Buffer.from(JSON.stringify(body)));
   if(!(mode==='volume435-missing-ack'&&ordinal===7))f.files.set(`${f.root}/selected-report/${context.sha256}.ack.json`,Buffer.from('{"recorded":true,"source":"native-adapter-reported-not-independent-settlement"}'));
   if(!(mode==='volume435-missing-ack'&&ordinal===7))rows.push({runId:f.binding.nativeRunId,seq:seq++,type:'tool_call_outcome_acknowledged',contextSha256:context.sha256});
  }
  f.journal=Buffer.from(rows.map(r=>JSON.stringify(r)).join('\n')+'\n');f.files.set(`${f.root}/events.jsonl`,f.journal);
  for(const name of [...f.files.keys()])if(name.includes('/clock-calibration-'))f.files.delete(name);
  for(let ordinal=1;ordinal<=435;ordinal++)for(const suffix of ['request','response','sent-bracket','received-bracket','response-metadata','body-read','bracket','failure','received-unavailable'])
   f.files.set(`${f.root}/clock-calibration-${String(ordinal).padStart(6,'0')}.${suffix}.json`,Buffer.from(JSON.stringify({declaredVolume:true,ordinal,suffix})));
  if(mode==='volume32'){
   f.files.delete(`${f.root}/clock-calibration-000435.failure.json`);
   f.files.set(`${f.root}/clock-acquisition-limit.json`,Buffer.from('{"cause":"SELECTED_CLOCK_ACQUISITION_LIMIT","requestDispatched":false,"authorityRenewed":false}'));
  }

 }
 try{
  const commands=[];
  const adb=async(args)=>{commands.push(args);if(['version','features'].includes(args[0]))return declaredProbe(args);
   const name=args.at(-1);if(args.includes('ls'))return volume?Buffer.from([...f.files.keys()].filter(n=>n.startsWith(name+'/')&&!n.slice(name.length+1).includes('/')).map(n=>n.slice(name.length+1)).join('\n')):Buffer.alloc(0);
   if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:base+'/'+name.slice('files/trollm/'.length));
   if(volume&&args.includes('tar')){const names=args.slice(args.indexOf('tar')+3);return Buffer.concat([...names.map(n=>{if(!f.files.has(n))throw Object.assign(Error('declared missing original'),{code:'ADB_COMMAND_FAILED'});return declaredTar(n,f.files.get(n)).subarray(0,-1024);}),Buffer.alloc(1024)]);}
   if(!f.files.has(name))throw Object.assign(Error('declared missing original'),{code:'ADB_COMMAND_FAILED'});
   return frame(args,f.files.get(name));};
  const index=await retainOriginalCustody({adb,binding:f.binding,snapshot:f.snapshot,journal:f.journal,resultDirectory:directory,signal:new AbortController().signal,check:()=>{}});
  if(volume){
   assert.equal(index.complete,['volume32','volume435'].includes(mode));
   assert.equal(index.members.filter(m=>m.logicalPath.includes('/clock-calibration-')).length,mode==='volume32'?3914:3915);
   if(mode==='volume32')assert(index.members.some(m=>m.logicalPath.endsWith('/clock-acquisition-limit.json')));
   if(mode==='volume-context-overflow'){assert(index.missing.some(m=>m.code==='NATIVE_CUSTODY_LIMIT'));assert.equal(index.nativeOutcomes,undefined);}
   else {assert.equal(index.nativeOutcomes.length,contextCount);assert.equal(index.nativeOutcomes.filter(r=>r.ack==='retained-native-response').length,mode==='volume435-missing-ack'?contextCount-1:contextCount);
    assert(index.nativeOutcomes.every(r=>r.delivery==='not-established'));
    assert.equal(new Set(index.members.filter(m=>m.logicalPath.includes('/artifacts/selected-result-')).map(m=>m.logicalPath)).size,contextCount*2);}
   const totalBytes=index.members.reduce((sum,member)=>sum+member.sizeBytes,0);
   const disjoint=new Set(index.members.map(member=>member.logicalPath));
   assert(index.members.length<=6144);assert(totalBytes<=134217728);
   if(mode==='volume435')assert(index.members.length>4096,'combined closure exercises old member overflow');
   t.diagnostic(JSON.stringify({declaredVolume:true,contexts:contextCount,calibrationFiles:3915,retainedMembers:index.members.length,disjointLogicalPaths:disjoint.size,totalBytes}));
   for(const member of index.members){const bytes=await fs.readFile(path.join(directory,'original-custody',member.file));assert.equal(sha(bytes),member.sha256);}
   assert(commands.filter(args=>args.includes('tar')&&args.length>9).length>=15,'calibration originals use bounded batches');
   return;
  }
  assert.equal(index.complete,mode==='valid');assert.equal(index.nativeOutcomes.length,1);
  const row=index.nativeOutcomes[0];assert.equal(row.delivery,'not-established');assert.equal(row.journalOutcomeAcknowledged,false);
  if(mode==='valid')assert.equal(row.ack,'retained-native-response');
  if(mode==='missing-ack')assert.equal(row.ack,'absent-or-unavailable');
  if(mode==='early-missing-request')assert.equal(row.request,'unavailable');
  else{
   const member=index.members.find(member=>member.logicalPath===requestPath);assert(member);
   assert((await fs.readFile(path.join(directory,'original-custody',member.file))).equals(requestBytes));
  }
  if(['foreign-context','foreign-selection','foreign-case','dirty-evidence','early-missing-request'].includes(mode))assert(!commands.some(args=>args.at(-1)===ackPath));
 }finally{await fs.rm(directory,{recursive:true,force:true});}
});

for(const format of ['ustar','gnu'])test(`launch custody ${format} preserves immutable originals without an inactive-worker claim`,async()=>{
 const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-launch-custody-'));
 const diagnostic=Buffer.from('{"original":"launch-http-refusal"}'),diagnosticName=`${f.root}/artifacts/mcp-http-failure-${sha(diagnostic)}.json`;
 f.files.set(diagnosticName,diagnostic);
 const pathDiagnostic=Buffer.from('{"scope":"original-local-path-operands-not-outcome-or-settlement","confined":false}');
 const pathDiagnosticName=`${f.root}/artifacts/native-result-path-${sha(pathDiagnostic)}.json`;
 f.files.set(pathDiagnosticName,pathDiagnostic);
 try{
  const commands=[];
  const adb=async(args,options)=>{commands.push(args);if(['version','features'].includes(args[0]))return declaredProbe(args);const name=args.at(-1);
    if(args.includes('ls'))return Buffer.from([...f.files.keys()].filter(key=>key.startsWith(name+'/')&&!key.slice(name.length+1).includes('/')).map(key=>key.slice(name.length+1)).join('\n'));
   if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:base+'/'+name.slice('files/trollm/'.length));
   if(!f.files.has(name))throw Object.assign(Error('unavailable'),{code:'ADB_COMMAND_FAILED'});return await frame(args,f.files.get(name),format);};
  const index=await retainOriginalCustody({adb,binding:{...f.binding,workerActive:true},resultDirectory:directory,
   phase:'launch',signal:new AbortController().signal,check:()=>{}});
  assert.equal(index.complete,true);assert.equal(index.workerInactiveAcknowledged,false);assert.equal(index.phase,'launch');
  assert(!commands.some(args=>args.at(-1).endsWith('state.snapshot.json')||args.at(-1).endsWith('events.jsonl')));
  assert(index.members.some(member=>member.logicalPath===diagnosticName));
  assert(index.members.some(member=>member.logicalPath===pathDiagnosticName&&member.sha256===sha(pathDiagnostic)));
  assert(index.members.some(member=>member.logicalPath.endsWith('input-admission.json')));
  assert(index.members.some(member=>member.logicalPath.endsWith('clock-calibration-000001.response.json')));
 }finally{await fs.rm(directory,{recursive:true,force:true});}
});

for(const mode of ['allocated-originals','no-run-id','missing-source','escape','abort','changed-installation','foreign-installation','dirty-read','wrong-invocation-claim']){
 test(`partial start custody ${mode} never constructs a binding, report or inactive proof`,async()=>{
  const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-partial-custody-'));
  const installation=Buffer.from('{"original":"private installation"}'),installationKey=sha(installation);
  const raw=Buffer.from(JSON.stringify({operation:'start',original:'request',installationKey,invocationKey:'declared-invocation'})),requestSha256=sha(raw);
  const logical=`files/trollm/selected-launcher/${requestSha256}.request.json`;
  f.files.set(logical,raw);
  f.files.set(`files/trollm/selected-launcher/invocation-${sha(Buffer.from('declared-invocation'))}.claim`,Buffer.from(requestSha256));
  f.files.set(`files/trollm/selected-installations/${installationKey}.json`,installation);
  f.files.set(`files/trollm/selected-launcher/${requestSha256}.claim`,Buffer.from([1]));
  if(mode==='wrong-invocation-claim')f.files.set(`files/trollm/selected-launcher/invocation-${sha(Buffer.from('declared-invocation'))}.claim`,Buffer.from('f'.repeat(64)));
  if(mode==='changed-installation')f.files.set(`files/trollm/selected-installations/${installationKey}.json`,Buffer.from('{}'));
  if(mode==='missing-source')f.files.delete(`${f.root}/source-association.json`);
  const diagnostic=Buffer.from('{"original":"partial-http-refusal"}'),diagnosticName=`${f.root}/artifacts/mcp-http-failure-${sha(diagnostic)}.json`;
  f.files.set(diagnosticName,diagnostic);
  const signal=new AbortController();if(mode==='abort')signal.abort();
  const commands=[];
  const adb=async(args,options)=>{
   commands.push(args);if(['version','features'].includes(args[0]))return declaredProbe(args);const name=args.at(-1);
    if(args.includes('ls'))return Buffer.from([...f.files.keys()].filter(key=>key.startsWith(name+'/')&&!key.slice(name.length+1).includes('/')).map(key=>key.slice(name.length+1)).join('\n'));
   if(args.includes('readlink'))return Buffer.from(name==='files/trollm'?base:
    mode==='escape'&&name===logical?'/foreign/source':base+'/'+name.slice('files/trollm/'.length));
   if(mode==='dirty-read'&&name.endsWith('/input-admission.json'))return Buffer.from('cat: private missing file: No such file or directory\n');
   if(!f.files.has(name))throw Object.assign(Error('private raw path'),{code:'ADB_COMMAND_FAILED'});
   return await frame(args,f.files.get(name));
  };
  try{
   const {retainPartialStartCustody}=require('./selected_native_custody.cjs');
   const value=await retainPartialStartCustody({adb,nativeRunId:mode==='no-run-id'?undefined:f.binding.nativeRunId,
    requestSha256,installationKey:mode==='foreign-installation'?'f'.repeat(64):installationKey,resultDirectory:directory,signal:signal.signal,
    check:()=>{if(signal.signal.aborted)throw Object.assign(Error('aborted'),{code:'CUSTODY_ABORTED'});}});
   assert.equal(value.complete,false);assert.equal(value.workerInactiveAcknowledged,false);
   assert.equal(value.nativeRunId,mode==='no-run-id'?null:f.binding.nativeRunId);
   assert(!value.members.some(m=>m.file==='worker-binding.json'||m.file==='completion.json'));
   assert(commands.every(args=>['version','features'].includes(args[0])||args.includes('run-as')&&(args[0]==='exec-out'||args[0]==='shell'&&args[1]==='-T')));
   if(mode==='allocated-originals'){
    assert(value.members.some(m=>m.logicalPath===diagnosticName));

    assert(value.members.some(m=>m.logicalPath.endsWith('/input-admission.json')));
    assert(value.members.some(m=>m.logicalPath.endsWith(`/${installationKey}.json`)));
    assert(value.members.some(m=>m.logicalPath.endsWith('.claim')));
   }
   if(mode==='missing-source')assert(value.missing.some(m=>m.logicalPath.endsWith('/source-association.json')));
   if(mode==='escape')assert(value.missing.some(m=>m.code==='NATIVE_CUSTODY_PATH_ESCAPE'));
   if(mode==='dirty-read'){
    assert(value.missing.some(m=>m.logicalPath.endsWith('/input-admission.json')&&m.code==='NATIVE_CUSTODY_FRAME_INVALID'));
    assert(!value.members.some(m=>m.logicalPath.endsWith('/input-admission.json')));
    assert(value.diagnostics.some(m=>m.logicalPath.endsWith('/input-admission.json')));
   }
   if(mode==='wrong-invocation-claim')assert(value.missing.some(m=>m.logicalPath.includes('/invocation-')&&m.code==='NATIVE_CUSTODY_HASH_MISMATCH'));
   if(mode==='changed-installation'||mode==='foreign-installation')
    assert(value.missing.some(m=>m.code==='NATIVE_CUSTODY_HASH_MISMATCH'));
   for(const member of value.members)assert.equal(sha(await fs.readFile(path.join(directory,'partial-start-custody',member.file))),member.sha256);
   assert.deepEqual(JSON.parse(await fs.readFile(path.join(directory,'partial-start-custody/index.json'))),value);
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 });
}

test('public ADB client rendering uses the same wrapper against declared host features only',
 {skip:!process.env.TROLLM_ADB_PUBLIC_CLIENT},async()=>{
  const net=require('node:net');
  const executable=process.env.TROLLM_ADB_PUBLIC_CLIENT;
  assert(path.isAbsolute(executable));
  const keep=process.env.TROLLM_ADB_PROBE_OUTPUT_DIRECTORY;
  if(keep)assert(path.isAbsolute(keep));
  const directory=keep?path.resolve(keep):await fs.mkdtemp(path.join(os.tmpdir(),'public-adb-custody-'));
  if(keep)await fs.mkdir(directory,{mode:0o700});
  const services=[],sockets=new Set();
  const server=net.createServer(socket=>{
   sockets.add(socket);socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>{});
   socket.setTimeout(2000,()=>socket.destroy());
   let pending=Buffer.alloc(0);
   socket.on('data',chunk=>{
    pending=Buffer.concat([pending,chunk]);
    if(pending.length<4)return;
    const prefix=pending.subarray(0,4).toString('ascii');
    if(!/^[a-fA-F0-9]{4}$/.test(prefix)||pending.length>4096){socket.destroy();return;}
    const size=parseInt(prefix,16);if(pending.length<4+size)return;
    const service=pending.subarray(4,4+size).toString('utf8');services.push(service);
    // This is a declared host-protocol fixture, NOT an Android capability proof.
    const body=service==='host:version'?'0029':service==='host-serial:emulator-declared:features'?'shell_v2,stat_v2':null;
    const raw=body??'declared features-only server';
    socket.end((body===null?'FAIL':'OKAY')+Buffer.byteLength(raw).toString(16).padStart(4,'0')+raw);
   });
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try{
   const bytes=await fs.readFile(executable),originalSha=sha(bytes);
   const {createAdb}=require('./selected_native_launcher.cjs');
   const adb=createAdb({executable,serverHost:'127.0.0.1',serverPort:server.address().port,
    serial:'emulator-declared',timeoutMs:5000});
   const {retainPartialStartCustody}=require('./selected_native_custody.cjs');
   const index=await retainPartialStartCustody({adb,requestSha256:'a'.repeat(64),installationKey:'b'.repeat(64),
    resultDirectory:directory,signal:AbortSignal.timeout(15000),check:()=>{}});
   assert.deepEqual(index.adbProbes.map(p=>[p.probe,p.status]),[['version','success'],['features','success']]);
   const feature=index.adbProbes[1].streams.find(s=>s.stream==='stdout');
   const raw=await fs.readFile(path.join(directory,'partial-start-custody',feature.file));
   assert.equal(raw.toString(),'shell_v2\nstat_v2\n');
   assert.equal(raw.toString().trim().split(',').includes('shell_v2'),false,'old comma-only predicate would miss this real CLI rendering');
   assert(!index.missing.some(m=>m.code==='NATIVE_CUSTODY_REMOTE_STATUS_UNAVAILABLE'||m.code==='NATIVE_CUSTODY_ADB_FEATURES_INVALID'));
   assert.equal(index.complete,false);assert.equal(index.workerInactiveAcknowledged,false);assert.deepEqual(index.members,[]);
   assert(services.includes('host-serial:emulator-declared:features'));
   assert.equal(sha(await fs.readFile(executable)),originalSha);
   const receipt={scope:'public-client-rendering-declared-host-protocol-not-Android-or-native-execution',
    executable,sha256:originalSha,sizeBytes:bytes.length,services,featureStdoutSha256:sha(raw),
    remoteFileOrTarAcceptance:false,workerInactiveAcknowledged:false};
   await fs.writeFile(path.join(directory,'public-client-receipt.json'),JSON.stringify(receipt),{flag:'wx',mode:0o600});
  }finally{
   for(const socket of sockets)socket.destroy();await new Promise(resolve=>server.close(resolve));
   if(!keep)await fs.rm(directory,{recursive:true,force:true});
  }
 });


for(const total of [99,100,101,2048,2049]){
 test(`GNU archive logical path length ${total} is exact and bounded through full custody`,async()=>{
  const f=fixture(),directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-longname-bound-'));
  try{
   const body=Buffer.from([0,255,10,13]),prefix=`files/trollm/bundle-revisions/${'a'.repeat(64)}/`;
   let remaining=total-prefix.length;const parts=[];
   while(remaining>240){parts.push('x'.repeat(239));remaining-=240;}
   parts.push('f'.repeat(remaining));const memberPath=parts.join('/');
   const manifest=Buffer.from(JSON.stringify({format:'trollm.flow-bundle.v1',files:[{path:memberPath,sha256:sha(body)}]}));
   const revision=sha(manifest),source=JSON.parse(f.binding.sourceAssociationJson);
   source.installation.installationSha256=revision;f.binding.sourceAssociationJson=JSON.stringify(source);
   const logical=`files/trollm/bundle-revisions/${revision}/${memberPath}`;assert.equal(logical.length,total);
   f.files.set(`${f.root}/source-association.json`,Buffer.from(f.binding.sourceAssociationJson));
   f.files.set(`files/trollm/bundle-revisions/${revision}/manifest.json`,manifest);f.files.set(logical,body);
   const adb=async(args)=>{
    if(['version','features'].includes(args[0]))return declaredProbe(args);
    const name=args.at(-1);
    if(args.includes('ls'))return Buffer.from('');
    if(args.includes('readlink'))return Buffer.from(name==='shared_prefs/moray-source-device.xml'?'/data/user/0/com.trollm.app/'+name:name==='files/trollm'?base:base+'/'+name.slice('files/trollm/'.length));
    if(!f.files.has(name))throw Object.assign(Error('declared missing'),{code:'ADB_COMMAND_FAILED'});
    return frame(args,f.files.get(name),'gnu');
   };
   const index=await retainOriginalCustody({adb,binding:f.binding,snapshot:f.snapshot,journal:f.journal,
    resultDirectory:directory,signal:new AbortController().signal,check:()=>{}});
   assert.equal(index.complete,total<=2048);
   if(total<=2048){const member=index.members.find(x=>x.logicalPath===logical);assert(member);
    assert.deepEqual(await fs.readFile(path.join(directory,'original-custody',member.file)),body);
   }else{assert(index.missing.some(x=>x.logicalPath===logical));assert(!index.members.some(x=>x.logicalPath===logical));}
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 });
}

// Original SDK35 shell-UID frames, not app-domain or native-run acceptance.
for(const vector of fixture.publicSdk35TarVectors.cases){
 test(`retained SDK35 original TAR ${vector.id} exact full path and binary`,()=>{
  const {inspectOriginalTar}=require('./selected_native_custody.cjs');
  const raw=Buffer.from(vector.rawTarBase64,'base64'),expected=Buffer.from(vector.expectedBase64,'base64');
  assert.equal(vector.status.childExitCode,0);assert.equal(vector.status.childSignal,null);
  assert.equal(vector.status.timedOut,false);assert.equal(vector.status.outputTruncated,false);
  assert.equal(vector.status.stdoutBytesObserved,raw.length);assert.equal(vector.status.stderrBytesObserved,0);
  assert.equal(sha(raw),vector.status.stdoutSha256);assert.equal(sha(expected),vector.sha256);
  assert.equal(expected.length,vector.bytes);
  assert.deepEqual(inspectOriginalTar(raw,vector.logicalPath,{maxBytes:vector.bytes}),expected);
  assert.throws(()=>inspectOriginalTar(raw,vector.logicalPath+'x',{maxBytes:vector.bytes}));
  const dirty=Buffer.from(raw);dirty[dirty.length-1]=1;
  assert.throws(()=>inspectOriginalTar(dirty,vector.logicalPath,{maxBytes:vector.bytes}));
 });
}

test('600-second calibration volume retains every original within explicit member and listing bounds', async()=>{
 const {retainPartialStartCustody}=require('./selected_native_custody.cjs');
 const run='12345678-1234-4234-8234-123456789abc',runRoot=`files/trollm/runs/${run}`;
 const suffixes=['request','response','sent-bracket','received-bracket','response-metadata','body-read','bracket','failure','received-unavailable'];
 const files=new Map();
 for(let i=1;i<=435;i++)for(const suffix of suffixes){
  const name=`clock-calibration-${String(i).padStart(6,'0')}.${suffix}.json`;
  files.set(`${runRoot}/${name}`,Buffer.from(JSON.stringify({ordinal:i,suffix,original:true})));
 }
 function tar(logical,bytes){
  const h=Buffer.alloc(512),split=logical.lastIndexOf('/');
  h.write(logical.slice(split+1),0,100);h.write(logical.slice(0,split),345,155);
  h.write('0000600\0',100,8);h.write('0000000\0',108,8);h.write('0000000\0',116,8);
  h.write(bytes.length.toString(8).padStart(11,'0')+'\0',124,12);h.fill(32,148,156);h[156]=48;
  h.write('ustar\0',257,6);h.write('00',263,2);
  const sum=[...h].reduce((a,b)=>a+b,0);h.write(sum.toString(8).padStart(6,'0')+'\0 ',148,8);
  return Buffer.concat([h,bytes,Buffer.alloc((512-bytes.length%512)%512),Buffer.alloc(1024)]);
 }
 for(const overLimit of [false,true]){
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),'native-volume-'));
  const adb=async(args,options={})=>{
   if(options.captureResult)return declaredProbe(args);
   if(args.includes('readlink'))return Buffer.from(args.at(-1)==='files/trollm'?base:base+'/'+args.at(-1).slice('files/trollm/'.length));
   if(args.includes('ls'))return Buffer.from(overLimit?Array.from({length:3916},(_,i)=>`clock-calibration-${String(i+1).padStart(6,'0')}.request.json`).join('\n'):[...files.keys()].map(n=>n.slice(runRoot.length+1)).join('\n'));
   const logicals=args.includes('tar')?args.slice(args.indexOf('tar')+3):[args.at(-1)];
   const chunks=logicals.map(logical=>{
    const bytes=files.get(logical);
    if(!bytes)throw Object.assign(new Error('declared missing'),{code:'ADB_COMMAND_FAILED',childExitCode:1});
    return tar(logical,bytes).subarray(0,-1024);
   });
   return Buffer.concat([...chunks,Buffer.alloc(1024)]);
  };
  try{
   const index=await retainPartialStartCustody({adb,nativeRunId:run,requestSha256:'a'.repeat(64),installationKey:'b'.repeat(64),resultDirectory:directory,signal:new AbortController().signal,check(){}});
   assert.equal(index.complete,false);assert.equal(index.workerInactiveAcknowledged,false);
   const captured=index.members.filter(m=>m.logicalPath.includes('/clock-calibration-'));
   if(overLimit){assert.equal(captured.length,0);assert(index.missing.some(m=>m.code==='NATIVE_CUSTODY_LIMIT'));}
   else{assert.equal(captured.length,3915);for(const m of captured){const bytes=await fs.readFile(path.join(directory,'partial-start-custody',m.file));assert.equal(sha(bytes),m.sha256);assert.deepEqual(bytes,files.get(m.logicalPath));}}
  }finally{await fs.rm(directory,{recursive:true,force:true});}
 }
});

for(const mode of ['valid','foreign','reordered','duplicate','missing','extra','checksum','padding','truncated','trailer','oversized'])
 test(`calibration batch framing ${mode} preserves exact selected membership`,()=>{
  const {inspectOriginalTarBatch}=require('./selected_native_custody.cjs');
  const names=['files/trollm/runs/declared/clock-calibration-000001.request.json','files/trollm/runs/declared/clock-calibration-000001.response.json'];
  const bodies=[Buffer.from([0,255,10]),Buffer.from('declared original response')];
  let selected=[...names],members=names.map((n,i)=>declaredTar(n,bodies[i]).subarray(0,-1024));
  if(mode==='foreign')selected[1]='files/trollm/runs/foreign/response.json';
  if(mode==='reordered')members.reverse();
  if(mode==='duplicate')selected=[names[0],names[0]];
  if(mode==='missing')members.pop();
  if(mode==='extra')members.push(declaredTar('files/trollm/foreign.json',Buffer.from('foreign')).subarray(0,-1024));
  if(mode==='oversized')members[1]=declaredTar(names[1],Buffer.alloc(65537)).subarray(0,-1024);
  let raw=Buffer.concat([...members,Buffer.alloc(1024)]);
  if(mode==='checksum')raw[0]^=1;
  if(mode==='padding')raw[512+bodies[0].length]=1;
  if(mode==='truncated')raw=raw.subarray(0,raw.length-512);
  if(mode==='trailer')raw[raw.length-1]=1;
  if(mode==='valid')assert.deepEqual(inspectOriginalTarBatch(raw,selected),bodies);
  else assert.throws(()=>inspectOriginalTarBatch(raw,selected),/NATIVE_CUSTODY_FRAME_INVALID/);
 });

test('actual GNU tar batch binds each LongLink to its original ordered member',async()=>{
 const {inspectOriginalTarBatch}=require('./selected_native_custody.cjs');
 const directory=await fs.mkdtemp(path.join(os.tmpdir(),'clock-gnu-batch-'));
 try{
  const names=[`files/trollm/runs/${'a'.repeat(36)}/clock-calibration-000001.request.json`,`files/trollm/runs/${'a'.repeat(36)}/clock-calibration-000001.response.json`];
  const bodies=[Buffer.from([0,255,13,10]),Buffer.from('{"declared":true}')];
  for(let i=0;i<names.length;i++){await fs.mkdir(path.dirname(path.join(directory,names[i])),{recursive:true});await fs.writeFile(path.join(directory,names[i]),bodies[i]);}
  const raw=require('node:child_process').execFileSync('tar',['--format=gnu','-C',directory,'-cf','-',...names]);
  assert.deepEqual(inspectOriginalTarBatch(raw,names),bodies);
  assert.throws(()=>inspectOriginalTarBatch(raw,[...names].reverse()),/NATIVE_CUSTODY_FRAME_INVALID/);
 }finally{await fs.rm(directory,{recursive:true,force:true});}
});
