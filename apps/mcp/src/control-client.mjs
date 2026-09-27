import { spawn } from 'node:child_process';
import { realpath, stat } from 'node:fs/promises';
import path from 'node:path';

const REQUEST_BYTES = 4 * 1024 * 1024;
const RESPONSE_BYTES = 8 * 1024 * 1024;
const STDERR_BYTES = 16 * 1024;
const MAX_PENDING = 8;
const OPERATIONS = new Set([
  'capabilities', 'nodes.list', 'nodes.describe', 'audio.inspect', 'graph.validate',
  'tasks.start', 'tasks.status', 'tasks.cancel', 'tasks.result', 'tasks.release',
]);

export class ControlClientError extends Error {
  constructor(code, message, { uncertain = false, forced = false } = {}) {
    super(message);
    this.name = 'ControlClientError';
    this.code = code;
    this.uncertain = uncertain;
    this.forced = forced;
  }
}

function rejected(message, code = 'control_request_rejected') {
  return new ControlClientError(code, message);
}

async function verifyPaths(enginePath, workspace) {
  const executable = await realpath(enginePath);
  const directory = await realpath(workspace);
  if (!(await stat(executable)).isFile()) throw rejected('enginePath must name a file.', 'invalid_host_config');
  if (!(await stat(directory)).isDirectory()) throw rejected('workspace must name an existing directory.', 'invalid_host_config');
  return { enginePath: executable, workspace: directory };
}

function validateJson(value, depth = 0, ancestors = new Set()) {
  if (depth > 64) throw rejected('Request nesting exceeds 64 levels, including the protocol envelope.');
  if (typeof value === 'string') {
    for (let index = 0; index < value.length; index++) {
      const unit = value.charCodeAt(index);
      if (unit >= 0xd800 && unit <= 0xdbff) {
        const next = value.charCodeAt(++index);
        if (!(next >= 0xdc00 && next <= 0xdfff)) throw rejected('Request contains an unpaired Unicode surrogate.');
      } else if (unit >= 0xdc00 && unit <= 0xdfff) throw rejected('Request contains an unpaired Unicode surrogate.');
    }
    return;
  }
  if (value === null || typeof value === 'boolean') return;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw rejected('Request numbers must be finite.');
    return;
  }
  if (typeof value !== 'object') throw rejected('Request must contain only JSON values.');
  if (ancestors.has(value)) throw rejected('Request contains a circular reference.');
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) {
    throw rejected('Request objects must be plain JSON objects.');
  }
  ancestors.add(value);
  if (!Array.isArray(value)) for (const key of Object.keys(value)) validateJson(key, depth, ancestors);
  for (const item of Object.values(value)) validateJson(item, depth + 1, ancestors);
  ancestors.delete(value);
}

/**
 * One trusted host configuration owns one control-cli process for its entire lifetime.
 * No shell, device permission flags, automatic restart or tasks.start retries are exposed.
 * testHooks is dependency injection for module tests, not a CLI/tool option or sandbox.
 */
export class ControlClient {
  #options;
  #spawn;
  #verify;
  #child = null;
  #startup = null;
  #spawned = false;
  #closed = false;
  #nextId = 1;
  #pending = new Map();
  #lineBuffer = Buffer.allocUnsafe(8192);
  #lineBytes = 0;
  #stderr = Buffer.alloc(0);
  #closePromise = null;
  #resolveClose = null;
  #rejectClose = null;
  #closeTimer = null;
  #killTimer = null;
  #closeReason = 'Connection closed; active work was asked to cancel.';
  #forced = false;
  #exitReport = null;

  constructor({ enginePath, workspace, requestTimeoutMs = 15000, shutdownGraceMs = 1000 } = {}, testHooks = {}) {
    for (const [name, value] of Object.entries({ enginePath, workspace })) {
      if (typeof value !== 'string' || value.length === 0 || value.includes('\0') || !path.isAbsolute(value)) {
        throw rejected(`${name} must be a trusted absolute path without NUL.`, 'invalid_host_config');
      }
    }
    if (!Number.isInteger(requestTimeoutMs) || requestTimeoutMs < 1 || requestTimeoutMs > 120000 ||
        !Number.isInteger(shutdownGraceMs) || shutdownGraceMs < 0 || shutdownGraceMs > 30000) {
      throw rejected('Invalid host timeout configuration.', 'invalid_host_config');
    }
    this.#options = { enginePath, workspace, requestTimeoutMs, shutdownGraceMs };
    this.#spawn = testHooks.spawnImpl ?? spawn;
    this.#verify = testHooks.verifyPaths ?? verifyPaths;
  }

