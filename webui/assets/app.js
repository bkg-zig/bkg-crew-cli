/* ════════════════════════════════════════════════════════════════════
   bkgcrewcli WebUI — application logic (native, no frameworks).
   Terminal console for the gateway: dashboard, agents, run, metrics,
   usage, crew tokens, gateway config, providers, model mappings,
   settings, sandbox and doctor — everything rendered as neon terminal
   components, never as raw JSON.
   ════════════════════════════════════════════════════════════════════ */
(() => {
  'use strict';

  const TOKEN_KEY = 'bkg_admin_token';

  const S = {
    token: sessionStorage.getItem(TOKEN_KEY) || '',
    unlocked: false,
    page: 'dashboard',
    runBusy: false,
  };

  /* ── helpers ─────────────────────────────────────────────────────── */
  const $ = (sel) => document.querySelector(sel);

  const esc = (v) => String(v ?? '')
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');

  const fmtInt = (n) => {
    if (n == null) return '–';
    const v = Number(n);
    if (!Number.isFinite(v)) return String(n);
    return v.toLocaleString('de-DE');
  };
  const fmtCost = (n) => {
    const v = Number(n) || 0;
    return `$${v.toFixed(v >= 100 ? 2 : 4)}`;
  };
  const fmtTime = (ms) => {
    if (!ms) return '–';
    const d = new Date(Number(ms));
    return d.toLocaleString('de-DE', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' });
  };
  const fmtDur = (ms) => {
    const v = Number(ms) || 0;
    if (!v) return '–';
    if (v < 1000) return `${v} ms`;
    if (v < 60000) return `${(v / 1000).toFixed(1)} s`;
    return `${Math.floor(v / 60000)}m ${Math.round((v % 60000) / 1000)}s`;
  };
  const shortModel = (m) => (m ? String(m).replace(/^(anthropic|openai|google)\//, '') : '–');

  async function api(path, opts = {}) {
    const headers = Object.assign({ 'content-type': 'application/json' }, opts.headers || {});
    if (S.token) headers['x-api-key'] = S.token;
    const res = await fetch(path, { ...opts, headers });
    if (res.status === 401) {
      const err = new Error('unauthorized');
      err.auth = true;
      throw err;
    }
    if (!res.ok) {
      let detail = '';
      try { detail = (await res.json())?.error?.message || (await res.json())?.error || ''; } catch { /* ignore */ }
      const err = new Error(`HTTP ${res.status}${detail ? `: ${detail}` : ''}`);
      throw err;
    }
    const type = res.headers.get('content-type') || '';
    return type.includes('json') ? res.json() : res.text();
  }

  function toast(msg, ac = 'mint') {
    const host = $('#toasts');
    const t = document.createElement('div');
    t.className = `ac-${ac} toast`;
    t.innerHTML = `<span class="neon">→</span> ${esc(msg)}`;
    host.appendChild(t);
    setTimeout(() => { t.style.opacity = '0'; t.style.transition = 'opacity .3s'; }, 2600);
    setTimeout(() => t.remove(), 3000);
  }

  /* ── navigation config ───────────────────────────────────────────── */
  const NAV = [
    { sec: 'Crew', items: [
      { id: 'dashboard', ic: '◈', label: 'Übersicht', ac: 'cyan' },
      { id: 'agents', ic: '⬡', label: 'Agents', ac: 'mint' },
      { id: 'run', ic: '▶', label: 'Run', ac: 'lime' },
    ]},
    { sec: 'Daten', items: [
      { id: 'metrics', ic: '▤', label: 'Metrics', ac: 'violet' },
      { id: 'usage', ic: '≡', label: 'Usage', ac: 'blue' },
      { id: 'tokens', ic: '◎', label: 'Tokens', ac: 'magenta', admin: true },
    ]},
    { sec: 'Gateway', items: [
      { id: 'gateway', ic: '⌁', label: 'Gateway', ac: 'orange', admin: true },
      { id: 'providers', ic: '⛁', label: 'Providers', ac: 'gold', admin: true },
      { id: 'mappings', ic: '⇄', label: 'Mappings', ac: 'cyan', admin: true },
    ]},
    { sec: 'Admin', items: [
      { id: 'settings', ic: '⚙', label: 'Einstellungen', ac: 'violet', admin: true },
      { id: 'sandbox', ic: '▣', label: 'Sandbox', ac: 'orange', admin: true },
      { id: 'doctor', ic: '✚', label: 'Doctor', ac: 'lime', admin: true },
    ]},
  ];
  const findItem = (id) => NAV.flatMap((s) => s.items).find((i) => i.id === id) || NAV[0].items[0];

  /* ── component builders ──────────────────────────────────────────── */
  const win = ({ title, ac = 'cyan', prompt, body }) => `
    <section class="ac-${ac} win">
      <div class="win-bar">
        <span class="dot d-r"></span><span class="dot d-y"></span><span class="dot d-g"></span>
        <span class="win-title">${esc(title)}</span>
      </div>
      <div class="win-body">
        ${prompt ? `<div class="tl cmd"><span class="p">$</span> ${esc(prompt)}<i class="cursor"></i></div>` : ''}
        ${body}
      </div>
    </section>`;

  const statsGrid = (items) => `
    <div class="stats">
      ${items.map((s) => `
        <div class="ac-${s.ac} stat">
          <div class="stat-label">${esc(s.label)}</div>
          <div class="stat-value">${esc(s.value)}</div>
          <div class="stat-sub">${esc(s.sub || '')}</div>
        </div>`).join('')}
    </div>`;

  const pill = (text, ac, withDot = true) =>
    `<span class="ac-${ac} pill">${withDot ? '<i></i>' : ''}${esc(text)}</span>`;

  const sheet = (headers, rows) => {
    if (!rows.length) return '<div class="empty">– keine Einträge –</div>';
    return `
      <div class="sheet-wrap">
        <table class="sheet">
          <thead><tr>${headers.map((h) => `<th>${esc(h)}</th>`).join('')}</tr></thead>
          <tbody>${rows.map((r) => `<tr>${r.map((c) => {
            const cls = typeof c === 'object' ? (c.cls || '') : '';
            const html = typeof c === 'object' ? c.html : c;
            return `<td class="${cls}">${html}</td>`;
          }).join('')}</tr>`).join('')}</tbody>
        </table>
      </div>`;
  };

  const chart = (rows, ac, fmt = fmtInt) => {
    if (!rows.length) return '<div class="empty">– keine Daten –</div>';
    const max = Math.max(...rows.map((r) => r.value), 1);
    return `
      <div class="ac-${ac} chart">
        ${rows.map((r) => `
          <div class="chart-row">
            <div class="lbl" title="${esc(r.label)}">${esc(r.label)}${r.sub ? ` <small>${esc(r.sub)}</small>` : ''}</div>
            <div class="track"><div class="fill" style="width:${Math.max(2, Math.round((r.value / max) * 100))}%"></div></div>
            <div class="val">${esc(fmt(r.value))}</div>
          </div>`).join('')}
      </div>`;
  };

  const field = ({ label, name, value = '', type = 'text', wide = false, options = null, ph = '', min = null, step = null, rows = null }) => {
    const id = `f_${name.replace(/[^\w]/g, '_')}`;
    let input;
    if (type === 'select' && options) {
      input = `<select id="${id}" data-name="${esc(name)}">${options.map((o) => {
        const [v, l] = Array.isArray(o) ? o : [o, o];
        return `<option value="${esc(v)}" ${String(v) === String(value) ? 'selected' : ''}>${esc(l)}</option>`;
      }).join('')}</select>`;
    } else if (type === 'textarea') {
      input = `<textarea id="${id}" data-name="${esc(name)}" rows="${rows || 3}" placeholder="${esc(ph)}">${esc(value)}</textarea>`;
    } else {
      const attrs = [
        type === 'checkbox' ? (value ? 'checked' : '') : '',
        `data-name="${esc(name)}"`,
        type !== 'checkbox' ? `value="${esc(value)}"` : '',
        ph ? `placeholder="${esc(ph)}"` : '',
        min != null ? `min="${min}"` : '',
        step != null ? `step="${step}"` : '',
      ].join(' ');
      input = `<input id="${id}" type="${type === 'pw' ? 'password' : type}" ${attrs} autocomplete="off">`;
    }
    return `
      <div class="field ${wide ? 'wide' : ''}">
        ${type === 'checkbox'
          ? `<label class="check" style="--ac:inherit"><input type="checkbox" ${attrs}> ${esc(label)}</label>`
          : `<label for="${id}">${esc(label)}</label>${input}`}
      </div>`;
  };

  const check = ({ label, name, value }) => `
    <label class="check"><input type="checkbox" data-name="${esc(name)}" ${value ? 'checked' : ''}> ${esc(label)}</label>`;

  const collect = (root) => {
    const out = {};
    root.querySelectorAll('[data-name]').forEach((el) => {
      out[el.dataset.name] = el.type === 'checkbox' ? el.checked : el.value;
    });
    return out;
  };

  const empty = (msg = '– keine Daten –') => `<div class="empty">${esc(msg)}</div>`;
  const skel = () => '<div class="skel">lade …</div>';

  /* ── shell rendering ─────────────────────────────────────────────── */
  function renderRail() {
    const it = findItem(S.page);
    $('#railDots').innerHTML = NAV.flatMap((s) => s.items).map((i) => `
      <div class="ac-${i.ac} rail-dot ${i.id === S.page ? 'on' : ''}" data-page="${i.id}" title="${esc(i.label)}">${i.ic}</div>
    `).join('');
    $('#railDots').querySelectorAll('[data-page]').forEach((el) => {
      el.addEventListener('click', () => go(el.dataset.page));
    });
  }

  function renderSidebar() {
    $('#sidebarNav').innerHTML = NAV.map((s) => `
      <div class="ac-violet sec-title">${esc(s.sec)}</div>
      ${s.items.map((i) => `
        <div class="ac-${i.ac} nav-item ${i.id === S.page ? 'on' : ''}" data-page="${i.id}">
          <span class="ic">${i.ic}</span> <span class="lbl">${esc(i.label)}</span>
          ${i.admin && !S.unlocked ? '<span class="lock">🔒</span>' : ''}
        </div>`).join('')}
    `).join('');
    $('#sidebarNav').querySelectorAll('[data-page]').forEach((el) => {
      el.addEventListener('click', () => { go(el.dataset.page); closeSidebar(); });
    });
    // about accordion
    const aboutToggle = $('#aboutToggle');
    const aboutPanel = $('#aboutPanel');
    aboutToggle.addEventListener('click', () => {
      const open = aboutPanel.style.display === 'block';
      aboutPanel.style.display = open ? 'none' : 'block';
      aboutToggle.classList.toggle('on', !open);
      aboutToggle.style.borderColor = open ? '' : 'rgba(255, 210, 77, .45)';
      aboutToggle.style.color = open ? '' : 'var(--gold)';
      aboutToggle.style.background = open ? '' : 'rgba(255, 210, 77, .06)';
    });

    // sidebar collapse (icons stay, labels hide)
    const sideToggle = $('#sideCollapse');
    const sidebarEl = $('#sidebar');
    sideToggle.addEventListener('click', () => {
      const collapsed = sidebarEl.classList.toggle('collapsed');
      sideToggle.classList.toggle('on', collapsed);
      sideToggle.style.borderColor = collapsed ? 'rgba(255, 210, 77, .45)' : '';
      sideToggle.style.color = collapsed ? 'var(--gold)' : '';
      sideToggle.style.background = collapsed ? 'rgba(255, 210, 77, .06)' : '';
      sideToggle.textContent = collapsed ? '▤' : '❐';
    });
  }

  function renderTopbar() {
    const it = findItem(S.page);
    $('#crumb').innerHTML = `<span class="path ac-cyan">bkgcrewcli</span><span class="path">/${esc(it.label)}</span>`;
    $('#tokenInput').value = S.token;
    $('#healthPill').innerHTML = '<i></i><span class="txt">live</span>';
  }

  function renderPageHead(title, sub, ac, actions = '') {
    return `
      <div class="ac-${ac} page-head">
        <h1>${esc(title)}</h1>
        <span class="sub">${esc(sub)}</span>
        <span class="fill"></span>${actions}
      </div>`;
  }

  function closeSidebar() { $('#sidebar').classList.remove('open'); $('#scrim').classList.remove('show'); }

  /* ── admin token flow ────────────────────────────────────────────── */
  async function unlock(token) {
    const prev = S.token;
    S.token = token;
    try {
      await api('/admin/app-config'); // admin-gated probe
      S.unlocked = true;
      sessionStorage.setItem(TOKEN_KEY, token);
      toast('Admin-Token akzeptiert', 'gold');
      renderSidebar();
      go(S.page);
      return true;
    } catch (e) {
      S.token = prev;
      if (e.auth) toast('Admin-Token ungültig (401)', 'red');
      else toast(`Gateway nicht erreichbar: ${e.message}`, 'red');
      return false;
    }
  }

  function requireAdmin() {
    if (S.unlocked) return true;
    toast('Diesen Bereich freischalten: Admin-Token oben eingeben', 'orange');
    $('#tokenInput').focus();
    return false;
  }

  /* ── pages ───────────────────────────────────────────────────────── */
  const pages = {};

  pages.dashboard = async (host) => {
    host.innerHTML = renderPageHead('Übersicht', 'Gateway-Dashboard', 'cyan') + skel();
    const [agents, metrics] = await Promise.all([
      api('/api/agents').catch(() => null),
      api('/api/metrics').catch(() => null),
    ]);
    let admin = null;
    if (S.unlocked) {
      admin = await api('/admin/doctor').catch(() => null);
    }
    const ags = agents?.agents || [];
    const avail = ags.filter((a) => a.available).length;
    const t = metrics?.totals || {};
    const cards = [
      { label: 'Agents', value: `${avail}/${ags.length}`, sub: 'verfügbar', ac: 'mint' },
      { label: 'Requests', value: fmtInt(t.requests ?? 0), sub: 'gateway total', ac: 'cyan' },
      { label: 'Tokens', value: fmtInt(t.total_tokens ?? 0), sub: 'in/out gesamt', ac: 'violet' },
      { label: 'Kosten', value: fmtCost(t.cost_usd ?? 0), sub: 'geschätzt', ac: 'gold' },
    ];
    if (S.unlocked) {
      cards.push({ label: 'Crew-Tokens', value: fmtInt(admin?.checks?.find((c) => c.name === 'tokens')?.detail?.count ?? '–'), sub: 'aktiv angelegt', ac: 'magenta' });
      cards.push({ label: 'Version', value: `v${admin?.version || '–'}`, sub: 'bkgcrewcli', ac: 'orange' });
    }
    const provRows = (metrics?.by_provider || []).map((p) => ({
      label: p.provider || 'unknown', value: p.requests || 0, sub: '',
    }));
    host.innerHTML = `
      ${renderPageHead('Übersicht', 'Gateway-Dashboard', 'cyan', pill('gateway live', 'mint'))}
      ${statsGrid(cards)}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://dashboard', ac: 'cyan', prompt: 'bkgcrewcli status --all',
        body: `
          <h3>Provider-Requests</h3>
          ${chart(provRows.slice(0, 6), 'cyan')}
          <h3>Schnellzugriff</h3>
          <div class="tl out">→ <a href="#" data-page="run">Task starten</a> · <a href="#" data-page="metrics">Metrics</a> · <a href="#" data-page="agents">Agents prüfen</a>${S.unlocked ? ' · <a href="#" data-page="doctor">Doctor</a>' : ' · <span class="tl note">Admin-Token für Gateway/Doctor nötig</span>'}</div>
          <div class="tl note"># alles weitere über die Navigation links</div>`,
      })}
      <div id="toastsBelow"></div>`;
    host.querySelectorAll('[data-page]').forEach((el) => el.addEventListener('click', (ev) => { ev.preventDefault(); go(el.dataset.page); }));
  };

  pages.agents = async (host) => {
    host.innerHTML = renderPageHead('Agents', 'verfügbare Coding-Agents', 'mint') + skel();
    const [agents, doctor] = await Promise.all([
      api('/api/agents'),
      S.unlocked ? api('/admin/doctor').catch(() => null) : Promise.resolve(null),
    ]);
    const docAgents = doctor?.checks?.find((c) => c.name === 'agents')?.detail || [];
    const rows = agents.agents.map((a, idx) => {
      const d = docAgents.find((x) => x.id === a.id) || {};
      return [
        { cls: 'td-idx', html: String(idx + 1).padStart(2, '0') },
        { cls: 'ac-mint td-id', html: esc(a.id) },
        esc(a.name),
        { html: pill(a.available ? 'verfügbar' : 'fehlt', a.available ? 'mint' : 'red') },
        { cls: 'td-mut', html: esc(a.version || '–') },
        d.model ? { cls: 'td-own', html: esc(shortModel(d.model)) } : { cls: 'td-mut', html: '–' },
        d.baseUrl ? { cls: 'td-mut', html: esc(d.baseUrl) } : { cls: 'td-mut', html: '–' },
        d.apiKeyConfigured ? pill('key ✓', 'gold', false) : '<span class="empty">–</span>',
      ];
    });
    const heads = ['#', 'Agent', 'Name', 'Status', 'Version', 'Modell', 'Base URL', 'API-Key'];
    host.innerHTML = `
      ${renderPageHead('Agents', 'verfügbare Coding-Agents', 'mint', pill(`${agents.agents.filter((a) => a.available).length} aktiv`, 'mint'))}
      ${win({
        title: 'gateway://agents', ac: 'mint', prompt: 'bkgcrewcli agents --status',
        body: sheet(heads, rows),
      })}
      <div class="tl note"># model/base-url/key stammen aus den Einstellungen (Admin)</div>`;
  };

  pages.run = async (host) => {
    const agents = await api('/api/agents').catch(() => ({ agents: [] }));
    const opts = agents.agents.filter((a) => a.available).map((a) => [a.id, a.name]);
    host.innerHTML = `
      ${renderPageHead('Run', 'Task an einen Agent geben', 'lime')}
      ${win({
        title: 'gateway://run', ac: 'lime', prompt: 'bkgcrewcli run --agent … --prompt …',
        body: `
          <div class="form-grid">
            ${field({ label: 'Agent', name: 'agent', type: 'select', options: opts.length ? opts : [['', 'keiner verfügbar']] })}
            ${field({ label: 'Modell (optional)', name: 'model', ph: 'z.B. anthropic/claude-sonnet-4-5' })}
            ${field({ label: 'Arbeitsverzeichnis (optional)', name: 'cwd', ph: '/srv/bkg/workspace/projekt' })}
            ${field({ label: 'Permissions', name: 'permissions', type: 'select', options: [['default', 'default'], ['acceptEdits', 'accept edits'], ['bypassPermissions', 'bypass'], ['plan', 'plan']] })}
            <div class="field wide">${field({ label: 'Prompt', name: 'prompt', type: 'textarea', rows: 5, ph: 'Was soll der Agent tun?' })}</div>
          </div>
          <div class="btn-row">
            <button id="runBtn" class="btn solid">▶ ausführen</button>
            <span class="hint">läuft synchron, Ausgabe erscheint darunter</span>
          </div>
          <div id="runOut" style="margin-top:12px"></div>`,
      })}
      ${win({
        title: 'gateway://run/history', ac: 'cyan', prompt: 'tail run.log',
        body: '<div class="tl note"># noch keine Ausführung in dieser Session</div>',
      })}`;
    $('#runBtn').addEventListener('click', async () => {
      if (S.runBusy) return;
      const form = collect($('#runOut').closest('.win-body'));
      const prompt = String(form.prompt || '').trim();
      if (!prompt) { toast('Prompt fehlt', 'orange'); return; }
      S.runBusy = true;
      const btn = $('#runBtn'); btn.disabled = true; btn.textContent = 'läuft …';
      const outHost = $('#runOut');
      outHost.innerHTML = '<div class="tl cmd"><span class="p">$</span> starte …<i class="cursor"></i></div>';
      try {
        const r = await api('/api/run', { method: 'POST', body: JSON.stringify({
          agent: form.agent || null,
          model: form.model || null,
          cwd: form.cwd || null,
          permissions: form.permissions || null,
          prompt,
        }) });
        const lines = String(r.output || '').split('\n').slice(-200).map((l) =>
          `<div class="tl out">→ ${esc(l)}</div>`).join('');
        outHost.innerHTML = `
          ${pill(`exit ${r.exit_code}`, r.exit_code === 0 ? 'mint' : 'red')}
          ${r.error ? `<div class="tl err">✗ ${esc(r.error)}</div>` : ''}
          ${r.events?.length ? pill(`${r.events.length} events`, 'violet', false) : ''}
          <div style="margin-top:8px">${lines || '<div class="tl note"># keine Ausgabe</div>'}</div>`;
        toast(r.exit_code === 0 ? 'Task fertig' : `Task beendet (exit ${r.exit_code})`, r.exit_code === 0 ? 'mint' : 'orange');
      } catch (e) {
        outHost.innerHTML = `<div class="tl err">✗ ${esc(e.message)}</div>`;
        toast(`Run fehlgeschlagen: ${e.message}`, 'red');
      } finally {
        S.runBusy = false; btn.disabled = false; btn.textContent = '▶ ausführen';
      }
    });
  };

  pages.metrics = async (host) => {
    host.innerHTML = renderPageHead('Metrics', 'Gateway-Nutzung nach Provider/Modell', 'violet') + skel();
    const m = await api('/api/metrics');
    const t = m.totals || {};
    const provRows = (m.by_provider || []).map((p) => ({ label: p.provider || 'unknown', value: p.requests || 0 }));
    const modelRows = (m.by_model || []).map((x) => ({ label: shortModel(x.model), value: x.total_tokens || 0, sub: `${fmtInt(x.requests)} req` }));
    const outcome = m.by_outcome || [];
    const outcomePills = outcome.length
      ? outcome.map((o) => pill(`${o.outcome || '?'}: ${fmtInt(o.requests)}`, (o.outcome || '').includes('ok') || (o.outcome || '').includes('success') ? 'mint' : 'orange', false)).join(' ')
      : empty();
    const provTable = sheet(['Provider', 'Requests', 'Tokens', 'Kosten'], (m.by_provider || []).map((p, i) => [
      { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
      { cls: 'ac-violet td-id', html: esc(p.provider || 'unknown') },
      { cls: 'td-num', html: fmtInt(p.requests) },
      { cls: 'td-num', html: fmtInt(p.total_tokens) },
      { cls: 'td-num', html: esc(fmtCost(p.cost_usd)) },
    ]));
    const modelTable = sheet(['Modell', 'Requests', 'In', 'Out', 'Tokens'], (m.by_model || []).map((x, i) => [
      { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
      { cls: 'ac-cyan td-id', html: esc(shortModel(x.model)) },
      { cls: 'td-num', html: fmtInt(x.requests) },
      { cls: 'td-num', html: fmtInt(x.input_tokens) },
      { cls: 'td-num', html: fmtInt(x.output_tokens) },
      { cls: 'td-num', html: fmtInt(x.total_tokens) },
    ]));
    host.innerHTML = `
      ${renderPageHead('Metrics', 'Gateway-Nutzung nach Provider/Modell', 'violet', pill(`${fmtInt(t.requests ?? 0)} requests`, 'violet'))}
      ${statsGrid([
        { label: 'Requests', value: fmtInt(t.requests ?? 0), ac: 'cyan' },
        { label: 'Input-Tokens', value: fmtInt(t.input_tokens ?? 0), ac: 'blue' },
        { label: 'Output-Tokens', value: fmtInt(t.output_tokens ?? 0), ac: 'mint' },
        { label: 'Kosten', value: esc(fmtCost(t.cost_usd ?? 0)), ac: 'gold' },
      ])}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://metrics/providers', ac: 'violet', prompt: 'usage by provider', body: `<h3>Requests</h3>${chart(provRows, 'violet')}${provTable}` })}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://metrics/models', ac: 'cyan', prompt: 'usage by model', body: `<h3>Tokens (in+out)</h3>${chart(modelRows.slice(0, 8), 'cyan')}${modelTable}` })}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://metrics/outcomes', ac: 'lime', prompt: 'usage by outcome', body: outcomePills })}`;
  };

  pages.usage = async (host) => {
    host.innerHTML = renderPageHead('Usage', 'letzte Gateway-Events (SQLite usage.db)', 'blue') + skel();
    const limit = Number(sessionStorage.getItem('bkg_usage_limit') || 100);
    const u = await api(`/usage?limit=${limit}`);
    const events = u.events || [];
    const maxTok = Math.max(...events.map((e) => e.total_tokens || 0), 1);
    const rows = events.map((e, i) => [
      { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
      { cls: 'td-mut', html: esc(fmtTime(e.created_at_ms)) },
      { cls: 'ac-blue td-id', html: esc(shortModel(e.actual_model || e.model)) },
      { cls: 'td-own', html: esc(e.provider || '–') },
      { cls: 'td-mut', html: esc(e.endpoint || '–') },
      { html: pill(String(e.status ?? '?'), (e.status || 500) < 400 ? 'mint' : 'red', false) },
      { cls: 'td-num', html: `${fmtInt(e.input_tokens ?? 0)} / ${fmtInt(e.output_tokens ?? 0)}` },
      { cls: 'td-num', html: `<span class="tbar" style="width:${Math.max(3, Math.round(((e.total_tokens || 0) / maxTok) * 60))}px"></span> ${fmtInt(e.total_tokens ?? 0)}` },
      { cls: 'td-num', html: esc(fmtCost(e.cost_usd)) },
      { cls: 'td-mut', html: esc(fmtDur(e.duration_ms)) },
    ]);
    host.innerHTML = `
      ${renderPageHead('Usage', 'letzte Gateway-Events (SQLite usage.db)', 'blue', `
        <select id="usageLimit" class="btn ghost" style="padding:4px 8px">
          ${[50, 100, 250, 500, 1000].map((n) => `<option value="${n}" ${n === limit ? 'selected' : ''}>${n} Events</option>`).join('')}
        </select>`)}
      ${win({
        title: 'gateway://usage/events', ac: 'blue', prompt: `select * from usage_events order by created_at desc limit ${limit};`,
        body: sheet(['#', 'Zeit', 'Modell', 'Provider', 'Endpoint', 'Status', 'In/Out', 'Total', 'Kosten', 'Dauer'], rows),
      })}`;
    $('#usageLimit').addEventListener('change', (ev) => {
      sessionStorage.setItem('bkg_usage_limit', ev.target.value);
      go('usage');
    });
  };

  pages.tokens = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Tokens', 'Crew-Tokens: scoped, laufend, sperrbar', 'magenta') + skel();
    const t = await api('/api/tokens');
    const rows = (t.tokens || []).map((tok, i) => {
      const live = !tok.revoked_at && (!tok.expires_at || tok.expires_at > Date.now() / 1000);
      const status = tok.revoked_at ? pill('revoked', 'red') : (live ? pill('live', 'mint') : pill('abgelaufen', 'orange'));
      return [
        { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
        { cls: 'ac-magenta td-id', html: esc(tok.id) },
        esc(tok.label || '–'),
        pill(tok.scope || 'client', 'violet', false),
        { cls: 'td-mut', html: esc(fmtTime(tok.created_at * 1000)) },
        { cls: 'td-mut', html: tok.expires_at ? esc(fmtTime(tok.expires_at * 1000)) : 'nie' },
        { cls: 'td-num', html: fmtInt(tok.request_count) },
        { cls: 'td-num', html: tok.budget_usd != null ? esc(fmtCost(tok.budget_usd)) : '–' },
        { html: status },
        { html: tok.revoked_at ? '' : `<button class="btn mini ghost" data-revoke="${esc(tok.id)}">revoke</button>` },
      ];
    });
    host.innerHTML = `
      ${renderPageHead('Tokens', 'Crew-Tokens: scoped, laufend, sperrbar', 'magenta', pill(`${(t.tokens || []).length} tokens`, 'magenta'))}
      ${win({
        title: 'gateway://tokens/issue', ac: 'magenta', prompt: 'bkgcrewcli tokens issue --label … --ttl …',
        body: `
          <div class="form-grid">
            ${field({ label: 'Label', name: 'label', ph: 'z.B. opencode-laptop' })}
            ${field({ label: 'Scope', name: 'scope', type: 'select', options: [['client', 'client'], ['gateway', 'gateway'], ['admin', 'admin']] })}
            ${field({ label: 'TTL', name: 'ttl', type: 'select', options: [['0', 'nie'], ['3600', '1 h'], ['28800', '8 h'], ['86400', '24 h'], ['604800', '7 Tage']] })}
            ${field({ label: 'Budget USD (optional)', name: 'budget', type: 'number', step: '0.01', ph: 'z.B. 5.00' })}
          </div>
          <div class="btn-row"><button id="issueBtn" class="btn solid">◎ Token ausstellen</button></div>
          <div id="issueOut" style="margin-top:10px"></div>`,
      })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://tokens/list', ac: 'magenta', prompt: 'select id,label,scope,status from crew_tokens;',
        body: sheet(['#', 'ID', 'Label', 'Scope', 'Erstellt', 'Läuft ab', 'Requests', 'Budget', 'Status', ''], rows),
      })}
      <div class="tl note"># das Secret wird nur einmal bei der Ausstellung angezeigt</div>`;
    $('#issueBtn').addEventListener('click', async () => {
      const f = collect($('#issueOut').closest('.win-body'));
      try {
        const r = await api('/api/tokens/issue', { method: 'POST', body: JSON.stringify({
          label: f.label || '', scope: f.scope || 'client',
          ttl_secs: Number(f.ttl) || 0, budget_usd: f.budget ? Number(f.budget) : null,
        })});
        $('#issueOut').innerHTML = `
          <div class="tl ok">✓ Token erstellt</div>
          <div class="tl cmd"><span class="p">$</span> <span id="tokSecret">${esc(r.token)}</span><i class="cursor"></i></div>
          <div class="btn-row"><button class="btn mini" id="copyTok">kopieren</button><span class="hint">jetzt kopieren – später nicht mehr sichtbar</span></div>`;
        $('#copyTok').addEventListener('click', () => {
          navigator.clipboard?.writeText(r.token).then(() => toast('Token kopiert', 'mint'));
        });
        toast('Crew-Token erstellt', 'magenta');
        setTimeout(() => go('tokens'), 400);
      } catch (e) { toast(`Ausstellung fehlgeschlagen: ${e.message}`, 'red'); }
    });
    host.querySelectorAll('[data-revoke]').forEach((b) => b.addEventListener('click', async () => {
      try {
        await api('/api/tokens/revoke', { method: 'POST', body: JSON.stringify({ id: b.dataset.revoke }) });
        toast('Token gesperrt', 'orange');
        go('tokens');
      } catch (e) { toast(`Revoke fehlgeschlagen: ${e.message}`, 'red'); }
    }));
  };

  pages.gateway = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Gateway', 'gateway.json — Auth, Flags, Limits', 'orange') + skel();
    const c = await api('/admin/config');
    const auth = c.auth || {};
    const keysMasked = typeof auth.apiKeys === 'number';
    const flags = [
      ['useMessagesApi', 'Messages-API nutzen (Anthropic-Pfad)'],
      ['useResponsesApiWebSocket', 'Responses-API über WebSocket'],
      ['useResponsesApiWebSearch', 'Web-Suche in Responses-API'],
      ['alphaSearchCodexPriority', 'Codex-Priorität für Alpha-Suche'],
      ['codexMultiAgentV1', 'Codex Multi-Agent v1'],
    ];
    host.innerHTML = `
      ${renderPageHead('Gateway', 'gateway.json — Auth, Flags, Limits', 'orange', pill('admin', 'orange'))}
      ${win({
        title: 'gateway://config/auth', ac: 'orange', prompt: 'vi ~/.bkgcrewcli/gateway.json [auth]',
        body: `
          <div class="form-grid">
            <div class="field wide">
              <label>Gateway-API-Keys (eine pro Zeile${keysMasked ? ` — ${auth.apiKeys} gespeichert, leer lassen = unverändert` : ''})</label>
              <textarea data-name="gw_api_keys" rows="2" placeholder="${keysMasked ? '*** maskiert — neue Keys hier eintragen' : 'sk-…'}">${keysMasked ? '' : esc((auth.apiKeys || []).join('\n'))}</textarea>
            </div>
            <div class="field wide">
              <label>Admin-Key ${auth.adminApiKey ? '(gesetzt — *** = unverändert)' : '(nicht gesetzt)'}</label>
              <input type="password" data-name="gw_admin_key" value="${auth.adminApiKey ? '***' : ''}" autocomplete="off">
            </div>
          </div>`,
      })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://config/flags', ac: 'cyan', prompt: 'set feature flags',
        body: `
          <div class="form-grid">
            ${flags.map(([n, l]) => check({ label: l, name: n, value: c[n] })).join('')}
          </div>
          <div class="form-grid" style="margin-top:10px">
            ${field({ label: 'Alpha-Suche Modell', name: 'alphaSearchModel', value: c.alphaSearchModel || '' })}
            ${field({ label: 'Messages Web-Suche Modell', name: 'messageApiWebSearchModel', value: c.messageApiWebSearchModel || '' })}
            ${field({ label: 'Claude Auto-Modell', name: 'claudeAutoModel', value: c.claudeAutoModel || '' })}
            ${field({ label: 'Claude Token-Multiplikator', name: 'claudeTokenMultiplier', type: 'number', step: '0.1', value: c.claudeTokenMultiplier ?? 1 })}
            ${field({ label: 'OpenCode Kontext-Fenster', name: 'opencodeModelContextWindow', type: 'number', value: c.opencodeModelContextWindow ?? 0 })}
            ${field({ label: 'Usage-Retention (Tage, 0=nie)', name: 'tokenUsageRetentionDays', type: 'number', value: c.tokenUsageRetentionDays ?? 0 })}
          </div>`,
      })}
      <div class="btn-row">
        <button id="gwSave" class="btn solid">⌁ Gateway-Konfiguration speichern</button>
        <span class="hint">Schreiben nach ~/.bkgcrewcli/gateway.json</span>
      </div>`;

    $('#gwSave').addEventListener('click', async () => {
      const body = { auth: {} };
      const keysRaw = String(host.querySelector('[data-name="gw_api_keys"]').value || '').split('\n').map((s) => s.trim()).filter(Boolean);
      if (keysRaw.length) body.auth.apiKeys = keysRaw; // masked → server keeps
      const adminKey = String(host.querySelector('[data-name="gw_admin_key"]').value || '');
      body.auth.adminApiKey = adminKey === '***' ? '***' : (adminKey.trim() || null);
      flags.forEach(([n]) => { body[n] = collect(host)[n] ?? false; });
      const scal = collect(host);
      if (scal.alphaSearchModel !== undefined) body.alphaSearchModel = scal.alphaSearchModel;
      if (scal.messageApiWebSearchModel !== undefined) body.messageApiWebSearchModel = scal.messageApiWebSearchModel;
      if (scal.claudeAutoModel !== undefined) body.claudeAutoModel = scal.claudeAutoModel || null;
      body.claudeTokenMultiplier = Number(scal.claudeTokenMultiplier) || 1;
      body.opencodeModelContextWindow = Number(scal.opencodeModelContextWindow) || 0;
      body.tokenUsageRetentionDays = Number(scal.tokenUsageRetentionDays) > 0 ? Number(scal.tokenUsageRetentionDays) : null;
      try {
        await api('/admin/config', { method: 'POST', body: JSON.stringify(body) });
        toast('Gateway-Konfiguration gespeichert', 'mint');
        go('gateway');
      } catch (e) { toast(`Speichern fehlgeschlagen: ${e.message}`, 'red'); }
    });
  };

  pages.providers = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Providers', 'Upstream-Provider (gateway.json + Keystore)', 'gold') + skel();
    const [p, cfg] = await Promise.all([api('/api/providers'), api('/admin/config').catch(() => null)]);
    const gwProv = (cfg && cfg.providers) || {};
    const rows = (p.providers || []).map((x, i) => [
      { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
      { cls: 'ac-gold td-id', html: esc(x.name) },
      pill(x.source === 'keystore' ? 'keystore' : 'gateway', x.source === 'keystore' ? 'violet' : 'cyan', false),
      { cls: 'td-mut', html: esc(x.type || '–') },
      { cls: 'td-mut', html: esc(x.base_url || '–') },
      { html: x.enabled ? pill('an', 'mint') : pill('aus', 'dim', false) },
      { cls: 'td-num', html: fmtInt((x.models || []).length) },
      { html: x.has_api_key ? pill('key ✓', 'gold', false) : '<span class="empty">–</span>' },
    ]);
    const gwRows = Object.entries(gwProv).map(([name, v], i) => [
      { cls: 'td-idx', html: String(i + 1).padStart(2, '0') },
      { cls: 'ac-cyan td-id', html: esc(name) },
      { cls: 'td-mut', html: esc(v?.baseUrl || v?.base_url || '–') },
      { html: v?.enabled === false ? pill('aus', 'dim', false) : pill('an', 'mint') },
      { html: v?.apiKey ? pill('key ✓', 'gold', false) : '<span class="empty">–</span>' },
      { html: `<button class="btn mini ghost" data-editprov="${esc(name)}">bearbeiten</button>` },
    ]);
    host.innerHTML = `
      ${renderPageHead('Providers', 'Upstream-Provider (gateway.json + Keystore)', 'gold', pill(`${(p.providers || []).length} provider`, 'gold'))}
      ${win({ title: 'gateway://providers', ac: 'gold', prompt: 'list providers --all', body: sheet(['#', 'Name', 'Quelle', 'Typ', 'Base URL', 'Status', 'Modelle', 'Key'], rows) })}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://providers/gateway-config', ac: 'cyan', prompt: 'gateway.json [providers]', body: sheet(['#', 'Name', 'Base URL', 'Status', 'Key', ''], gwRows) + `
        <h3>Provider anlegen / bearbeiten</h3>
        <div class="form-grid">
          ${field({ label: 'Name', name: 'prov_name', ph: 'z.B. eysho' })}
          ${field({ label: 'Typ', name: 'prov_type', type: 'select', options: ['openai', 'anthropic', 'custom'] })}
          ${field({ label: 'Base URL', name: 'prov_url', ph: 'https://api.eysho.info' })}
          ${field({ label: 'API-Key (*** = unverändert)', name: 'prov_key', type: 'pw', ph: 'optional' })}
        </div>
        <div class="btn-row">
          <button id="provSave" class="btn solid">⛁ Provider speichern</button>
          <span class="hint">legt/aktualisiert den Eintrag in gateway.json</span>
        </div>` })}
      <div class="tl note"># Keystore-Provider sind schreibgeschützt (Verwaltung über CLI)</div>`;
    const fillEdit = (name) => {
      const v = gwProv[name] || {};
      const set = (n, val) => { const el = host.querySelector(`[data-name="${n}"]`); if (el) el.value = val ?? ''; };
      set('prov_name', name); set('prov_type', v.providerType || v.provider_type || v.type || 'openai');
      set('prov_url', v.baseUrl || v.base_url || ''); set('prov_key', v.apiKey ? '***' : '');
    };
    host.querySelectorAll('[data-editprov]').forEach((b) => b.addEventListener('click', () => fillEdit(b.dataset.editprov)));
    $('#provSave').addEventListener('click', async () => {
      const f = collect(host);
      const name = String(f.prov_name || '').trim();
      if (!name) { toast('Provider-Name fehlt', 'orange'); return; }
      const providers = JSON.parse(JSON.stringify(gwProv));
      providers[name] = {
        ...(providers[name] || {}),
        providerType: f.prov_type || 'openai',
        baseUrl: String(f.prov_url || '').trim(),
        enabled: true,
      };
      if (f.prov_key && f.prov_key !== '***') providers[name].apiKey = f.prov_key;
      try {
        await api('/admin/config', { method: 'POST', body: JSON.stringify({ providers }) });
        toast('Provider gespeichert', 'mint');
        go('providers');
      } catch (e) { toast(`Speichern fehlgeschlagen: ${e.message}`, 'red'); }
    });
  };

  pages.mappings = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Mappings', 'Modell-Aliase: angefragt → upstream', 'cyan') + skel();
    const m = await api('/admin/config/model-mappings');
    const entries = Object.entries(m || {});
    const render = (map) => {
      const rows = Object.entries(map).map(([k, v], i) => `
        <div class="chart-row" style="grid-template-columns: 1fr 22px 1fr 30px">
          <input class="map-from" value="${esc(k)}" placeholder="angefragtes modell">
          <span class="tl note" style="text-align:center">→</span>
          <input class="map-to" value="${esc(v)}" placeholder="upstream modell">
          <button class="btn mini ghost" data-del="${esc(k)}">✕</button>
        </div>`).join('');
      return `<div class="map-list" style="display:flex;flex-direction:column;gap:6px">${rows || '<div class="empty">– keine Mappings –</div>'}</div>`;
    };
    host.innerHTML = `
      ${renderPageHead('Mappings', 'Modell-Aliase: angefragt → upstream', 'cyan', pill(`${entries.length} mappings`, 'cyan'))}
      ${win({
        title: 'gateway://config/model-mappings', ac: 'cyan', prompt: 'model_mappings:',
        body: `${render(m || {})}
          <div class="btn-row">
            <button id="mapAdd" class="btn ghost">+ Zeile</button>
            <button id="mapSave" class="btn solid">⇄ Mappings speichern</button>
            <span class="hint">Standard-Mappings werden serverseitig immer ergänzt</span>
          </div>`,
      })}`;
    $('#mapAdd').addEventListener('click', () => {
      const list = host.querySelector('.map-list');
      const div = document.createElement('div');
      div.className = 'chart-row';
      div.style.gridTemplateColumns = '1fr 22px 1fr 30px';
      div.innerHTML = `<input class="map-from" placeholder="angefragtes modell"><span class="tl note" style="text-align:center">→</span><input class="map-to" placeholder="upstream modell"><button class="btn mini ghost">✕</button>`;
      div.querySelector('button').addEventListener('click', () => div.remove());
      list.appendChild(div);
    });
    host.querySelectorAll('[data-del]').forEach((b) => b.addEventListener('click', () => b.closest('.chart-row').remove()));
    $('#mapSave').addEventListener('click', async () => {
      const map = {};
      host.querySelectorAll('.chart-row').forEach((row) => {
        const from = row.querySelector('.map-from')?.value.trim();
        const to = row.querySelector('.map-to')?.value.trim();
        if (from && to) map[from] = to;
      });
      try {
        await api('/admin/config/model-mappings', { method: 'POST', body: JSON.stringify(map) });
        toast('Mappings gespeichert', 'mint');
        go('mappings');
      } catch (e) { toast(`Speichern fehlgeschlagen: ${e.message}`, 'red'); }
    });
  };

  pages.settings = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Einstellungen', 'config.toml — Agents, Memory, Tasks', 'violet') + skel();
    const r = await api('/admin/app-config');
    const st = r.settings || {};
    const agents = st.agents || {};
    const kinds = ['claude', 'opencode', 'codex', 'cursor', 'pi', 'copilot'];
    const agentRows = kinds.map((k) => {
      const a = agents[k] || {};
      return `
        <div class="ac-mint h3" style="color:var(--mint);font-size:11px;letter-spacing:1px;margin:10px 0 6px">[agents.${esc(k)}]</div>
        <div class="form-grid">
          ${field({ label: 'Binary', name: `ag_${k}_binary`, value: a.binary || '', ph: 'auto' })}
          ${field({ label: 'Modell', name: `ag_${k}_model`, value: a.model || '', ph: 'default' })}
          ${field({ label: 'Base URL', name: `ag_${k}_base_url`, value: a.base_url || '', ph: 'default' })}
          ${field({ label: 'API-Key (*** = unverändert)', name: `ag_${k}_api_key`, type: 'pw', value: a.api_key || '' })}
          <div class="field wide">${field({ label: 'Extra Args (Komma-getrennt)', name: `ag_${k}_extra_args`, value: (a.extra_args || []).join(', '), ph: '--flag wert, --flag2' })}</div>
        </div>`;
    }).join('');
    const mem = st.memory || {};
    const tasks = st.tasks || {};
    host.innerHTML = `
      ${renderPageHead('Einstellungen', 'config.toml — Agents, Memory, Tasks', 'violet', pill(esc(r.configPath || 'config.toml'), 'dim', false))}
      ${win({
        title: 'gateway://settings/general', ac: 'violet', prompt: 'vi ~/.config/bkgcrewcli/config.toml [general]',
        body: `
          <div class="form-grid">
            ${field({ label: 'Default Agent', name: 'def_agent', type: 'select', options: kinds.map((k) => [k, k]), value: st.default_agent || 'claude' })}
            ${field({ label: 'Default Modell', name: 'def_model', value: st.default_model || '', ph: 'agent default' })}
            ${field({ label: 'Permissions', name: 'def_permissions', type: 'select', options: [['default', 'default'], ['acceptEdits', 'accept edits'], ['bypassPermissions', 'bypass'], ['plan', 'plan']], value: st.permissions || 'default' })}
          </div>`,
      })}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://settings/agents', ac: 'mint', prompt: 'config.toml [agents.*]', body: agentRows })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://settings/memory', ac: 'blue', prompt: 'config.toml [memory] — vault / cognition / mender',
        body: `
          <div class="form-grid">
            ${check({ label: 'Vault (Wissensspeicher) an', name: 'mem_vault', value: mem.vault_enabled !== false })}
            ${check({ label: 'Cognition (Signale) an', name: 'mem_cognition', value: mem.cognition_enabled !== false })}
            ${check({ label: 'Mender (Fehler-Lernen) an', name: 'mem_mender', value: mem.mender_enabled !== false })}
            ${field({ label: 'Scope', name: 'mem_scope', value: mem.scope || 'default' })}
            ${field({ label: 'Max Kontext-Zeichen', name: 'mem_max_chars', type: 'number', value: mem.max_context_chars ?? 4000 })}
          </div>`,
      })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://settings/tasks', ac: 'lime', prompt: 'config.toml [tasks] — persistent queue',
        body: `
          <div class="form-grid">
            ${check({ label: 'Task-Queue an', name: 'task_enabled', value: tasks.enabled !== false })}
            ${check({ label: 'Auto-Chaining', name: 'task_auto_chain', value: tasks.auto_chain !== false })}
            ${field({ label: 'Max parallel', name: 'task_max_parallel', type: 'number', min: '1', value: tasks.max_parallel ?? 2 })}
            ${field({ label: 'Timeout (Sek)', name: 'task_timeout', type: 'number', value: tasks.default_timeout_secs ?? 900 })}
            ${field({ label: 'DB-Pfad', name: 'task_db', value: tasks.db_path || 'bkgcrewcli/tasks.db' })}
          </div>`,
      })}
      <div class="btn-row">
        <button id="setSave" class="btn solid">⚙ Einstellungen speichern</button>
        <span class="hint">Schreibgeschützt 0600 · Secrets bleiben bei *** erhalten</span>
      </div>`;

    $('#setSave').addEventListener('click', async () => {
      const f = collect(host);
      const next = JSON.parse(JSON.stringify(st));
      next.default_agent = f.def_agent || next.default_agent;
      next.default_model = f.def_model || null;
      next.permissions = f.def_permissions || next.permissions;
      next.agents = next.agents || {};
      kinds.forEach((k) => {
        const a = { ...(next.agents[k] || {}) };
        const set = (prop, val, num = false) => {
          const v = String(val ?? '').trim();
          if (v === '') { if (prop === 'api_key') a[prop] = null; else delete a[prop]; return; }
          a[prop] = num ? Number(v) : v;
        };
        set('binary', f[`ag_${k}_binary`]);
        set('model', f[`ag_${k}_model`]);
        set('base_url', f[`ag_${k}_base_url`]);
        if (f[`ag_${k}_api_key`] === '***') { /* keep */ } else set('api_key', f[`ag_${k}_api_key`]);
        const args = String(f[`ag_${k}_extra_args`] || '').split(',').map((s) => s.trim()).filter(Boolean);
        if (args.length) a.extra_args = args; else delete a.extra_args;
        if (Object.keys(a).length) next.agents[k] = a; else delete next.agents[k];
      });
      next.memory = {
        vault_enabled: !!f.mem_vault, cognition_enabled: !!f.mem_cognition, mender_enabled: !!f.mem_mender,
        scope: f.mem_scope || 'default', max_context_chars: Number(f.mem_max_chars) || 4000,
      };
      next.tasks = {
        enabled: !!f.task_enabled, auto_chain: !!f.task_auto_chain,
        max_parallel: Number(f.task_max_parallel) || 2,
        default_timeout_secs: Number(f.task_timeout) || 900,
        db_path: f.task_db || 'bkgcrewcli/tasks.db',
      };
      try {
        await api('/admin/app-config', { method: 'POST', body: JSON.stringify({ settings: next }) });
        toast('Einstellungen gespeichert', 'mint');
        go('settings');
      } catch (e) { toast(`Speichern fehlgeschlagen: ${e.message}`, 'red'); }
    });
  };

  pages.sandbox = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Sandbox', 'Isolierte Ausführung: local / container / capsule', 'orange') + skel();
    const r = await api('/admin/app-config');
    const sb = r.settings?.sandbox || {};
    const imgRows = Object.entries(sb.images || {}).map(([k, v], i) => `
      <div class="chart-row img-row" style="grid-template-columns: 130px 1fr 30px">
        <input class="img-k" value="${esc(k)}" placeholder="stack">
        <input class="img-v" value="${esc(v)}" placeholder="image">
        <button class="btn mini ghost">✕</button>
      </div>`).join('');
    const envRows = Object.entries(sb.env || {}).map(([k, v]) => `
      <div class="chart-row env-row" style="grid-template-columns: 130px 1fr 30px">
        <input class="env-k" value="${esc(k)}" placeholder="VAR">
        <input class="env-v" value="${esc(v)}" placeholder="wert">
        <button class="btn mini ghost">✕</button>
      </div>`).join('');
    host.innerHTML = `
      ${renderPageHead('Sandbox', 'Isolierte Ausführung: local / container / capsule', 'orange', pill(sb.enabled ? 'aktiv' : 'inaktiv', sb.enabled ? 'mint' : 'dim'))}
      ${win({
        title: 'gateway://sandbox/core', ac: 'orange', prompt: 'config.toml [sandbox]',
        body: `
          <div class="form-grid">
            ${check({ label: 'Sandbox aktiv', name: 'sb_enabled', value: !!sb.enabled })}
            ${field({ label: 'Provider', name: 'sb_provider', type: 'select', options: [['local', 'local'], ['container', 'container'], ['capsule', 'capsule']], value: sb.provider || 'local' })}
            ${field({ label: 'Stack', name: 'sb_stack', type: 'select', options: ['default', 'rust', 'python', 'node', 'go'], value: sb.stack || 'default' })}
            ${field({ label: 'Projekt', name: 'sb_project', value: sb.project || 'default' })}
            ${field({ label: 'Runtime', name: 'sb_runtime', type: 'select', options: [['docker', 'docker'], ['podman', 'podman']], value: sb.runtime || 'docker' })}
            ${check({ label: 'Images auto-pullen', name: 'sb_auto_pull', value: sb.auto_pull !== false })}
            ${check({ label: 'Netz-Isolation', name: 'sb_network', value: sb.network_isolation !== false })}
          </div>`,
      })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://sandbox/capsule', ac: 'magenta', prompt: 'capsule backend',
        body: `
          <div class="form-grid">
            ${field({ label: 'Capsule Endpoint', name: 'sb_capsule_url', value: sb.capsule_endpoint || '', ph: 'https://capsule:8443' })}
            ${field({ label: 'Capsule API-Key (*** = unverändert)', name: 'sb_capsule_key', type: 'pw', value: sb.capsule_api_key || '' })}
            ${field({ label: 'Capsule Template', name: 'sb_capsule_tpl', value: sb.capsule_template || '', ph: 'bkg-base' })}
            ${field({ label: 'Timeout (Sek)', name: 'sb_capsule_timeout', type: 'number', value: sb.capsule_timeout_secs ?? 300 })}
          </div>`,
      })}
      <div style="height:4px"></div>
      ${win({
        title: 'gateway://sandbox/resources', ac: 'cyan', prompt: 'images, devices, volumes, limits',
        body: `
          <h3>Images (stack → image)</h3>
          <div class="img-list" style="display:flex;flex-direction:column;gap:6px">${imgRows || '<div class="empty">– keine Images definiert –</div>'}</div>
          <div class="btn-row"><button class="btn ghost" id="imgAdd">+ Image-Zeile</button></div>
          <h3>Env (VAR → wert)</h3>
          <div class="env-list" style="display:flex;flex-direction:column;gap:6px">${envRows || '<div class="empty">– keine Env-Variablen –</div>'}</div>
          <div class="btn-row"><button class="btn ghost" id="envAdd">+ Env-Zeile</button></div>
          <div class="form-grid" style="margin-top:12px">
            <div class="field wide"><label>Devices (eine pro Zeile)</label><textarea data-name="sb_devices" rows="2">${esc((sb.devices || []).join('\n'))}</textarea></div>
            <div class="field wide"><label>Volumes (eine pro Zeile, host:container)</label><textarea data-name="sb_volumes" rows="2">${esc((sb.volumes || []).join('\n'))}</textarea></div>
            ${field({ label: 'CPU-Limit', name: 'sb_cpu', value: sb.cpu_limit || '', ph: '2' })}
            ${field({ label: 'Memory-Limit', name: 'sb_mem', value: sb.memory_limit || '', ph: '4g' })}
          </div>`,
      })}
      <div class="btn-row">
        <button id="sbSave" class="btn solid">▣ Sandbox speichern</button>
        <span class="hint">Teil der zentralen config.toml</span>
      </div>`;

    const rowAdd = (sel, clsK, clsV, kPh, vPh) => {
      const list = host.querySelector(sel);
      const div = document.createElement('div');
      div.className = `chart-row ${sel.includes('img') ? 'img-row' : 'env-row'}`;
      div.style.gridTemplateColumns = '130px 1fr 30px';
      div.innerHTML = `<input class="${clsK}" placeholder="${kPh}"><input class="${clsV}" placeholder="${vPh}"><button class="btn mini ghost">✕</button>`;
      div.querySelector('button').addEventListener('click', () => div.remove());
      list.appendChild(div);
    };
    $('#imgAdd').addEventListener('click', () => rowAdd('.img-list', 'img-k', 'img-v', 'stack', 'image'));
    $('#envAdd').addEventListener('click', () => rowAdd('.env-list', 'env-k', 'env-v', 'VAR', 'wert'));
    host.querySelectorAll('.img-row button, .env-row button').forEach((b) => b.addEventListener('click', () => b.closest('.chart-row').remove()));

    $('#sbSave').addEventListener('click', async () => {
      const f = collect(host);
      const images = {}; const env = {};
      host.querySelectorAll('.img-row').forEach((r) => { const k = r.querySelector('.img-k')?.value.trim(); const v = r.querySelector('.img-v')?.value.trim(); if (k && v) images[k] = v; });
      host.querySelectorAll('.env-row').forEach((r) => { const k = r.querySelector('.env-k')?.value.trim(); const v = r.querySelector('.env-v')?.value.trim(); if (k && v) env[k] = v; });
      const lines = (n) => String(f[n] || '').split('\n').map((s) => s.trim()).filter(Boolean);
      const next = JSON.parse(JSON.stringify(r.settings || {}));
      next.sandbox = {
        ...sb,
        enabled: !!f.sb_enabled,
        provider: f.sb_provider || 'local',
        stack: f.sb_stack || 'default',
        project: f.sb_project || 'default',
        runtime: f.sb_runtime || 'docker',
        auto_pull: !!f.sb_auto_pull,
        network_isolation: !!f.sb_network,
        capsule_endpoint: f.sb_capsule_url || null,
        capsule_api_key: f.sb_capsule_key === '***' ? (sb.capsule_api_key ?? null) : (f.sb_capsule_key || null),
        capsule_template: f.sb_capsule_tpl || null,
        capsule_timeout_secs: Number(f.sb_capsule_timeout) || 300,
        images, env,
        devices: lines('sb_devices'),
        volumes: lines('sb_volumes'),
        cpu_limit: f.sb_cpu || null,
        memory_limit: f.sb_mem || null,
      };
      try {
        await api('/admin/app-config', { method: 'POST', body: JSON.stringify({ settings: next }) });
        toast('Sandbox gespeichert', 'mint');
        go('sandbox');
      } catch (e) { toast(`Speichern fehlgeschlagen: ${e.message}`, 'red'); }
    });
  };

  pages.doctor = async (host) => {
    if (!requireAdmin()) { go('dashboard'); return; }
    host.innerHTML = renderPageHead('Doctor', 'Diagnose: was läuft, was fehlt', 'lime') + skel();
    const d = await api('/admin/doctor');
    const kvSheet = (obj) => sheet(['Feld', 'Wert'], Object.entries(obj || {}).map(([k, v]) => [
      { cls: 'td-mut', html: esc(k) },
      { html: typeof v === 'boolean' ? (v ? pill('ja', 'mint', false) : pill('nein', 'dim', false)) : esc(Array.isArray(v) ? v.join(', ') : String(v ?? '–')) },
    ]));
    const checkWin = (c) => {
      let body = '';
      if (c.name === 'agents' && Array.isArray(c.detail)) {
        body = sheet(['Agent', 'Status', 'Version', 'Modell', 'Key'], c.detail.map((a) => [
          { cls: 'ac-lime td-id', html: esc(a.id) },
          { html: a.available ? pill('verfügbar', 'mint') : pill('fehlt', 'red') },
          { cls: 'td-mut', html: esc(a.version || '–') },
          { cls: 'td-own', html: esc(shortModel(a.model)) },
          { html: a.apiKeyConfigured ? pill('key ✓', 'gold', false) : '<span class="empty">–</span>' },
        ]));
      } else if (c.detail && typeof c.detail === 'object') {
        body = kvSheet(c.detail);
      } else {
        body = empty();
      }
      return `<div style="margin-bottom:4px">${c.ok ? pill(c.name + ' ok', 'mint') : pill(c.name + ' fehlt', 'red')}</div>${body}`;
    };
    host.innerHTML = `
      ${renderPageHead('Doctor', 'Diagnose: was läuft, was fehlt', 'lime', d.ok ? pill('alles ok', 'mint') : pill('Probleme gefunden', 'red'))}
      ${win({ title: 'gateway://doctor', ac: 'lime', prompt: `bkgcrewcli doctor --version v${d.version || '?'}`, body: d.checks.map(checkWin).join('<div style="height:10px"></div>') })}
      <div style="height:4px"></div>
      ${win({ title: 'gateway://doctor/paths', ac: 'cyan', prompt: 'data locations', body: kvSheet({
        config: d.paths?.config || '–',
        gatewayConfig: d.paths?.gatewayConfig || '–',
        usageDb: d.paths?.usageDb || '–',
        dataDir: d.paths?.dataDir || '–',
      }) })}`;
  };

  /* ── router ──────────────────────────────────────────────────────── */
  async function go(page) {
    const it = findItem(page);
    if (it.admin && !S.unlocked) {
      toast('Admin-Bereich: erst Token eingeben', 'orange');
      $('#tokenInput').focus();
      page = S.unlocked ? page : 'dashboard';
      if (!S.unlocked) { S.page = 'dashboard'; }
    }
    S.page = page;
    const host = $('#page');
    renderRail(); renderSidebar(); renderTopbar();
    host.innerHTML = `<div class="ac-cyan">${skel()}</div>`;
    window.history.replaceState(null, '', `#${page}`);
    try {
      await (pages[page] || pages.dashboard)(host);
    } catch (e) {
      const ac = findItem(page).ac;
      host.innerHTML = `
        ${renderPageHead(findItem(page).label, 'Fehler', ac)}
        ${win({ title: 'gateway://error', ac: 'red', prompt: 'request failed', body: `<div class="tl err">✗ ${esc(e.message)}</div>${e.auth ? '<div class="tl warn">→ Admin-Token oben eingeben, dann erneut öffnen</div>' : ''}` })}`;
    }
  }

  /* ── boot ────────────────────────────────────────────────────────── */
  function boot() {
    $('#burger').addEventListener('click', () => {
      $('#sidebar').classList.toggle('open');
      $('#scrim').classList.toggle('show');
    });
    $('#scrim').addEventListener('click', closeSidebar);
    $('#tokenApply').addEventListener('click', async () => {
      const v = $('#tokenInput').value.trim();
      if (!v) { sessionStorage.removeItem(TOKEN_KEY); S.token = ''; S.unlocked = false; toast('Token entfernt', 'orange'); renderSidebar(); return; }
      await unlock(v);
    });
    $('#tokenInput').addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') $('#tokenApply').click();
    });
    $('#brand').addEventListener('click', () => go('dashboard'));
    // restore session unlock
    if (S.token) {
      api('/admin/app-config').then(() => {
        S.unlocked = true;
        renderSidebar();
      }).catch(() => { S.unlocked = false; });
    }
    // live health indicator
    api('/api/health').then(() => {
      $('#healthPill').className = 'health-pill ac-mint';
      $('#healthPill').innerHTML = '<i></i><span class="txt">live</span>';
    }).catch(() => {
      $('#healthPill').className = 'health-pill bad';
      $('#healthPill').innerHTML = '<i></i><span class="txt">offline</span>';
    });
    const initial = (location.hash || '#dashboard').slice(1);
    go(NAV.flatMap((s) => s.items).some((i) => i.id === initial) ? initial : 'dashboard');
  }

  document.addEventListener('DOMContentLoaded', boot);
})();
