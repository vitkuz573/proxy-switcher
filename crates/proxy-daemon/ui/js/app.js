const API = '/api/v1';

let state = { status: null, proxies: [], stats: null, scrapeStatus: null, sources: [] };

function $(sel, ctx) { return (ctx || document).querySelector(sel); }
function $$(sel, ctx) { return Array.from((ctx || document).querySelectorAll(sel)); }

function fmtLatency(ms) {
  if (ms == null) return '—';
  if (ms < 1000) return ms + 'ms';
  return (ms / 1000).toFixed(1) + 's';
}

function fmtScore(s) { return s != null ? s.toFixed(1) : '—'; }
function scoreCls(s) { return s == null ? '' : s >= 50 ? 'score-high' : s >= 20 ? 'score-mid' : 'score-low'; }
function scoreBar(s) {
  const pct = s != null ? Math.min(s, 100) : 0;
  return `<span class="score-bar"><span class="score-bar-fill ${scoreCls(s)}" style="width:${pct}%"></span></span>`;
}

function isHealthy(p) { return p.last_checked != null && p.latency_ms != null; }
function healthBadge(p) {
  if (p.last_checked == null) return '<span class="badge unchecked">unchecked</span>';
  if (p.latency_ms != null) return '<span class="badge" style="background:var(--green);color:#fff">alive</span>';
  return '<span class="badge dead">dead</span>';
}

async function fetchJSON(url, opts) {
  try {
    const res = await fetch(url, opts);
    if (res.status === 204 || res.status === 202) return { _status: res.status };
    if (!res.ok) return null;
    return await res.json();
  } catch (e) {
    console.error('fetch:', url, e);
    return null;
  }
}

// ── Load ──────────────────────────────────────────────────────────────

async function loadStatus() { const d = await fetchJSON(API + '/status'); if (d) state.status = d; }
async function loadProxies() { const d = await fetchJSON(API + '/proxies'); if (d) state.proxies = d; }
async function loadStats() { const d = await fetchJSON(API + '/stats'); if (d) state.stats = d; }
async function loadScrapeStatus() { const d = await fetchJSON(API + '/scrape/status'); if (d) state.scrapeStatus = d; }
async function loadSources() { const d = await fetchJSON(API + '/sources'); if (d) state.sources = d; }

async function refresh() {
  await Promise.all([loadStatus(), loadProxies(), loadStats(), loadScrapeStatus(), loadSources()]);
  renderAll();
}

// ── Navigation ────────────────────────────────────────────────────────

$$('.nav-links a').forEach(a => {
  a.addEventListener('click', e => {
    e.preventDefault();
    $$('.nav-links a').forEach(x => x.classList.remove('active'));
    a.classList.add('active');
    $$('.view').forEach(v => v.classList.remove('active'));
    const v = document.getElementById('view-' + a.dataset.view);
    if (v) v.classList.add('active');
    $('#view-title').textContent = a.textContent.trim();
    // Refresh scrapes when switching to scraper tab
    if (a.dataset.view === 'scraper') renderScraper();
  });
});

// ── Render ────────────────────────────────────────────────────────────

function renderAll() {
  renderStatus();
  renderDashboardProxies();
  renderProxyTable();
  renderConnections();
  renderDns();
  renderScraper();
}

function renderStatus() {
  const b = $('#status-badge');
  if (state.stats) {
    b.textContent = 'Online';
    b.className = 'status-badge';
  } else {
    b.textContent = 'Offline';
    b.className = 'status-badge offline';
  }
}

