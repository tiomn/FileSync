/* ================= FileSync 前端逻辑 ================= */
(() => {
  'use strict';

  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const listen = T.event.listen;

  const DEFAULT_EXCLUDES = [
    '*.tmp', '~$*', '.git', 'node_modules', 'Thumbs.db', 'desktop.ini',
    '*.crdownload', '*.part', '*.swp',
  ];

  const $ = (id) => document.getElementById(id);

  // ---------- 应用状态 ----------
  let cfg = null;          // 完整配置（对应后端 Config）
  let plans = [];
  let runtime = {
    watching: false, syncing: false, pending: false, pendingCount: 0,
    waiting: false, retryAttempt: 0, retryMax: 0, retryTimeoutSecs: 0,
    retryInSecs: 0, waitReason: null,
  };
  let watchAlert = '';     // 「自动停止监听」等异常提示
  let saveTimer = null;
  let pendingMirrorRun = null; // 确认弹窗后的动作

  // ================= 工具函数 =================
  function toast(text, kind) {
    const host = $('toastHost');
    const el = document.createElement('div');
    el.className = 'toast-item' + (kind ? ' ' + kind : '');
    el.textContent = text;
    host.appendChild(el);
    setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .25s'; }, 2600);
    setTimeout(() => el.remove(), 3000);
  }

  function fmtTime(iso) {
    if (!iso) return '—';
    const d = new Date(iso);
    if (isNaN(d)) return iso;
    const pad = (n) => String(n).padStart(2, '0');
    const today = new Date();
    const sameDay = d.toDateString() === today.toDateString();
    const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
    return sameDay ? `今天 ${hm}` : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}`;
  }

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
      ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  }

  function fmtDuration(ms) {
    const s = Math.floor(ms / 1000);
    const m = Math.floor(s / 60);
    const r = s % 60;
    return `${String(m).padStart(2, '0')}:${String(r).padStart(2, '0')}`;
  }

  function fmtSecs(sec) {
    const s = Number(sec) || 0;
    if (s >= 3600 && s % 3600 === 0) return (s / 3600) + ' 小时';
    if (s >= 60) {
      const m = s / 60;
      return (Number.isInteger(m) ? m : m.toFixed(1)) + ' 分钟';
    }
    return s + ' 秒';
  }

  // ================= 配置读写 =================
  async function loadAll() {
    cfg = await invoke('get_config');
    runtime = await invoke('get_runtime');
    plans = await invoke('list_plans');
    const info = await invoke('app_info');
    $('version').textContent = 'v' + info.version;
    $('logDir').textContent = info.logDir;
    $('setDataDir').textContent = info.dataDir;
    if (cfg.app.autoStart !== undefined) {
      // 以后端实际注册状态为准
      try { cfg.app.autoStart = await invoke('get_autostart'); } catch (_) { /* ignore */ }
    }
    renderAll();
  }

  function collectConfig() {
    cfg.current.source = cfg.current.source || '';
    cfg.current.target = cfg.current.target || '';
    return cfg;
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await invoke('save_config', { config: collectConfig() });
      } catch (e) {
        toast('保存配置失败：' + e, 'err');
      }
    }, 250);
  }

  // ================= 渲染 =================
  function applyTheme() {
    const t = (cfg && cfg.app.theme) || 'system';
    if (t === 'system') document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', t);
  }

  // 通用确认框（返回 Promise<boolean>）
  let confirmResolve = null;
  function confirmBox(title, body, okText, cancelText, kind) {
    $('confirmTitle').textContent = title;
    $('confirmBody').textContent = body;
    $('btnConfirmOk').textContent = okText || '确定';
    $('btnConfirmCancel').textContent = cancelText || '取消';
    $('btnConfirmOk').className = 'btn ' + (kind === 'primary' ? 'primary' : 'danger');
    $('confirmMask').classList.remove('hidden');
    return new Promise((resolve) => { confirmResolve = resolve; });
  }
  function settleConfirm(value) {
    $('confirmMask').classList.add('hidden');
    if (confirmResolve) { const r = confirmResolve; confirmResolve = null; r(value); }
  }

  function renderDirs() {
    const set = (el, val) => {
      el.textContent = val || '未选择';
      el.classList.toggle('ph', !val);
      el.title = val || '';
    };
    set($('sourceText'), cfg.current.source);
    set($('targetText'), cfg.current.target);
  }

  function renderRadios() {
    document.querySelectorAll('[data-group="mode"]').forEach((el) => {
      el.classList.toggle('sel', el.dataset.value === cfg.current.mode);
    });
    document.querySelectorAll('[data-group="trigger"]').forEach((el) => {
      el.classList.toggle('sel', el.dataset.value === cfg.current.trigger);
    });
    document.querySelectorAll('[data-group="notify"]').forEach((el) => {
      const k = el.dataset.value;
      let on;
      if (k === 'closeToTray') on = cfg.app.closeToTray;
      else on = cfg.notify[k];
      el.classList.toggle('sel', !!on);
    });
    document.querySelectorAll('[data-group="app"]').forEach((el) => {
      const k = el.dataset.value;
      const on = k === 'mirrorDeleteToRecycleBin' ? cfg.current.mirrorDeleteToRecycleBin
        : k === 'strictHash' ? cfg.current.strictHash
          : cfg.app[k];
      el.classList.toggle('sel', !!on);
    });
    $('mirrorWarn').classList.toggle('hidden', cfg.current.mode !== 'mirror');
    $('debounce').value = String(cfg.current.debounceMs);
    if (![...$('debounce').options].some((o) => o.value === String(cfg.current.debounceMs))) {
      const o = document.createElement('option');
      o.value = String(cfg.current.debounceMs);
      o.textContent = (cfg.current.debounceMs / 1000) + ' 秒';
      $('debounce').appendChild(o);
      $('debounce').value = o.value;
    }
  }

  function renderPlans() {
    const body = $('plansBody');
    $('planCount').textContent = `共 ${plans.length} 条记录`;
    if (!plans.length) {
      body.innerHTML = '<tr class="empty"><td colspan="7">暂无历史方案，配置好目录后点击「保存当前为方案」</td></tr>';
      return;
    }
    body.innerHTML = plans.map((p) => {
      const lr = p.lastResult;
      let lastCell = fmtTime(p.lastRunAt);
      if (lr) {
        lastCell += lr.ok
          ? ` <span class="status-ok">成功 ${lr.added + lr.updated}</span>`
          : ` <span class="status-err">失败 ${lr.failed}</span>`;
      }
      return `<tr data-id="${esc(p.id)}">
        <td>${esc(p.name)}</td>
        <td><span class="path" title="${esc(p.source)}">${esc(p.source)}</span></td>
        <td><span class="path" title="${esc(p.target)}">${esc(p.target)}</span></td>
        <td><span class="tag ${p.mode === 'mirror' ? 'mirror' : 'incr'}">${p.mode === 'mirror' ? '整体' : '增量'}</span></td>
        <td><span class="tag ${p.trigger === 'auto' ? 'auto' : 'manual'}">${p.trigger === 'auto' ? '自动' : '手动'}</span></td>
        <td class="muted">${lastCell}</td>
        <td><div class="row-actions">
          <button class="btn sm" data-act="sync">立即同步</button>
          <button class="btn sm" data-act="detail">详情</button>
          <button class="btn sm danger" data-act="del">删除</button>
        </div></td>
      </tr>`;
    }).join('');
  }

  function renderRuntime() {
    const pill = $('pill');
    const dot = $('pillDot');
    let cls = 'off', text = '未监听', pulse = false;
    if (runtime.syncing) { cls = 'busy'; text = '同步中…'; pulse = true; }
    else if (runtime.waiting) { cls = 'warn'; text = `等待目录 · 第 ${runtime.retryAttempt} 次`; pulse = true; }
    else if (runtime.pending) { cls = 'warn'; text = `待同步 ${runtime.pendingCount} 项`; }
    else if (runtime.watching) { cls = 'on'; text = '监听中'; }
    pill.className = 'pill ' + cls;
    dot.classList.toggle('pulse', pulse);
    $('pillText').textContent = text;

    $('btnToggleWatch').textContent = runtime.watching ? '■ 停止监听' : '▶ 开始监听';
    $('btnSyncNow').disabled = runtime.syncing;
    $('btnCancel').style.display = runtime.syncing ? '' : 'none';

    renderWatchWarn();

    if (!runtime.syncing) {
      $('progressFill').style.width = '0%';
      if (runtime.waiting) {
        $('progressCurrent').textContent = runtime.waitReason || '等待监听目录…';
        const cap = runtime.retryMax ? `/${runtime.retryMax}` : '';
        $('progressStats').textContent = `${runtime.retryInSecs} 秒后重试（第 ${runtime.retryAttempt}${cap} 次）`;
      } else if (!runtime.pending) {
        $('progressCurrent').textContent = '就绪';
        $('progressStats').textContent = '—';
      } else {
        $('progressCurrent').textContent = '检测到变更，等待同步…';
        $('progressStats').textContent = `${runtime.pendingCount} 项待处理`;
      }
    }
  }

  // 监听目录缺失 / 自动停止 的提示条
  // 注意：等待期间每秒都会刷新，因此只更新文本节点，不能整体重写 innerHTML，
  // 否则「取消监听」的链接会在鼠标按下与抬起之间被替换掉，导致点不动。
  function renderWatchWarn() {
    const el = $('watchWarn');
    if (!el) return;
    const txt = $('watchWarnText');
    const act = $('watchWarnAction');
    if (runtime.waiting) {
      const parts = [];
      parts.push(runtime.retryMax
        ? `第 ${runtime.retryAttempt} / ${runtime.retryMax} 次`
        : `第 ${runtime.retryAttempt} 次`);
      if (!runtime.retryMax) parts.push('不限次数');
      if (runtime.retryTimeoutSecs) parts.push(`最长等待 ${fmtSecs(runtime.retryTimeoutSecs)}`);
      el.className = 'warnbox';
      txt.textContent = `⚠ ${runtime.waitReason || '监听目录不可用'}　` +
        `将在 ${runtime.retryInSecs} 秒后重试（${parts.join('，')}）`;
      act.textContent = '取消监听';
      act.style.display = '';
    } else if (watchAlert) {
      el.className = 'warnbox danger';
      txt.textContent = `⚠ ${watchAlert}`;
      act.textContent = '知道了';
      act.style.display = '';
    } else {
      el.className = 'warnbox hidden';
      txt.textContent = '';
      act.textContent = '';
      act.style.display = 'none';
    }
  }

  function renderAll() {
    applyTheme();
    renderDirs();
    renderRadios();
    renderPlans();
    renderRuntime();
    $('setTheme').value = cfg.app.theme || 'system';
    $('setMaxHistory').value = cfg.app.maxHistory || 50;
    $('retryInterval').value = String(cfg.current.retryIntervalSecs ?? 10);
    $('retryMax').value = String(cfg.current.retryMax ?? 30);
    $('retryTimeout').value = String(cfg.current.retryTimeoutSecs ?? 300);
  }

  // ================= 目录选择 =================
  async function pickDir(which) {
    const cur = which === 'source' ? cfg.current.source : cfg.current.target;
    try {
      const picked = await invoke('pick_directory', {
        title: which === 'source' ? '选择监听目录' : '选择目标目录',
        current: cur || null,
      });
      if (!picked) return;
      if (which === 'source') cfg.current.source = picked;
      else cfg.current.target = picked;
      renderDirs();
      scheduleSave();
    } catch (e) {
      toast('打开目录选择器失败：' + e, 'err');
    }
  }

  // ================= 同步动作 =================
  async function doSync() {
    if (cfg.current.mode === 'mirror') {
      pendingMirrorRun = async () => {
        try { await invoke('sync_now'); }
        catch (e) { toast(String(e), 'err'); }
      };
      $('mirrorSrc').textContent = cfg.current.source || '—';
      $('mirrorDst').textContent = cfg.current.target || '—';
      $('mirrorMask').classList.remove('hidden');
      return;
    }
    try {
      await invoke('sync_now');
    } catch (e) {
      toast(String(e), 'err');
    }
  }

  // ================= 事件绑定 =================
  function bind() {
    // 目录
    $('btnPickSource').addEventListener('click', () => pickDir('source'));
    $('btnPickTarget').addEventListener('click', () => pickDir('target'));
    $('btnOpenSource').addEventListener('click', () => openPath(cfg.current.source));
    $('btnOpenTarget').addEventListener('click', () => openPath(cfg.current.target));
    $('sourceText').addEventListener('click', () => pickDir('source'));
    $('targetText').addEventListener('click', () => pickDir('target'));
    $('btnSwap').addEventListener('click', () => {
      const s = cfg.current.source;
      cfg.current.source = cfg.current.target;
      cfg.current.target = s;
      renderDirs();
      scheduleSave();
    });

    // 单选项
    document.querySelectorAll('[data-group="mode"]').forEach((el) =>
      el.addEventListener('click', () => {
        cfg.current.mode = el.dataset.value;
        renderRadios();
        scheduleSave();
      }));
    document.querySelectorAll('[data-group="trigger"]').forEach((el) =>
      el.addEventListener('click', () => {
        cfg.current.trigger = el.dataset.value;
        renderRadios();
        scheduleSave();
      }));
    document.querySelectorAll('[data-group="notify"]').forEach((el) =>
      el.addEventListener('click', () => {
        const k = el.dataset.value;
        if (k === 'closeToTray') cfg.app.closeToTray = !cfg.app.closeToTray;
        else cfg.notify[k] = !cfg.notify[k];
        renderRadios();
        scheduleSave();
      }));
    document.querySelectorAll('[data-group="app"]').forEach((el) =>
      el.addEventListener('click', async () => {
        const k = el.dataset.value;
        if (k === 'autoStart') {
          try { cfg.app.autoStart = await invoke('set_autostart', { enabled: !cfg.app.autoStart }); }
          catch (e) { toast(String(e), 'err'); }
        } else if (k === 'mirrorDeleteToRecycleBin') {
          cfg.current.mirrorDeleteToRecycleBin = !cfg.current.mirrorDeleteToRecycleBin;
        } else if (k === 'strictHash') {
          cfg.current.strictHash = !cfg.current.strictHash;
        } else {
          cfg.app[k] = !cfg.app[k];
        }
        renderRadios();
        scheduleSave();
      }));

    $('debounce').addEventListener('change', (e) => {
      cfg.current.debounceMs = Number(e.target.value);
      scheduleSave();
    });

    // 目录异常处理：重试间隔 / 最大次数 / 最长等待（0 = 不限）
    const bindNum = (id, key, min, max) => {
      const el = $(id);
      el.addEventListener('change', () => {
        const raw = Number(el.value);
        const ok = el.value.trim() !== '' && Number.isFinite(raw);
        const v = ok ? Math.min(max, Math.max(min, Math.round(raw))) : cfg.current[key];
        cfg.current[key] = v;
        el.value = String(v);
        scheduleSave();
      });
    };
    bindNum('retryInterval', 'retryIntervalSecs', 1, 3600);
    bindNum('retryMax', 'retryMax', 0, 9999);
    bindNum('retryTimeout', 'retryTimeoutSecs', 0, 86400);

    // 监听异常提示条
    $('watchWarn').addEventListener('click', (e) => {
      if (!e.target.closest('#watchWarnAction')) return;
      watchAlert = '';
      if (runtime.waiting) {
        // 「取消监听」：立刻收起提示，避免等待状态事件到达前的闪烁
        runtime.waiting = false;
        invoke('stop_watching').catch((err) => toast(String(err), 'err'));
      }
      renderWatchWarn();
    });

    // 底部操作
    $('btnToggleWatch').addEventListener('click', async () => {
      if (!runtime.watching) watchAlert = '';
      try {
        if (runtime.watching) await invoke('stop_watching');
        else await invoke('start_watching');
      } catch (e) { toast(String(e), 'err'); }
      renderWatchWarn();
    });
    $('btnSyncNow').addEventListener('click', doSync);
    $('btnCancel').addEventListener('click', () => invoke('cancel_sync'));

    // 历史方案
    $('btnSavePlan').addEventListener('click', () => {
      $('planName').value = '';
      $('planMask').classList.remove('hidden');
      $('planName').focus();
    });
    $('btnPlanOk').addEventListener('click', async () => {
      plans = await invoke('save_plan', { name: $('planName').value });
      $('planMask').classList.add('hidden');
      renderPlans();
      toast('方案已保存', 'ok');
    });
    $('btnPlanCancel').addEventListener('click', () => $('planMask').classList.add('hidden'));
    $('btnClearPlans').addEventListener('click', async () => {
      if (!plans.length) { toast('暂无可清空的历史方案'); return; }
      const ok = await confirmBox('清空历史方案', `将删除全部 ${plans.length} 条历史方案记录，该操作不可撤销。确定继续吗？`);
      if (!ok) return;
      plans = await invoke('clear_plans');
      renderPlans();
      toast('历史方案已清空');
    });

    $('plansBody').addEventListener('click', async (ev) => {
      const btn = ev.target.closest('button[data-act]');
      const tr = ev.target.closest('tr[data-id]');
      if (!tr) return;
      const id = tr.dataset.id;
      if (!btn) { await applyPlan(id); return; }
      const act = btn.dataset.act;
      if (act === 'sync') {
        try { await invoke('sync_plan_now', { id }); toast('已开始同步', 'ok'); }
        catch (e) { toast(String(e), 'err'); }
      } else if (act === 'del') {
        const p = plans.find((x) => x.id === id);
        const ok = await confirmBox('删除方案', `确定删除历史方案「${p ? p.name : id}」吗？该操作不可撤销。`);
        if (!ok) return;
        plans = await invoke('delete_plan', { id });
        renderPlans();
      } else if (act === 'detail') {
        showDetail(id);
      }
    });

    // 排除规则
    $('btnExcludes').addEventListener('click', () => {
      $('excludeArea').value = (cfg.current.excludes || []).join('\n');
      $('excludeMask').classList.remove('hidden');
    });
    $('btnExcludeReset').addEventListener('click', () => {
      $('excludeArea').value = DEFAULT_EXCLUDES.join('\n');
    });
    $('btnExcludeCancel').addEventListener('click', () => $('excludeMask').classList.add('hidden'));
    $('btnExcludeOk').addEventListener('click', () => {
      cfg.current.excludes = $('excludeArea').value
        .split('\n').map((s) => s.trim()).filter(Boolean);
      $('excludeMask').classList.add('hidden');
      scheduleSave();
      toast('排除规则已保存', 'ok');
    });

    // 设置
    $('btnSettings').addEventListener('click', () => {
      $('setTheme').value = cfg.app.theme || 'system';
      $('setMaxHistory').value = cfg.app.maxHistory || 50;
      $('settingsMask').classList.remove('hidden');
    });
    $('btnSettingsOk').addEventListener('click', () => {
      cfg.app.theme = $('setTheme').value;
      cfg.app.maxHistory = Math.min(200, Math.max(5, Number($('setMaxHistory').value) || 50));
      $('settingsMask').classList.add('hidden');
      applyTheme();
      scheduleSave();
    });
    $('setTheme').addEventListener('change', () => {
      cfg.app.theme = $('setTheme').value;
      applyTheme();
    });
    $('btnTestNotify').addEventListener('click', () => invoke('test_notify'));
    $('btnOpenLogs').addEventListener('click', () => openPath($('logDir').textContent));

    // 整体替换确认
    $('btnMirrorCancel').addEventListener('click', () => {
      pendingMirrorRun = null;
      $('mirrorMask').classList.add('hidden');
    });
    $('btnMirrorOk').addEventListener('click', async () => {
      $('mirrorMask').classList.add('hidden');
      if (pendingMirrorRun) { const f = pendingMirrorRun; pendingMirrorRun = null; await f(); }
    });

    $('btnDetailOk').addEventListener('click', () => $('detailMask').classList.add('hidden'));

    // 通用确认框
    $('btnConfirmOk').addEventListener('click', () => settleConfirm(true));
    $('btnConfirmCancel').addEventListener('click', () => settleConfirm(false));

    // ESC 关闭弹窗
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        document.querySelectorAll('.mask:not(.hidden)').forEach((m) => m.classList.add('hidden'));
        pendingMirrorRun = null;
        settleConfirm(false);
      }
    });
  }

  async function applyPlan(id) {
    cfg = await invoke('apply_plan', { id });
    renderDirs();
    renderRadios();
    applyTheme();
    toast('已载入方案', 'ok');
  }

  function showDetail(id) {
    const p = plans.find((x) => x.id === id);
    if (!p) return;
    const lr = p.lastResult;
    const body = $('detailBody');
    body.innerHTML = `
      <div class="kv">
        <div class="k">方案名称</div><div class="v">${esc(p.name)}</div>
        <div class="k">监听目录</div><div class="v">${esc(p.source)}</div>
        <div class="k">目标目录</div><div class="v">${esc(p.target)}</div>
        <div class="k">同步模式</div><div class="v">${p.mode === 'mirror' ? '整体替换' : '增量替换'}</div>
        <div class="k">执行方式</div><div class="v">${p.trigger === 'auto' ? '自动同步' : '手动同步'}</div>
        <div class="k">使用次数</div><div class="v">${p.useCount || 0}</div>
        <div class="k">上次同步</div><div class="v">${esc(p.lastRunAt || '—')}</div>
        <div class="k">上次结果</div><div class="v">${lr
        ? `${lr.ok ? '成功' : '失败'} · 新增 ${lr.added} · 更新 ${lr.updated} · 删除 ${lr.deleted} · 跳过 ${lr.skipped} · 失败 ${lr.failed} · ${(lr.ms / 1000).toFixed(1)} 秒`
        : '—'}</div>
      </div>`;
    $('detailMask').classList.remove('hidden');
  }

  async function openPath(path) {
    if (!path) { toast('请先选择目录', 'err'); return; }
    try { await invoke('open_path', { path }); }
    catch (e) { toast(String(e), 'err'); }
  }

  // ================= 后端事件 =================
  async function bindEvents() {
    await listen('sync://state', (e) => {
      runtime = e.payload;
      renderRuntime();
    });
    await listen('sync://progress', (e) => {
      const p = e.payload;
      const pct = p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : 0;
      $('progressFill').style.width = pct + '%';
      $('progressCurrent').textContent = p.current ? '正在复制 ' + p.current : '同步中…';
      $('progressStats').textContent =
        `${p.done}/${p.total} · 新增${p.added} 更新${p.updated} 删除${p.deleted} 失败${p.failed} · ${fmtDuration(p.elapsedMs)}`;
    });
    await listen('sync://done', async (e) => {
      const { ok, message } = e.payload;
      toast((ok ? '同步完成：' : '同步结束：') + message, ok ? 'ok' : 'err');
      plans = await invoke('list_plans');
      renderPlans();
      renderRuntime();
    });
    // 托盘菜单「立即同步」：走前端流程，保留整体替换的二次确认
    await listen('tray://sync', () => { doSync(); });
    // 监听被自动停止等异常
    await listen('sync://alert', (e) => {
      const p = e.payload || {};
      watchAlert = p.message || '监听出现异常';
      renderWatchWarn();
      toast(watchAlert, 'err');
    });

    // 开始监听 / 恢复监听后，后端比对两个目录的结果
    // 后端已把窗口唤出并临时置顶，这里无论是否真的弹窗，结束时都要解除置顶
    await listen('sync://diff', async (e) => {
      const d = e.payload || {};
      const release = () => invoke('set_prompt_mode', { on: false }).catch(() => {});
      // 已在同步 / 已有弹窗时不再打扰
      if (!d.total || runtime.syncing
        || !$('confirmMask').classList.contains('hidden')
        || !$('mirrorMask').classList.contains('hidden')
        || !$('detailMask').classList.contains('hidden')) {
        await release();
        return;
      }

      const parts = [];
      if (d.added) parts.push(`新增 ${d.added}`);
      if (d.updated) parts.push(`更新 ${d.updated}`);
      if (d.deleted) parts.push(`删除 ${d.deleted}`);
      let body = `监听目录与目标目录存在 ${d.total} 项差异（${parts.join(' · ')}）。`;
      if (cfg.current.mode === 'mirror' && d.deleted) {
        body += `\n整体替换模式会把目标目录多余的 ${d.deleted} 个文件移入回收站。`;
      }
      body += '\n是否立即同步？';

      let yes = false;
      try {
        yes = await confirmBox('发现目录差异', body, '立即同步', '暂不同步', 'primary');
      } finally {
        await release();
      }
      if (!yes) return;
      try {
        await invoke('sync_now');
      } catch (err) {
        toast(String(err), 'err');
      }
    });
  }

  // ================= 启动 =================
  window.addEventListener('DOMContentLoaded', async () => {
    try {
      await loadAll();
      bind();
      await bindEvents();
    } catch (e) {
      document.body.innerHTML =
        `<pre style="padding:20px;font-family:Consolas">初始化失败：${esc(e)}</pre>`;
    }
  });
})();