  get closed() { return this.#closed; }

  // Idempotent startup handshake; a stopped/failed client is never restarted.
  start() {
    if (this.#closed) return Promise.reject(rejected('Client has already been closed.', 'connection_closed'));
    if (this.#startup) return this.#startup;
    this.#startup = this.#start();
    return this.#startup;
  }

  async #start() {
    try {
      const verified = await this.#verify(this.#options.enginePath, this.#options.workspace);
      if (this.#closed) throw rejected('Startup was cancelled before spawning.', 'connection_closed');
      const child = this.#spawn(verified.enginePath, ['--workspace', verified.workspace], {
        cwd: verified.workspace, shell: false, windowsHide: true,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      this.#child = child;
      child.on('error', (error) => this.#fail('child_failed', `control-cli process error: ${error.message}`));
      child.once('close', (code, signal) => this.#onClosed(code, signal));
      if (!child.stdin || !child.stdout || !child.stderr) {
        throw new ControlClientError('invalid_child_streams', 'control-cli did not provide the required pipes.');
      }
      child.stdout.on('data', (chunk) => this.#readStdout(chunk));
      child.stdout.on('error', (error) => this.#fail('stdout_failed', `Reading control-cli stdout failed: ${error.message}`));
      child.stderr.on('data', (chunk) => this.#readStderr(chunk));
      child.stderr.on('error', () => {}); // Continue draining other streams; process exit still settles pending RPCs.
      child.stdin.on('error', (error) => {
        if (!this.#closed) this.#fail('stdin_failed', `Writing control-cli stdin failed: ${error.message}`);
      });
      child.stdout.on('end', () => {
        if (!this.#closed) this.#fail('connection_closed', 'control-cli stdout ended; pending task outcomes may be unknown.');
      });
      await new Promise((resolve, reject) => {
        const onSpawn = () => { cleanup(); this.#spawned = true; resolve(); };
        const onError = (error) => { cleanup(); reject(error); };
        const onClose = () => { cleanup(); reject(new Error('control-cli exited before startup.')); };
        const cleanup = () => {
          child.off('spawn', onSpawn); child.off('error', onError); child.off('close', onClose);
        };
        child.once('spawn', onSpawn); child.once('error', onError); child.once('close', onClose);
      });
      const capabilities = await this.request({ op: 'capabilities' });
      if (!capabilities.success) {
        await this.close('control-cli capabilities handshake failed.');
        throw new ControlClientError('handshake_failed', `Capabilities were rejected: ${JSON.stringify(capabilities.errors ?? [])}`);
      }
      return capabilities;
    } catch (error) {
      await this.close('control-cli startup failed.').catch(() => {});
      if (error instanceof ControlClientError) throw error;
      throw new ControlClientError('startup_failed', `Unable to start control-cli: ${error.message ?? error}`);
    }
  }

  request(request) {
    try {
      if (this.#closed) throw new ControlClientError('connection_closed', 'The connection is no longer usable. Do not automatically retry tasks.start.', { uncertain: true, forced: this.#forced });
      if (!this.#spawned || !this.#child) throw rejected('Call and await start() before requesting work.', 'not_started');
      if (!request || typeof request !== 'object' || Array.isArray(request) || !OPERATIONS.has(request.op)) {
        throw rejected('Unsupported operation; device discovery and arbitrary execution are not exposed.');
      }
      if (this.#pending.size >= MAX_PENDING) throw rejected('At most eight requests may be pending.', 'client_busy');
      if (!Number.isSafeInteger(this.#nextId)) throw rejected('Request IDs are exhausted for this connection.', 'request_id_exhausted');
      const id = `mcp-${this.#nextId++}`;
      const envelope = { ...request, schema_version: 1, id };
      validateJson(envelope);
      const serialized = JSON.stringify(envelope);
      if (Buffer.byteLength(serialized, 'utf8') > REQUEST_BYTES) throw rejected('Request exceeds 4 MiB including its protocol envelope.');
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          this.#fail('request_timeout', 'Control request timed out. The task may already have started; the connection is being closed to cancel work. Do not automatically retry tasks.start.');
        }, this.#options.requestTimeoutMs);
        this.#pending.set(id, { resolve, reject, timer });
        try {
          // Node preserves write order. Pending-count and request-size bounds also bound its write queue.
          this.#child.stdin.write(`${serialized}\n`, 'utf8', (error) => {
            if (error && !this.#closed) this.#fail('stdin_failed', `Unable to send control request: ${error.message}`);
          });
        } catch (error) {
          this.#fail('stdin_failed', `Unable to send control request: ${error.message}`);
        }
      });
    } catch (error) {
      return Promise.reject(error instanceof ControlClientError ? error : rejected(error.message ?? String(error)));
    }
  }

  #readStdout(chunk) {
    if (this.#closed) return; // Still consume the stream, but never dispatch stale frames after invalidation.
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    let offset = 0;
    while (offset < bytes.length) {
      const newline = bytes.indexOf(10, offset);
      const end = newline < 0 ? bytes.length : newline;
      const piece = bytes.subarray(offset, end);
      if (this.#lineBytes + piece.length > RESPONSE_BYTES) {
        this.#fail('response_too_large', 'control-cli response exceeded 8 MiB.');
        return;
      }
      if (piece.length) {
        const required = this.#lineBytes + piece.length;
        if (this.#lineBuffer.length < required) {
          let capacity = this.#lineBuffer.length || 8192;
          while (capacity < required) capacity = Math.min(RESPONSE_BYTES, capacity * 2);
          const buffer = Buffer.allocUnsafe(capacity);
          this.#lineBuffer.copy(buffer, 0, 0, this.#lineBytes);
          this.#lineBuffer = buffer;
        }
        piece.copy(this.#lineBuffer, this.#lineBytes);
        this.#lineBytes = required;
      }
      if (newline < 0) return;
      const frame = this.#lineBuffer.subarray(0, this.#lineBytes);
      this.#lineBytes = 0;
      try { this.#dispatch(frame); }
      catch (error) { this.#fail('invalid_response', `Invalid control-cli response: ${error.message}`); return; }
      offset = end + 1;
    }
  }

  #dispatch(frame) {
    // Fatal decoding prevents malformed bytes from silently changing IDs, keys or error text.
    const text = new TextDecoder('utf-8', { fatal: true }).decode(frame);
    const response = JSON.parse(text);
    if (!response || typeof response !== 'object' || Array.isArray(response) || response.schema_version !== 1 ||
        typeof response.success !== 'boolean' || typeof response.id !== 'string' || response.id.length === 0) {
      throw new Error('Missing version, success flag or response correlation ID.');
    }
    const pending = this.#pending.get(response.id);
    if (!pending) return; // Unknown or late IDs must never be matched to a different request.
    clearTimeout(pending.timer);
    this.#pending.delete(response.id);
    pending.resolve(response); // Preserve successful and failed P5 envelopes unchanged.
  }

  #readStderr(chunk) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    if (bytes.length >= STDERR_BYTES) this.#stderr = Buffer.from(bytes.subarray(bytes.length - STDERR_BYTES));
    else this.#stderr = Buffer.concat([this.#stderr.subarray(Math.max(0, this.#stderr.length + bytes.length - STDERR_BYTES)), bytes]);
  }

  #fail(code, message) {
    if (this.#closed) return;
    this.#beginClose(message, code);
  }

  #rejectPending(code, message, closing) {
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const item of pending) clearTimeout(item.timer);
    // Wait for cooperative cleanup/owned kill before reporting whether termination was forced.
    closing.then((report) => {
      for (const item of pending) item.reject(new ControlClientError(code, `${message} ${report.message}`, { uncertain: true, forced: report.forced }));
    }, (error) => {
      for (const item of pending) item.reject(new ControlClientError(code, `${message} ${error.message}`, { uncertain: true, forced: this.#forced }));
    });
  }

  close(reason = 'Connection closed; active work was asked to cancel.') {
    return this.#beginClose(typeof reason === 'string' ? reason : 'Connection closed; active work was asked to cancel.', 'connection_closed');
  }

  #beginClose(reason, code) {
    if (this.#closePromise) return this.#closePromise;
    this.#closed = true;
    this.#closeReason = reason;
    this.#lineBuffer = Buffer.alloc(0); this.#lineBytes = 0;
    this.#closePromise = new Promise((resolve, reject) => { this.#resolveClose = resolve; this.#rejectClose = reject; });
    // Shutdown can be initiated by a stream event with no caller waiting; prevent unhandled rejection.
    this.#closePromise.catch(() => {});
    this.#rejectPending(code, reason, this.#closePromise);
    if (!this.#child) {
      this.#onClosed(null, null);
      return this.#closePromise;
    }
    if (this.#exitReport) { this.#resolveClose(this.#exitReport); return this.#closePromise; }
    try {
      if (!this.#child.stdin.destroyed && !this.#child.stdin.writableEnded) this.#child.stdin.end();
    } catch { /* The owned child monitor below still enforces shutdown. */ }
    if (this.#exitReport) return this.#closePromise;
    this.#closeTimer = setTimeout(() => {
      if (this.#exitReport) return;
      this.#forced = true;
      try { this.#child.kill('SIGKILL'); } catch { /* Report if exit cannot be confirmed. */ }
      if (this.#exitReport) return;
      this.#killTimer = setTimeout(() => {
        if (!this.#exitReport) this.#rejectClose(new ControlClientError('shutdown_unconfirmed',
          'The owned child did not confirm exit after forced termination. Work may be interrupted; this client cannot restart.',
          { uncertain: true, forced: true }));
      }, 2000);
    }, this.#options.shutdownGraceMs);
    return this.#closePromise;
  }

  #onClosed(code, signal) {
    clearTimeout(this.#closeTimer);
    clearTimeout(this.#killTimer);
    const unexpected = !this.#closed;
    const detail = this.#stderr.toString('utf8').trim();
    const message = this.#forced
      ? 'Only this connection\'s owned control-cli was forcibly terminated after the shutdown grace period. Tasks may be interrupted and partial files may remain.'
      : `control-cli exited (code=${code}, signal=${signal ?? 'none'}).${detail ? ` ${detail}` : ''}`;
    this.#exitReport = { forced: this.#forced, message, code, signal };
    if (unexpected) this.#beginClose('control-cli exited unexpectedly; pending outcomes may be unknown.', 'child_exited');
    this.#resolveClose?.(this.#exitReport);
  }
}
