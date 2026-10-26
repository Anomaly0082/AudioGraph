// Real React hook, mocked IPC, headless browser only. No model, user data or native UI.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
const require = createRequire(import.meta.url);
const app = fileURLToPath(new URL("../", import.meta.url));
if (!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE || !process.env.AUDIOGRAPH_CHROMIUM)
  throw Error("Supply existing Playwright and Chromium paths.");
const { chromium } = require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const mock = `
export async function invokeDesktop(command,args) {
  window.calls.push({command,args});
  if(command==='agent_spaces')return{user_root:'C:/mock',ai_root:'C:/mock-ai',tools:[]};
  if(command==='conversation_list')return{records:[window.records[args.sessionId]],warnings:[],truncated:false};
  if(command==='conversation_load') {
    if(window.failLoad)throw Error('history unavailable');
    const reply=structuredClone(window.records[args.sessionId]);
    if(window.delayLoad){window.delayLoad=false;await new Promise(resolve=>window.finishLoad=resolve);}
    return reply;
  }
  if(command==='agent_turn') {
    if(window.delayTurn){window.delayTurn=false;await new Promise(resolve=>window.finishTurn=resolve);}
    if(window.failTurn){window.failTurn=false;throw Error('request disconnected');}
    const state=window.cancelled?'cancelled':'completed';window.cancelled=false;
    const reply={request_id:args.requestId,conversation_id:args.conversationId,state,text:'local reply',events:[],model_calls:1,tool_calls:0};
    const record=window.records[args.sessionId];
    record.turns.push({id:args.requestId,prompt:args.prompt,state,reply});record.turn_count=record.turns.length;
    return reply;
  }
  if(command==='agent_cancel') {
    if(window.failCancel){window.failCancel=false;throw Error('cancel unavailable');}
    window.cancelled=true;if(!window.deferCancel)window.finishTurn?.();return{};
  }
  throw Error('Unexpected command '+command);
}`;
const fixture = `
import React,{useState}from'react';import{createRoot}from'react-dom/client';
import{useAgentTools}from'./src/hooks/useAgentTools';
window.calls=[];
const record=id=>({id:id.repeat(64),title:id,mode:'graph',created_at_ms:1,updated_at_ms:1,turn_count:0,last_state:'completed',turns:[],has_more:false});
window.records={a:record('a'),b:record('b')};
window.records.reconnected=window.records.a;
function Fixture(){const[session,setSession]=useState('a');window.setSession=setSession;
const agent=useAgentTools({connection:session?{sessionId:session,workspace:'C:/mock-'+(session==='reconnected'?'a':session)}:null,settings:{config:{baseUrl:'https://local-mock.invalid/v1',apiKey:'test-only',model:'mock'}},blocked:()=>false});
window.agent=agent;return <textarea aria-label="prompt" value={agent.prompt} onChange={e=>agent.setPrompt(e.target.value)}/>;}
createRoot(document.getElementById('root')).render(<Fixture/>);
`;
const js = (await build({stdin:{contents:fixture,loader:"tsx",resolveDir:app},bundle:true,write:false,format:"iife",platform:"browser",define:{"process.env.NODE_ENV":'"production"'},plugins:[{name:"mock-ipc",setup(b){
  b.onResolve({filter:/api\/desktop$/},()=>({path:"desktop",namespace:"mock"}));
  b.onLoad({filter:/.*/,namespace:"mock"},()=>({contents:mock,loader:"js"}));
}}]})).outputFiles[0].text;
const browser = await chromium.launch({headless:true,executablePath:process.env.AUDIOGRAPH_CHROMIUM});
const errors=[],requests=[];
let checks=0;
try {
  async function fresh(){
    const page=await browser.newPage();
    page.on("pageerror",e=>errors.push(e.message));
    await page.route("**/*",r=>{requests.push(r.request().url());return r.abort();});
    await page.setContent('<div id="root"></div>');await page.addScriptTag({content:js});
    await page.waitForFunction(()=>window.agent&&!window.agent.loading&&window.agent.conversationId==='a'.repeat(64));
    return page;
  }
  async function send(page,text){await page.getByRole('textbox').fill(text);await page.evaluate(()=>window.agent.send());}
  // A transport error and a failed history read must not lose the retry draft.
  {
    const page=await fresh();
    await page.evaluate(()=>{window.failTurn=true;window.failLoad=true;});
    await send(page,'keep my interrupted request');
    assert.equal(await page.getByRole('textbox').inputValue(),'keep my interrupted request');
    assert.deepEqual(await page.evaluate(()=>[window.agent.busy,window.agent.readBusy(),window.agent.turns.at(-1).state]),[false,false,'interrupted']);
    assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.command==='agent_turn').length),1,'never retry automatically');
    await page.evaluate(()=>window.failLoad=false);
    await page.evaluate(()=>window.setSession(null));
    await page.waitForFunction(()=>window.agent.conversationId===null&&!window.agent.loading);
    await page.evaluate(()=>window.setSession('reconnected'));
    await page.waitForFunction(()=>window.agent.conversationId==='a'.repeat(64)&&!window.agent.loading);
    assert.equal(await page.getByRole('textbox').inputValue(),'keep my interrupted request','recovered input survives a later reconnect');
    await send(page,'manual retry');
    assert.equal(await page.evaluate(()=>window.agent.turns.at(-1).state),'completed');
    assert.equal(await page.getByRole('textbox').inputValue(),'');checks++;await page.close();
  }
  // Failed cancellation releases only the stop button; retry cancellation then send again.
  {
    const page=await fresh();await page.getByRole('textbox').fill('cancel this');
    await page.evaluate(()=>{window.delayTurn=true;void window.agent.send();});
    await page.waitForFunction(()=>!!window.finishTurn);
    await page.evaluate(()=>{window.failCancel=true;return window.agent.stop();});
    assert.deepEqual(await page.evaluate(()=>[window.agent.busy,window.agent.stopping]),[true,false]);
    await page.evaluate(()=>window.agent.stop());await page.waitForFunction(()=>!window.agent.busy);
    assert.equal(await page.evaluate(()=>window.agent.turns.at(-1).state),'cancelled');
    await send(page,'after cancellation');
    assert.equal(await page.evaluate(()=>window.agent.turns.at(-1).state),'completed');checks++;await page.close();
  }
  // A history read error after a received reply leaves it visible and does not block new input.
  {
    const page=await fresh();await page.evaluate(()=>window.failLoad=true);
    await send(page,'reply before read failure');
    assert.equal(await page.evaluate(()=>window.agent.turns.at(-1).reply.text),'local reply');
    assert.equal(await page.evaluate(()=>window.agent.readBusy()),false);
    await page.evaluate(()=>window.failLoad=false);await send(page,'next request');
    assert.equal(await page.evaluate(()=>window.agent.turns.length),2);checks++;await page.close();
  }
  // Late history response must not replace the newly opened workspace.
  {
    const page=await fresh();
    await page.evaluate(()=>{window.delayLoad=true;void window.agent.selectConversation('a'.repeat(64));});
    await page.waitForFunction(()=>!!window.finishLoad);
    await page.evaluate(()=>window.setSession('b'));
    await page.waitForFunction(()=>window.agent.conversationId==='b'.repeat(64)&&!window.agent.loading);
    await page.getByRole('textbox').fill('workspace B draft');
    await page.evaluate(()=>window.finishLoad());await page.waitForFunction(()=>!window.agent.readBusy());
    assert.equal(await page.evaluate(()=>window.agent.conversationId),'b'.repeat(64));
    assert.equal(await page.getByRole('textbox').inputValue(),'workspace B draft');
    await send(page,'workspace B request');
    assert.equal(await page.evaluate(()=>window.records.a.turn_count),0);checks++;await page.close();
  }
  // An unsent draft survives reconnecting the same directory, never leaks to another.
  {
    const page=await fresh();await page.getByRole('textbox').fill('unsent local draft');
    await page.evaluate(()=>window.setSession(null));
    await page.waitForFunction(()=>window.agent.conversationId===null&&!window.agent.loading);
    await page.evaluate(()=>window.setSession('reconnected'));
    await page.waitForFunction(()=>window.agent.conversationId==='a'.repeat(64)&&!window.agent.loading);
    assert.equal(await page.getByRole('textbox').inputValue(),'unsent local draft');
    assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.command==='agent_turn').length),0);
    await page.evaluate(()=>window.setSession('b'));
    await page.waitForFunction(()=>window.agent.conversationId==='b'.repeat(64)&&!window.agent.loading);
    assert.equal(await page.getByRole('textbox').inputValue(),'');checks++;await page.close();
  }
  // Disconnect during a request, reconnect, then explicitly send. No stale reply or auto replay.
  {
    const page=await fresh();await page.getByRole('textbox').fill('old workspace request');
    await page.evaluate(()=>{window.delayTurn=true;void window.agent.send();});
    await page.waitForFunction(()=>!!window.finishTurn);
    await page.evaluate(()=>window.setSession(null));
    await page.waitForFunction(()=>!window.agent.busy&&window.agent.conversationId===null);
    await page.evaluate(()=>window.setSession('b'));
    await page.waitForFunction(()=>window.agent.conversationId==='b'.repeat(64)&&!window.agent.loading);
    assert.equal(await page.evaluate(()=>window.agent.turns.length),0);
    assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.command==='agent_turn').length),1);
    await send(page,'new workspace request');
    assert.equal(await page.evaluate(()=>window.records.b.turn_count),1);checks++;await page.close();
  }
  // The old AI response arrives only after the new workspace is fully loaded.
  {
    const page=await fresh();await page.getByRole('textbox').fill('late old request');
    await page.evaluate(()=>{window.delayTurn=true;window.deferCancel=true;void window.agent.send();});
    await page.waitForFunction(()=>!!window.finishTurn);
    await page.evaluate(()=>window.setSession('b'));
    await page.waitForFunction(()=>window.agent.conversationId==='b'.repeat(64)&&!window.agent.loading);
    await page.evaluate(()=>window.finishTurn());await page.waitForFunction(()=>!window.agent.busy);
    assert.equal(await page.evaluate(()=>window.agent.conversationId),'b'.repeat(64));
    assert.deepEqual(await page.evaluate(()=>window.agent.turns),[]);
    assert.equal(await page.evaluate(()=>window.agent.error),'');
    await send(page,'after old reply');
    assert.equal(await page.evaluate(()=>window.records.b.turn_count),1);checks++;await page.close();
  }
  assert.deepEqual(errors,[]);assert.deepEqual(requests,[]);
  console.log(`Conversation recovery: ${checks} scenarios passed; no network requests or native UI.`);
} finally {await browser.close();}
