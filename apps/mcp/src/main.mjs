#!/usr/bin/env node

import { Transform } from 'node:stream';
import { access, stat } from 'node:fs/promises';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { ControlClient } from './control-client.mjs';
import { createAudioServer } from './server.mjs';

export const MAX_MCP_FRAME_BYTES = 4 * 1024 * 1024;
const MAX_JSON_DEPTH = 128;

export function parseArgs(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if ((flag !== '--workspace' && flag !== '--engine') || value === undefined || value.startsWith('--')) {
      throw new Error('Usage: node src/main.mjs --workspace <existing-root> --engine <absolute-control-cli.exe>');
    }
    const key = flag.slice(2);
    if (values[key] !== undefined) throw new Error(`Duplicate argument: ${flag}`);
    values[key] = value;
  }
  if (!values.workspace || !values.engine) {
    throw new Error('Both --workspace and --engine are required.');
  }
  if (!path.isAbsolute(values.engine)) throw new Error('--engine must be an absolute path.');
  return { workspace: path.resolve(values.workspace), enginePath: path.normalize(values.engine) };
}

async function validateHostPaths({ workspace, enginePath }) {
  const workspaceInfo = await stat(workspace);
  if (!workspaceInfo.isDirectory()) throw new Error('--workspace must name an existing directory.');
  const engineInfo = await stat(enginePath);
  if (!engineInfo.isFile()) throw new Error('--engine must name an existing file.');
  await access(enginePath);
}

function validateBackendScope(capabilities) {
  if (!capabilities?.success) throw new Error('The control backend rejected its capabilities handshake.');
  if (capabilities.data?.policy?.allow_devices !== false || capabilities.data?.policy?.allow_monitor !== false) {
    throw new Error('Refusing a control backend with device or monitor access enabled.');
  }
}

// JSON.parse accepts duplicate object keys and keeps only the last value. Reject them before
// the SDK parser so tool arguments and inline graphs cannot be silently changed.
export function assertUnambiguousJson(text) {
  let cursor = 0;
  let graphVersionToken;
  const whitespace = () => {
    while (/\s/u.test(text[cursor] ?? '')) cursor += 1;
  };
  const fail = message => { throw new SyntaxError(`${message} at character ${cursor}`); };
  const stringValue = () => {
    if (text[cursor] !== '"') fail('Expected JSON string');
    const start = cursor++;
    while (cursor < text.length) {
      if (text[cursor] === '"') {
        cursor += 1;
        try { return JSON.parse(text.slice(start, cursor)); }
        catch { fail('Invalid JSON string'); }
      }
      if (text[cursor] === '\\') cursor += 2;
      else cursor += 1;
    }
    fail('Unterminated JSON string');
  };
  const value = (depth, jsonPath = []) => {
    if (depth > MAX_JSON_DEPTH) fail(`JSON nesting exceeds ${MAX_JSON_DEPTH}`);
    whitespace();
    const token = text[cursor];
    if (token === '{') {
      cursor += 1;
      whitespace();
      const keys = new Set();
      if (text[cursor] === '}') { cursor += 1; return; }
      while (true) {
        whitespace();
        const key = stringValue();
        if (keys.has(key)) fail(`Duplicate JSON key ${JSON.stringify(key)}`);
        keys.add(key);
        whitespace();
        if (text[cursor++] !== ':') fail('Expected colon');
        whitespace();
        const valueStart = cursor;
        const childPath = [...jsonPath, key];
        value(depth + 1, childPath);
        if (childPath.length === 4 && childPath[0] === 'params' && childPath[1] === 'arguments' &&
            childPath[2] === 'graph' && childPath[3] === 'schema_version') {
          graphVersionToken = text.slice(valueStart, cursor);
        }
        whitespace();
        const separator = text[cursor++];
        if (separator === '}') return;
        if (separator !== ',') fail('Expected comma or closing brace');
      }
    }
    if (token === '[') {
      cursor += 1;
      whitespace();
      if (text[cursor] === ']') { cursor += 1; return; }
      let index = 0;
      while (true) {
        value(depth + 1, [...jsonPath, index++]);
        whitespace();
        const separator = text[cursor++];
        if (separator === ']') return;
        if (separator !== ',') fail('Expected comma or closing bracket');
      }
    }
    if (token === '"') { stringValue(); return; }
    const remainder = text.slice(cursor);
    const primitive = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/u.exec(remainder)?.[0];
    if (!primitive) fail('Invalid JSON value');
    cursor += primitive.length;
  };
  value(0);
  whitespace();
  if (cursor !== text.length) fail('Unexpected trailing JSON data');
  const parsed = JSON.parse(text);
  if (parsed?.method === 'tools/call' &&
      Object.prototype.hasOwnProperty.call(parsed?.params?.arguments?.graph ?? {}, 'schema_version') &&
      graphVersionToken !== '1') {
    fail('Graph schema_version must use the integer token 1');
  }
}

