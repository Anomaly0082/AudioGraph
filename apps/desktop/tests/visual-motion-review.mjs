// Independent offline review. Reuses the visual fixture but does not modify its runner.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import vm from 'node:vm';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
import {build} from 'esbuild';
const require=createRequire(import.meta.url),app=fileURLToPath(new URL('../',import.meta.url));
if(!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE||!process.env.AUDIOGRAPH_CHROMIUM)throw Error('Supply existing headless runtime paths');
const {chromium}=require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE),output=path.resolve(app,'../../build/visual-motion-review');
async function styles(file){const text=await fs.readFile(file,'utf8');return(await Promise.all([...text.matchAll(/^@import "([^"]+)";$/gm)].map(m=>styles(path.resolve(path.dirname(file),m[1]))))).join('\n')+text.replace(/^@import "[^\"]+";$/gm,'');}
const source=await fs.readFile(path.join(app,'tests/visual-theme-smoke.mjs'),'utf8');
const match=source.match(/const fixture = (`[\s\S]*?`);/);assert.ok(match,'existing visual fixture missing');
const fixture=vm.runInNewContext(match[1])+`
agent.conversations[0].title='现场采访录音清理与最终交付文件核对：这是用于检查顶栏长会话名称截断和层级的完整标题';
plugins.view={directory:'C:/AudioGraph/plugins',restart_required:true,warnings:[],entries:[{folder:'audio-utilities',plugin_id:'audio.utilities',plugin_version:'1.2.0',active:true,enabled:true,pending_uninstall:false,node_types:['audio_meter','normalize'],can_manage:true,revision:'r1'},{folder:'transcript-tools',plugin_id:'transcript.tools',plugin_version:'0.4.2',active:false,enabled:false,pending_uninstall:true,node_types:['transcribe'],can_manage:true,revision:'r2'}]};
window.showWorkflow=()=>root.render(<AppShell page='editor' onNavigate={noop} workspace='C:/demo' onWorkspace={noop}><WorkflowEditorPage connected onOpenRun={noop} editor={{sourceSpace:'user',path:'workflows/clean.workflow.json',fileLabel:'clean.workflow.json',text:JSON.stringify({schema_version:1,inputs:{audio:{type:'FilePath'}},steps:[{id:'process',type:'graph',graph:'graphs/clean.json'}],outputs:{result:'process.output'}},null,2),busy:false,validationCurrent:true,modified:false,setSourceSpace:noop,setPath:noop,load:noop,save:noop,saveAs:noop,resetBlank:noop,editText:noop,validate:noop,run:noop}}/></AppShell>);
function DragFixture(){const[view,setView]=React.useState({positions:{},selectedId:'gain',zoom:1,pan:{x:0,y:0}});window.dragView=view;return <AppShell page='editor' onNavigate={noop} workspace='C:/demo' onWorkspace={noop}><GraphCanvas graph={graph} nodes={nodes} view={view} onView={setView} onConnect={noop} onDisconnect={noop}/></AppShell>};window.showDrag=()=>root.render(<DragFixture/>);`;
const js=(await build({stdin:{contents:"import GraphCanvas from './src/components/GraphCanvas';import WorkflowEditorPage from './src/pages/WorkflowEditorPage';\n"+fixture,resolveDir:app,loader:'tsx'},bundle:true,write:false,format:'iife',platform:'browser',define:{'process.env.NODE_ENV':'"production"'}})).outputFiles[0].text;
const browser=await chromium.launch({headless:true,executablePath:process.env.AUDIOGRAPH_CHROMIUM});
try{
 await fs.mkdir(output,{recursive:true});const page=await browser.newPage({viewport:{width:1200,height:800}});await page.route('**/*',r=>r.abort());const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.setContent(`<!doctype html><html><head><style>${await styles(path.join(app,'src/styles.css'))}</style></head><body><div id='root'></div></body></html>`);await page.addScriptTag({content:js});await page.locator('.agent-composer').waitFor();
 const settle=()=>page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
 for(const width of [1200,800])for(const name of ['workbench','editor','tasks','settings']){await page.setViewportSize({width,height:800});await page.evaluate(name=>window.showCase(name),name);await settle();await page.screenshot({path:path.join(output,`${name}-${width}.png`),fullPage:true,animations:'disabled'});}
 for(const width of [1200,800]){await page.setViewportSize({width,height:800});await page.evaluate(()=>window.showWorkflow());await settle();await page.screenshot({path:path.join(output,`workflow-${width}.png`),fullPage:true,animations:'disabled'});}
 for(const[width,height]of [[800,650],[1050,820],[1600,1000]]){await page.setViewportSize({width,height});await page.evaluate(()=>window.showCase('workbench'));await settle();const composer=await page.locator('.agent-composer').boundingBox();assert.ok(composer.y+composer.height<=height+1,'chat composer outside viewport');await page.screenshot({path:path.join(output,`chat-${width}x${height}.png`),animations:'disabled'});}
 await page.evaluate(()=>window.showCase('workbench'));await settle();
 const button=page.getByRole('button',{name:'查看工作区与可用工具',exact:true});await button.focus();
 const focus=await button.evaluate(e=>({outline:getComputedStyle(e).outlineStyle,shadow:getComputedStyle(e).boxShadow}));assert.ok(focus.outline!=='none'||focus.shadow!=='none','keyboard focus is invisible');await page.screenshot({path:path.join(output,'focus.png')});
 await button.click();assert.equal(await page.getByRole('button',{name:'收起工作区与可用工具',exact:true}).getAttribute('aria-expanded'),'true');await settle();await page.screenshot({path:path.join(output,'disclosure-open.png')});
 const composer=await page.locator('.agent-composer').boundingBox();await page.locator('.agent-conversation').evaluate(e=>e.scrollTop=e.scrollHeight);assert.ok(Math.abs((await page.locator('.agent-composer').boundingBox()).y-composer.y)<1,'message scrolling moved composer');
 await page.evaluate(()=>window.showDrag());await page.locator('.canvas-node-header').first().waitFor();await settle();
 const header=page.getByRole('button',{name:'选择节点 gain',exact:true});await header.hover();
 const motion=await header.evaluate(e=>{const s=getComputedStyle(e),n=getComputedStyle(e.closest('.canvas-node')),surface=getComputedStyle(document.querySelector('.canvas-surface'));return{transform:s.transform,transition:s.transitionProperty,nodeTransition:n.transitionProperty,surfaceTransition:surface.transitionProperty};});
 assert.equal(motion.transform,'none','hover transform changes graph drag origin');assert.ok(!/(^|,\s*)(all|transform)(,|$)/.test(motion.transition),'node header transform transitions affect drag');
 const before=await header.boundingBox();await page.mouse.move(before.x+35,before.y+12);await page.mouse.down();assert.equal(await header.evaluate(e=>getComputedStyle(e).transform),'none','pressed transform affects drag');await page.mouse.move(before.x+95,before.y+52,{steps:5});await page.mouse.up();await settle();const after=await header.boundingBox();assert.ok(Math.abs(after.x-before.x-60)<2&&Math.abs(after.y-before.y-40)<2,`graph drag drift: ${JSON.stringify({before,after,motion})}`);await page.screenshot({path:path.join(output,'graph-after-drag.png')});
 await page.emulateMedia({reducedMotion:'reduce'});await page.evaluate(()=>window.showCase('workbench'));await settle();
 const activeMotion=await page.evaluate(()=>[...document.querySelectorAll('button,.disclosure-content,.agent-turn,.workspace-file-context-menu')].filter(e=>e.getClientRects().length).map(e=>{const s=getComputedStyle(e);return{selector:e.className,transition:s.transitionDuration,animation:s.animationDuration};}).filter(e=>[e.transition,e.animation].some(value=>value.split(',').some(n=>parseFloat(n)>.01))));assert.deepEqual(activeMotion,[],'reduced-motion left visible transitions or animations');
 assert.deepEqual(errors,[]);console.log(JSON.stringify({passed:true,motion,screenshots:output}));
}finally{await browser.close();}
