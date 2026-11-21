'use strict';
// Original launch/inactive-run custody only. Never produces a report or effects.
const fs=require('node:fs/promises');
const path=require('node:path');
const {createHash}=require('node:crypto');
const {identityProjection}=require('./selected_native_launcher.cjs');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const hash=/^[a-f0-9]{64}$/;
const fail=code=>{throw Object.assign(Error(code),{code});};
function adbFailureStatus(error){
 return {...(Number.isInteger(error?.childExitCode)?{adbExitCode:error.childExitCode}:{}),
  ...(Number.isInteger(error?.stderrBytes)?{stderrBytes:error.stderrBytes}:{})};
}
function relative(value){
  if(typeof value!=='string'||value.length>2048||!value.split('/').every(part=>/^[A-Za-z0-9_.-]+$/.test(part)&&part!=='.'&&part!=='..'))
    fail('NATIVE_CUSTODY_PATH_INVALID');
  return value;
}
async function writeOriginal(directory,name,bytes){
  const file=await fs.open(path.join(directory,name),'wx',0o600);
  try{await file.writeFile(bytes);await file.sync();}finally{await file.close();}
}
async function saveReadDiagnostics(directory,index,logical,error,budget){
  let stderrRetained=false;
  for(const [stream,raw] of [['stdout',error.custodyRaw],['stderr',error.custodyStderr]]){
    if(!raw||budget.used+raw.length>134217728||index.diagnostics.length>=512)continue;
    const file=`diagnostic-${index.diagnostics.length}.raw`;await writeOriginal(directory,file,raw);budget.used+=raw.length;
    index.diagnostics.push({logicalPath:logical,stream,file,sha256:sha(raw),sizeBytes:raw.length,meaning:'failed-read-transport-not-file-original'});
    if(stream==='stderr')stderrRetained=true;
  }
  return stderrRetained;
}
// Structural decoding only, never remote status, app identity or report proof.
function inspectOriginalTar(raw,logical,{maxBytes}){
  if(!Buffer.isBuffer(raw)||!Number.isInteger(maxBytes)||maxBytes<0||maxBytes>33554432||
      raw.length>maxBytes+65536)fail('NATIVE_CUSTODY_FRAME_INVALID');
  relative(logical);
  if(raw.length<1536||raw.length%512)fail('NATIVE_CUSTODY_FRAME_INVALID');
  const text=bytes=>{
    const end=bytes.indexOf(0),value=end<0?bytes:bytes.subarray(0,end);
    if(end>=0&&bytes.subarray(end).some(byte=>byte!==0))fail('NATIVE_CUSTODY_FRAME_INVALID');
    try{return new TextDecoder('utf-8',{fatal:true}).decode(value);}catch{fail('NATIVE_CUSTODY_FRAME_INVALID');}
  };
  const headerAt=offset=>{
    if(raw.length<offset+512)fail('NATIVE_CUSTODY_FRAME_INVALID');
    const header=raw.subarray(offset,offset+512);
    const octal=(start,length)=>{
      const value=header.subarray(start,start+length).toString('latin1');
      if(!/^ *[0-7]+[ \0]*$/.test(value))fail('NATIVE_CUSTODY_FRAME_INVALID');
      const number=Number.parseInt(value,8);
      if(!Number.isSafeInteger(number))fail('NATIVE_CUSTODY_FRAME_INVALID');
      return number;
    };
    let checksum=0;for(let i=0;i<512;i++)checksum+=i>=148&&i<156?32:header[i];
    if(checksum!==octal(148,8))fail('NATIVE_CUSTODY_FRAME_INVALID');
    const magic=header.subarray(257,265),gnu=magic.equals(Buffer.from('ustar  \0'));
    const ustar=magic.equals(Buffer.from('ustar\0'+'00'));
    // V7 has no prefix; GNU's fields at345 are not a USTAR prefix.
    if(!gnu&&!ustar&&magic.some(byte=>byte!==0))fail('NATIVE_CUSTODY_FRAME_INVALID');
    const prefix=ustar?text(header.subarray(345,500)):'';
    return {header,type:header[156],name:text(header.subarray(0,100)),prefix,
      link:text(header.subarray(157,257)),size:octal(124,12),gnu,ustar};
  };
  const paddedEnd=(offset,size)=>{
    const end=offset+512+Math.ceil(size/512)*512;
    if(end>raw.length||raw.subarray(offset+512+size,end).some(byte=>byte!==0))
      fail('NATIVE_CUSTODY_FRAME_INVALID');
    return end;
  };
  let offset=0,member=headerAt(0),longName=false;
  if(member.type===76){
    // One GNU LongLink NAME extension, immediately followed by our one
    // regular member. No long-link target, PAX, arbitrary metadata or files.
    const requested=Buffer.from(relative(logical));
    if(!(member.gnu||member.ustar)||member.name!=='././@LongLink'||member.prefix||member.link||
        requested.length<=100||requested.length>2048||member.size!==requested.length+1)
      fail('NATIVE_CUSTODY_FRAME_INVALID');
    const end=paddedEnd(0,member.size),payload=raw.subarray(512,512+member.size);
    if(!payload.equals(Buffer.concat([requested,Buffer.from([0])])))fail('NATIVE_CUSTODY_FRAME_INVALID');
    offset=end;member=headerAt(offset);longName=true;
    // GNU regular-header name is the exact100-byte prefix, not an unrelated
    // name overridden by uploaded metadata. Full name came from exact bytes.
    if(member.prefix||!member.header.subarray(0,100).equals(requested.subarray(0,100)))
      fail('NATIVE_CUSTODY_FRAME_INVALID');
  }
  const name=(member.prefix?member.prefix+'/':'')+member.name;
  if((!longName&&name!==logical)||![0,48].includes(member.type)||member.link||member.size>maxBytes)
    fail('NATIVE_CUSTODY_FRAME_INVALID');
  const end=paddedEnd(offset,member.size);
  if(raw.length<end+1024||raw.subarray(end).some(value=>value!==0))fail('NATIVE_CUSTODY_FRAME_INVALID');
  return raw.subarray(offset+512,offset+512+member.size);
}

