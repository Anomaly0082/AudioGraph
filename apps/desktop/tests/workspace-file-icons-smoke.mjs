// Independent offline explorer visual review. Actual page, hook, and app shell.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import {createRequire} from "node:module";
import {fileURLToPath} from "node:url";
import {build} from "esbuild";
const require=createRequire(import.meta.url),app=fileURLToPath(new URL("../",import.meta.url));
if(!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE||!process.env.AUDIOGRAPH_CHROMIUM)throw Error("Supply existing headless runtime paths");
const {chromium}=require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const output=path.resolve(app,"../../build/workspace-file-icons-review");
async function styles(file){const text=await fs.readFile(file,"utf8");return(await Promise.all([...text.matchAll(/^@import "([^"]+)";$/gm)].map(m=>styles(path.resolve(path.dirname(file),m[1]))))).join("\n")+text.replace(/^@import "[^"]+";$/gm,"");}
const mock=`
export const desktopAvailable=()=>true;export const listenWorkspaceFileDrop=async()=>()=>{};
const entry=(path,kind='file',bytes=321456)=>({name:path.split('/').pop(),path,kind,bytes:kind==='directory'?null:bytes,modified_at_ms:null});
const files=[entry('音频素材','directory'),entry('音频素材/原始录音','directory'),entry('音频素材/原始录音/采访片段.wav'),entry('音频素材/background.mp3'),entry('音频素材/环境音.flac'),entry('配置文件','directory'),entry('配置文件/graph.json'),entry('配置文件/workflow.yaml'),entry('README.md','file',1800),entry('效果图.png','file',13500),entry('采样参数.txt','file',357),entry('这是一个很长的中文音频文件名称用于检查大小列是否挤压名称显示空间_混音最终版本.wav'),entry('archive.zip')];
export async function invokeDesktop(command,args){if(command==='workspace_browser_list'){const entries=files.filter(e=>e.path.split('/').slice(0,-1).join('/')===args.path);return{space:args.space,path:args.path,entries,offset:0,next_offset:null,total:entries.length,partial:false,warnings:[]};}if(command==='workspace_browser_text')return{space:args.space,path:args.path,text:'示例配置文件',bytes:24,truncated:false};throw Error(command);}`;
const fixture=`import React from'react';import{createRoot}from'react-dom/client';import AppShell from'./src/components/AppShell';import WorkspaceBrowserPage from'./src/pages/WorkspaceBrowserPage';import{useWorkspaceBrowser}from'./src/hooks/useWorkspaceBrowser';const connection={sessionId:'review',workspace:'C:/音频项目'};function Fixture(){const browser=useWorkspaceBrowser(connection,true,{enabled:true});window.browserState=browser;return <AppShell page='files' onNavigate={()=>{}} workspace={connection.workspace} onWorkspace={()=>{}}><WorkspaceBrowserPage browser={browser} connected onOpenGraph={()=>{}} onOpenWorkflow={()=>{}}/></AppShell>}createRoot(document.getElementById('root')).render(<Fixture/>);`;
const js=(await build({stdin:{contents:fixture,resolveDir:app,loader:"tsx"},bundle:true,write:false,format:"iife",platform:"browser",define:{"process.env.NODE_ENV":'"production"'},plugins:[{name:"native-mock",setup(b){b.onResolve({filter:/api\/desktop$/},()=>({path:"native",namespace:"mock"}));b.onLoad({filter:/.*/,namespace:"mock"},()=>({contents:mock,loader:"js"}));}}]})).outputFiles[0].text;
const browser=await chromium.launch({headless:true,executablePath:process.env.AUDIOGRAPH_CHROMIUM});
try{
 await fs.mkdir(output,{recursive:true});const page=await browser.newPage({viewport:{width:1200,height:800}});await page.route('**/*',r=>r.abort());const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.setContent(`<!doctype html><html lang='zh-CN'><head><style>${await styles(path.join(app,'src/styles.css'))}</style></head><body><div id='root'></div></body></html>`);await page.addScriptTag({content:js});
 const row=name=>page.getByRole('treeitem',{name,exact:true});await row('音频素材').waitFor();await row('音频素材').click();await row('原始录音').click();await row('配置文件').click();await row('采访片段.wav').waitFor();
 for(const width of [1200,800]){
  await page.setViewportSize({width,height:800});await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  const result=await page.locator('.workspace-tree-row').evaluateAll(rows=>rows.map(row=>{const name=row.querySelector('.workspace-tree-name'),rect=name.getBoundingClientRect();return{name:name.textContent,svg:row.querySelectorAll('svg').length,nameWidth:rect.width,rowWidth:row.getBoundingClientRect().width,text:row.textContent};}));
  assert.ok(result.every(row=>row.svg>=1),'all entries should use actual SVG icons');assert.ok(result.every(row=>row.nameWidth>=100),`filename width too small at ${width}: ${JSON.stringify(result)}`);assert.ok(result.every(row=>!/[⌄›·]/.test(row.text)),'placeholder glyph remains');
  const aligned=await page.locator('.workspace-tree-row').evaluateAll(rows=>rows.map(row=>{const name=row.querySelector('.workspace-tree-name'),icon=row.querySelector('.workspace-entry-icon'),n=name.getBoundingClientRect(),i=icon.getBoundingClientRect();return{depth:Number(row.getAttribute('aria-level')),nameX:n.x,iconX:i.x,offset:Math.abs((n.y+n.height/2)-(i.y+i.height/2)),title:name.title};}));
  for(const item of aligned){assert.ok(item.offset<2,'icon and text vertical centers diverge');assert.ok(item.title,'ellipsized names need full-name tooltip');const peer=aligned.find(other=>other.depth===item.depth);assert.equal(item.nameX,peer.nameX,'same-level filenames misaligned');assert.equal(item.iconX,peer.iconX,'same-level folder and file icons misaligned');}
  assert.equal(await page.getByRole('button',{name:'上一页',exact:true}).count(),0);assert.equal(await page.getByRole('button',{name:'下一页',exact:true}).count(),0);
  await page.screenshot({path:path.join(output,`tree-${width}.png`)});
  await row('README.md').click({button:'right'});await page.getByRole('menu').waitFor();await page.screenshot({path:path.join(output,`menu-${width}.png`)});await page.keyboard.press('Escape');
  await row('README.md').press('F2');await page.getByRole('textbox',{name:'重命名名称'}).waitFor();await page.getByRole('textbox',{name:'重命名名称'}).press('Escape');
 }
 assert.deepEqual(errors,[]);console.log(JSON.stringify({passed:true,screenshots:output}));
}finally{await browser.close();}
