/// One-command desktop harness (issue #7): builds and launches a debug
/// window seeded with fixtures, then drives it with the `tauri-pilot`
/// CLI. See `harness/desktop/README.md` for the manual step-by-step flow
/// an agent should use to debug live UI work instead.
///
/// Requires the CLI once: `cargo install tauri-pilot-cli`.
import { execFile, spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import path from 'node:path';
import process from 'node:process';

const ROOT = path.resolve(import.meta.dirname, '../..');
const SCENARIO = 'harness/desktop/scenarios/desktop-sweep.toml';
const ARTIFACTS = 'test-results/desktop-harness';
const PING_TIMEOUT_MS = 5 * 60_000;

function run(cmd, args, options = {}) {
  return new Promise((resolve, reject) => {
    execFile(cmd, args, { cwd: ROOT, ...options }, (error, stdout, stderr) => {
      if (error) {
        error.stdout = stdout;
        error.stderr = stderr;
        reject(error);
      } else {
        resolve(stdout + stderr);
      }
    });
  });
}

const delay = (ms) => new Promise((r) => setTimeout(r, ms));

/// Keep enough of the app output to explain a failed launch; every
/// diagnostic a stale terminal would show lives in `tail` (last 60 lines).
function launchApp() {
  // Command as one string: shell:true + args triggers Node's DEP0190
  // warning, and on Windows `yarn` is a .cmd only reachable via shell.
  const child = spawn('yarn tauri dev --features pilot', {
    cwd: ROOT,
    shell: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const tail = [];
  for (const stream of [child.stdout, child.stderr]) {
    stream.on('data', (chunk) => {
      for (const line of chunk.toString().split('\n')) {
        if (line.trim()) tail.push(line.trim());
        if (tail.length > 60) tail.shift();
      }
    });
  }
  return { child, tail };
}

function killTree(child) {
  if (!child.pid || child.killed) return Promise.resolve();
  return new Promise((resolve) => {
    execFile('taskkill', ['/PID', String(child.pid), '/T', '/F'], () => {
      child.kill();
      setTimeout(resolve, 1_500);
    });
  });
}

async function main() {
  // Fast-fail on a missing CLI before building anything.
  try {
    await run('tauri-pilot', ['--help']);
  } catch (error) {
    if (error.code === 'ENOENT') {
      console.error(
        'tauri-pilot CLI not found. Install it once with:\n  cargo install tauri-pilot-cli'
      );
      process.exit(1);
    }
    throw error;
  }

  console.log('launching the dev build (tauri dev, pilot plugin)…');
  const app = launchApp();
  let scenarioOk = false;

  try {
    // ping still succeeds while the webview sits on about:blank — window
    // creation precedes the devUrl load, and both commands precede a
    // still-compiling app. Poll them together under one deadline; IPC is
    // origin-pinned, so don't seed until the page is on the app URL.
    const deadline = Date.now() + PING_TIMEOUT_MS;
    for (;;) {
      if (app.child.exitCode !== null) {
        throw new Error(`app exited (${app.child.exitCode}):\n${app.tail.join('\n')}`);
      }
      let pinged = false;
      try {
        await run('tauri-pilot', ['ping']);
        pinged = true;
      } catch (error) {
        if (Date.now() > deadline) {
          throw new Error(`no tauri-pilot socket appeared:\n${app.tail.join('\n')}`);
        }
      }
      if (pinged) {
        try {
          const url = await run('tauri-pilot', ['url']);
          if (url.includes('http://localhost:1420')) {
            console.log('app page loaded');
            break;
          }
        } catch {
          // ping passes during the race; url is the real readiness probe
        }
      }
      await delay(1_000);
    }

    // The seed can still land while the page's bridge hello is in flight
    // ("RPC error (-32603): command was pinned to about:blank…"); retry
    // on that one error shape before giving up.
    mkdirSync(path.join(ROOT, ARTIFACTS), { recursive: true });

    console.log('seeding fixtures via the test IPC surface…');
    for (let attempt = 1; ; attempt += 1) {
      try {
        await run('tauri-pilot', ['ipc', 'harness_seed']);
        break;
      } catch (error) {
        if (attempt >= 5 || !String(error).includes('pinned to about:blank')) {
          throw error;
        }
        await delay(2_000);
      }
    }

    await delay(1_000); // let the frontend drain the pending buffer

    console.log(`running scenario ${SCENARIO}…`);
    const result = await run('tauri-pilot', [
      'run',
      SCENARIO,
      '--screenshots-dir',
      `${ARTIFACTS}/failures`,
    ]);
    console.log(result);
    scenarioOk = true;

    // Debug aid: console errors from the driven session.
    try {
      const logs = await run('tauri-pilot', ['logs', '--level', 'error']);
      if (logs.trim()) {
        console.log(`webview error logs:\n${logs}`);
      }
    } catch {
      console.log('could not read error logs (session already torn down)');
    }
  } catch (error) {
    console.error(`\nharness failed:\n${error.message ?? error}`);
    if (error.stdout?.trim()) console.error(`stdout: ${error.stdout}`);
    if (error.stderr?.trim()) console.error(`stderr: ${error.stderr}`);
  } finally {
    await killTree(app.child);
  }

  if (!scenarioOk) process.exitCode = 1;
}

(() => main())();