// Exact ordered multi-file framing for bounded calibration batches only. Every
// member still passes the single-member strict decoder, including GNU LongLink.
function inspectOriginalTarBatch(raw,logicals,{maxBytes=65536}={}){
  if(!Number.isInteger(maxBytes)||maxBytes<0||maxBytes>65536||!Array.isArray(logicals)||!logicals.length||logicals.length>64||new Set(logicals).size!==logicals.length||
    !Buffer.isBuffer(raw)||raw.length%512||raw.length>logicals.length*(maxBytes+4096)+1024)fail('NATIVE_CUSTODY_FRAME_INVALID');
  let offset=0;const values=[];
  const next=()=>{
    if(offset+512>raw.length)fail('NATIVE_CUSTODY_FRAME_INVALID');
    const h=raw.subarray(offset,offset+512),field=h.subarray(124,136).toString('latin1');
    if(!/^ *[0-7]+[ \0]*$/.test(field))fail('NATIVE_CUSTODY_FRAME_INVALID');
    const size=Number.parseInt(field,8);
    if(!Number.isSafeInteger(size)||size>maxBytes)fail('NATIVE_CUSTODY_FRAME_INVALID');
    offset+=512+Math.ceil(size/512)*512;
    if(offset>raw.length)fail('NATIVE_CUSTODY_FRAME_INVALID');
    return h[156];
  };
  for(const logical of logicals){
    relative(logical);const start=offset;
    if(next()===76)next();
    values.push(inspectOriginalTar(Buffer.concat([raw.subarray(start,offset),Buffer.alloc(1024)]),logical,{maxBytes}));
  }
  if(raw.length<offset+1024||raw.subarray(offset).some(b=>b!==0))fail('NATIVE_CUSTODY_FRAME_INVALID');
  return values;
}
async function readClockBatch(adb,logicals,{canonicalRoot,runRoot,signal,check}){
  const confined=async()=>{
    check();
    const current=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',runRoot],{signal,maxBytes:4096})).toString().trim();
    if(current!==canonicalRoot+'/'+runRoot.slice('files/trollm/'.length))fail('NATIVE_CUSTODY_PATH_ESCAPE');
  };
  await confined();let raw;
  try{raw=await adb(['shell','-T','run-as','com.trollm.app','tar','-cf','-',...logicals],
    {maxBytes:logicals.length*(65536+4096)+1024,signal,captureFailureOutput:true});}
  catch(error){error.custodyRaw=error.privateStdout;error.custodyStderr=error.privateStderr;throw error;}
  try{const values=inspectOriginalTarBatch(raw,logicals);await confined();check();return values;}
  catch(error){error.custodyRaw=raw;throw error;}
}

