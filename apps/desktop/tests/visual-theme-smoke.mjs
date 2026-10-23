// Optional, isolated visual check. No native UI, profiles, API keys or audio devices.
// Supply existing Playwright/Chromium paths; this script never installs/downloads them.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const app = fileURLToPath(new URL("../", import.meta.url));
const output = path.resolve(app, "../../build/light-theme-preview");
const playwrightModule = process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE;
const chromiumPath = process.env.AUDIOGRAPH_CHROMIUM;
if (!playwrightModule || !chromiumPath) throw new Error("Supply AUDIOGRAPH_PLAYWRIGHT_MODULE and AUDIOGRAPH_CHROMIUM; use a headless-only browser executable.");
const { chromium } = require(playwrightModule);
await fs.mkdir(output, { recursive: true });
async function styles(file) {
  const text = await fs.readFile(file, "utf8");
  const imports = [...text.matchAll(/^@import "([^"]+)";$/gm)];
  const children = await Promise.all(imports.map(match => styles(path.resolve(path.dirname(file), match[1]))));
  return children.join("\n") + text.replace(/^@import "[^"]+";$/gm, "");
}
const css = await styles(path.join(app, "src/styles.css"));
const fixture = `
import React from 'react';
import { createRoot } from 'react-dom/client';
import AppShell from './src/components/AppShell';
import WorkbenchPage from './src/pages/WorkbenchPage';
import EditorPage from './src/pages/EditorPage';
import SettingsPage from './src/pages/SettingsPage';
import WorkspaceBrowserPage from './src/pages/WorkspaceBrowserPage';
import RunRecordsPage from './src/pages/RunRecordsPage';
const noop = () => {};
window.__calls = [];
const nodes = [
 {typeId:'wav_input',displayName:'WAV Input',execution_domain:'synchronous',inputs:[],outputs:[{id:'audio',type:'Audio',required:true}],parameters:[{id:'path',type:'file_path',required:true}]},
 {typeId:'gain',displayName:'Gain',execution_domain:'synchronous',inputs:[{id:'audio',type:'Audio',required:true}],outputs:[{id:'audio',type:'Audio',required:true}],parameters:[{id:'gain_db',type:'number',required:false,default:0,minimum:-24,maximum:12,unit:'dB'}]},
 {typeId:'wav_output',displayName:'WAV Output',execution_domain:'synchronous',inputs:[{id:'audio',type:'Audio',required:true}],outputs:[{id:'path',type:'FilePath',required:true}],parameters:[{id:'path',type:'file_path',required:true}]}
];
const graph = {schema_version:1,nodes:[{id:'input',type:'wav_input',parameters:{path:'voice.wav'}},{id:'gain',type:'gain',parameters:{gain_db:-3}},{id:'output',type:'wav_output',parameters:{path:'voice-clean.wav'}}],connections:[{from:{node:'input',port:'audio'},to:{node:'gain',port:'audio'}},{from:{node:'gain',port:'audio'},to:{node:'output',port:'audio'}}],exports:[]};
const agent = {
 mode:'graph',busy:false,loading:false,canStop:false,error:'',warnings:[],spaces:{user_root:'C:/Audio/Demo',ai_root:'C:/Audio/Demo/AI',tools:['nodes_list','graph_validate']},
 conversationId:'demo',conversations:[{id:'demo',mode:'graph',title:'录音处理',turn_count:1}],prompt:'把输出文件名改成 voice-clean.wav',attachGraph:true,
 setMode:noop,selectConversation:noop,newConversation:noop,setPrompt:noop,setAttachGraph:noop,loadOlder:noop,
 turns:[{id:'example-1',prompt:'帮我降低这段录音的音量，保留原始文件。\\n结果保存为一个新的 WAV 文件。',state:'completed',reply:{state:'completed',text:'已经准备好配置，原始录音不会被覆盖。\\n\\n**处理顺序**\\n\\n读取音频 → 调整增益 → 写入新文件\\n\\n- 增益设为 **−3 dB**\\n- 输出文件：\u0060voice-clean.wav\u0060\\n\\n你可以先在编辑器里检查参数，再手动运行。',model_calls:2,tool_calls:2,events:[{kind:'tool',tool:'graph_validate',success:true,arguments:{mode:'offline'},result:{valid:true}}]}}]
};
const session = {desktop:true,connection:{workspace:'C:/Audio/Demo',capabilities:{nodes,plugins:{errors:[]}},allowDevices:false,allowMonitor:false},devices:{inputs:[],outputs:[]},busy:'',task:null,taskBlocked:false,allowDevices:false,allowMonitor:false};
const draft = {localGraph:{graph,error:''},graphText:JSON.stringify(graph,null,2),mode:'offline',template:'wav',blockFrames:'256',duration:'10',probe:true,validationCurrent:true,fileLabel:'voice.graph.json',modified:false,canvasView:{positions:{},selectedId:'gain',zoom:0.8,pan:{x:0,y:0}},editGraph:noop,setCanvasView:noop,setTemplate:noop,setMode:noop,setBlockFrames:noop,setDuration:noop,setProbe:noop};
const record = {id:'run-demo',name:'voice.graph.json',kind:'graph',origin:'manual',state:'succeeded',started_at_ms:1792680000000,duration_ms:1250,parent_id:null,files:[{space:'user',path:'voice-clean.wav',role:'output',size_bytes:384044}],configuration:graph,result:{outputs:{output_file:{type:'FilePath',value:'voice-clean.wav'}}}};
const browser = {space:'user',path:'',loading:false,error:'',entries:[{path:'voice.wav',name:'voice.wav',kind:'file',bytes:384044,modified_at_ms:1792680000000},{path:'transcript.txt',name:'transcript.txt',kind:'file',bytes:96,modified_at_ms:1792680000000}],selected:{path:'transcript.txt',name:'transcript.txt',kind:'file',bytes:96,modified_at_ms:1792680000000},preview:{text:'这是一段用来检查浅色界面的示例文字。\\n这里展示文件内容，不会执行文件。',truncated:false},listing:{total:2,next_offset:null,warnings:[]},refresh:noop,setSpace:noop,up:noop,goTo:noop,select:noop,previousPage:noop,nextPage:noop};
const settings={config:{baseUrl:'https://example.invalid/v1',model:'demo-model',apiKey:''},settingsPath:'本机配置',settingsLoading:false,settingsBusy:false,settingsError:'',settingsNotice:'',configStatus:'未填写 API Key',editConfig:noop,saveCurrentSettings:noop,clearSavedSettings:noop};
const plugins={view:null,loading:false,busy:false,operation:null,error:'',notice:'',refresh:noop,install:noop,setEnabled:noop,uninstall:noop,cancelUninstall:noop};
const root = createRoot(document.getElementById('root'));
window.showCase = (name) => {
 let current = ['long','error','busy','empty'].includes(name) ? 'workbench' : name;
 let selectedAgent = agent;
 if(name==='long') selectedAgent={...agent,conversations:[{...agent.conversations[0],title:"C:\\\\AAAProject\\\\Temp\\\\TestSwitch\\\\10.14项目管理上.m4a C:\\\\AAAProject\\\\Temp\\\\TestSwitch\\\\10.12实验四要求.m4a 这两个文件,帮我把音量调大10dB"}],turns:[{...agent.turns[0],prompt:"C:\\\\AAAProject\\\\Temp\\\\TestSwitch\\\\10.14项目管理上.m4a C:\\\\AAAProject\\\\Temp\\\\TestSwitch\\\\10.12实验四要求.m4a 这两个文件,帮我把音量调大10dB",reply:{...agent.turns[0].reply,text:"已按这两个输入文件准备音量调整配置。\\n\\n处理顺序：读取音频 → 增益 +10 dB → 写入新文件。\\n\\n原文件保留，输出使用新的文件名。运行前可在编辑器检查增益参数与输出路径。\\n\\n如果峰值超过上限，输出可能出现削波；可以先查看音频检查结果，再决定是否运行。"}}]};
 if(name==='error') selectedAgent={...agent,error:'输出文件已存在，请检查输出路径。',turns:[{id:'failed',prompt:'请处理录音。',error:'任务没有完成，原文件未改动。',state:'failed',events:[{kind:'tool',tool:'graph_run',success:false,result:{code:'output_exists'}}]}]};
 if(name==='busy') selectedAgent={...agent,busy:true,canStop:true,stopping:false};
 if(name==='empty') selectedAgent={...agent,turns:[]};
 const body = current === 'editor' ? <EditorPage draft={draft} session={session} selection={{search:''}} onSelection={noop} submittingLocked={false} onValidate={noop} onRun={noop} onLoadTemplate={noop} onLoad={noop} onSave={noop} onSaveAs={noop} onDevices={noop} onTasks={noop}/> : current === 'settings' ? <SettingsPage settings={settings} session={session} plugins={plugins} workflowBusy={false} onPermissions={noop}/> : current==='files' ? <WorkspaceBrowserPage browser={browser} connected={true} onOpenGraph={noop} onOpenWorkflow={noop}/> : current==='tasks' ? <RunRecordsPage records={{records:[record],record,selectedId:record.id,warnings:[],loading:false,error:'',refresh:noop,select:noop,checkFiles:noop}} connected={true}/> : <WorkbenchPage connected onOpenSettings={noop} agent={selectedAgent} agentBlocked={false} onAgentSend={()=>window.__calls.push('send')} onAgentApply={noop}/>;
 root.render(<AppShell page={current} onNavigate={window.showCase} workspace='C:/Audio/Demo' onWorkspace={noop}>{body}</AppShell>);
};
window.showCase('workbench');
`;
const js = (await build({ stdin: { contents: fixture, loader: "tsx", resolveDir: app }, bundle: true, write: false, format: "iife", platform: "browser", define: { "process.env.NODE_ENV": '"production"' } })).outputFiles[0].text;
const browser = await chromium.launch({ headless: true, executablePath: chromiumPath });
const errors = [];
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
  page.setDefaultTimeout(8000);
  await page.route("**/*", route => route.abort());
  page.on("pageerror", error => errors.push(error.message));
  await page.setContent('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><style>' + css + '</style></head><body><div id="root"></div></body></html>');
  await page.addScriptTag({ content: js });
  await page.getByRole("group", { name: "用户消息", exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "快捷模板", exact: true }).count(), 0);
  assert.equal(await page.getByRole("group", { name: "准备方案的方式", exact: true }).count(), 0);
  assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).colorScheme), "light");
  const positions = await page.evaluate(() => ({ user: document.querySelector('.agent-user > p').getBoundingClientRect().x, assistant: document.querySelector('.agent-answer').getBoundingClientRect().x }));
  assert(positions.user > positions.assistant, "User and assistant must have clearly distinct alignment");
  await page.getByRole('button', { name: '发送', exact: true }).click();
  assert.deepEqual(await page.evaluate(() => window.__calls), ['send']);
  const details = page.getByRole('button', { name: '查看调用详情', exact: true });
  await details.click();
  assert.equal(await page.getByRole('button', { name: '收起调用详情', exact: true }).getAttribute('aria-expanded'), 'true');
  await page.getByRole('button', { name: '收起调用详情', exact: true }).click();
  for (const name of ['workbench', 'editor', 'files', 'tasks', 'settings', 'error', 'long']) {
    for (const width of [1440, 1050, 800, 390]) {
      await page.setViewportSize({ width, height: width === 1050 || width === 800 ? 820 : 1000 });
      await page.evaluate(name => window.showCase(name), name);
      await page.waitForTimeout(100);
      await page.evaluate(() => window.scrollTo(0, 0));
      assert.deepEqual(errors, [], name + ' renders without errors');
      const sizes = await page.evaluate(() => ({ width: innerWidth, content: document.documentElement.scrollWidth }));
      assert(sizes.content <= sizes.width + 1, name + ' overflows viewport at ' + width);
      await page.screenshot({ path: path.join(output, name + '-' + width + '.png'), fullPage: true });
    }
  }
  assert.deepEqual(errors, [], 'No render errors');
  for (const [width,height] of [[800,650],[1050,820],[1440,1000]]) {
    await page.setViewportSize({width,height});
    for (const name of ['empty','workbench','long','error','busy']) {
      await page.evaluate(name=>window.showCase(name),name);
      await page.waitForTimeout(60);
      await page.evaluate(()=>window.scrollTo(0,0));
      const box=await page.evaluate(()=>{
        const composer=document.querySelector('.agent-composer').getBoundingClientRect();
        const messages=document.querySelector('.agent-conversation').getBoundingClientRect();
        const input=document.querySelector('#agent-prompt').getBoundingClientRect();
        return {bottom:composer.bottom,top:composer.top,messagesBottom:messages.bottom,messagesHeight:messages.height,inputHeight:input.height,height:innerHeight,scroll:document.documentElement.scrollHeight};
      });
      assert(box.bottom<=height+1, name+' composer visible without page scrolling at '+width+'x'+height);
      assert(box.messagesHeight>0 && box.messagesBottom<=box.top+1, name+' messages stay above composer');
      assert(box.inputHeight>=64 && box.inputHeight<=100, 'input stays bounded');
      assert(box.scroll<=height+1,'workbench uses bounded page height');
      await page.screenshot({path:path.join(output,'composer-'+name+'-'+width+'x'+height+'.png')});
    }
    await page.evaluate(()=>window.showCase('workbench'));
    await page.getByRole('button',{name:'查看工作区与可用工具',exact:true}).click();
    const info=page.locator('.agent-spaces > .disclosure-content');
    assert((await info.boundingBox()).height<=140.1,'expanded low-frequency information is bounded');
    assert((await page.locator('.agent-composer').boundingBox()).y+(await page.locator('.agent-composer').boundingBox()).height<=height+1);
    await page.getByRole('button',{name:'收起工作区与可用工具',exact:true}).click();
  }
  assert.deepEqual(errors, [], 'No compact workbench errors');
  console.log('Light-theme visual smoke passed: 28 responsive screenshots plus 15 compact workbench screenshots, composer visible at 800x650, 1050x820 and 1440x1000 in empty/long/error/busy states, bounded disclosures, send callback. ' + output);
} finally { await browser.close(); }
