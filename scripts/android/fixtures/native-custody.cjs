'use strict';
// Declared transport data only; never actual device evidence.
const {createHash}=require('node:crypto');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const base='/data/user/0/com.trollm.app/files/trollm';
function fixture(id='11111111-2222-4333-8444-555555555555'){
 const root=`files/trollm/runs/${id}`,raw=Buffer.from('{"selectionSha256":"'+'a'.repeat(64)+'"}');
 const admission={schema:'trollm.source-input-admission.v1',raw:{sha256:sha(raw),sizeBytes:raw.length},
  normalized:{sha256:sha(raw),sizeBytes:raw.length},apkNormalized:raw.toString()};
 const setup=Buffer.from('{"input":{"fields":[{"key":"selectionSha256"}]}}');
 const manifest=Buffer.from(JSON.stringify({format:'trollm.flow-bundle.v1',files:[{path:'setup.json',sha256:sha(setup)}]}));
 const source={nativeRunId:id,inputAdmission:admission,installation:{installationSha256:sha(manifest)}};
 const binding={nativeRunId:id,workerActive:false,sourceAssociationJson:JSON.stringify(source),claimJson:'{"original":"claim"}',privateBindingJson:'{"original":"private"}'};
 const item=Buffer.from('{"id":"foreign-preserved","quantityText":"1"}\n');
 const snapshot=Buffer.from(JSON.stringify({runId:id,status:'failed',foreachCursors:{items:{snapshotRef:'artifacts/items.snapshot.jsonl'}}}));
 const journal=Buffer.from(JSON.stringify({runId:id,seq:1,type:'artifact_written',path:`${base}/runs/${id}/artifacts/items.snapshot.jsonl`,sha256:sha(item),bytes:item.length})+'\n');
 const files=new Map([['shared_prefs/moray-source-device.xml',Buffer.from('<map><string name="id">original-declared-device</string></map>')],[`${root}/source-association.json`,Buffer.from(binding.sourceAssociationJson)],
  [`${root}/selected-invocation.json`,Buffer.from(binding.claimJson)],[`${root}/selected-private-installation.json`,Buffer.from(binding.privateBindingJson)],
  [`${root}/configuration.json`,Buffer.from('{"original":"config"}')],[`${root}/input.original.json`,raw],[`${root}/input.normalized.json`,raw],
  [`${root}/input-admission.json`,Buffer.from(JSON.stringify(admission))],[`${root}/artifacts/items.snapshot.jsonl`,item],
  [`${root}/state.snapshot.json`,snapshot],[`${root}/events.jsonl`,journal],[`files/trollm/bundle-revisions/${sha(manifest)}/manifest.json`,manifest],
  [`files/trollm/bundle-revisions/${sha(manifest)}/setup.json`,setup]]);
 return {root,binding,snapshot,journal,files};
}
module.exports=fixture;
