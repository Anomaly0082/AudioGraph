// Real React/editor pointer and keyboard regression in an isolated headless page.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
const require=createRequire(import.meta.url);
const app=fileURLToPath(new URL("../",import.meta.url));
if(!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE || !process.env.AUDIOGRAPH_CHROMIUM) throw Error("Supply existing headless Playwright and Chromium paths");
const {chromium}=require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const fixture=`
import React,{useEffect,useState} from 'react';import{createRoot}from'react-dom/client';
import EditorPage from './src/pages/EditorPage';import{useGraphDraft}from'./src/hooks/useGraphDraft';
const graph={schema_version:1,nodes:[{id:'a',type:'text_input',parameters:{text:'hello'}},{id:'b',type:'text_output'}],connections:[{from:{node:'a',port:'text'},to:{node:'b',port:'text'}}]};
const nodes=[{typeId:'text_input',displayName:'Text Input',execution_domain:'synchronous',inputs:[],outputs:[{id:'text',type:'Text'}],parameters:[{id:'text',type:'text'}]},{typeId:'text_output',displayName:'Text Output',execution_domain:'synchronous',inputs:[{id:'text',type:'Text',required:true}],outputs:[],parameters:[]}];
function Fixture(){const draft=useGraphDraft('fixture','C:/fixture');const[selection,setSelection]=useState({search:''});
useEffect(()=>{draft.replaceGraph(JSON.stringify(graph));},[]);
window.snapshot=()=>({graph:JSON.parse(draft.graphText),view:draft.canvasView,canUndo:draft.canUndo,canRedo:draft.canRedo});
const noop=()=>{};return <EditorPage draft={draft} session={{desktop:false,connection:{sessionId:'fixture',workspace:'C:/fixture',capabilities:{nodes}},devices:{inputs:[],outputs:[]},busy:null}} selection={selection} onSelection={setSelection} submittingLocked={false} onValidate={noop} onRun={noop} onLoadTemplate={noop} onLoad={noop} onSave={noop} onSaveAs={noop} onDevices={noop} onTasks={noop}/>}
createRoot(document.getElementById('root')).render(<Fixture/>);`;
async function styles(file){const text=await fs.readFile(file,'utf8');return(await Promise.all([...text.matchAll(/^@import "([^"]+)";$/gm)].map(m=>styles(path.resolve(path.dirname(file),m[1]))))).join('\n')+text.replace(/^@import "[^"]+";$/gm,'');}
const js=(await build({stdin:{contents:fixture,loader:'tsx',resolveDir:app},bundle:true,write:false,format:'iife',platform:'browser',define:{'process.env.NODE_ENV':'"production"'}})).outputFiles[0].text;
const browser=await chromium.launch({headless:true,executablePath:process.env.AUDIOGRAPH_CHROMIUM});
try {
 const page=await browser.newPage({viewport:{width:1400,height:950}});const errors=[];
 page.on('pageerror',e=>errors.push(e.message)); await page.route('**/*',r=>r.abort());
 await page.setContent(`<style>${await styles(path.join(app,'src/styles.css'))}</style><div id="root"></div>`);await page.addScriptTag({content:js});
 const node=id=>page.getByRole('button',{name:'选择节点 '+id,exact:true});
 await node('a').waitFor();await node('a').click();await node('b').click({modifiers:['Shift']});
 assert.equal(await page.locator('.canvas-node.selected').count(),2);
 await page.keyboard.press('Control+c');await page.keyboard.press('Control+v');
 await node('a_copy').waitFor();let state=await page.evaluate(()=>window.snapshot());
 assert.equal(state.graph.nodes.length,4);assert.equal(state.graph.connections.length,2);
 assert.equal(state.graph.connections[1].from.node,'a_copy');assert.equal(state.graph.connections[1].to.node,'b_copy');
 await page.keyboard.press('Control+z');await page.waitForFunction(()=>window.snapshot().graph.nodes.length===2);
 await page.keyboard.press('Control+Shift+z');await node('a_copy').waitFor();
 await page.getByRole('button',{name:'撤销',exact:true}).click();await page.waitForFunction(()=>window.snapshot().graph.nodes.length===2);
 // Multiple pointer moves must form one undoable drag.
 const header=node('a');await header.scrollIntoViewIfNeeded();let box=await header.boundingBox();
 await page.mouse.move(box.x+70,box.y+25);await page.mouse.down();
 await page.mouse.move(box.x+160,box.y+80,{steps:8});await page.mouse.up();
 state=await page.evaluate(()=>window.snapshot());assert.ok(state.view.positions['["a","text_input"]']);
 await page.keyboard.press('Control+z');await page.waitForFunction(()=>Object.keys(window.snapshot().view.positions).length===0);
 assert.equal((await page.evaluate(()=>window.snapshot())).canRedo,true);
 // Native text shortcuts must not paste nodes or trigger graph undo.
 const search=page.getByRole('textbox',{name:'搜索节点'});await search.fill('Text');
 await search.press('Control+a');await search.press('Control+c');await search.press('Control+v');
 assert.equal((await page.evaluate(()=>window.snapshot())).graph.nodes.length,2);
 await search.fill('');await page.getByRole('button',{name:'添加节点 Text Input',exact:true}).click();
 await page.waitForFunction(()=>window.snapshot().graph.nodes.length===3);
 await page.getByRole('button',{name:'撤销',exact:true}).click();
 await page.waitForFunction(()=>window.snapshot().graph.nodes.length===2);
 assert.equal((await page.evaluate(()=>window.snapshot())).canRedo,true);
 assert.deepEqual(errors,[]);console.log('Graph editor headless: multi-copy, links, undo/redo, drag grouping, native text shortcuts and atomic add passed');
} finally {await browser.close();}
