// Real media decode/seek against the Rust range reader. No visible window or sound.
// This tests Chromium+reader, not native WebView2 custom-protocol registration.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
for (const name of ['AUDIOGRAPH_PLAYWRIGHT_MODULE', 'AUDIOGRAPH_CHROMIUM', 'AUDIOGRAPH_TEST_EXE']) {
  if (!process.env[name]) throw Error(`Supply existing ${name}; this test never downloads dependencies.`);
}
const { chromium } = require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const child = spawn(process.env.AUDIOGRAPH_TEST_EXE, [
  'workspace_audio_stream::playback_tests::headless_range_playback_bridge', '--exact', '--ignored', '--nocapture',
], { windowsHide: true, env: { ...process.env, AUDIOGRAPH_AUDIO_BRIDGE: '1' }, stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
const exit = new Promise((resolve, reject) => { child.once('exit', resolve); child.once('error', reject); });
child.stdout.on('data', data => { output += data.toString(); });
child.stderr.on('data', data => { output += data.toString(); });
let browser;
let ready;
try {
  const deadline = Date.now() + 15000;
  while (!ready && Date.now() < deadline) {
    const match = output.match(/AUDIO_PREVIEW_READY (\{[^\n]+\})/);
    if (match) ready = JSON.parse(match[1]);
    else {
      if (child.exitCode !== null) throw Error(`Bridge exited: ${output}`);
      await new Promise(resolve => setTimeout(resolve, 30));
    }
  }
  assert(ready, `Bridge did not start: ${output}`);
  assert(ready.bytes > 32 * 1024 * 1024);
  browser = await chromium.launch({ headless: true, executablePath: process.env.AUDIOGRAPH_CHROMIUM });
  const page = await browser.newPage();
  page.setDefaultTimeout(15000);
  await page.setContent('<audio muted controls preload="metadata"></audio>');
  await page.evaluate(url => {
    const audio = document.querySelector('audio');
    audio.muted = true;
    audio.src = url;
    window.mediaErrors = [];
    audio.addEventListener('error', () => window.mediaErrors.push(audio.error?.code));
  }, ready.url);
  await page.waitForFunction(() => Number.isFinite(document.querySelector('audio').duration));
  assert.equal(await page.$eval('audio', audio => audio.duration), 240);
  assert.equal(await page.$eval('audio', audio => audio.paused), true, 'no autoplay');
  await page.$eval('audio', audio => audio.play());
  await page.waitForFunction(() => document.querySelector('audio').currentTime > 0.1);
  await page.$eval('audio', audio => { audio.pause(); audio.currentTime = 230; });
  await page.waitForFunction(() => { const a = document.querySelector('audio'); return !a.seeking && a.readyState >= 2 && a.currentTime >= 230; });
  await page.$eval('audio', audio => audio.play());
  await page.waitForFunction(() => document.querySelector('audio').currentTime > 230.1);
  assert.deepEqual(await page.evaluate(() => window.mediaErrors), []);
  await page.$eval('audio', audio => { audio.pause(); audio.removeAttribute('src'); audio.load(); });
  await browser.close(); browser = null;
  assert.equal((await fetch(ready.stop)).status, 200);
  const code = await exit;
  assert.equal(code, 0, output);
  const stats = JSON.parse(output.match(/AUDIO_PREVIEW_STATS (\{[^\n]+\})/)[1]);
  assert(stats.max_body <= 2 * 1024 * 1024 && stats.max_offset > 32 * 1024 * 1024);
  console.log(`Real audio playback passed: ${ready.bytes} bytes, duration 240s, play/pause/seek to 230s; ${JSON.stringify(stats)}`);
} finally {
  if (browser) await browser.close();
  if (child.exitCode === null && ready) { try { await fetch(ready.stop); } catch {} }
  if (child.exitCode === null) child.kill();
  await exit;
}