async function readAppOriginal(adb,logical,{maxBytes,signal}){
  // shell_v2 and -T preserve remote status and binary stdout without a PTY.
  // Archive framing independently binds success to one actual regular file.
  // No shell compound command or text-pattern error heuristic is involved.
  let raw;
  try{raw=await adb(['shell','-T','run-as','com.trollm.app','tar','-cf','-',logical],
    {maxBytes:maxBytes+65536,signal,captureFailureOutput:true});}
  catch(error){error.custodyRaw=error.privateStdout;error.custodyStderr=error.privateStderr;throw error;}
  try{return inspectOriginalTar(raw,logical,{maxBytes});}
  catch(error){error.custodyRaw=raw;throw error;}
}
// Only the exact, complete remote stat response can establish absence.
async function statOutcomeMember(adb,logical,signal){
  let raw;
  try{
    raw=await adb(['shell','-T','run-as','com.trollm.app','stat','-c','%F',logical],
      {maxBytes:4096,signal,captureFailureOutput:true});
  }catch(error){
    if(error.privateStdout?.length)error.custodyRaw=error.privateStdout;
    error.custodyStderr=error.privateStderr;
    const absent=Buffer.from(`stat: '${logical}': No such file or directory\n`);
    if(error.code==='ADB_COMMAND_FAILED'&&error.childExitCode===1&&error.childSignal===null&&
      error.timedOut===false&&error.outputTruncated===false&&error.stdoutBytes===0&&
      error.privateStdout?.length===0&&error.stderrBytes===absent.length&&error.privateStderr?.equals(absent))return error;
    throw error;
  }
  if(!['regular file','regular empty file'].includes(raw.toString('utf8').trimEnd())){
    throw Object.assign(Error('NATIVE_CUSTODY_OUTCOME_TYPE_INVALID'),{
      code:'NATIVE_CUSTODY_OUTCOME_TYPE_INVALID',custodyRaw:raw});
  }
  return null;
}
async function requireShellV2(adb,signal,directory,index,budget,check){
  index.adbProbes=[];
  const probe=async(name)=>{
    check();
    let result,error;
    try{result=await adb([name],{maxBytes:8192,signal,captureResult:true,captureFailureOutput:true});}
    catch(failure){error=failure;result={stdout:failure.privateStdout,stderr:failure.privateStderr,
      childExitCode:failure.childExitCode,childSignal:failure.childSignal??null,stdoutBytes:failure.stdoutBytes,
      stderrBytes:failure.stderrBytes,command:failure.adbCommand,outputTruncated:failure.outputTruncated,timedOut:failure.timedOut};}
    // A Buffer alone establishes no explicit command/status provenance.
    const stdout=Buffer.isBuffer(result?.stdout)?result.stdout:Buffer.isBuffer(result)?result:Buffer.alloc(0);
    const stderr=Buffer.isBuffer(result?.stderr)?result.stderr:Buffer.alloc(0);
    const row={probe:name,meaning:'adb-command-original-not-app-file',
      command:result?.command??null,childExitCode:Number.isInteger(result?.childExitCode)?result.childExitCode:null,
      childSignal:result?.childSignal??null,stdoutBytesObserved:Number.isInteger(result?.stdoutBytes)?result.stdoutBytes:null,
      stderrBytesObserved:Number.isInteger(result?.stderrBytes)?result.stderrBytes:null,
      outputTruncated:result?.outputTruncated===true,timedOut:result?.timedOut===true,streams:[]};
    for(const [stream,bytes] of [['stdout',stdout],['stderr',stderr]]){
      if(budget.used+bytes.length>134217728||index.diagnostics.length>=512)fail('NATIVE_CUSTODY_LIMIT');
      const file=`diagnostic-${index.diagnostics.length}.raw`;await writeOriginal(directory,file,bytes);budget.used+=bytes.length;
      const observed=stream==='stdout'?row.stdoutBytesObserved:row.stderrBytesObserved;
      const member={logicalPath:`adb/${name}`,stream,file,sha256:sha(bytes),sizeBytes:bytes.length,
        coverage:observed===bytes.length?'complete':'bounded-prefix-or-unavailable',
        meaning:'adb-command-original-not-app-file'};
      index.diagnostics.push(member);row.streams.push(member);
    }
    const valid=!!row.command&&typeof row.command.executable==='string'&&Array.isArray(row.command.argv)&&
      row.command.argv.at(-1)===name&&row.childExitCode===0&&row.childSignal===null&&!row.outputTruncated&&!row.timedOut&&
      row.stdoutBytesObserved===stdout.length&&row.stderrBytesObserved===stderr.length&&!error;
    row.status=valid?'success':'unavailable';index.adbProbes.push(row);
    check();
    if(!valid){index.missing.push({logicalPath:`adb/${name}`,code:'NATIVE_CUSTODY_ADB_PROBE_FAILED',
      ...adbFailureStatus(result)});return null;}
    return stdout;
  };
  // Collect both independent probe results even when version discovery fails.
  await probe('version');
  const raw=await probe('features');
  if(raw===null)fail('NATIVE_CUSTODY_REMOTE_STATUS_UNAVAILABLE');
  let text;try{text=new TextDecoder('utf-8',{fatal:true}).decode(raw);}catch{fail('NATIVE_CUSTODY_ADB_FEATURES_INVALID');}
  const features=text.trim().split(/[,\t\r\n ]+/);
  if(!features.length||features.some(value=>!/^[_a-zA-Z0-9.-]+$/.test(value)))fail('NATIVE_CUSTODY_ADB_FEATURES_INVALID');
  if(!features.includes('shell_v2'))fail('NATIVE_CUSTODY_REMOTE_STATUS_UNAVAILABLE');
}
async function clockOriginalNames(adb,runRoot,signal,check){
  check();
  let raw;try{raw=await adb(['shell','-T','run-as','com.trollm.app','ls','-1',runRoot],
    {maxBytes:262144,signal,captureFailureOutput:true});}
  catch(error){error.custodyRaw=error.privateStdout;error.custodyStderr=error.privateStderr;throw error;}
  check();
  try{
    let text;try{text=new TextDecoder('utf-8',{fatal:true}).decode(raw);}catch{fail('NATIVE_CUSTODY_CLOCK_DIAGNOSTIC_INVALID');}
    const names=text.trimEnd().split('\n').filter(name=>name.startsWith('clock-calibration-')||name==='clock-acquisition-limit.json');
    if(names.some(name=>name!=='clock-acquisition-limit.json'&&!/^clock-calibration-[0-9]{6,20}\.(?:request|response|sent-bracket|received-bracket|received-unavailable|response-metadata|body-read|bracket|failure)\.json$/.test(name)))
      fail('NATIVE_CUSTODY_CLOCK_DIAGNOSTIC_INVALID');
    if(names.length>3915||new Set(names).size!==names.length)fail('NATIVE_CUSTODY_LIMIT');
    return names.sort().map(name=>`${runRoot}/${name}`);
  }catch(error){error.custodyRaw=raw;throw error;}
}
async function httpDiagnosticNames(adb,runRoot,signal,check){
  check();
  let raw;try{raw=await adb(['shell','-T','run-as','com.trollm.app','ls','-1',`${runRoot}/artifacts`],
    {maxBytes:65536,signal,captureFailureOutput:true});}
  catch(error){error.custodyRaw=error.privateStdout;error.custodyStderr=error.privateStderr;throw error;}
  check();
  let names;try{names=new TextDecoder('utf-8',{fatal:true}).decode(raw).trimEnd().split('\n');}
  catch{fail('NATIVE_CUSTODY_HTTP_DIAGNOSTIC_INVALID');}
  names=names.filter(name=>name.startsWith('mcp-http-failure-')||name.startsWith('native-result-path-')||name.startsWith('selected-result-'));
  if(names.length>512||new Set(names).size!==names.length)fail('NATIVE_CUSTODY_LIMIT');
  try{return names.sort().map(name=>{
    const selected=/^selected-result-([a-f0-9]{64})\.json(\.meta\.json)?$/.exec(name);
    if(selected)return {logical:`${runRoot}/artifacts/${name}`,maxBytes:selected[2]?4096:1048576,
      custodyAssociation:'selected-result-file-original-not-a-journal-commitment-or-ACK'};
    const match=/^(?:mcp-http-failure|native-result-path)-([a-f0-9]{64})\.json$/.exec(name);
    if(!match)fail((name.startsWith('native-result-path-')||name.startsWith('selected-result-'))?'NATIVE_CUSTODY_RESULT_DIAGNOSTIC_INVALID':'NATIVE_CUSTODY_HTTP_DIAGNOSTIC_INVALID');
    return {logical:`${runRoot}/artifacts/${name}`,sha256:match[1]};
  });}catch(error){error.custodyRaw=raw;throw error;}
}
async function retainOriginalCustody({adb,binding,snapshot,journal,resultDirectory,signal,check,phase='inactive'}){
  if(!['launch','inactive'].includes(phase)||typeof binding?.workerActive!=='boolean'||(phase==='inactive'&&binding.workerActive!==false)||!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(binding.nativeRunId||''))fail('NATIVE_STOP_UNCONFIRMED');
  const runId=binding.nativeRunId,runRoot=`files/trollm/runs/${runId}`;
  const index={schema:'trollm.original-inactive-custody.v1-proposal',nativeRunId:runId,
    phase,workerInactiveAcknowledged:phase==='inactive',complete:false,meaning:'original-custody-only-not-a-report',
    retentionScope:'selected-app-root-members-not-whole-run-or-app-tree',members:[],missing:[],diagnostics:[]};
  const directory=path.join(resultDirectory,phase==='launch'?'original-launch-custody':'original-custody');
  await fs.mkdir(directory,{mode:0o700});
  const budget={used:0};
  const retain=async(logical,bytes)=>{
    if(index.members.length>=6144||bytes.length>33554432||budget.used+bytes.length>134217728)fail('NATIVE_CUSTODY_LIMIT');
    const digest=sha(bytes),name=`${index.members.length.toString().padStart(4,'0')}-${digest}.raw`;
    await writeOriginal(directory,name,bytes);budget.used+=bytes.length;
    index.members.push({logicalPath:logical,file:name,sha256:digest,sizeBytes:bytes.length});
    return bytes;
  };
  const missing=(logical,error,expected)=>index.missing.push({logicalPath:logical,...adbFailureStatus(error),...(expected?{...(expected.sha256!==undefined?{expectedSha256:expected.sha256}:{}),...(expected.sizeBytes!==undefined?{expectedSizeBytes:String(expected.sizeBytes)}:{})}:{}),code:
    typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});
  const absentOutcome=Symbol('confirmed absent outcome original');
  const read=async(logical,expected)=>{
    try{
      check();
      if(expected?.sizeBytes!==undefined&&(!/^[0-9]+$/.test(String(expected.sizeBytes))||BigInt(expected.sizeBytes)>33554432n))fail('NATIVE_CUSTODY_LIMIT');
      const expectedPath=logical==='shared_prefs/moray-source-device.xml'?canonicalRoot.replace(/\/files\/trollm$/,'/'+logical):canonicalRoot+'/'+logical.slice('files/trollm/'.length);
      if(expected?.optionalOutcome){
        // The report directory itself may never have been created. Resolve it
        // as the final component before considering a missing child.
        const parent=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',path.posix.dirname(logical)],
          {maxBytes:4096,signal})).toString('utf8').trim();
        if(parent!==path.posix.dirname(expectedPath))fail('NATIVE_CUSTODY_PATH_ESCAPE');
        const absence=await statOutcomeMember(adb,logical,signal);check();
        if(absence){
          if(await saveReadDiagnostics(directory,index,logical,absence,budget))return absentOutcome;
          missing(logical,{childExitCode:absence.childExitCode,stderrBytes:absence.stderrBytes,code:'NATIVE_CUSTODY_LIMIT'});
          return null;
        }
      }
      const canonical=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',logical],{maxBytes:4096,signal})).toString('utf8').trim();
      if(canonical!==expectedPath)fail('NATIVE_CUSTODY_PATH_ESCAPE');
      const bytes=await readAppOriginal(adb,logical,{maxBytes:expected?.maxBytes??33554432,signal});check();
      // Keep actual bytes even on identity mismatch; never repair them.
      await retain(logical,bytes);
      if(expected?.originalArtifactPath)index.members.at(-1).originalArtifactPath=expected.originalArtifactPath;
      if(expected?.custodyAssociation)index.members.at(-1).custodyAssociation=expected.custodyAssociation;
      if(expected&&(expected.sha256!==undefined&&expected.sha256!==sha(bytes)||expected.sizeBytes!==undefined&&BigInt(expected.sizeBytes)!==BigInt(bytes.length)))
        fail('NATIVE_CUSTODY_HASH_MISMATCH');
      return bytes;
    }catch(error){
      await saveReadDiagnostics(directory,index,logical,error,budget);
      missing(logical,error,expected);return null;
    }
  };
  let canonicalRoot;
  try{
    await requireShellV2(adb,signal,directory,index,budget,check);
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
    try{
      const names=await clockOriginalNames(adb,runRoot,signal,check);
      if(names.length<=16){for(const logical of names)await read(logical);}
      else for(let i=0;i<names.length;i+=64){
        const batch=names.slice(i,i+64);
        try{const values=await readClockBatch(adb,batch,{canonicalRoot,runRoot,signal,check});
          for(let j=0;j<batch.length;j++)await retain(batch[j],values[j]);}
        catch(error){for(const logical of batch)missing(logical,error);
          await saveReadDiagnostics(directory,index,`${runRoot}/clock-batch-${i}`,error,budget);}
      }
    }catch(error){missing(`${runRoot}/clock-calibration-originals`,error);
      await saveReadDiagnostics(directory,index,`${runRoot}/clock-calibration-originals`,error,budget);}
    try{
      for(const item of await httpDiagnosticNames(adb,runRoot,signal,check))await read(item.logical,
        {sha256:item.sha256,maxBytes:item.maxBytes??524288,custodyAssociation:item.custodyAssociation});
    }catch(error){missing(`${runRoot}/http-response-originals`,error);
      await saveReadDiagnostics(directory,index,`${runRoot}/http-response-originals`,error,budget);}
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
    if(state.runId!==runId)fail('NATIVE_CUSTODY_SOURCE_MISMATCH');
    const committed=new Map(),preparedContexts=new Map(),outcomeAcknowledged=new Set();let sequence=0n;
    for(const line of journal.toString('utf8').trimEnd().split('\n')){
      const event=identityProjection(line);
      if(event.runId!==runId||!/^\d+$/.test(event.seq||'')||BigInt(event.seq)!==++sequence)
        fail('NATIVE_CUSTODY_JOURNAL_INVALID');
      if(event.type==='tool_call_prepared'&&event.nativeContext){
        const native=event.nativeContext;
        if(typeof native.json!=='string'||Buffer.byteLength(native.json)>65536||!hash.test(native.sha256||'')||sha(Buffer.from(native.json))!==native.sha256||
          identityProjection(native.json).runId!==runId||preparedContexts.has(native.sha256))fail('NATIVE_CUSTODY_CONTEXT_INVALID');
        preparedContexts.set(native.sha256,native);
        if(preparedContexts.size>128)fail('NATIVE_CUSTODY_LIMIT');
      }
      if(event.type==='tool_call_outcome_acknowledged'){
        if(!preparedContexts.has(event.contextSha256))fail('NATIVE_CUSTODY_CONTEXT_INVALID');
        outcomeAcknowledged.add(event.contextSha256);
      }
      if(event.type!=='artifact_written')continue;
      if(!hash.test(event.sha256||'')||!/^\d+$/.test(event.bytes||''))fail('NATIVE_CUSTODY_ARTIFACT_INVALID');
      const originalPath=/^\/data\/(?:user\/0|data)\/com\.trollm\.app\/files\/trollm\/runs\/([^/]+)\/(.+)$/.exec(event.path??'');
      if(!originalPath||originalPath[1]!==runId)fail('NATIVE_CUSTODY_ARTIFACT_PATH');
      const suffix=relative(originalPath[2]),canonicalPath=canonicalRoot+`/runs/${runId}/${suffix}`;
      const previous=committed.get(canonicalPath);
      if(previous&&(previous.sha256!==event.sha256||BigInt(previous.sizeBytes)!==BigInt(event.bytes)))
        fail('NATIVE_CUSTODY_COMMITMENT_CONFLICT');
      // Preserve the original journal spelling. Only the private lookup key is
      // canonical-root-relative; read() still proves actual guest confinement.
      committed.set(canonicalPath,{suffix,sha256:event.sha256,sizeBytes:event.bytes,originalArtifactPath:event.path});
    }
    if(state.eventWatermark!==undefined&&(!/^\d+$/.test(state.eventWatermark)||BigInt(state.eventWatermark)>sequence))
      fail('NATIVE_CUSTODY_JOURNAL_INVALID');
    for(const record of committed.values())await read(`${runRoot}/${record.suffix}`,record);
    index.nativeOutcomes=[];
    const same=(left,right)=>{
      const sort=value=>value&&typeof value==='object'?(Array.isArray(value)?value.map(sort):
        Object.fromEntries(Object.keys(value).sort().map(key=>[key,sort(value[key])]))):value;
      return JSON.stringify(sort(left))===JSON.stringify(sort(right));
    };
    for(const [contextSha256,native] of preparedContexts){
      const requestPath=`${runRoot}/selected-report/${contextSha256}.request.json`,ackPath=`${runRoot}/selected-report/${contextSha256}.ack.json`;
      const row={contextSha256,request:'unavailable',ack:'unavailable',delivery:'not-established',journalOutcomeAcknowledged:outcomeAcknowledged.has(contextSha256),
        meaning:'native-request-and-response-originals-not-independent-settlement'};
      index.nativeOutcomes.push(row);
      const readOutcome=(logical,maxBytes)=>read(logical,{maxBytes,
        optionalOutcome:!row.journalOutcomeAcknowledged&&!committed.has(canonicalRoot+'/'+logical.slice('files/trollm/'.length))});
      const raw=await readOutcome(requestPath,262144);
      if(raw===absentOutcome){
        row.request='confirmed-absent';
        const ack=await readOutcome(ackPath,65536);
        if(ack===absentOutcome)row.ack='confirmed-absent';
        else if(ack){
          row.ack='retained-unverified';
          missing(ackPath,{code:'NATIVE_CUSTODY_OUTCOME_REQUEST_UNAVAILABLE'});
        }
        continue;
      }
      if(!raw)continue;
      row.request='retained-unverified';
      let stagePath=requestPath;
      try{
        const request=identityProjection(raw.toString('utf8')),guard=request.guardRequest;
        const expected=source.descriptor?.source,claim=identityProjection(binding.claimJson),installation=identityProjection(binding.privateBindingJson);
        const context=identityProjection(native.json),wire=request.nativeContext;
        const selectionMember=index.members.find(member=>member.logicalPath===`files/trollm/bundle-revisions/${revision}/selection.json`);
        if(!selectionMember)fail('NATIVE_CUSTODY_OUTCOME_SELECTION_UNAVAILABLE');
        const selectionBytes=await fs.readFile(path.join(directory,selectionMember.file));
        if(sha(selectionBytes)!==expected?.selectionRef?.sha256)fail('NATIVE_CUSTODY_OUTCOME_SELECTION_UNAVAILABLE');
        const selection=identityProjection(selectionBytes.toString('utf8'));
        if(request.schema!=='trollm.private-collection-guard.v1'||!expected?.selectionRef||
          selection.schema!=='moray.declared-collection-selection.v1-proposal'||claim.nativeRunId!==runId||installation.nativeRunId!==runId||
          request.invocationKey!==claim.identity?.invocationKey||request.invocationKey!==installation.invocationKey||
          request.invocationKey!==selection.invocationKey||guard?.selectionId!==selection.selectionId||guard?.caseId!==selection.caseId||
          !same(selection.owner,expected.owner)||
          !same(request.selectionRef,expected.selectionRef)||!same(guard?.selectionRef,expected.selectionRef)||
          !same(guard?.owner,expected.owner)||guard?.selectionRawSha256!==expected.selectionRef.sha256||
          guard?.kind!=='outcome'||guard?.outcome!=='OBSERVATION_RETURNED'||guard?.contextSha256!==contextSha256||
          wire?.sha256!==contextSha256||wire?.json!==native.json||!same(wire?.callContext,native.callContext)||
          !same(wire?.callContext,context.callContext))fail('NATIVE_CUSTODY_OUTCOME_ASSOCIATION_INVALID');
        const evidence=guard.journalEvidence,bytes=Buffer.from(evidence?.bytesBase64??'','base64');
        if(bytes.length>16384||bytes.toString('base64')!==evidence?.bytesBase64||sha(bytes)!==evidence?.sha256)
          fail('NATIVE_CUSTODY_OUTCOME_EVIDENCE_INVALID');
        row.request='retained-associated';
        stagePath=ackPath;
        const ack=await readOutcome(ackPath,65536);
        if(ack===absentOutcome){row.ack='confirmed-absent';continue;}
        if(!ack){row.ack='absent-or-unavailable';continue;}
        row.ack='retained-unverified';
        const value=identityProjection(ack.toString('utf8'));
        if(Object.keys(value).length!==2||value.recorded!==true||value.source!=='native-adapter-reported-not-independent-settlement')
          fail('NATIVE_CUSTODY_OUTCOME_ACK_INVALID');
        row.ack='retained-native-response';
      }catch(error){missing(stagePath,error);}
    }
    const cursors=[...Object.values(state.foreachCursors||{}),
      ...(state.stack||[]).flatMap(frame=>Object.values(frame.foreachCursors||{}))];
    for(const cursor of cursors){
      const suffix=relative(cursor.snapshotRef),logical=`${runRoot}/${suffix}`;
      if(!committed.has(`${canonicalRoot}/runs/${runId}/${suffix}`)||
         !index.members.some(member=>member.logicalPath===logical)||index.missing.some(member=>member.logicalPath===logical))
        index.missing.push({logicalPath:logical,code:'NATIVE_CUSTODY_CURSOR_UNAVAILABLE'});
    }
    // A raw second read detects a changing source after claimed inactivity.
    const reread=await readAppOriginal(adb,`${runRoot}/state.snapshot.json`,{maxBytes:262144,signal});check();
    if(!reread.equals(snapshot))fail('NATIVE_CUSTODY_SNAPSHOT_CHANGED');
    const journalAgain=await readAppOriginal(adb,`${runRoot}/events.jsonl`,{maxBytes:8388608,signal});check();
    if(!journalAgain.equals(journal))fail('NATIVE_CUSTODY_JOURNAL_CHANGED');
    index.complete=index.missing.length===0;
  }catch(error){
    await saveReadDiagnostics(directory,index,'custody-collection',error,budget);
    missing('custody-collection',error);
  }
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
    meaning:'partial-originals-only-not-a-binding-report-or-stop-proof',
    retentionScope:'selected-app-root-members-not-whole-run-or-app-tree',members:[],missing:[],diagnostics:[]};
  const budget={used:0};
  try{
    check();
    await requireShellV2(adb,signal,directory,index,budget,check);
    const base=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f','files/trollm'],{signal,maxBytes:4096})).toString().trim();
    if(!/^\/data\/(?:user\/0|data)\/com\.trollm\.app\/files\/trollm$/.test(base))fail('NATIVE_CUSTODY_ROOT_INVALID');
    const files=['request.json','result.json','started.json','claim','done'].map(suffix=>[`files/trollm/selected-launcher/${requestSha256}.${suffix}`,1048576]);
    files.push([`files/trollm/selected-installations/${installationKey}.json`,1048576]);
    if(nativeRunId)for(const name of ['source-association.json','installation.json','input.original.json','input.normalized.json',
      'input-admission.json','configuration.json','selected-start-failure.json','selected-invocation.json',
      'selected-private-installation.json','state.snapshot.json','events.jsonl'])
      files.push([`files/trollm/runs/${nativeRunId}/${name}`,name==='events.jsonl'?8388608:1048576]);
    if(nativeRunId){
      try{
        const runRoot=`files/trollm/runs/${nativeRunId}`,names=await clockOriginalNames(adb,runRoot,signal,check);
        if(names.length<=16){for(const logical of names)files.push([logical,65536]);}
        else for(let i=0;i<names.length;i+=64){
          const batch=names.slice(i,i+64);
          try{const values=await readClockBatch(adb,batch,{canonicalRoot:base,runRoot,signal,check});
            for(let j=0;j<batch.length;j++){
              const bytes=values[j];if(index.members.length>=6144||budget.used+bytes.length>134217728)fail('NATIVE_CUSTODY_LIMIT');
              const digest=sha(bytes),file=`${index.members.length.toString().padStart(4,'0')}-${digest}.raw`;
              await writeOriginal(directory,file,bytes);budget.used+=bytes.length;
              index.members.push({logicalPath:batch[j],file,sha256:digest,sizeBytes:bytes.length});
            }}catch(error){await saveReadDiagnostics(directory,index,`${runRoot}/clock-batch-${i}`,error,budget);
              for(const logical of batch)index.missing.push({logicalPath:logical,...adbFailureStatus(error),code:error.code??'NATIVE_CUSTODY_READ_FAILED'});}
        }
      }catch(error){await saveReadDiagnostics(directory,index,`files/trollm/runs/${nativeRunId}/clock-calibration-originals`,error,budget);
        index.missing.push({logicalPath:`files/trollm/runs/${nativeRunId}/clock-calibration-originals`,
        ...adbFailureStatus(error),code:typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});}
    }
    if(nativeRunId){
      try{for(const item of await httpDiagnosticNames(adb,`files/trollm/runs/${nativeRunId}`,signal,check))
        files.push([item.logical,item.maxBytes??524288,item.sha256,item.custodyAssociation]);}
      catch(error){await saveReadDiagnostics(directory,index,`files/trollm/runs/${nativeRunId}/http-response-originals`,error,budget);
        index.missing.push({logicalPath:`files/trollm/runs/${nativeRunId}/http-response-originals`,
          code:typeof error?.code==='string'&&/^[A-Z][A-Z0-9_]{0,63}$/.test(error.code)?error.code:'NATIVE_CUSTODY_READ_FAILED'});}
    }
    for(const [logical,maximum,expectedDiagnosticSha,custodyAssociation]of files){
      try{
        check();
        const real=(await adb(['exec-out','run-as','com.trollm.app','readlink','-f',logical],{signal,maxBytes:4096})).toString().trim();
        if(real!==base+'/'+logical.slice('files/trollm/'.length))fail('NATIVE_CUSTODY_PATH_ESCAPE');
        const bytes=await readAppOriginal(adb,logical,{signal,maxBytes:maximum});check();
        if(index.members.length>=6144||budget.used+bytes.length>134217728)fail('NATIVE_CUSTODY_LIMIT');
        const digest=sha(bytes),file=`${index.members.length.toString().padStart(4,'0')}-${digest}.raw`;
        await writeOriginal(directory,file,bytes);
        budget.used+=bytes.length;
        index.members.push({logicalPath:logical,file,sha256:digest,sizeBytes:bytes.length,...(custodyAssociation?{custodyAssociation}:{})});
        if(expectedDiagnosticSha&&digest!==expectedDiagnosticSha)fail('NATIVE_CUSTODY_HASH_MISMATCH');
        if(logical.endsWith(`/${requestSha256}.request.json`)){
          const request=JSON.parse(bytes);
          if(digest!==requestSha256||request.installationKey!==installationKey)fail('NATIVE_CUSTODY_HASH_MISMATCH');
          if(typeof request.invocationKey!=='string'||!request.invocationKey||request.invocationKey.length>1024)fail('NATIVE_CUSTODY_INVOCATION_INVALID');
          files.push([`files/trollm/selected-launcher/invocation-${sha(Buffer.from(request.invocationKey))}.claim`,1048576]);
        }
        if(logical.includes('/selected-launcher/invocation-')&&!bytes.equals(Buffer.from(requestSha256)))
          fail('NATIVE_CUSTODY_HASH_MISMATCH');
        if(logical.endsWith(`/selected-installations/${installationKey}.json`)&&digest!==installationKey)
          fail('NATIVE_CUSTODY_HASH_MISMATCH');
      }catch(error){
        await saveReadDiagnostics(directory,index,logical,error,budget);
        index.missing.push({logicalPath:logical,...adbFailureStatus(error),code:
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
module.exports={retainOriginalCustody,retainPartialStartCustody,writeOriginal,inspectOriginalTar,inspectOriginalTarBatch};
