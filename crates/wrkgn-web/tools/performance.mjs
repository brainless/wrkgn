#!/usr/bin/env node
// Maintainer-run HTTP benchmark. Run against optimized, local binaries only.
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import path from 'node:path';
const [binary = 'target/release/wrkgn-web', full, synthetic, rawSamples = '1000'] = process.argv.slice(2);
if (!full || !synthetic) throw new Error('usage: performance.mjs <server-binary> <full-snapshot-root> <100k-snapshot-root> [samples=1000]');
const samples = Number(rawSamples);
if (!Number.isSafeInteger(samples) || samples < 100) throw new Error('samples must be an integer >=100');
const origin = 'http://127.0.0.1:3099';
const rss = pid => Number(execFileSync('ps', ['-o', 'rss=', '-p', String(pid)], { encoding: 'utf8' }).trim());
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const reports = [];
for (const [label, root] of [['full', full], ['synthetic100k', synthetic]]) {
  const child = spawn(path.resolve(binary), ['--snapshot', path.resolve(root), '--bind', '127.0.0.1:3099'], { stdio: ['ignore', 'ignore', 'inherit'] });
  let peak = 0;
  let timer;
  try {
    let health;
    for (let n = 0; n < 1200; n++) {
      if (child.exitCode !== null) throw new Error('Server exited during startup');
      try { const response = await fetch(`${origin}/healthz`); if (response.ok) { health = await response.json(); break; } } catch {}
      await pause(100);
    }
    if (!health) throw new Error('Server startup exceeded 120 seconds');
    const paths = ['/', '/?q=a', '/?q=caf%C3%A9', '/?q=xy', '/?q=no-such-result', '/?sort=comments-desc', '/?sort=comments-asc', '/?sort=date-asc'];
    for (let n = 0; n < 40; n++) await (await fetch(origin + paths[n % paths.length])).arrayBuffer();
    const baseline = rss(child.pid);
    const latencies = [];
    for (let n = 0; n < samples; n++) {
      const start = performance.now();
      const response = await fetch(origin + paths[n % paths.length]);
      if (response.status !== 200) throw new Error(`Unexpected HTTP ${response.status}`);
      await response.arrayBuffer();
      latencies.push(performance.now() - start);
    }
    latencies.sort((a, b) => a - b);
    const percentile = p => latencies[Math.min(samples - 1, Math.ceil(samples * p) - 1)];
    let reloadError = null;
    timer = setInterval(() => { try { peak = Math.max(peak, rss(child.pid)); } catch {} }, 25);
    // Reload the same immutable snapshot to exercise simultaneous old/new loads.
    child.kill('SIGHUP');
    for (let n = 0; n < 100; n++) {
      const response = await fetch(origin + paths[n % paths.length]);
      if (response.status !== 200) reloadError = `HTTP ${response.status}`;
      await response.arrayBuffer();
      await pause(10);
    }
    clearInterval(timer);
    const report = { label, snapshot: health, samples, p50_ms: percentile(.5), p95_ms: percentile(.95), p99_ms: percentile(.99), budget_ms: 50, budget_met: percentile(.99) < 50, resident_kib: baseline, sampled_reload_peak_kib: Math.max(baseline, peak), reload_error: reloadError, note: 'Loopback end-to-end latency, sequential warm requests; sampled RSS can miss peaks. Run memory_probe for exactly two retained snapshots.' };
    reports.push(report);
    console.log(JSON.stringify(report));
  } finally {
    clearInterval(timer);
    if (child.exitCode === null) { child.kill('SIGTERM'); await once(child, 'exit'); }
  }
}
if (reports.some(r => !r.budget_met || r.reload_error)) process.exitCode = 1;
