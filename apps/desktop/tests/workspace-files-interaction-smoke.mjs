// Optional headless integration test. Uses the real hook/page and a mocked native boundary.
// Reuses existing Playwright/Chromium; never installs browsers or opens native apps.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const app = fileURLToPath(new URL("../", import.meta.url));
const output = path.resolve(app, "../../build/workspace-files-interaction-preview");
if (!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE || !process.env.AUDIOGRAPH_CHROMIUM)
  throw new Error("Supply existing AUDIOGRAPH_PLAYWRIGHT_MODULE and AUDIOGRAPH_CHROMIUM.");
const { chromium } = require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
async function styles(file) {
  const text = await fs.readFile(file, "utf8");
  const imports = [...text.matchAll(/^@import "([^"]+)";$/gm)];
  return (await Promise.all(imports.map(match => styles(path.resolve(path.dirname(file), match[1]))))).join("\n") + text.replace(/^@import "[^"]+";$/gm, "");
}
const mock = `
export const desktopAvailable = () => true;
export const listenWorkspaceFileDrop = async callback => { window.nativeDrop = callback; return () => { window.nativeDrop = null; }; };
export async function invokeDesktop(command, args) {
  window.calls.push({command, args});
  if(command === 'workspace_browser_list') {
    const entries = window.entries.filter(e => e.path.split('/').slice(0,-1).join('/') === args.path);
    return {space:args.space,path:args.path,entries,offset:0,next_offset:null,total:entries.length,partial:false,warnings:[]};
  }
  if(command === 'workspace_browser_text') return {space:args.space,path:args.path,text:'preview',bytes:7,truncated:false};
  if(command === 'workspace_files_mutate') {
    if(window.delayNext) { window.delayNext=false; await new Promise(resolve => window.resolveMutation=resolve); }
    if(window.failNext) { window.failNext=false; throw Error('模拟目标写入失败'); }
    const source=window.entries.find(e=>e.path===args.path);
    let target=args.target ?? args.path;
    if(args.operation==='copy' && target===args.path) target=target.replace(/(\\.[^/.]+)?$/, ' 副本$1');
    if(args.operation==='mkdir') window.entries.push({name:args.path.split('/').pop(),path:args.path,kind:'directory',bytes:null,modified_at_ms:null});
    if(['copy','move','rename'].includes(args.operation)) {
      const group=window.entries.filter(e=>e.path===args.path || e.path.startsWith(args.path+'/'));
      if(args.operation!=='copy') window.entries=window.entries.filter(e=>!group.includes(e));
      window.entries.push(...group.map(e=>({...e,path:target+e.path.slice(args.path.length),name:e.path===args.path?target.split('/').pop():e.name})));
    }
    if(args.operation==='trash') window.entries=window.entries.filter(e=>e.path!==args.path && !e.path.startsWith(args.path+'/'));
    return {operation:args.operation,path:target,paths:[target],message:'操作已完成'};
  }
  throw Error('Unexpected command: '+command);
}`;
const fixture = `
import React, {useState} from 'react';
import {createRoot} from 'react-dom/client';
import {useWorkspaceBrowser} from './src/hooks/useWorkspaceBrowser';
import WorkspaceBrowserPage from './src/pages/WorkspaceBrowserPage';
window.calls=[];
window.entries=[{name:'folder',path:'folder',kind:'directory',bytes:null,modified_at_ms:null}, {name:'config.json',path:'folder/config.json',kind:'file',bytes:7,modified_at_ms:null},{name:'clip.wav',path:'clip.wav',kind:'file',bytes:7,modified_at_ms:null}, ...Array.from({length:20},(_,i)=>({name:'file'+i+'.txt',path:'file'+i+'.txt',kind:'file',bytes:7,modified_at_ms:null}))];
window.protected=[];
function Fixture(){
 const [connection,setConnection]=useState({sessionId:'a',workspace:'C:/a'});
 const [visible,setVisible]=useState(true);
 window.setBrowserVisible=setVisible;
 const browser=useWorkspaceBrowser(connection,visible,{enabled:true,protectedPaths:()=>window.protected,blockedReason:()=>window.blocked ?? null});
 window.browserState=browser;
 window.changeWorkspace=()=>setConnection({sessionId:'b',workspace:'C:/b'});
 return visible ? <WorkspaceBrowserPage browser={browser} connected onOpenGraph={()=>{}} onOpenWorkflow={()=>{}}/> : <div>其他页面</div>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
`;
const js = (await build({stdin:{contents:fixture,loader:"tsx",resolveDir:app},bundle:true,write:false,format:"iife",platform:"browser",define:{"process.env.NODE_ENV":'"production"'},plugins:[{name:"mock-desktop",setup(builder){builder.onResolve({filter:/api\/desktop$/},()=>({path:"desktop",namespace:"mock"}));builder.onLoad({filter:/.*/,namespace:"mock"},()=>({contents:mock,loader:"js"}));}}]})).outputFiles[0].text;
const browser = await chromium.launch({headless:true,executablePath:process.env.AUDIOGRAPH_CHROMIUM});
const errors=[];
try {
  await fs.mkdir(output,{recursive:true});
  const page=await browser.newPage({viewport:{width:1200,height:800}});
  page.setDefaultTimeout(7000);
  page.on("pageerror",error=>errors.push(error.message));
  await page.route("**/*",route=>route.abort());
  await page.setContent(`<!doctype html><html lang="zh-CN"><head><style>${await styles(path.join(app,"src/styles.css"))}</style></head><body><div id="root"></div></body></html>`);
  await page.addScriptTag({content:js});
  const tree=page.getByRole('tree',{name:'工作区文件列表'});
  const row=name=>page.getByRole('treeitem',{name,exact:true});
  await row('folder').waitFor();
  assert.equal(await page.getByRole('navigation',{name:'文件路径'}).count(),0);
  assert.equal(await page.getByText('根目录',{exact:true}).count(),0);
  assert.equal(await page.locator('.workspace-files-destination').count(),0);
  assert.equal(await page.getByText(/^目标目录/).count(),0);
  const mutations=()=>page.evaluate(()=>window.calls.filter(c=>c.command==='workspace_files_mutate'));
  const count=async()=> (await mutations()).length;
  const waitDone=()=>page.waitForFunction(()=>!window.browserState.readMutating());
  const context=async(name,label)=>{
    if(name) await row(name).click({button:'right'}); else await tree.click({position:{x:5,y:230},button:'right'});
    await page.getByRole('menu').getByRole('menuitem',{name:new RegExp('^'+label)}).click();
  };
  const cancel=()=>page.getByRole('dialog').getByRole('button',{name:'取消',exact:true}).click();
  const confirm=async label=>{await page.getByRole('dialog').getByRole('button',{name:'确认'+label,exact:true}).click();await page.getByRole('dialog').waitFor({state:'detached'});await waitDone();};
  const fileCount=await page.getByRole('treeitem').count();
  await row('folder').click();
  await row('config.json').waitFor();
  assert.equal(await page.getByRole('treeitem').count(),fileCount+1,'folder expands in place');
  assert.equal(await row('file0.txt').count(),1,'root remains visible');
  // Names only, nested mkdir, composition, duplicate Enter and retained context.
  await page.getByRole('button',{name:'新建文件夹',exact:true}).click();
  let input=page.getByRole('textbox',{name:'新文件夹名称'});
  await input.fill('nested');
  let before=await count();
  await input.dispatchEvent('compositionstart');
  await input.press('Enter');
  assert.equal(await count(),before,'IME Enter does not submit');
  await input.dispatchEvent('compositionend');
  await page.evaluate(()=>window.delayNext=true);
  await input.press('Enter');
  await input.press('Enter').catch(()=>{});
  assert.equal(await count(),before+1,'duplicate Enter invokes once');
  await page.evaluate(()=>window.resolveMutation());
  await row('nested').waitFor();
  await waitDone();
  assert.equal(await page.locator('.banner:not(.error)').count(),0,'successful operations stay quiet');
  assert.equal(await page.getByText('操作已完成',{exact:true}).count(),0);
  assert.equal((await mutations()).at(-1).args.path,'folder/nested');
  assert.equal(await page.evaluate(()=>window.browserState.destination),'folder/nested');
  assert.equal(await row('folder').getAttribute('aria-expanded'),'true');
  // Blur and Escape cancel inline edits.
  await row('file0.txt').click();
  await row('file0.txt').press('F2');
  input=page.getByRole('textbox',{name:'重命名名称'});
  await input.fill('cancelled.txt');
  await page.getByRole('button',{name:'刷新',exact:true}).focus();
  await input.waitFor({state:'detached'});
  assert.equal(await count(),before+1);
  await row('file0.txt').focus();
  await row('file0.txt').press('F2');
  await page.getByRole('textbox',{name:'重命名名称'}).press('Escape');
  assert.equal(await count(),before+1);
  // F2 name-only rename, input-local Ctrl+X must not populate file clipboard.
  await row('file0.txt').press('F2');
  input=page.getByRole('textbox',{name:'重命名名称'});
  await input.press('Control+x');
  assert.equal(await page.evaluate(()=>window.browserState.clipboard),null);
  await input.fill('renamed.txt');
  await input.press('Enter');
  await row('renamed.txt').waitFor();await waitDone();
  assert.equal(await row('file0.txt').count(),0);
  assert.equal((await mutations()).at(-1).args.target,'renamed.txt');
  // Same-directory copy uses backend-assigned actual path.
  await row('renamed.txt').press('Control+c');
  await row('renamed.txt').press('Control+v');
  await row('renamed 副本.txt').waitFor();await waitDone();
  assert.equal(await page.evaluate(()=>window.browserState.activeEntry.path),'renamed 副本.txt');
  // Navigate to a folder, then paste copy.
  await row('renamed.txt').click();await row('renamed.txt').press('Control+c');
  await row('folder').click();
  await row('folder').press('Control+v');
  await waitDone();
  await page.waitForFunction(()=>window.entries.some(e=>e.path==='folder/renamed.txt'));
  assert.equal((await mutations()).at(-1).args.target,'folder/renamed.txt');
  // Cut retains original and clipboard on failed paste; successful retry consumes both.
  await row('file1.txt').click();await row('file1.txt').press('Control+x');
  await row('folder').click();
  await page.evaluate(()=>window.failNext=true);
  await row('folder').press('Control+v');await waitDone();
  await page.getByRole('alert').waitFor();
  assert.equal(await page.evaluate(()=>window.entries.some(e=>e.path==='file1.txt')),true);
  assert.equal(await page.evaluate(()=>window.browserState.clipboard.entry.path),'file1.txt');
  assert.equal(await page.evaluate(()=>window.browserState.destination),'folder');
  await tree.focus();await tree.press('Control+v');await waitDone();
  await page.waitForFunction(()=>!window.entries.some(e=>e.path==='file1.txt'));
  assert.equal(await page.evaluate(()=>window.browserState.clipboard),null);
  await row('file1.txt').waitFor();
  // Context action recycles immediately once and retains the expanded parent.
  before=await count();await context('file1.txt','移到回收站');await waitDone();
  assert.equal(await page.getByRole('dialog').count(),0);
  assert.equal(await count(),before+1);
  assert.equal((await mutations()).at(-1).args.operation,'trash');
  assert.equal((await mutations()).at(-1).args.path,'folder/file1.txt');
  assert.equal(await page.evaluate(()=>window.entries.some(e=>e.path==='folder/file1.txt')),false);
  assert.equal(await row('folder').getAttribute('aria-expanded'),'true');
  // Repeated Delete while pending invokes once; failure preserves source and selection.
  await row('file3.txt').click();
  before=await count();
  await page.evaluate(()=>{window.delayNext=true;window.failNext=true;});
  await row('file3.txt').press('Delete');
  await tree.dispatchEvent('keydown',{key:'Delete',bubbles:true});
  await tree.dispatchEvent('keydown',{key:'Delete',bubbles:true});
  assert.equal(await count(),before+1);
  assert.equal(await page.getByRole('dialog').count(),0);
  await page.evaluate(()=>window.resolveMutation());await waitDone();
  assert.equal(await page.evaluate(()=>window.entries.some(e=>e.path==='file3.txt')),true);
  assert.equal(await page.evaluate(()=>window.browserState.activeEntry.path),'file3.txt');
  assert.match(await page.getByRole('alert').innerText(),/模拟目标写入失败/);
  await row('file3.txt').press('Delete');await waitDone();
  assert.equal(await count(),before+2);
  assert.equal(await page.evaluate(()=>window.entries.some(e=>e.path==='file3.txt')),false);
  assert.equal(await page.getByRole('dialog').count(),0);
  assert.equal(await page.locator('.banner:not(.error)').count(),0);
  await page.screenshot({path:path.join(output,'trash-complete.png')});
  // Dirty ancestor and global task guards reject mutations.
  await page.evaluate(()=>window.protected=[{space:'user',path:'folder/config.json'}]);
  before=await count();await context('folder','重命名');
  assert.equal(await page.getByRole('textbox',{name:'重命名名称'}).count(),0);
  assert.match(await page.getByRole('alert').innerText(),/未保存编辑/);
  await row('folder').focus();await row('folder').press('Delete');
  assert.equal(await count(),before);
  assert.equal(await page.evaluate(()=>window.entries.some(e=>e.path==='folder')),true);
  await context('folder','复制');
  assert.equal(await page.evaluate(()=>window.browserState.clipboard.operation),'copy');
  await page.evaluate(()=>window.blocked='任务正在运行');
  await context('folder','剪切');
  assert.match(await page.getByRole('alert').innerText(),/任务正在运行/);
  await row('file4.txt').click();await row('file4.txt').press('Delete');
  assert.equal(await count(),before);
  await page.evaluate(()=>{window.blocked=null;window.protected=[];});
  // File selection previews text, audio selection never starts playback.
  await row('clip.wav').click();
  assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.command==='workspace_browser_audio').length),0);
  // Native external imports pass an opaque token only.
  await tree.click({position:{x:3,y:230}});
  before=await count();
  await page.evaluate(()=>window.nativeDrop({kind:'drop',sessionId:'a',token:'cancel-token',names:['cancel.wav']}));
  await page.getByRole('dialog').waitFor();
  await cancel();assert.equal(await count(),before,'import cancellation makes no native mutation');
  await page.evaluate(()=>window.nativeDrop({kind:'drop',sessionId:'a',token:'opaque-native-token',names:['external.wav']}));
  await page.getByRole('dialog').waitFor();
  assert.match(await page.getByRole('dialog').innerText(),/原文件保留/);
  assert.equal(await page.getByRole('dialog').getByRole('alert').count(),0,'valid drop clears old errors');
  await page.screenshot({path:path.join(output,'import-confirmation.png')});
  await confirm('导入外部文件');
  const imported=(await mutations()).at(-1).args;
  assert.deepEqual(imported,{sessionId:'a',space:'user',operation:'import',path:'',dropToken:'opaque-native-token'});
  // Space/session invalidation clears clipboard, tree cache and inline form.
  await row('file2.txt').click();await row('file2.txt').press('Control+c');
  await page.getByRole('combobox',{name:'文件工作区'}).selectOption('ai');
  assert.equal(await page.evaluate(()=>window.browserState.clipboard),null);
  assert.deepEqual(await page.evaluate(()=>window.browserState.branches),{});
  await page.getByRole('combobox',{name:'文件工作区'}).selectOption('user');
  await row('file2.txt').click();await row('file2.txt').press('Control+c');
  await row('file2.txt').press('F2');before=await count();
  await page.evaluate(()=>window.changeWorkspace());
  await page.getByRole('textbox',{name:'重命名名称'}).waitFor({state:'detached'});
  assert.equal(await count(),before);
  assert.equal(await page.evaluate(()=>window.browserState.clipboard),null);
  await row('folder').click();await row('config.json').click();
  await page.evaluate(()=>window.setBrowserVisible(false));
  await page.getByText('其他页面').waitFor();
  await page.evaluate(()=>window.setBrowserVisible(true));
  await row('folder').waitFor();
  assert.equal(await page.evaluate(()=>window.browserState.activeEntry),null);
  assert.equal(await page.evaluate(()=>window.browserState.destination),'');
  assert.deepEqual(await page.evaluate(()=>window.browserState.branches),{});
  // Keyboard navigation and last-row fixed context menu never change row layout.
  await row('file18.txt').click();await row('file18.txt').press('ArrowDown');
  assert.equal(await page.evaluate(()=>window.browserState.activeEntry.name),'file19.txt');
  const bounds=await row('file19.txt').boundingBox();
  await row('file19.txt').press('Shift+F10');
  const menu=page.getByRole('menu');
  await menu.waitFor();
  const rect=await menu.boundingBox();
  assert(rect.x>=0 && rect.y>=0 && rect.x+rect.width<=1200 && rect.y+rect.height<=800);
  assert.equal((await row('file19.txt').boundingBox()).height,bounds.height,'menu does not stretch row');
  await page.screenshot({path:path.join(output,'last-row-menu.png')});
  await menu.press('Escape');
  assert.equal(await menu.count(),0);
  assert.deepEqual(errors,[]);
  console.log('Workspace explorer headless interactions passed: quiet root/success UI, direct context/Delete recycle with repeat/failure/retry guards, tree, inline name/IME/blur/double Enter, copy/cut/paste, dirty/task/space/session guards, confirmed token import. Screenshots: '+output);
} finally {await browser.close();}