function renderDashboardProxies() {
  const tbody = $('#dashboard-proxy-list');
  const active = state.status?.active_proxy;
  const healthy = state.proxies.filter(isHealthy);
  const checking = state.scrapeStatus?.checking_progress;
  const list = healthy.length > 0 ? healthy.slice(0, 10) : state.proxies.slice(0, 10);

  if (list.length === 0) {
    if (checking) {
      tbody.innerHTML = `<tr><td colspan="7" class="empty"><span class="spinner"></span> Health check in progress (${checking[0]}/${checking[1]})</td></tr>`;
    } else {
      tbody.innerHTML = '<tr><td colspan="7" class="empty">No proxies in pool</td></tr>';
    }
  } else {
    tbody.innerHTML = list.map(p => {
      const ia = active && p.id === active.id;
      return `<tr${ia ? ' class="active"' : ''}>
        <td><code>${esc(p.id)}</code></td>
        <td>${esc(p.host)}</td>
        <td>${p.port}</td>
        <td>${p.protocol}</td>
        <td>${fmtLatency(p.latency_ms)}</td>
        <td>${fmtScore(p.score)} ${scoreBar(p.score)} ${healthBadge(p)}</td>
        <td>${ia ? '<span class="badge" style="background:var(--green);color:#fff">active</span>' : (isHealthy(p) ? `<button class="btn btn-sm btn-green" onclick="switchProxy('${esc(p.id)}')">Use</button>` : '<span class="text-muted">—</span>')}</td>
      </tr>`;
    }).join('');
  }

  const ap = state.status?.active_proxy;
  const el = $('#stat-active-proxy');
  if (ap) {
    el.textContent = `${ap.host}:${ap.port}`;
    el.className = 'stat-value wrap';
  } else {
    el.textContent = 'None';
    el.className = 'stat-value';
  }
  $('#stat-active-score').textContent = ap ? `Score: ${fmtScore(ap.score)}` : '';
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
  const list = state.proxies.slice().sort((a, b) => isHealthy(b) - isHealthy(a));

  if (list.length === 0) {
    tbody.innerHTML = '<tr><td colspan="10" class="empty">No proxies</td></tr>';
  } else {
    tbody.innerHTML = list.map(p => {
      const ia = active && p.id === active.id;
      return `<tr${ia ? ' class="active"' : ''}>
        <td><code>${esc(p.id)}</code></td>
        <td>${esc(p.host)}</td>
        <td>${p.port}</td>
        <td>${p.protocol}</td>
        <td>${p.anonymity}</td>
        <td>${p.country || '—'}</td>
        <td>${fmtLatency(p.latency_ms)}</td>
        <td>${fmtScore(p.score)} ${scoreBar(p.score)}</td>
        <td>${healthBadge(p)}</td>
        <td>${ia ? '<span class="badge" style="background:var(--green);color:#fff">active</span>' : (isHealthy(p) ? `<button class="btn btn-sm btn-green" onclick="switchProxy('${esc(p.id)}')">Use</button>` : '<span class="text-muted">—</span>')}</td>
        <td><button class="btn btn-sm" style="color:var(--red);border-color:var(--red)" onclick="deleteProxy('${esc(p.id)}')">Del</button></td>
      </tr>`;
    }).join('');
  }
  $('#proxy-count').textContent = list.length;
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
    tbody.innerHTML = '<tr><td colspan="2" class="empty">No cached entries</td></tr>';
    $('#dns-count').textContent = '0';
    return;
  }
  $('#dns-count').textContent = entries.length;
  tbody.innerHTML = entries.map(e => `<tr><td><code>${esc(e.ip)}</code></td><td>${esc(e.hostname)}</td></tr>`).join('');
}

function renderScraper() {
  if (!state.scrapeStatus) return;
  const s = state.scrapeStatus;

  const checking = s.checking_progress;

  // Dashboard scraper panel
  const dash = $('#scraper-status-dash');
  if (s.running) {
    dash.innerHTML = '<p><span class="spinner"></span> Scraping sources...</p>';
  } else if (checking) {
    const pct = checking[1] > 0 ? Math.round(checking[0] / checking[1] * 100) : 0;
    dash.innerHTML = `<p><span class="spinner"></span> Health check: <strong>${checking[0]}</strong> / <strong>${checking[1]}</strong> (${pct}%)</p>`;
  } else if (s.last_run) {
    const t = new Date(s.last_run).toLocaleTimeString();
    dash.innerHTML = `<p>Last run: <strong>${t}</strong> &mdash; found <strong>${s.proxies_found}</strong> proxies, <strong>${s.healthy_count}</strong> alive${s.errors.length ? ', <span style="color:var(--red)">' + s.errors.length + ' errors</span>' : ''}</p>`;
  }

  // Scraper page
  const full = $('#scraper-status-full');
  if (s.running) {
    full.innerHTML = '<p><span class="spinner"></span> Scraping sources...</p>';
  } else if (checking) {
    const pct = checking[1] > 0 ? Math.round(checking[0] / checking[1] * 100) : 0;
    full.innerHTML = `<p><span class="spinner"></span> Checking proxy health: <strong>${checking[0]}</strong> / <strong>${checking[1]}</strong> (${pct}%) &mdash; pool has proxies but health check running</p>`;
  } else if (s.last_run) {
    const t = new Date(s.last_run).toLocaleString();
    full.innerHTML = `<table class="kv"><tr><td>Last run</td><td>${t}</td></tr>
      <tr><td>Proxies found</td><td><strong>${s.proxies_found}</strong></td></tr>
      <tr><td>Healthy</td><td><strong>${s.healthy_count}</strong></td></tr>
      <tr><td>Errors</td><td>${s.errors.length ? '<span style="color:var(--red)">' + esc(s.errors.join('; ')) + '</span>' : 'none'}</td></tr></table>`;
  }

  // Scrape history
  const hist = $('#scrape-history');
  if (s.last_run) {
    hist.innerHTML = `<tr>
      <td>${new Date(s.last_run).toLocaleString()}</td>
      <td>${s.proxies_found}</td>
      <td>${s.healthy_count}</td>
      <td>${s.errors.length ? '<span style="color:var(--red)">' + esc(s.errors.join('; ')) + '</span>' : '—'}</td>
    </tr>`;
  }

  // Sources
  const srcTbody = $('#sources-list');
  if (state.sources.length === 0) {
    srcTbody.innerHTML = '<tr><td colspan="2" class="empty">No custom sources — using built-in defaults</td></tr>';
  } else {
    srcTbody.innerHTML = state.sources.map(url => `<tr>
      <td><code>${esc(url)}</code></td>
      <td><button class="btn btn-sm" style="color:var(--red);border-color:var(--red)" onclick="deleteSource('${esc(url)}')">Del</button></td>
    </tr>`).join('');
  }
}

