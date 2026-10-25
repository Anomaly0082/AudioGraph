// Independent hook regressions. Run directly: node tests/workspace-files-review.test.mjs
// Compiles current source in memory; no native API, user files, browser, or build output involved.
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import vm from "node:vm";
const require=createRequire(import.meta.url), ts=require("typescript");
function load(relative, imports) {
  const filename=new URL(relative,import.meta.url);
  const code=ts.transpileModule(readFileSync(filename,"utf8"),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS}}).outputText;
  const exports={};vm.runInThisContext(`(function(require,exports){${code}\n})`,{filename:filename.pathname})(id=>{
    if (!(id in imports)) throw Error(`Unexpected import ${id}`);
    return imports[id];
  },exports);return exports;
}
function host(api) {
  const slots=[];let cursor=0,dirty=false,effects=[],result;
  const react={
    useState(initial){const i=cursor++;if(!(i in slots))slots[i]=typeof initial==="function"?initial():initial;return [slots[i],value=>{const next=typeof value==="function"?value(slots[i]):value;if(!Object.is(next,slots[i])){slots[i]=next;dirty=true;}}];},
    useRef(initial){const i=cursor++;if(!(i in slots))slots[i]={current:initial};return slots[i];},
    useCallback(callback){cursor++;return callback;},
    useEffect(effect,deps){const i=cursor++,old=slots[i];if(!old||deps.some((v,n)=>!Object.is(v,old[n]))){slots[i]=deps;effects.push(effect);}},
  };
  react.useLayoutEffect=react.useEffect;
  const model=load("../src/workspace-browser-model.ts",{"./model":{parseGraph:()=>{throw Error("Unexpected graph parse");}}});
  const hook=load("../src/hooks/useWorkspaceBrowser.ts",{react,"../api/desktop":{desktopAvailable:()=>true,audioPreviewUrl:token=>`http://audiopreview.localhost/${token}`,closeAudioPreview:token=>api("workspace_audio_close",{token}),listenWorkspaceFileDrop:async()=>()=>{},invokeDesktop:api},"../model":{formatError:String},"../workspace-browser-model":model}).useWorkspaceBrowser;
  const connection={sessionId:"review-session",workspace:"C:/review"};
  function render(){do{dirty=false;cursor=0;effects=[];result=hook(connection,true,{enabled:true});for(const effect of effects)effect();}while(dirty);return result;}
  async function flush(){await new Promise(resolve=>setImmediate(resolve));return render();}
  render();return {render,flush};
}
const entry=(path,kind="file")=>({name:path.split("/").pop(),path,kind,bytes:0,modified_at_ms:null});
const listing=(args,entries,next=null)=>({space:args.space,path:args.path,entries,offset:args.offset??0,next_offset:next,total:entries.length,partial:false,warnings:[]});

test("path copying invokes only scoped native clipboard command and reports failure",async()=>{
  const copies=[];let fail=false;
  const h=host(async(command,args)=>{
    if(command==="workspace_browser_list")return listing(args,[entry("clip.wav")]);
    assert.equal(command,"workspace_copy_path");copies.push(args);
    if(fail)throw Error("clipboard unavailable");
  });
  let b=await h.flush();
  await b.copyPath(entry("clip.wav"));await b.copyPath(entry("clip.wav"),true);
  assert.deepEqual(copies,[
    {sessionId:"review-session",space:"user",path:"clip.wav",relative:false},
    {sessionId:"review-session",space:"user",path:"clip.wav",relative:true},
  ]);
  await b.copyPath(entry("untrusted.bin","unsupported"));assert.equal(copies.length,2);
  fail=true;await b.copyPath(entry("clip.wav"));b=h.render();assert.match(b.error,/clipboard unavailable/);
});

test("paging and refresh clear invisible active targets and pending rename",async()=>{
  let replacement=false;
  const h=host(async(command,args)=>{
    assert.equal(command,"workspace_browser_list");
    return args.offset?listing(args,[entry("page-two.bin")]):listing(args,[entry(replacement?"replacement.bin":"page-one.bin")],100);
  });
  let b=await h.flush();b.focusEntry(entry("page-one.bin"));b=h.render();
  b.beginMutation("rename",b.activeEntry);b=h.render();assert.ok(b.mutationDialog);
  await b.nextPage();b=h.render();
  assert.equal(b.activeEntry,null,"paging retained invisible mutation target");
  assert.equal(b.mutationDialog,null,"paging retained pending rename for hidden row");
  b.focusEntry(entry("page-two.bin"));b=h.render();await b.previousPage();b=h.render();
  assert.equal(b.activeEntry,null,"previous-page navigation retained hidden target");
  b.focusEntry(entry("page-one.bin"));b=h.render();b.beginMutation("rename",b.activeEntry);b=h.render();
  replacement=true;await b.refresh();b=h.render();
  assert.equal(b.activeEntry,null,"refresh retained removed mutation target");
  assert.equal(b.mutationDialog,null,"refresh retained removed pending rename");
});

test("initial directory expansions respect total tree cap without load-more",async()=>{
  let branchRequests=0;
  const h=host(async(command,args)=>{
    assert.equal(command,"workspace_browser_list");
    if(!args.path)return listing(args,Array.from({length:30},(_,i)=>entry(`folder-${i}`,"directory")));
    branchRequests++;
    return listing(args,Array.from({length:100},(_,i)=>entry(`${args.path}/file-${i}.bin`)));
  });
  let b=await h.flush();
  for(let i=0;i<21;i++){await b.toggleDirectory(entry(`folder-${i}`,"directory"));b=h.render();}
  assert.equal(branchRequests,20,"initial expansions bypassed the 2000-entry cap");
  assert.equal(Object.values(b.branches).reduce((sum,branch)=>sum+(branch.listing?.entries.length??0),0),2000);
  assert.match(b.error,/上限|2000/);
});

test("concurrent initial responses cannot publish beyond the tree entry cap",async()=>{
  const pending=[];
  const h=host(async(command,args)=>{
    assert.equal(command,"workspace_browser_list");
    if(!args.path)return listing(args,[]);
    const reply=listing(args,Array.from({length:100},(_,i)=>entry(`${args.path}/file-${i}.bin`)));
    if(["folder-19","folder-20"].includes(args.path))return new Promise(resolve=>pending.push(()=>resolve(reply)));
    return reply;
  });
  let b=await h.flush();
  for(let i=0;i<19;i++){await b.toggleDirectory(entry(`folder-${i}`,"directory"));b=h.render();}
  const first=b.toggleDirectory(entry("folder-19","directory"));
  const second=b.toggleDirectory(entry("folder-20","directory"));
  assert.equal(pending.length,2);
  pending[0]();await first;pending[1]();await second;b=h.render();
  assert.ok(Object.values(b.branches).reduce((sum,branch)=>sum+(branch.listing?.entries.length??0),0)<=2000,"concurrent responses exceeded 2000 entries");
  assert.match(b.error,/上限|2000/);
});
