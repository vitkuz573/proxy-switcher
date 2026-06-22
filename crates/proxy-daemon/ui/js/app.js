const API = '/api/v1';

let state = { status: null, proxies: [], stats: null };

function $(sel, ctx) { return (ctx || document).querySelector(sel); }
function $$(sel, ctx) { return Array.from((ctx || document).querySelectorAll(sel)); }

function fmtLatency(ms) {
  if (ms == null) return '—';
  if (ms < 1000) return ms + 'ms';
  return (ms / 1000).toFixed(1) + 's';
}

function fmtScore(score) {
  return score != null ? score.toFixed(1) : '—';
}

function scoreClass(score) {
  if (score == null) return '';
  if (score >= 50) return 'score-high';
  if (score >= 20) return 'score-mid';
  return 'score-low';
}

function scoreBar(score) {
  const pct = score != null ? Math.min(score, 100) : 0;
  const cls = scoreClass(score);
  return `<span class="score-bar"><span class="score-bar-fill ${cls}" style="width:${pct}%"></span></span>`;
}

async function fetchJSON(url) {
  try {
    const res = await fetch(url);
    if (!res.ok) throw new Error(res.statusText);
    return await res.json();
  } catch (e) {
    console.error('fetch failed:', url, e);
    return null;
  }
}

async function loadStatus() {
  const data = await fetchJSON(API + '/status');
  if (data) state.status = data;
}

async function loadProxies() {
  const data = await fetchJSON(API + '/proxies');
  if (data) state.proxies = data;
}

async function loadStats() {
  const data = await fetchJSON(API + '/stats');
  if (data) state.stats = data;
}

async function refresh() {
  await Promise.all([loadStatus(), loadProxies(), loadStats()]);
  renderAll();
}

// ── Navigation ──────────────────────────────────────────────────────────

$$('.nav-links a').forEach(a => {
  a.addEventListener('click', e => {
    e.preventDefault();
    $$('.nav-links a').forEach(x => x.classList.remove('active'));
    a.classList.add('active');
    $$('.view').forEach(v => v.classList.remove('active'));
    const view = document.getElementById('view-' + a.dataset.view);
    if (view) view.classList.add('active');
    $('#view-title').textContent = a.textContent.trim();
  });
});

// ── Render ──────────────────────────────────────────────────────────────

function renderAll() {
  renderStatus();
  renderDashboardProxies();
  renderProxyTable();
  renderConnections();
  renderDns();
}

function renderStatus() {
  const badge = $('#status-badge');
  if (state.stats) {
    badge.textContent = 'Online';
    badge.className = 'status-badge';
  } else {
    badge.textContent = 'Offline';
    badge.className = 'status-badge offline';
  }
}

function renderDashboardProxies() {
  const tbody = $('#dashboard-proxy-list');
  const active = state.status?.active_proxy;
  const proxies = state.proxies.slice(0, 10);

  if (proxies.length === 0) {
    tbody.innerHTML = '<tr><td colspan="7" class="empty">No proxies in pool</td></tr>';
    return;
  }

  tbody.innerHTML = proxies.map(p => {
    const isActive = active && p.id === active.id;
    const cls = isActive ? ' class="active"' : '';
    return `<tr${cls}>
      <td><code>${esc(p.id)}</code></td>
      <td>${esc(p.host)}</td>
      <td>${p.port}</td>
      <td>${p.protocol}</td>
      <td>${fmtLatency(p.latency_ms)}</td>
      <td>${fmtScore(p.score)} ${scoreBar(p.score)}</td>
      <td>${isActive ? '<span class="badge" style="background:var(--green);color:#fff">active</span>' : `<button class="btn btn-sm btn-green" onclick="switchProxy('${esc(p.id)}')">Switch</button>`}</td>
    </tr>`;
  }).join('');

  // Dashboard summary
  const activeProxy = state.status?.active_proxy;
  $('#stat-active-proxy').textContent = activeProxy ? `${activeProxy.host}:${activeProxy.port}` : 'None';
  $('#stat-active-score').textContent = activeProxy ? `Score: ${fmtScore(activeProxy.score)}` : '';

  if (state.stats) {
    $('#stat-pool-healthy').textContent = state.stats.healthy_count;
    $('#stat-pool-total').textContent = state.proxies.length;
    $('#stat-tcp').textContent = state.stats.tcp_connections;
    $('#stat-udp').textContent = state.stats.udp_flows;
  }
}

function renderProxyTable() {
  const tbody = $('#proxy-list');
  const active = state.status?.active_proxy;
  const proxies = state.proxies;

  if (proxies.length === 0) {
    tbody.innerHTML = '<tr><td colspan="10" class="empty">No proxies</td></tr>';
    return;
  }

  tbody.innerHTML = proxies.map(p => {
    const isActive = active && p.id === active.id;
    const cls = isActive ? ' class="active"' : '';
    return `<tr${cls}>
      <td><code>${esc(p.id)}</code></td>
      <td>${esc(p.host)}</td>
      <td>${p.port}</td>
      <td>${p.protocol}</td>
      <td>${p.anonymity}</td>
      <td>${p.country || '—'}</td>
      <td>${fmtLatency(p.latency_ms)}</td>
      <td>${fmtScore(p.score)} ${scoreBar(p.score)}</td>
      <td>${isActive ? '<span class="badge" style="background:var(--green);color:#fff">✓</span>' : '—'}</td>
      <td>${isActive ? '—' : `<button class="btn btn-sm btn-green" onclick="switchProxy('${esc(p.id)}')">Switch</button>`}</td>
    </tr>`;
  }).join('');

  $('#proxy-count').textContent = proxies.length;
}

function renderConnections() {
  if (!state.stats) return;
  $('#conn-tcp').textContent = state.stats.tcp_connections;
  $('#conn-udp').textContent = state.stats.udp_flows;
}

async function renderDns() {
  const tbody = $('#dns-list');
  const entries = await fetchJSON(API + '/dns');

  if (!entries || entries.length === 0) {
    tbody.innerHTML = '<tr><td colspan="2" class="empty">No cached DNS entries</td></tr>';
    $('#dns-count').textContent = '0';
    return;
  }

  $('#dns-count').textContent = entries.length;
  tbody.innerHTML = entries.map(e =>
    `<tr><td><code>${esc(e.ip)}</code></td><td>${esc(e.hostname)}</td></tr>`
  ).join('');
}

// Refresh status for active proxy indicator
function startPolling() {
  refresh();
  setInterval(refresh, 3000);
}

// ── Actions ─────────────────────────────────────────────────────────────

async function switchProxy(id) {
  const data = await fetchJSON(`${API}/proxies/${encodeURIComponent(id)}/switch`);
  if (data) {
    state.status = { ...state.status, active_proxy: data };
    await loadProxies();
    renderAll();
  }
}

async function rotateProxy() {
  const data = await fetchJSON(`${API}/rotate`);
  if (data) {
    state.status = { ...state.status, active_proxy: data };
    await loadProxies();
    renderAll();
  }
}

// ── Helpers ─────────────────────────────────────────────────────────────

function esc(s) {
  if (s == null) return '';
  const d = document.createElement('div');
  d.textContent = String(s);
  return d.innerHTML;
}

// ── Init ────────────────────────────────────────────────────────────────

document.addEventListener('DOMContentLoaded', startPolling);
