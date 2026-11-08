'use strict';
// Original launch/inactive-run custody only. Never produces a report or effects.
const fs=require('node:fs/promises');
const path=require('node:path');
const {createHash}=require('node:crypto');
const {identityProjection}=require('./selected_native_launcher.cjs');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const hash=/^[a-f0-9]{64}$/;
const fail=code=>{throw Object.assign(Error(code),{code});};
function relative(value){
  if(typeof value!=='string'||value.length>2048||!value.split('/').every(part=>/^[A-Za-z0-9_.-]+$/.test(part)&&part!=='.'&&part!=='..'))
    fail('NATIVE_CUSTODY_PATH_INVALID');
  return value;
}
async function writeOriginal(directory,name,bytes){
  const file=await fs.open(path.join(directory,name),'wx',0o600);
  try{await file.writeFile(bytes);await file.sync();}finally{await file.close();}
}
async function retainOriginalCustody({adb,binding,snapshot,journal,resultDirectory,signal,check,phase='inactive'}){
  if(!['launch','inactive'].includes(phase)||typeof binding?.workerActive!=='boolean'||(phase==='inactive'&&binding.workerActive!==false)||!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(binding.nativeRunId||''))fail('NATIVE_STOP_UNCONFIRMED');
  const runId=binding.nativeRunId,runRoot=`files/trollm/runs/${runId}`;
  const index={schema:'trollm.original-inactive-custody.v1-proposal',nativeRunId:runId,
    phase,workerInactiveAcknowledged:phase==='inactive',complete:false,meaning:'original-custody-only-not-a-report',members:[],missing:[]};
  const directory=path.join(resultDirectory,phase==='launch'?'original-launch-custody':'original-custody');
  await fs.mkdir(directory,{mode:0o700});
  let total=0;
  const retain=async(logical,bytes)=>{
    if(index.members.length>=512||bytes.length>33554432||total+bytes.length>134217728)fail('NATIVE_CUSTODY_LIMIT');
    const digest=sha(bytes),name=`${index.members.length.toString().padStart(4,'0')}-${digest}.raw`;
    await writeOriginal(directory,name,bytes);total+=bytes.length;
    index.members.push({logicalPath:logical,file:name,sha256:digest,sizeBytes:bytes.length});
    return bytes;
  };
  const missing=(logical,error,expected)=>index.missing.push({logicalPath:logical,...(expected?{expectedSha256:expected.sha256,...(expected.sizeBytes!==undefined?{expectedSizeBytes:String(expected.sizeBytes)}:{})}:{}),code:
    typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});
  const read=async(logical,expected)=>{
    try{
      check();
      if(expected?.sizeBytes!==undefined&&(!/^[0-9]+$/.test(String(expected.sizeBytes))||BigInt(expected.sizeBytes)>33554432n))fail('NATIVE_CUSTODY_LIMIT');
      const canonical=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',logical],{maxBytes:4096,signal})).toString('utf8').trim();
      const expectedPath=logical==='shared_prefs/moray-source-device.xml'?canonicalRoot.replace(/\/files\/trollm$/,'/'+logical):canonicalRoot+'/'+logical.slice('files/trollm/'.length);
      if(canonical!==expectedPath)fail('NATIVE_CUSTODY_PATH_ESCAPE');
      const bytes=await adb(['exec-out','run-as','com.trollm.app','cat',logical],{maxBytes:33554432,signal});check();
      // Keep actual bytes even on identity mismatch; never repair them.
      await retain(logical,bytes);
      if(expected?.originalArtifactPath)index.members.at(-1).originalArtifactPath=expected.originalArtifactPath;
      if(expected&&(expected.sha256!==sha(bytes)||expected.sizeBytes!==undefined&&BigInt(expected.sizeBytes)!==BigInt(bytes.length)))
        fail('NATIVE_CUSTODY_HASH_MISMATCH');
      return bytes;
    }catch(error){missing(logical,error,expected);return null;}
  };
  let canonicalRoot;
  try{
    if(phase==='inactive'){
      await retain(`${runRoot}/state.snapshot.json`,snapshot);
      await retain(`${runRoot}/events.jsonl`,journal);
    }
    await retain('worker-binding.json',Buffer.from(JSON.stringify(binding)));
    await retain(`${runRoot}/source-association.embedded-original.json`,Buffer.from(binding.sourceAssociationJson));
    canonicalRoot=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f','files/trollm'],{maxBytes:4096,signal})).toString('utf8').trim();
    if(!/^\/data\/(?:user\/0|data)\/com\.trollm\.app\/files\/trollm$/.test(canonicalRoot))fail('NATIVE_CUSTODY_ROOT_INVALID');
    await read('shared_prefs/moray-source-device.xml');
    const source=identityProjection(binding.sourceAssociationJson);
    if(source.nativeRunId!==runId)fail('NATIVE_CUSTODY_SOURCE_MISMATCH');
    const sourceBytes=await read(`${runRoot}/source-association.json`,{sha256:sha(Buffer.from(binding.sourceAssociationJson))});
    if(!sourceBytes)index.missing.push({logicalPath:'post-admission-source',code:'NATIVE_CUSTODY_SOURCE_UNAVAILABLE'});
    for(const [name,original]of [['selected-invocation.json',binding.claimJson],['selected-private-installation.json',binding.privateBindingJson]])
      await read(`${runRoot}/${name}`,{sha256:sha(Buffer.from(original))});
    await read(`${runRoot}/configuration.json`);
    if(source.inputAdmission){
      await read(`${runRoot}/input.original.json`,source.inputAdmission.raw);
      await read(`${runRoot}/input.normalized.json`,source.inputAdmission.normalized);
      const sealed=await read(`${runRoot}/input-admission.json`);
      const logical=value=>value&&typeof value==='object'?(Array.isArray(value)?value.map(logical):
        Object.fromEntries(Object.keys(value).sort().map(key=>[key,logical(value[key])]))):value;
      if(sealed&&JSON.stringify(logical(identityProjection(sealed.toString('utf8'))))!==JSON.stringify(logical(source.inputAdmission)))
        index.missing.push({logicalPath:`${runRoot}/input-admission.json`,code:'NATIVE_CUSTODY_ADMISSION_MISMATCH'});
    }
    const revision=source.installation?.installationSha256;
    if(!hash.test(revision||''))fail('NATIVE_CUSTODY_REVISION_INVALID');
    const bundleRoot=`files/trollm/bundle-revisions/${revision}`;
    const manifestBytes=await read(`${bundleRoot}/manifest.json`,{sha256:revision});
    if(manifestBytes){
      const manifest=identityProjection(manifestBytes.toString('utf8'));
      if(manifest.format!=='trollm.flow-bundle.v1'||!Array.isArray(manifest.files)||manifest.files.length>256)fail('NATIVE_CUSTODY_MANIFEST_INVALID');
      const names=new Set();
      for(const member of manifest.files){
        const name=relative(member.path);
        if(names.has(name)||!hash.test(member.sha256||''))fail('NATIVE_CUSTODY_MANIFEST_INVALID');
        names.add(name);await read(`${bundleRoot}/${name}`,{sha256:member.sha256});
      }
    }
    if(phase==='launch'){index.complete=index.missing.length===0;return index;}
    const state=identityProjection(snapshot.toString('utf8'));
    if(state.terminalReason==='cancelled')await read(`${runRoot}/cancellation-cause.json`);
    const committed=new Map();
    for(const line of journal.toString('utf8').trimEnd().split('\n')){
      const event=identityProjection(line);
      if(event.type!=='artifact_written')continue;
      if(!hash.test(event.sha256||'')||!/^\d+$/.test(event.bytes||''))fail('NATIVE_CUSTODY_ARTIFACT_INVALID');
      // Absolute native path is retained unchanged in the index/journal. Only
      // the equivalent app-relative read path is derived; no record is rewritten.
      const prefix=canonicalRoot+`/runs/${runId}/`;
      if(typeof event.path!=='string'||!event.path.startsWith(prefix))fail('NATIVE_CUSTODY_ARTIFACT_PATH');
      const suffix=relative(event.path.slice(prefix.length));
      const key=event.path+'@'+event.sha256;
      if(!committed.has(key)){
        committed.set(key,true);
        const retained=await read(`${runRoot}/${suffix}`,{sha256:event.sha256,sizeBytes:event.bytes,originalArtifactPath:event.path});
        if(retained)index.members.at(-1).originalArtifactPath=event.path;
      }
    }
    for(const cursor of Object.values(state.foreachCursors||{})){
      const suffix=relative(cursor.snapshotRef);
      const logical=`${runRoot}/${suffix}`;
      if(!index.members.some(member=>member.logicalPath===logical)||index.missing.some(member=>member.logicalPath===logical))
        index.missing.push({logicalPath:logical,code:'NATIVE_CUSTODY_CURSOR_UNAVAILABLE'});
    }
    // A raw second read detects a changing source after claimed inactivity.
    const reread=await adb(['exec-out','run-as','com.trollm.app','cat',`${runRoot}/state.snapshot.json`],{maxBytes:262144,signal});check();
    if(!reread.equals(snapshot))fail('NATIVE_CUSTODY_SNAPSHOT_CHANGED');
    const journalAgain=await adb(['exec-out','run-as','com.trollm.app','cat',`${runRoot}/events.jsonl`],{maxBytes:8388608,signal});check();
    if(!journalAgain.equals(journal))fail('NATIVE_CUSTODY_JOURNAL_CHANGED');
    index.complete=index.missing.length===0;
  }catch(error){missing('custody-collection',error);}
  finally{
    await writeOriginal(directory,'index.json',Buffer.from(JSON.stringify(index)));
    const handle=await fs.open(directory,'r');try{await handle.sync();}finally{await handle.close();}
  }
  return index;
}
async function retainPartialStartCustody({adb,nativeRunId,requestSha256,installationKey,resultDirectory,signal,check}){
  if(!hash.test(requestSha256)||!hash.test(installationKey)||nativeRunId!==undefined&&
    !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(nativeRunId))fail('NATIVE_CUSTODY_PATH_INVALID');
  const directory=path.join(resultDirectory,'partial-start-custody');await fs.mkdir(directory,{mode:0o700});
  const index={schema:'trollm.partial-start-custody.v1-proposal',nativeRunId:nativeRunId??null,
    requestSha256,installationKey,complete:false,workerInactiveAcknowledged:false,
    meaning:'partial-originals-only-not-a-binding-report-or-stop-proof',members:[],missing:[]};
  try{
    check();
    const base=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f','files/trollm'],{signal,maxBytes:4096})).toString().trim();
    if(!/^\/data\/(?:user\/0|data)\/com\.trollm\.app\/files\/trollm$/.test(base))fail('NATIVE_CUSTODY_ROOT_INVALID');
    const files=['request.json','result.json','started.json','claim','done'].map(suffix=>[`files/trollm/selected-launcher/${requestSha256}.${suffix}`,1048576]);
    files.push([`files/trollm/selected-installations/${installationKey}.json`,1048576]);
    if(nativeRunId)for(const name of ['source-association.json','installation.json','input.original.json','input.normalized.json',
      'input-admission.json','configuration.json','selected-start-failure.json','selected-invocation.json',
      'selected-private-installation.json','state.snapshot.json','events.jsonl'])
      files.push([`files/trollm/runs/${nativeRunId}/${name}`,name==='events.jsonl'?8388608:1048576]);
    for(const [logical,maximum]of files){
      try{
        check();
        const real=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',logical],{signal,maxBytes:4096})).toString().trim();
        if(real!==base+'/'+logical.slice('files/trollm/'.length))fail('NATIVE_CUSTODY_PATH_ESCAPE');
        const bytes=await adb(['exec-out','run-as','com.trollm.app','cat',logical],{signal,maxBytes:maximum});check();
        const digest=sha(bytes),file=`${index.members.length.toString().padStart(4,'0')}-${digest}.raw`;
        await writeOriginal(directory,file,bytes);
        index.members.push({logicalPath:logical,file,sha256:digest,sizeBytes:bytes.length});
        if(logical.endsWith(`/${requestSha256}.request.json`)&&
          (digest!==requestSha256||JSON.parse(bytes).installationKey!==installationKey))
          fail('NATIVE_CUSTODY_HASH_MISMATCH');
        if(logical.endsWith(`/selected-installations/${installationKey}.json`)&&digest!==installationKey)
          fail('NATIVE_CUSTODY_HASH_MISMATCH');
      }catch(error){index.missing.push({logicalPath:logical,code:
        typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});}
    }
  }catch(error){index.missing.push({logicalPath:'partial-start-collection',code:
    typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});}
  finally{
    await writeOriginal(directory,'index.json',Buffer.from(JSON.stringify(index)));
    const handle=await fs.open(directory,'r');try{await handle.sync();}finally{await handle.close();}
  }
  return index;
}
module.exports={retainOriginalCustody,retainPartialStartCustody,writeOriginal};