export function createGuardedInput(maxFrameBytes = MAX_MCP_FRAME_BYTES) {
  let pending = Buffer.alloc(0);
  const decoder = new TextDecoder('utf-8', { fatal: true });
  return new Transform({
    transform(chunk, _encoding, callback) {
      try {
        pending = Buffer.concat([pending, chunk]);
        let newline;
        while ((newline = pending.indexOf(0x0a)) !== -1) {
          const frame = pending.subarray(0, newline);
          pending = pending.subarray(newline + 1);
          if (frame.length > maxFrameBytes) throw new Error(`MCP frame exceeds ${maxFrameBytes} bytes`);
          const withoutCr = frame.at(-1) === 0x0d ? frame.subarray(0, -1) : frame;
          assertUnambiguousJson(decoder.decode(withoutCr));
          this.push(frame);
          this.push(Buffer.from('\n'));
        }
        if (pending.length > maxFrameBytes) throw new Error(`MCP frame exceeds ${maxFrameBytes} bytes`);
        callback();
      } catch (error) {
        callback(error);
      }
    },
    flush(callback) {
      if (pending.length === 0) callback();
      else callback(new Error('MCP stdio input ended with an incomplete frame'));
    },
  });
}

export async function main(argv = process.argv.slice(2)) {
  const host = parseArgs(argv);
  await validateHostPaths(host);

  const client = new ControlClient(host);
  const server = createAudioServer(client);
  const guardedInput = createGuardedInput();
  let shuttingDown;

  const shutdown = reason => {
    if (shuttingDown) return shuttingDown;
    shuttingDown = (async () => {
      process.stdin.unpipe(guardedInput);
      process.stdin.pause();
      process.stdin.destroy();
      guardedInput.destroy();
      await server.close().catch(error => {
        process.exitCode = 1;
        console.error(`MCP close failed: ${error.message}`);
      });
      try {
        const outcome = await client.close(reason);
        if (outcome?.forced) {
          process.exitCode = 1;
          console.error(outcome.message || 'The audio backend required forced shutdown.');
        }
      } catch (error) {
        process.exitCode = 1;
        console.error(`Audio backend shutdown was not confirmed: ${error.message}`);
      }
    })();
    return shuttingDown;
  };

  guardedInput.once('error', error => {
    process.exitCode = 1;
    console.error(`Rejected MCP stdio input: ${error.message}`);
    void shutdown('MCP输入无效，已请求取消活动任务。');
  });
  process.stdin.once('end', () => {
    // Let pipe() finish the guarded transform first so an unterminated final frame is rejected.
    setImmediate(() => { void shutdown('MCP标准输入已关闭，已请求取消活动任务。'); });
  });
  process.stdin.on('error', error => {
    process.exitCode = 1;
    console.error(`MCP stdin failed: ${error.message}`);
    void shutdown('MCP标准输入失败，已请求取消活动任务。');
  });
  process.stdout.on('error', error => {
    process.exitCode = 1;
    console.error(`MCP stdout failed: ${error.message}`);
    void shutdown('MCP标准输出失败，已请求取消活动任务。');
  });
  process.once('SIGINT', () => {
    process.exitCode = 130;
    void shutdown('MCP收到SIGINT，已请求取消活动任务。');
  });
  process.once('SIGTERM', () => {
    process.exitCode = 143;
    void shutdown('MCP收到SIGTERM，已请求取消活动任务。');
  });

  try {
    validateBackendScope(await client.start());
    const transport = new StdioServerTransport(guardedInput, process.stdout, {
      maxBufferSize: MAX_MCP_FRAME_BYTES + 1,
    });
    await server.connect(transport);
    process.stdin.pipe(guardedInput);
  } catch (error) {
    await shutdown('MCP启动失败，已请求取消活动任务。');
    throw error;
  }
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  main().catch(async error => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
