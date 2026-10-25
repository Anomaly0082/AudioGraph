// Optional headless-only integration test. Every native operation is mocked;
// no real conversations, clipboard, audio, run records, or online APIs are used.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
const require = createRequire(import.meta.url);
const app = fileURLToPath(new URL("../", import.meta.url));
if (!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE || !process.env.AUDIOGRAPH_CHROMIUM)
  throw new Error("Supply existing AUDIOGRAPH_PLAYWRIGHT_MODULE and AUDIOGRAPH_CHROMIUM paths.");
const { chromium } = require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const mock = `
export async function invokeDesktop(command,args) {
  window.calls.push({command,args});
  const records=window.records[args.sessionId];
  if(command==='agent_spaces')return{user_root:'C:/mock',ai_root:'C:/mock-ai',tools:[]};
  if(command==='conversation_list')return{records:records.map(record=>record.summary),warnings:[],truncated:false};
  if(command==='conversation_load') {
    const record=records.find(record=>record.summary.id===args.conversationId);
    const reply={...record.summary,created_at_ms:1,has_more:false,turns:record.turns};
    if(window.delayLoad){window.delayLoad=false;await new Promise(resolve=>window.finishLoad=resolve);}
    return reply;
  }
  if(command==='conversation_delete') {
    if(window.delayDelete){window.delayDelete=false;await new Promise(resolve=>window.finishDelete=resolve);}
    if(window.failDelete){window.failDelete=false;throw Error('模拟会话移除失败');}
    window.records[args.sessionId]=records.filter(record=>record.summary.id!==args.conversationId);
    return{id:args.conversationId,recoverable:true};
  }
  throw Error('Unexpected native command '+command);
}`;
const fixture = `
import React,{useState}from'react';import{createRoot}from'react-dom/client';
import{useAgentTools}from'./src/hooks/useAgentTools';import Panel from'./src/components/AgentToolsPanel';
window.calls=[];
const record=(id,title)=>({summary:{id,title,mode:'graph',updated_at_ms:1,turn_count:1,last_state:'completed'},turns:[{id:'request-'+id,prompt:title,state:'completed',reply:{request_id:'request-'+id,state:'completed',text:'Saved '+title,events:[],model_calls:1,tool_calls:0}}]});
window.primary='a'.repeat(64);window.other='b'.repeat(64);window.newSession='c'.repeat(64);
window.records={a:[record(window.primary,'Primary'),record(window.other,'Other')],b:[record(window.newSession,'New workspace')]};
function Fixture(){const[session,setSession]=useState('a');window.changeWorkspace=()=>setSession('b');window.session=session;
const agent=useAgentTools({connection:{sessionId:session,workspace:'C:/mock-'+session},settings:{},blocked:()=>false});window.agent=agent;
return <Panel agent={agent} blocked={false} onSend={()=>void agent.send()} onApplyGraph={()=>{}} onSettings={()=>{}}/>;}
createRoot(document.getElementById('root')).render(<Fixture/>);
`;
const js = (await build({ stdin: { contents: fixture, loader: "tsx", resolveDir: app }, bundle: true, write: false, format: "iife", platform: "browser", define: { "process.env.NODE_ENV": '"production"' }, plugins: [{ name: "mock-native", setup(builder) {
  builder.onResolve({ filter: /api\/desktop$/ }, () => ({ path: "desktop", namespace: "mock" }));
  builder.onLoad({ filter: /.*/, namespace: "mock" }, () => ({ contents: mock, loader: "js" }));
} }] })).outputFiles[0].text;
const browser = await chromium.launch({ headless: true, executablePath: process.env.AUDIOGRAPH_CHROMIUM });
try {
  const page = await browser.newPage();
  const errors = [], requests = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.route("**/*", route => { requests.push(route.request().url()); return route.abort(); });
  async function reset(delayLoad = false) {
    await page.setContent('<div id="root"></div>');
    await page.evaluate(delayLoad => { window.delayLoad = delayLoad; window.finishLoad = null; }, delayLoad);
    await page.addScriptTag({ content: js });
    if (!delayLoad) await page.waitForFunction(() => window.agent && !window.agent.loading && window.agent.conversationId === window.primary);
  }
  // While the initial saved history is loading the composer owns a separate
  // new-graph draft. It must survive later deletion of a saved conversation.
  await reset(true);
  await page.waitForFunction(() => typeof window.finishLoad === "function");
  await page.locator("#agent-prompt").fill("unsent new conversation draft");
  await page.evaluate(() => window.finishLoad());
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === window.primary);
  const remove = page.getByRole("button", { name: "删除当前会话", exact: true });
  const picker = page.getByRole("combobox", { name: "选择AI会话" });
  const prompt = page.locator("#agent-prompt");
  const deletionCalls = () => page.evaluate(() => window.calls.filter(call => call.command === "conversation_delete"));
  assert.equal(await remove.getAttribute("title"), "删除当前会话");
  await remove.click();
  assert.match(await page.getByRole("dialog").innerText(), /保留本机备份；音频和运行记录不变/);
  await page.getByRole("dialog").press("Escape");
  assert.equal((await deletionCalls()).length, 0);
  // Failed deletion preserves history and draft.
  await prompt.fill("keep this draft");
  await page.evaluate(() => window.failDelete = true);
  await remove.click();await page.getByRole("dialog").getByRole("button", { name: "删除", exact: true }).click();
  await page.getByRole("alert").waitFor();
  assert.equal(await prompt.inputValue(), "keep this draft");
  assert.equal(await page.getByText("Saved Primary", { exact: true }).count(), 1);
  // Another conversation's draft must survive removing the selected one.
  await picker.selectOption(await page.evaluate(() => window.other));
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === window.other);
  await prompt.fill("other conversation draft");
  await picker.selectOption(await page.evaluate(() => window.primary));
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === window.primary);
  await page.evaluate(() => window.delayDelete = true);
  await remove.click();await page.getByRole("dialog").getByRole("button", { name: "删除", exact: true }).click();
  await page.waitForFunction(() => typeof window.finishDelete === "function");
  assert.equal(await remove.isDisabled(), true);
  assert.equal(await picker.isDisabled(), true);
  const before = await page.evaluate(() => window.calls.length);
  await page.evaluate(async () => {
    await Promise.all([window.agent.deleteConversation(window.primary), window.agent.selectConversation(window.other), window.agent.newConversation(), window.agent.send()]);
    window.agent.setMode("workflow");
  });
  assert.equal(await page.evaluate(() => window.calls.length), before, "Delete lock blocks duplicate deletion, send and conversation switching.");
  assert.equal(await page.evaluate(() => window.agent.mode), "graph");
  await page.evaluate(() => window.finishDelete());
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === null);
  assert.equal(await page.locator(".agent-turn").count(), 0);
  assert.equal(await prompt.inputValue(), "unsent new conversation draft", "Deleting the saved conversation preserves the separate new-mode draft.");
  assert.equal(await remove.isDisabled(), true);
  assert.equal(await picker.locator("option").count(), 2, "New-conversation placeholder and the other saved conversation remain.");
  assert.deepEqual((await deletionCalls()).at(-1).args, { sessionId: "a", conversationId: "a".repeat(64) });
  assert.equal(await page.evaluate(() => window.calls.some(call => call.command === "agent_turn")), false);
  await picker.selectOption(await page.evaluate(() => window.other));
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === window.other);
  assert.equal(await prompt.inputValue(), "other conversation draft");

  // A deletion reply from an old session cannot clear the new workspace.
  await page.evaluate(() => { window.delayDelete = true; window.finishDelete = null; });
  await remove.click();await page.getByRole("dialog").getByRole("button", { name: "删除", exact: true }).click();
  await page.waitForFunction(() => typeof window.finishDelete === "function");
  await page.evaluate(() => window.changeWorkspace());
  await page.waitForFunction(() => window.agent.conversationId === window.newSession && !window.agent.loading);
  await page.evaluate(() => window.finishDelete());
  await page.waitForFunction(() => !window.agent.readBusy());
  assert.equal(await page.getByText("Saved New workspace", { exact: true }).count(), 1);
  assert.equal(await page.evaluate(() => window.agent.conversationId), "c".repeat(64));

  // A previously started old-session history read cannot restore a deleted ID.
  await reset(true);
  await page.waitForFunction(() => typeof window.finishLoad === "function");
  await page.evaluate(() => window.changeWorkspace());
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === window.newSession);
  await remove.click();await page.getByRole("dialog").getByRole("button", { name: "删除", exact: true }).click();
  await page.waitForFunction(() => !window.agent.loading && window.agent.conversationId === null);
  await page.evaluate(() => window.finishLoad());
  await page.waitForFunction(() => !window.agent.readBusy());
  assert.equal(await page.evaluate(() => window.agent.conversationId), null);
  assert.equal(await page.locator(".agent-turn").count(), 0);
  assert.deepEqual(errors, []);
  assert.deepEqual(requests, []);
  console.log("Conversation deletion headless regressions passed: confirmation/cancel, failure preservation, lock/draft/history, other conversation preservation, stale-session reads/replies; zero network requests.");
} finally { await browser.close(); }
