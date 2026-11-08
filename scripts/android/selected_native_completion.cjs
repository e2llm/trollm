'use strict';
// Private reporting callback only. Native remains the sole effect sender.
const fs=require('node:fs/promises');
const path=require('node:path');
const {createHash}=require('node:crypto');
const {createAdb,identityProjection}=require('./selected_native_launcher.cjs');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const fail=code=>{throw Object.assign(Error(code),{code});};
// Inspect a top-level numeric token without parsing it through a JS Number.
// Full JSON syntax is checked by identityProjection before this is consulted.
function topLevelU64(raw,name,code='NATIVE_SNAPSHOT_WATERMARK_MISMATCH'){
  let depth=0,found;
  for(let i=0;i<raw.length;){
    const ch=raw[i];
    if(ch==='"'){
      const start=i++;
      while(i<raw.length){if(raw[i]==='\\'){i+=2;continue;}if(raw[i++]==='"')break;}
      let next=i;while(/\s/.test(raw[next]||'') && next<raw.length)next++;
      if(depth===1 && raw[next]===':' && JSON.parse(raw.slice(start,i))===name){
        if(found!==undefined)fail(code);
        const match=raw.slice(next+1).match(/^\s*(0|[1-9][0-9]*)(?=\s*[,}])/);
        if(!match)fail(code);
        found=BigInt(match[1]);
      }
    }else{if(ch==='{'||ch==='[')depth++;if(ch==='}'||ch===']')depth--;i++;}
  }
  if(found===undefined || found>18446744073709551615n)fail(code);
  return found;
}
function inspectProgress(snapshotBytes,journalBytes,runId,allowInterrupted){
  if(snapshotBytes.length>262144 || journalBytes.length>8388608)fail('NATIVE_COMPLETION_SIZE');
  const snapshot=identityProjection(snapshotBytes.toString('utf8'));
  if(snapshot.runId!==runId)fail('NATIVE_COMPLETION_RUN_MISMATCH');
  const lines=journalBytes.toString('utf8').split('\n');
  if(lines.pop()!=='')fail('NATIVE_JOURNAL_PARTIAL');
  let previous=0n,last;
  for(const line of lines){
    const event=identityProjection(line);
    if(event.runId!==runId || !/^[1-9][0-9]*$/.test(event.seq||''))fail('NATIVE_JOURNAL_ASSOCIATION');
    const seq=topLevelU64(line,'seq','NATIVE_JOURNAL_SEQUENCE');
    if(seq!==previous+1n)fail('NATIVE_JOURNAL_SEQUENCE');
    previous=seq;last=event;
  }
  const watermark=topLevelU64(snapshotBytes.toString('utf8'),'eventWatermark');
  if(allowInterrupted && ['running','paused'].includes(snapshot.status)){
    // Only the caller's authentic same-worker inactive binding permits this
    // passive path. A readable prefix never settles possible effects.
    if(!last || watermark>previous)fail('NATIVE_SNAPSHOT_WATERMARK_MISMATCH');
    return 'UNKNOWN';
  }
  if(!['succeeded','failed'].includes(snapshot.status))fail('NATIVE_SNAPSHOT_STATUS');
  if(!last || last.type!=='run_ended')fail('NATIVE_TERMINAL_RECORD_MISSING');
  if(watermark!==previous)fail('NATIVE_SNAPSHOT_WATERMARK_MISMATCH');
  // A definite native terminal outcome is still not the independent cart oracle.
  return snapshot.status==='succeeded' && last.outcome==='succeeded'?'COMPLETED':'UNKNOWN';
}
function inspectTerminal(snapshotBytes,journalBytes,runId){
  return inspectProgress(snapshotBytes,journalBytes,runId,false);
}
// Does not prove inactivity itself. Use only with the original authenticated
// inactive binding; unlike inspectTerminal this can retain interrupted UNKNOWN.
function inspectInactiveProgress(snapshotBytes,journalBytes,runId){
  return inspectProgress(snapshotBytes,journalBytes,runId,true);
}
function createSelectedAndroidCompletion({adb:configuration,resultDirectory}){
  if(!path.isAbsolute(resultDirectory||''))fail('NATIVE_RESULT_DIRECTORY_REQUIRED');
  const adb=createAdb(configuration);
  let used=false;
  return async function useNative(handle,ready,{signal}){
    if(used)fail('NATIVE_COMPLETION_ALREADY_USED');used=true;
    await fs.mkdir(resultDirectory,{mode:0o700});
    const current=()=>{if(signal.aborted || Date.now()>=Date.parse(ready.expiresAt))fail('NATIVE_COMPLETION_EXPIRED');};
    current();
    const stopped=await handle.waitInactive();current();
    if(stopped.workerActive!==false || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(stopped.nativeRunId||''))fail('NATIVE_STOP_UNCONFIRMED');
    const runId=stopped.nativeRunId,root=`files/trollm/runs/${runId}`;
    const snapshot=await adb(['exec-out','run-as','com.trollm.app','cat',`${root}/state.snapshot.json`],{maxBytes:262144,signal});current();
    const journal=await adb(['exec-out','run-as','com.trollm.app','cat',`${root}/events.jsonl`],{maxBytes:8388608,signal});current();
    const status=inspectInactiveProgress(snapshot,journal,runId);
    const again=await handle.binding();current();
    if(again.workerActive!==false || again.nativeRunId!==runId ||
      ['privateBindingJson','claimJson','sourceAssociationJson'].some(key=>again[key]!==stopped[key]))fail('NATIVE_STOP_ASSOCIATION_CHANGED');
    const intentLogFile=path.join(resultDirectory,'events.jsonl'),reportFile=path.join(resultDirectory,'completion.json');
    for(const [name,bytes]of [['state.snapshot.json',snapshot],['events.jsonl',journal],['worker-binding.json',Buffer.from(JSON.stringify(again))]])
      await fs.writeFile(path.join(resultDirectory,name),bytes,{flag:'wx',mode:0o600});
    await fs.writeFile(reportFile,JSON.stringify({schema:'trollm.private-selected-completion.v1-proposal',
      invocationKey:ready.invocationKey,nativeRunId:runId,status,workerInactiveAcknowledged:true,
      snapshot:{sha256:sha(snapshot),sizeBytes:snapshot.length},journal:{sha256:sha(journal),sizeBytes:journal.length},
      progressDisposition:['running','paused'].includes(identityProjection(snapshot.toString('utf8')).status)?'interrupted':'terminal',
      businessOutcome:'not-established'}),{flag:'wx',mode:0o600});current();
    return {invocationKey:ready.invocationKey,status,intentLogFile,reportFile};
  };
}
module.exports={createSelectedAndroidCompletion,inspectTerminal,inspectInactiveProgress};
