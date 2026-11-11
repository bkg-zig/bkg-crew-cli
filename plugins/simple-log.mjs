#!/usr/bin/env node
import readline from 'node:readline';
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on('line', (line) => {
  let msg; try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'init') console.error('[simple-log] init', JSON.stringify(msg.server));
  else if (msg.type === 'event' && (msg.name === 'request.end' || msg.name === 'usage.recorded')) console.error('[simple-log]', msg.name, JSON.stringify(msg.event));
});
