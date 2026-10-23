// Standalone offline geometry regression: actual Shell + Workbench + messages.
// Reuses existing headless Chromium; never opens a native app or accesses a workspace.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import {createRequire} from "node:module";
import {fileURLToPath} from "node:url";
import {build} from "esbuild";
const require=createRequire(import.meta.url);
const app=fileURLToPath(new URL("../",import.meta.url));
const playwright=process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE;
const executable=process.env.AUDIOGRAPH_CHROMIUM;
if(!playwright||!executable)throw Error("Supply existing AUDIOGRAPH_PLAYWRIGHT_MODULE and AUDIOGRAPH_CHROMIUM paths");
const {chromium}=require(playwright);
async function styles(file){const text=await fs.readFile(file,"utf8");const imports=[...text.matchAll(/^@import "([^"]+)";$/gm)];return(await Promise.all(imports.map(m=>styles(path.resolve(path.dirname(file),m[1]))))).join("\n")+text.replace(/^@import "[^"]+";$/gm,"");}
const fixture=`
import React,{useState} from 'react';import{createRoot}from'react-dom/client';
import AppShell from './src/components/AppShell';import WorkbenchPage from './src/pages/WorkbenchPage';import WorkspacePanel from './src/components/WorkspacePanel';
function Fixture(){const[s,set]=useState({connected:true,warning:false,activity:false,page:'workbench'});window.update=s=>set(old=>({...old,...s}));
const agent={mode:'graph',busy:false,loading:false,prompt:'测试发送内容',attachGraph:false,conversationId:'one',conversations:[{id:'one',title:'长对话测试',mode:'graph'}],spaces:s.connected?{user_root:'C:/中文工作区/'+ 'long/'.repeat(40),ai_root:'C:/app/files',tools:Array.from({length:40},(_,i)=>'tool_'+i)}:null,warnings:s.warning?Array.from({length:8},(_,i)=>'警告'+i+'：'+ '详细消息。'.repeat(10)):[],turns:Array.from({length:25},(_,i)=>({id:'turn'+i,prompt:'用户请求'+i+'。'.repeat(50),reply:{state:'completed',text:('回复内容与说明。'.repeat(30)+'\\n\\n').repeat(5),events:[],model_calls:1,tool_calls:0}})),setPrompt:()=>{},setAttachGraph:()=>{},setMode:()=>{},selectConversation:()=>{},newConversation:()=>{}};
agent.error=s.warning?'模型调用失败：'+ '错误详情。'.repeat(20):'';
return <AppShell page={s.page} onNavigate={page=>window.update({page})} workspace={s.connected?'C:/中文工作区':null} onWorkspace={()=>{}} status={<span className='topbar-status'>连接成功：工作区已就绪</span>} notices={<>{s.panelOpen&&<WorkspacePanel session={{desktop:true,workspace:'C:/测试',connection:null,busy:false,setWorkspace:()=>{},recentWorkspaces:{paths:Array.from({length:25},(_,i)=>'C:/最近目录/'+i),busy:false,clear:()=>{},remove:()=>{}}}} aiBusy={false} onClose={()=>window.update({panelOpen:false})} onChoose={()=>window.chosen=true} onConnect={()=>{}} onDisconnect={()=>{}} onSettings={()=>{}}/>}{s.warning&&<p className='banner error'>全局警告：{'请检查运行状态。'.repeat(20)}</p>}</>} activity={s.activity?<div className='global-activity'><span className='activity-pulse'/><span>{'正在运行任务；'.repeat(12)}</span><button>停止</button></div>:null}>{s.page==='workbench'?<WorkbenchPage connected={s.connected} onOpenSettings={()=>{}} agent={agent} agentBlocked={false} onAgentSend={()=>{}} onAgentApply={()=>{}}/>:<section data-testid='other' style={{height:1400}}>其他页面<div style={{marginTop:1300}} data-testid='other-bottom'>底部内容</div></section>}</AppShell>;
}createRoot(document.getElementById('root')).render(<Fixture/>);`;
const js=(await build({stdin:{contents:fixture.replace('<AppShell page=', '<AppShell noticesExpanded={!!s.panelOpen} page='),loader:"tsx",resolveDir:app},bundle:true,write:false,format:"iife",platform:"browser",define:{"process.env.NODE_ENV":'"production"'}})).outputFiles[0].text;
const browser=await chromium.launch({headless:true,executablePath:executable});
const errors=[];let checks=0;
try{
 const page=await browser.newPage();page.on("pageerror",e=>errors.push(e.message));await page.route("**/*",route=>route.abort());
 await page.setContent(`<!doctype html><html lang='zh-CN'><head><style>${await styles(path.join(app,"src/styles.css"))}</style></head><body><div id='root'></div></body></html>`);await page.addScriptTag({content:js});await page.locator('#agent-prompt').waitFor();
 async function verify(label){
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  const geometry=await page.evaluate(()=>{const box=selector=>{const r=document.querySelector(selector).getBoundingClientRect();return{x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height};};const c=document.querySelector('.agent-conversation');return{height:innerHeight,width:innerWidth,input:box('#agent-prompt'),composer:box('.agent-composer'),send:box('.agent-composer .primary'),conversation:box('.agent-conversation'),scrollHeight:c.scrollHeight,clientHeight:c.clientHeight,body:document.documentElement.scrollHeight};});
  for(const name of ['input','send','composer']){const r=geometry[name];assert.ok(r.y>=0&&r.bottom<=geometry.height+1,`${label}: ${name} outside viewport ${JSON.stringify(geometry)}`);assert.ok(r.x>=0&&r.right<=geometry.width+1,`${label}: ${name} clipped horizontally`);assert.ok(r.height>0,`${label}: ${name} collapsed`);}
  const panel=await page.locator('.agent-tools-panel').boundingBox();
  assert.ok(geometry.composer.bottom<=panel.y+panel.height+1,`${label}: composer clipped by panel bottom ${panel.y+panel.height}, composer bottom ${geometry.composer.bottom}`);
  if(geometry.clientHeight<=20)console.log(await page.evaluate(()=>Object.fromEntries(['.app-sidebar','.topbar','.app-shell-notices','#page-content','.workbench-page','.agent-tools-panel','.agent-toolbar','.agent-spaces','.agent-alerts','.global-activity'].map(selector=>{const e=document.querySelector(selector),r=e?.getBoundingClientRect();return[selector,r?{y:r.y,height:r.height,bottom:r.bottom}:null];}))));
  assert.ok(geometry.clientHeight>20,`${label}: conversation unusably collapsed ${JSON.stringify(geometry)}`);
  assert.ok(geometry.scrollHeight>geometry.clientHeight,`${label}: long conversation does not own scroll`);
  assert.ok(geometry.body<=geometry.height+1,`${label}: workbench document overflows`);
  const before=geometry.composer.y;await page.locator('.agent-conversation').evaluate(e=>e.scrollTop=e.scrollHeight);const after=await page.locator('.agent-composer').boundingBox();assert.ok(Math.abs(before-after.y)<1,`${label}: scrolling moved composer`);checks++;
 }
 for(const [width,height]of [[1050,820],[800,650],[1280,720]]){
  await page.setViewportSize({width,height});await page.evaluate(()=>window.update({connected:true,warning:false,activity:false,page:'workbench'}));await verify(`${width}x${height} base`);
  await page.getByRole('button',{name:'工作区与可用工具'}).click();await verify(`${width}x${height} expanded tools`);
  await page.evaluate(()=>window.update({warning:true,activity:true}));await verify(`${width}x${height} warnings and activity`);
  await page.evaluate(()=>window.update({connected:false}));await verify(`${width}x${height} disconnected`);
  await page.getByRole('button',{name:'工作区与可用工具'}).click();
 }
 await page.setViewportSize({width:800,height:650});await page.evaluate(()=>window.update({panelOpen:true,warning:false,activity:false}));await page.locator('.workspace-panel').waitFor();
 const overlay=await page.locator('.app-shell-notices').boundingBox();assert.ok(overlay.height>200&&overlay.y>=0&&overlay.y+overlay.height<=651,'workspace picker overlay clipped');
 await page.getByRole('button',{name:'选择目录',exact:true}).click();assert.equal(await page.evaluate(()=>window.chosen),true);
 await page.getByRole('button',{name:'查看设备权限设置',exact:true}).scrollIntoViewIfNeeded();await page.getByRole('button',{name:'收起工作区设置',exact:true}).click();await page.locator('.workspace-panel').waitFor({state:'detached'});await verify('800x650 workspace picker closed');checks++;
 for(const other of ['settings','tasks','files','editor']){await page.evaluate(page=>window.update({page,warning:false,activity:false}),other);await page.locator('[data-testid=other]').waitFor();await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));const measured=await page.evaluate(()=>({height:document.documentElement.scrollHeight,viewport:innerHeight,overflow:getComputedStyle(document.querySelector('.app-layout')).overflow}));assert.ok(measured.height>measured.viewport,`${other}: tall content clipped`);assert.notEqual(measured.overflow,'hidden',`${other}: workbench height leaked`);await page.locator('[data-testid=other-bottom]').scrollIntoViewIfNeeded();checks++;}
 assert.deepEqual(errors,[]);console.log(JSON.stringify({passed:true,checks,viewports:['1050x820','800x650','1280x720']}));
}finally{await browser.close();}
