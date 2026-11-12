#!/usr/bin/env node
// bkgcrewcli gateway/run event plugin (NDJSON stdin protocol)
// Writes: inflight-<port>-<pid>.json and timings-YYYY-MM.jsonl into BKGCREWCLI_MONITOR_DIR (or cwd)
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';

const outDir = process.env.BKGCREWCLI_MONITOR_DIR || process.cwd();
fs.mkdirSync(outDir, { recursive: true });
let server = { port: 0, pid: process.pid, component: 'bkgcrewcli' };
const inflight = new Map(); // id -> info

function inflightPath() { return path.join(outDir, `inflight-${server.port}-${server.pid}.json`); }
function timingsPath() {
  const d = new Date();
  const ym = `${d.getUTCFullYear()}-${String(d.getUTCMonth()+1).padStart(2,'0')}`;
  return path.join(outDir, `timings-${ym}.jsonl`);
}
function flushInflight() {
  const snap = { pid: server.pid, port: String(server.port), updated: Math.floor(Date.now()/1000), requests: {} };
  for (const [id, v] of inflight.entries()) snap.requests[id] = v;
  const p = inflightPath(), tmp = p + '.tmp';
  try { fs.writeFileSync(tmp, JSON.stringify(snap)); fs.renameSync(tmp, p); } catch {}
}
function logTiming(ev) {
  const row = {
    t: Math.floor((ev.started_at || Date.now())/1000),
    port: String(ev.port ?? server.port),
    model: ev.model ?? null,
    actual_model: ev.actual_model ?? null,
    user_agent: ev.user_agent ?? '',
    session_id: ev.session_id ?? null,
    agent: ev.agent ?? null,
    ms: ev.duration_ms ?? null,
    ttft_ms: ev.first_token_ms ?? null,
    status: ev.status ?? null,
    outcome: ev.outcome ?? null,
    id: ev.id,
  };
  try { fs.appendFileSync(timingsPath(), JSON.stringify(row) + '\n'); } catch {}
}

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'init') { server = msg.server || server; }
  else if (msg.type === 'event') {
    const e = msg.event || {};
    switch (msg.name) {
      case 'request.start':
        inflight.set(e.id, { t: Math.floor((e.started_at||Date.now())/1000), model: e.model, actual_model: e.actual_model, user_agent: e.user_agent, session_id: e.session_id, agent: e.agent, ttft_ms: null });
        flushInflight(); break;
      case 'request.model':
        if (inflight.has(e.id)) { inflight.get(e.id).model = e.model; flushInflight(); } break;
      case 'request.actual_model':
        if (inflight.has(e.id)) { inflight.get(e.id).actual_model = e.actual_model; flushInflight(); } break;
      case 'request.session':
        if (inflight.has(e.id)) { inflight.get(e.id).session_id = e.session_id; flushInflight(); } break;
      case 'request.first_token':
        if (inflight.has(e.id)) { inflight.get(e.id).ttft_ms = e.first_token_ms; flushInflight(); } break;
      case 'request.end':
        inflight.delete(e.id); flushInflight(); logTiming(e); break;
      case 'usage.recorded':
        // optional: could write separate usage log
        break;
    }
  } else if (msg.type === 'shutdown') {
    try { fs.unlinkSync(inflightPath()); } catch {}
    process.exit(0);
  }
});
process.on('SIGTERM', () => { try { fs.unlinkSync(inflightPath()); } catch {} process.exit(0); });
