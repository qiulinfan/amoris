const { EventEmitter } = require('node:events');
const { spawn } = require('node:child_process');
const fs = require('node:fs/promises');
const http = require('node:http');
const path = require('node:path');

function loopbackOrigin(value) {
  try {
    const u = new URL(value);
    if (u.protocol !== 'http:' || u.hostname !== '127.0.0.1' || !u.port ||
        u.username || u.password || u.search || u.hash || u.pathname !== '/') return null;
    const port = Number(u.port);
    return Number.isInteger(port) && port >= 1 && port <= 65535 ? u.origin : null;
  } catch { return null; }
}

async function validateProject(project) {
  const root = await fs.realpath(project);
  if (!(await fs.stat(root)).isDirectory()) throw new Error('Choose a project folder.');
  for (const name of ['project.toml', 'scene.json']) {
    if (!(await fs.stat(path.join(root, name)).catch(() => null))?.isFile()) {
      throw new Error(`Choose an Amoris project containing ${name}.`);
    }
  }
  return root;
}

async function alreadyServed(project, timeoutMs = 1500) {
  let metadata;
  try { metadata = JSON.parse(await fs.readFile(path.join(project, '.pocket/host.json'), 'utf8')); }
  catch { return false; }
  const origin = loopbackOrigin(metadata.url);
  if (!origin || Number(new URL(origin).port) !== metadata.port) return false;
  return new Promise(resolve => {
    let done = false;
    let deadline;
    const finish = value => { if (!done) { done = true; clearTimeout(deadline); resolve(value); } };
    const req = http.request(origin + '/api/call', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      timeout: timeoutMs,
    }, res => {
      let bytes = 0, body = '';
      res.on('data', chunk => {
        bytes += chunk.length;
        if (bytes > 65536) { req.destroy(); finish(true); }
        else body += chunk;
      });
      res.on('end', async () => {
        try {
          const data = JSON.parse(body);
          if (res.statusCode === 200 && typeof data.result?.root === 'string') {
            finish(await fs.realpath(data.result.root) === project);
          } else finish(true); // Reachable authority cannot be verified: do not replace it.
        } catch { finish(true); }
      });
      res.on('error', () => finish(true));
    });
    deadline = setTimeout(() => { req.destroy(); finish(true); }, timeoutMs);
    req.on('timeout', () => { req.destroy(); finish(true); });
    req.on('error', error => finish(error.code !== 'ECONNREFUSED'));
    req.end(JSON.stringify({ id: 1, method: 'project.info', params: {} }));
  });
}

class HostProcess extends EventEmitter {
  constructor(resources, { spawnChild = spawn, startupMs = 30000, shutdownMs = 5000,
                          terminate = null } = {}) {
    super();
    this.resources = resources;
    this.spawnChild = spawnChild;
    this.startupMs = startupMs;
    this.shutdownMs = shutdownMs;
    this.terminate = terminate || ((child, signal) => {
      try {
        if (process.platform !== 'win32') process.kill(-child.pid, signal);
        else child.kill(signal);
      } catch (error) { if (error.code !== 'ESRCH') throw error; }
    });
    this.child = null;
    this.expectedExit = false;
    this.stderr = '';
    this.origin = null;
    this.stopPromise = null;
  }

  async start(project) {
    this.project = await validateProject(project);
    if (await alreadyServed(this.project)) {
      throw new Error('Project already open in another host. Close that host before opening it here.');
    }
    this.child = this.spawnChild(this.resources.pocket,
      ['serve', this.project, '--port', '0', '--editor', this.resources.editor], {
        cwd: this.project, windowsHide: true, detached: process.platform !== 'win32',
        env: { ...process.env, POCKET_WEB_VIEWPORT: this.resources.viewport, POCKET_TSC: this.resources.tsc },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
    const child = this.child;
    child.stderr.on('data', chunk => { this.stderr = (this.stderr + chunk).slice(-8192); });
    this.exited = new Promise(resolve => {
      const finish = (code, signal) => {
        if (this.exitResult) return;
        this.exitResult = { code, signal };
        resolve(this.exitResult);
        if (this.origin && !this.expectedExit) this.emit('host-exit', this.exitResult);
      };
      child.once('exit', finish);
      child.once('close', finish);
      child.once('error', () => finish(null, null));
    });
    try {
      const info = await new Promise((resolve, reject) => {
        let buffer = '', complete = false;
        const finish = (error, data) => {
          if (complete) return;
          complete = true;
          clearTimeout(timer);
          child.stdout.removeListener('data', read);
          child.removeListener('error', failed);
          child.removeListener('exit', earlyExit);
          error ? reject(error) : resolve(data);
        };
        const failed = error => finish(new Error(`Could not start the engine: ${error.code || 'spawn failed'}`));
        const earlyExit = code => finish(new Error(`Engine exited before opening the project (${code}).\n${this.stderr.trim()}`));
        const read = chunk => {
          buffer += chunk;
          if (buffer.length > 65536) return finish(new Error('Engine readiness response is too large.'));
          const newline = buffer.indexOf('\n');
          if (newline < 0) return;
          try {
            const data = JSON.parse(buffer.slice(0, newline));
            const origin = loopbackOrigin(data.url);
            if (!origin || data.pid !== child.pid || Number(new URL(origin).port) !== data.port ||
                data.project !== this.project) throw new Error('Invalid readiness identity');
            finish(null, { ...data, origin });
          } catch { finish(new Error('Engine returned an invalid readiness response.')); }
        };
        const timer = setTimeout(() => finish(new Error('The engine did not become ready in time.')), this.startupMs);
        child.once('error', failed);
        child.once('exit', earlyExit);
        child.stdout.on('data', read);
      });
      this.origin = info.origin;
      return info;
    } catch (error) {
      await this.stop();
      throw error;
    }
  }

  stop() {
    if (this.stopPromise) return this.stopPromise;
    this.expectedExit = true;
    this.stopPromise = (async () => {
      const child = this.child;
      if (!child?.pid) return;
      if (!this.exitResult) {
        this.terminate(child, 'SIGTERM');
        let timer;
        const forced = new Promise(resolve => {
          timer = setTimeout(() => { this.terminate(child, 'SIGKILL'); resolve(); }, this.shutdownMs);
        });
        await Promise.race([this.exited, forced]);
        clearTimeout(timer);
        // Wait for the owned process after force-kill; never consult or kill a metadata PID.
        if (!this.exitResult) await this.exited;
      }
      try {
        const file = path.join(this.project, '.pocket/host.json');
        const metadata = JSON.parse(await fs.readFile(file, 'utf8'));
        if (metadata.pid === child.pid) await fs.unlink(file);
      } catch {}
    })();
    return this.stopPromise;
  }
}

module.exports = { HostProcess, loopbackOrigin, validateProject, alreadyServed };