// ── Actions ────────────────────────────────────────────────────────────

async function switchProxy(id) {
  const d = await fetchJSON(`${API}/switch?id=${encodeURIComponent(id)}`, { method: 'POST' });
  if (d) {
    state.status = { ...state.status, active_proxy: d };
    await loadProxies();
    renderAll();
  }
}

async function deleteProxy(id) {
  if (!confirm(`Delete proxy ${id}?`)) return;
  const res = await fetch(`${API}/proxies/${encodeURIComponent(id)}`, { method: 'DELETE' });
  if (res.ok) {
    await loadProxies();
    renderAll();
  }
}

async function triggerScrape() {
  const btn = $('#btn-scrape') || $('#btn-scrape-full');
  if (btn) btn.disabled = true;
  await fetchJSON(API + '/scrape', { method: 'POST' });
  // Poll for completion
  const poll = setInterval(async () => {
    await loadScrapeStatus();
    renderScraper();
    if (!state.scrapeStatus?.running) {
      clearInterval(poll);
      if (btn) btn.disabled = false;
      await refresh();
    }
  }, 1500);
}

// ── Modal ──────────────────────────────────────────────────────────────

function showAddModal() { $('#add-modal').style.display = 'flex'; }
function closeAddModal() {
  $('#add-modal').style.display = 'none';
  $('#add-host').value = '';
  $('#add-port').value = '';
  $('#add-country').value = '';
}

async function submitAddProxy() {
  const host = $('#add-host').value.trim();
  const port = parseInt($('#add-port').value);
  if (!host || !port) { alert('Host and port required'); return; }
  const proto = $('#add-proto').value;
  const country = $('#add-country').value.trim() || null;

  const res = await fetch(API + '/proxies', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ host, port, protocol: proto, country }),
  });
  if (res.status === 201) {
    closeAddModal();
    await loadProxies();
    renderAll();
  } else {
    alert('Failed to add proxy');
  }
}

// ── Source Modal ────────────────────────────────────────────────────────

function showAddSourceModal() { $('#add-source-modal').style.display = 'flex'; }
function closeAddSourceModal() {
  $('#add-source-modal').style.display = 'none';
  $('#add-source-url').value = '';
}

async function submitAddSource() {
  const url = $('#add-source-url').value.trim();
  if (!url) { alert('URL required'); return; }

  const res = await fetch(API + '/sources', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ url }),
  });
  if (res.status === 201) {
    closeAddSourceModal();
    await loadSources();
    renderScraper();
  } else if (res.status === 409) {
    alert('Source already exists');
  } else {
    alert('Failed to add source');
  }
}

async function deleteSource(url) {
  if (!confirm(`Delete source: ${url}?`)) return;
  const res = await fetch(`${API}/sources/${encodeURIComponent(url)}`, { method: 'DELETE' });
  if (res.ok) {
    await loadSources();
    renderScraper();
  }
}

// ── Helpers ────────────────────────────────────────────────────────────

function esc(s) {
  if (s == null) return '';
  const d = document.createElement('div');
  d.textContent = String(s);
  return d.innerHTML;
}

// ── Init ────────────────────────────────────────────────────────────────

document.addEventListener('DOMContentLoaded', () => {
  refresh();
  setInterval(refresh, 3000);
});

// Rotate button
document.addEventListener('click', e => {
  if (e.target.id === 'btn-rotate') {
    (async () => {
      const d = await fetchJSON(API + '/rotate', { method: 'POST' });
      if (d) { state.status = { ...state.status, active_proxy: d }; await loadProxies(); renderAll(); }
    })();
  }
});
