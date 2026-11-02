import assert from "node:assert/strict";
import {mkdtempSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {SessionStore, type SavedSession} from "../../src/session-store";

async function main() {
 const directory=mkdtempSync(join(tmpdir(),"neoism-bundled-recovery-"));
 const env={NEOISM_GPUIX_STATE_PATH:join(directory,"recovery.json")};
 const a=new SessionStore({endpoint:"ws://fixture-a/session"},env);
 const b=new SessionStore({endpoint:"ws://fixture-b/session"},env);
 const snapshot:SavedSession={version:1,surfaces:[],editors:{file:{text:"unsaved α"}},providerStates:{},drafts:{},composerDrafts:{home:{text:"draft"}},ui:{}};
 try {
  await a.writeAsync(snapshot,{reuseEditors:true});
  await b.writeAsync({...snapshot,editors:{},composerDrafts:{home:{text:"other host"}}});
  await a.writeAsync({...snapshot,composerDrafts:{home:{text:"new draft"}}},{reuseEditors:true});
  assert.deepEqual(a.read()?.editors,snapshot.editors);
  assert.equal(a.read()?.composerDrafts.home.text,"new draft");
  assert.equal(b.read()?.composerDrafts.home.text,"other host");
  await a.writeAsync({...snapshot,editors:{file:{text:"new unsaved β"}}},{reuseEditors:true});
  assert.deepEqual(a.read()?.editors,{file:{text:"new unsaved β"}});
  console.log("bundled recovery worker preserved profiles, cached snapshots, and edits");
 } finally {a.dispose();b.dispose();rmSync(directory,{recursive:true,force:true});}
}
main().catch(error=>{console.error(error);process.exitCode=1;});
