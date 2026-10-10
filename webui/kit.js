/*
 * iter5 webui kit — the small shared pieces both graph views (the Project
 * graph and the Settings graph) and the page use:
 *
 *   IterKit.esc / md            HTML escaping; a small, safe markdown renderer
 *   IterKit.toast               transient notes, bottom right
 *   IterKit.modal               one dialog shape (title, body, buttons, error line)
 *   IterKit.ask / confirm / choose   prompt(), confirm() and a radio picker, styled
 *   IterKit.configure           the Configure… lightbox: an editable key/value list
 *   IterKit.menu                a context menu inside a host element
 *   IterKit.help                the keyboard-shortcut popover
 *   IterKit.endpointHandles     drag handles on a selected Cytoscape edge's two ends
 *   IterKit.clip                the copied edge (⌘C / ⌘V), one per graph scope
 *
 * No dependencies; every class is prefixed kit-. Colours come from the page's
 * :root tokens (index.html).
 */
(function () {
  'use strict';
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
  const MOD = isMac ? '⌘' : 'Ctrl';

  // ------------------------------------------------------------------ markdown (escape first, then a few safe rules)
  function inline(t) {
    return t
      .replace(/`([^`]+)`/g, (m, c) => `<code>${c}</code>`)
      .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>')
      .replace(/(^|[\s(])\*([^*\s][^*]*)\*(?=[\s).,;:!?]|$)/g, '$1<i>$2</i>')
      .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (m, label, url) => `<a href="${url}" target="_blank" rel="noopener">${label}</a>`);
  }
  /** A small markdown subset: headings, lists, fenced code, quotes, paragraphs, inline code/bold/italic/links. */
  function md(src) {
    const lines = esc(String(src || '').replace(/\r\n?/g, '\n')).split('\n');
    const out = []; let para = []; let list = null; let code = null;
    const flushPara = () => { if (para.length) { out.push(`<p>${inline(para.join(' '))}</p>`); para = []; } };
    const flushList = () => { if (list) { out.push(`<${list.tag}>${list.items.map((i) => `<li>${inline(i)}</li>`).join('')}</${list.tag}>`); list = null; } };
    for (const raw of lines) {
      if (code) {
        if (/^\s*```/.test(raw)) { out.push(`<pre class="kit-pre">${code.join('\n')}</pre>`); code = null; } else code.push(raw);
        continue;
      }
      if (/^\s*```/.test(raw)) { flushPara(); flushList(); code = []; continue; }
      const h = raw.match(/^(#{1,6})\s+(.*)$/);
      if (h) { flushPara(); flushList(); const lv = Math.min(6, h[1].length + 2); out.push(`<h${lv} class="kit-mdh">${inline(h[2])}</h${lv}>`); continue; }
      const li = raw.match(/^\s*([-*+]|\d+[.)])\s+(.*)$/);
      if (li) {
        flushPara();
        const tag = /\d/.test(li[1]) ? 'ol' : 'ul';
        if (!list || list.tag !== tag) { flushList(); list = { tag, items: [] }; }
        list.items.push(li[2]); continue;
      }
      if (/^\s*&gt;\s?/.test(raw)) { flushPara(); flushList(); out.push(`<blockquote>${inline(raw.replace(/^\s*&gt;\s?/, ''))}</blockquote>`); continue; }
      if (/^\s*(---+|\*\*\*+)\s*$/.test(raw)) { flushPara(); flushList(); out.push('<hr>'); continue; }
      if (!raw.trim()) { flushPara(); flushList(); continue; }
      if (list && /^\s{2,}\S/.test(raw)) { list.items[list.items.length - 1] += ' ' + raw.trim(); continue; }
      flushList(); para.push(raw.trim());
    }
    if (code) out.push(`<pre class="kit-pre">${code.join('\n')}</pre>`);
    flushPara(); flushList();
    return `<div class="kit-md">${out.join('')}</div>`;
  }

  // ------------------------------------------------------------------ toasts
  function toast(msg, opts) {
    const o = Object.assign({ kind: 'info', ms: 3200, html: false }, opts || {});
    let box = document.getElementById('kit-toasts');
    if (!box) { box = document.createElement('div'); box.id = 'kit-toasts'; box.setAttribute('role', 'status'); document.body.appendChild(box); }
    // a toast shown while a modal dialog is open must live in that dialog (the top layer)
    const host = document.querySelector('dialog[open]');
    if (host && box.parentElement !== host) host.appendChild(box);
    else if (!host && box.parentElement !== document.body) document.body.appendChild(box);
    const t = document.createElement('div');
    t.className = 'kit-toast kit-' + o.kind;
    if (o.html) t.innerHTML = msg; else t.textContent = msg;
    box.appendChild(t);
    requestAnimationFrame(() => t.classList.add('kit-in'));
    const kill = () => { t.classList.remove('kit-in'); setTimeout(() => t.remove(), 220); };
    t.addEventListener('click', (e) => { if (!e.target.closest('a,button')) kill(); });
    setTimeout(kill, o.ms);
    return t;
  }

  // ------------------------------------------------------------------ modal dialogs
  /**
   * modal({title, sub, body, wide, buttons:[{label, value, primary, danger, ghost}], onOpen(dlg), onButton(value, dlg)})
   * → Promise<value|null>. onButton may return false (or a Promise of false) to keep the dialog open;
   * a thrown error is shown on the dialog's error line.
   */
  function modal(o) {
    return new Promise((resolve) => {
      const d = document.createElement('dialog');
      d.className = 'kit-dlg' + (o.wide ? ' kit-wide' : '') + (o.cls ? ' ' + o.cls : '');
      const btns = (o.buttons || [{ label: 'Close', value: null }]).map((b, i) =>
        `<button type="button" class="${b.primary ? 'kit-primary' : b.danger ? 'kit-danger' : 'kit-ghost'}" data-i="${i}"${b.title ? ` title="${esc(b.title)}"` : ''}>${esc(b.label)}</button>`).join('');
      d.innerHTML = `<div class="kit-head"><div class="kit-titles"><h3>${esc(o.title || '')}</h3>${o.sub ? `<div class="kit-sub">${o.sub}</div>` : ''}</div>
        <button type="button" class="kit-x" aria-label="Close" title="Close (Esc)">×</button></div>
        <div class="kit-body">${o.body || ''}</div>
        <div class="kit-foot"><span class="kit-err" role="alert"></span><span class="kit-grow"></span>${btns}</div>`;
      (o.host || document.body).appendChild(d);
      let done = false;
      const finish = (v) => { if (done) return; done = true; try { d.close(); } catch (e) { /* closed */ } d.remove(); resolve(v); };
      d.dirty = false;
      d.addEventListener('input', () => { d.dirty = true; });
      const cancel = () => {
        if (d.dirty && o.confirmDiscard !== false && !window.confirm('Discard your changes?')) return;
        finish(null);
      };
      d.addEventListener('cancel', (e) => { e.preventDefault(); cancel(); });
      d.addEventListener('mousedown', (e) => { d._downOnBackdrop = e.target === d; });
      d.addEventListener('click', (e) => { if (e.target === d && d._downOnBackdrop) cancel(); });
      d.querySelector('.kit-x').onclick = cancel;
      const err = d.querySelector('.kit-err');
      d.setError = (m) => { err.textContent = m || ''; };
      d.finish = finish;
      d.querySelectorAll('.kit-foot button[data-i]').forEach((btn) => {
        btn.onclick = async () => {
          const b = o.buttons[+btn.dataset.i];
          if (b.value === null || b.cancel) { cancel(); return; }
          err.textContent = '';
          if (o.onButton) {
            btn.disabled = true;
            try {
              const r = await o.onButton(b.value, d);
              if (r === false) { btn.disabled = false; return; }
              finish(r === undefined || r === true ? b.value : r);
            } catch (e) { err.textContent = (e && e.message) || String(e); btn.disabled = false; }
          } else finish(b.value);
        };
      });
      // Enter in a single-line input presses the primary button
      d.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && !e.shiftKey && e.target.matches('input:not([type=checkbox]):not([type=radio])')) {
          const p = d.querySelector('.kit-foot .kit-primary'); if (p) { e.preventDefault(); p.click(); }
        }
        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { const p = d.querySelector('.kit-foot .kit-primary'); if (p) { e.preventDefault(); p.click(); } }
      });
      d.showModal();
      if (o.onOpen) o.onOpen(d);
      const f = d.querySelector('[autofocus]') || d.querySelector('.kit-body input:not([type=hidden]):not([disabled]), .kit-body textarea:not([disabled]), .kit-body select');
      if (f) setTimeout(() => f.focus(), 0);
    });
  }

  /** prompt(), styled: resolves to the trimmed text, or null when cancelled. */
  function ask(o) {
    const id = 'kit-ask-' + Math.random().toString(36).slice(2);
    const ctl = o.multiline
      ? `<textarea id="${id}" rows="${o.rows || 4}" placeholder="${esc(o.placeholder || '')}">${esc(o.value || '')}</textarea>`
      : `<input id="${id}" type="text" value="${esc(o.value || '')}" placeholder="${esc(o.placeholder || '')}" autocomplete="off">`;
    return modal({
      title: o.title, sub: o.sub,
      body: `${o.text ? `<p class="kit-text">${o.text}</p>` : ''}<label class="kit-field"><span>${esc(o.label || '')}</span>${ctl}${o.help ? `<small>${o.help}</small>` : ''}</label>`,
      buttons: [{ label: 'Cancel', value: null }, { label: o.okLabel || 'OK', value: 'ok', primary: !o.danger, danger: !!o.danger }],
      confirmDiscard: false,
      onButton: (v, d) => {
        const val = d.querySelector('#' + id).value.trim();
        if (o.required && !val) throw new Error(o.requiredMsg || 'This is required.');
        return { value: val };
      },
    }).then((r) => (r && typeof r === 'object' ? r.value : null));
  }
  function confirmBox(o) {
    return modal({ title: o.title, body: `<p class="kit-text">${o.text || ''}</p>`, confirmDiscard: false,
      buttons: [{ label: 'Cancel', value: null }, { label: o.okLabel || 'OK', value: 'ok', primary: !o.danger, danger: !!o.danger }] }).then((v) => v === 'ok');
  }
  /** A radio list: resolves to the chosen option's value, or null. */
  function choose(o) {
    const name = 'kit-ch-' + Math.random().toString(36).slice(2);
    const opts = o.options.map((x, i) => `<label class="kit-radio"><input type="radio" name="${name}" value="${i}" ${i === (o.selected || 0) ? 'checked' : ''}>
      <span><b>${x.labelHtml || esc(x.label)}</b>${x.desc ? `<small>${esc(x.desc)}</small>` : ''}</span></label>`).join('');
    return modal({ title: o.title, sub: o.sub, body: `${o.text ? `<p class="kit-text">${o.text}</p>` : ''}<div class="kit-radios">${opts}</div>`, confirmDiscard: false,
      buttons: [{ label: 'Cancel', value: null }, { label: o.okLabel || 'OK', value: 'ok', primary: true }],
      onButton: (v, d) => { const c = d.querySelector(`input[name="${name}"]:checked`); return { i: c ? +c.value : -1 }; } })
      .then((r) => (r && r.i >= 0 ? o.options[r.i].value : null));
  }

  // ------------------------------------------------------------------ Configure… lightbox
  const typeOf = (v) => (typeof v === 'boolean' ? 'bool' : typeof v === 'number' ? 'number'
    : (v && typeof v === 'object') ? 'json' : (typeof v === 'string' && (v.includes('\n') || v.length > 90)) ? 'textarea' : 'text');
  function editorHtml(f, i, ro) {
    const v = f.value; const t = f.type || typeOf(v); const dis = ro || f.readOnly ? ' disabled' : '';
    const id = `kit-cf-${i}`;
    if (t === 'readonly') return `<div class="kit-ro">${f.html != null ? f.html : esc(v == null || v === '' ? '—' : typeof v === 'object' ? JSON.stringify(v) : v)}</div>`;
    if (t === 'bool') return `<label class="kit-switch"><input type="checkbox" id="${id}" ${v ? 'checked' : ''}${dis}><span></span><em>${v ? 'true' : 'false'}</em></label>`;
    if (t === 'number') return `<input id="${id}" type="number" step="any" value="${esc(v == null ? '' : v)}"${dis}>`;
    if (t === 'select') return `<select id="${id}"${dis}>${(f.options || []).map((o) => { const ov = typeof o === 'object' ? o.value : o; const ol = typeof o === 'object' ? o.label : o; return `<option value="${esc(ov)}" ${String(ov) === String(v == null ? '' : v) ? 'selected' : ''}>${esc(ol)}</option>`; }).join('')}</select>`;
    if (t === 'list') return `<textarea id="${id}" rows="${Math.max(2, Math.min(8, (v || []).length + 1))}" class="kit-mono" placeholder="one per line"${dis}>${esc((v || []).join('\n'))}</textarea>`;
    if (t === 'json') return `<textarea id="${id}" rows="${Math.max(2, Math.min(14, JSON.stringify(v == null ? null : v, null, 2).split('\n').length))}" class="kit-mono"${dis}>${esc(v == null ? '' : JSON.stringify(v, null, 2))}</textarea>`;
    if (t === 'textarea' || t === 'markdown') return `<textarea id="${id}" rows="${f.rows || Math.max(3, Math.min(18, String(v || '').split('\n').length + 1))}" class="${t === 'markdown' ? 'kit-mono' : ''}"${dis}>${esc(v == null ? '' : v)}</textarea>`;
    return `<input id="${id}" type="text" value="${esc(v == null ? '' : v)}"${dis}>`;
  }
  function readEditor(d, f, i) {
    const t = f.type || typeOf(f.value);
    const el = d.querySelector(`#kit-cf-${i}`);
    if (!el) return f.value;
    el.classList.remove('kit-bad');
    try {
      if (t === 'bool') return el.checked;
      if (t === 'number') { if (el.value.trim() === '') return f.nullable ? null : 0; const n = Number(el.value); if (Number.isNaN(n)) throw new Error('not a number'); return n; }
      if (t === 'list') return el.value.split('\n').map((x) => x.trim()).filter(Boolean);
      if (t === 'json') return el.value.trim() === '' ? (f.emptyValue !== undefined ? f.emptyValue : null) : JSON.parse(el.value);
      return el.value;
    } catch (e) { el.classList.add('kit-bad'); throw new Error(`${f.label || f.key}: ${e.message}`); }
  }
  /**
   * configure({title, sub, meta:[[label, html]], fields:[{key, label, value, type, options, help, readOnly, removable, group}],
   *            allowAdd, readOnly, extra, onSave({values, changed, removed, added}) })
   * values = every editable field's parsed value; changed = only those that differ from the start;
   * removed = keys removed with ×; added = new keys. onSave may throw (shown inline) or return false to stay open.
   */
  function configure(o) {
    const fields = o.fields.map((f) => Object.assign({}, f));
    const ro = !!o.readOnly;
    let groups = []; fields.forEach((f) => { const g = f.group || ''; if (!groups.includes(g)) groups.push(g); });
    const rowHtml = (f, i) => `<div class="kit-kv" data-row="${i}">
        <div class="kit-k"><span title="${esc(f.key)}">${esc(f.label || f.key)}</span>${f.badge ? `<em class="kit-badge">${esc(f.badge)}</em>` : ''}</div>
        <div class="kit-v">${editorHtml(f, i, ro)}${f.help ? `<small>${f.help}</small>` : ''}</div>
        <div class="kit-del">${!ro && f.removable ? `<button type="button" class="kit-icon" data-rm="${i}" title="Remove this setting">×</button>` : ''}</div></div>`;
    const body = `${(o.meta || []).length ? `<div class="kit-meta">${o.meta.map(([k, v]) => `<div><span>${esc(k)}</span><b>${v}</b></div>`).join('')}</div>` : ''}
      ${o.note ? `<p class="kit-text kit-dim">${o.note}</p>` : ''}
      <div class="kit-kvs">${groups.map((g) => `${g ? `<div class="kit-group">${esc(g)}</div>` : ''}${fields.map((f, i) => ((f.group || '') === g ? rowHtml(f, i) : '')).join('')}`).join('')}</div>
      ${!ro && o.allowAdd ? `<div class="kit-addrow"><input type="text" class="kit-newk" placeholder="new key" autocomplete="off" spellcheck="false">
        <select class="kit-newt" title="value type"><option value="text">text</option><option value="number">number</option><option value="bool">true/false</option><option value="json">JSON</option><option value="textarea">long text</option></select>
        <button type="button" class="kit-ghost kit-sm kit-add">+ Add setting</button></div>` : ''}
      ${o.extra || ''}`;
    const removed = new Set(); const added = new Set();
    return modal({
      title: o.title, sub: o.sub, wide: true, body, cls: 'kit-config',
      buttons: ro ? [{ label: 'Close', value: null }] : [{ label: 'Cancel', value: null }, { label: o.saveLabel || 'Save', value: 'save', primary: true }],
      onOpen: (d) => {
        d.querySelectorAll('.kit-switch input').forEach((c) => { c.onchange = () => { c.parentElement.querySelector('em').textContent = c.checked ? 'true' : 'false'; }; });
        const wireRm = () => d.querySelectorAll('[data-rm]').forEach((b) => { b.onclick = () => { const i = +b.dataset.rm; removed.add(fields[i].key); added.delete(fields[i].key); d.querySelector(`[data-row="${i}"]`).remove(); d.dirty = true; }; });
        wireRm();
        const add = d.querySelector('.kit-add');
        if (add) add.onclick = () => {
          const kEl = d.querySelector('.kit-newk'); const k = kEl.value.trim(); const t = d.querySelector('.kit-newt').value;
          if (!k) { d.setError('Type the new key first.'); kEl.focus(); return; }
          if (fields.some((f, i) => f.key === k && d.querySelector(`[data-row="${i}"]`))) { d.setError(`"${k}" is already in the list.`); return; }
          d.setError('');
          const f = { key: k, value: t === 'number' ? 0 : t === 'bool' ? false : t === 'json' ? {} : '', type: t, removable: true, isNew: true };
          fields.push(f); added.add(k); removed.delete(k);
          const i = fields.length - 1;
          const wrap = document.createElement('div'); wrap.innerHTML = rowHtml(f, i);
          d.querySelector('.kit-kvs').appendChild(wrap.firstElementChild);
          wireRm(); kEl.value = ''; d.dirty = true;
          const ed = d.querySelector(`#kit-cf-${i}`); if (ed) ed.focus();
        };
        if (o.onOpen) o.onOpen(d);
      },
      onButton: async (v, d) => {
        const values = {}; const changed = {};
        fields.forEach((f, i) => {
          if (removed.has(f.key) || f.readOnly || (f.type === 'readonly')) return;
          if (!d.querySelector(`[data-row="${i}"]`)) return;
          const val = readEditor(d, f, i);
          values[f.key] = val;
          if (f.isNew || JSON.stringify(val) !== JSON.stringify(f.value)) changed[f.key] = val;
        });
        const r = await o.onSave({ values, changed, removed: [...removed], added: [...added], dialog: d });
        return r === false ? false : true;
      },
    });
  }

  // ------------------------------------------------------------------ context menu
  let openMenuEl = null;
  function closeMenu() { if (openMenuEl) { openMenuEl.remove(); openMenuEl = null; } }
  document.addEventListener('mousedown', (e) => { if (openMenuEl && !openMenuEl.contains(e.target)) closeMenu(); }, true);
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && openMenuEl) { e.stopPropagation(); closeMenu(); } }, true);
  window.addEventListener('blur', closeMenu);
  /**
   * menu(host, {x, y} (host-relative px), title, items[{key, label, hint, kbd, danger, disabled, sep}], onPick(key))
   * The menu stays inside the host; on a phone it is a bottom sheet.
   */
  function menu(host, pos, title, items, onPick) {
    closeMenu();
    const m = document.createElement('div');
    m.className = 'kit-menu';
    m.setAttribute('role', 'menu');
    m.innerHTML = (title ? `<div class="kit-menu-title">${esc(title)}</div>` : '') + items.map((it, i) => it.sep ? '<div class="kit-menu-sep"></div>'
      : `<button type="button" role="menuitem" data-i="${i}" class="${it.danger ? 'kit-danger' : ''}" ${it.disabled ? 'disabled' : ''} title="${esc(it.hint || '')}"><span>${esc(it.label)}</span>${it.kbd ? `<kbd>${esc(it.kbd)}</kbd>` : ''}</button>`).join('');
    host.appendChild(m);
    const hw = host.clientWidth; const hh = host.clientHeight;
    const x = Math.max(4, Math.min(pos.x + 6, hw - m.offsetWidth - 6));
    const y = Math.max(4, Math.min(pos.y + 6, hh - m.offsetHeight - 6));
    m.style.left = x + 'px'; m.style.top = y + 'px';
    m.querySelectorAll('button[data-i]').forEach((b) => { b.onclick = (e) => { e.stopPropagation(); const it = items[+b.dataset.i]; closeMenu(); onPick(it.key, it); }; });
    openMenuEl = m;
    const first = m.querySelector('button:not([disabled])'); if (first) first.focus({ preventScroll: true });
    m.addEventListener('keydown', (e) => {
      const bs = [...m.querySelectorAll('button:not([disabled])')]; const k = bs.indexOf(document.activeElement);
      if (e.key === 'ArrowDown') { e.preventDefault(); (bs[k + 1] || bs[0]).focus(); }
      if (e.key === 'ArrowUp') { e.preventDefault(); (bs[k - 1] || bs[bs.length - 1]).focus(); }
    });
    return m;
  }

  // ------------------------------------------------------------------ help popover
  function help(title, groups, foot) {
    const body = `<div class="kit-help">${groups.map(([g, rows]) => `<section><h4>${esc(g)}</h4><dl>${rows.map(([k, d]) =>
      `<dt>${String(k).split(' / ').map((x) => x.split('+').map((p) => `<kbd>${esc(p.replace('Mod', MOD))}</kbd>`).join('+')).join(' / ')}</dt><dd>${esc(d)}</dd>`).join('')}</dl></section>`).join('')}</div>${foot ? `<p class="kit-text kit-dim">${foot}</p>` : ''}`;
    return modal({ title, body, wide: true, buttons: [{ label: 'Close', value: null, primary: true }], confirmDiscard: false });
  }

  // ------------------------------------------------------------------ edge endpoint handles (Cytoscape)
  const HELPER_NODE = { label: '', color: '#4da3ff', border: '#4da3ff', shape: 'ellipse', w: 6, h: 6, font: 10 };
  const HELPER_EDGE = { label: '', color: '#4da3ff', width: 2, line: 'dashed', arrow: 'triangle', cpd: 0 };
  /**
   * endpointHandles(cy, {canEdit(edge) -> bool, accept(edge, end, node) -> true | "why not", onDrop(edge, end, node)})
   * Select an edge: a small handle sits on each end. Drag one onto another node and drop: onDrop is called
   * with end = 'source' | 'target'. The handle is a real, unselectable Cytoscape node, so the drag is
   * Cytoscape's own (mouse and touch); while it moves, a dashed ghost line follows it and the node under it lights.
   */
  function endpointHandles(cy, opts) {
    const o = opts || {};
    let edge = null; let drag = null;
    const H = { source: '__kit_ep_s', target: '__kit_ep_t' };
    cy.style().selector('node.kit-ep').style({
      width: 'data(sz)', height: 'data(sz)', shape: 'ellipse', 'background-color': '#4da3ff', 'border-color': '#f8fafc', 'border-width': 'data(bw)',
      label: '', 'z-index': 999, 'z-compound-depth': 'top', 'overlay-opacity': 0, events: 'yes', 'background-opacity': 1, opacity: 1,
    }).selector('node.kit-ep.kit-ep-src').style({ 'background-color': '#f8fafc', 'border-color': '#4da3ff' })
      .selector('edge.kit-ep-ghost').style({ 'line-color': '#4da3ff', 'line-style': 'dashed', width: 2.5, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#4da3ff',
        'curve-style': 'straight', opacity: 1, events: 'no', 'z-index': 998, label: '' })
      .selector('edge.kit-ep-moving').style({ opacity: 0.15 })
      .selector('node.kit-ep-hover').style({ 'overlay-color': '#4da3ff', 'overlay-opacity': 0.3, 'overlay-padding': 8 })
      .selector('node.kit-ep-bad').style({ 'overlay-color': '#e05252', 'overlay-opacity': 0.35, 'overlay-padding': 8 })
      .update();
    const size = () => { const z = cy.zoom() || 1; return { sz: 13 / z, bw: 2 / z }; };
    // every data field the host graphs' stylesheets map, so Cytoscape has nothing to warn about
    const ND = HELPER_NODE; const ED = HELPER_EDGE;
    function clear() {
      const els = cy.$(`#${H.source}, #${H.target}, #__kit_ep_ghost`);
      if (els.nonempty()) cy.remove(els);
      cy.$('.kit-ep-hover, .kit-ep-bad').removeClass('kit-ep-hover kit-ep-bad');
      cy.$('.kit-ep-moving').removeClass('kit-ep-moving');
      drag = null;
    }
    // an edge selected before its first render has no endpoints yet: use its nodes' centres until it does
    const ok = (p) => p && Number.isFinite(p.x) && Number.isFinite(p.y);
    const ends = (e) => { const s = e.sourceEndpoint(); const t = e.targetEndpoint(); return { s: ok(s) ? s : e.source().position(), t: ok(t) ? t : e.target().position() }; };
    function place() {
      if (!edge || edge.removed() || drag) return;
      const { s, t } = ends(edge);
      const hs = cy.getElementById(H.source); const ht = cy.getElementById(H.target);
      if (hs.nonempty() && s) hs.position(s);
      if (ht.nonempty() && t) ht.position(t);
    }
    function show(e) {
      clear(); edge = null;
      if (!e || e.removed() || !e.visible() || (o.canEdit && !o.canEdit(e))) return;
      edge = e;
      const { s, t } = ends(e); const z = size();
      cy.one('render', place);
      cy.add([
        { group: 'nodes', data: Object.assign({}, ND, { id: H.source, end: 'source' }, z), position: { x: s.x, y: s.y }, classes: 'kit-ep kit-ep-src', selectable: false, grabbable: true },
        { group: 'nodes', data: Object.assign({}, ND, { id: H.target, end: 'target' }, z), position: { x: t.x, y: t.y }, classes: 'kit-ep', selectable: false, grabbable: true },
      ]);
    }
    function nodeAt(p) {
      let best = null; let area = Infinity;
      cy.nodes().forEach((n) => {
        if (n.hasClass('kit-ep') || n.hasClass('anchor') || !n.visible() || n.id().startsWith('__')) return;
        const b = n.boundingBox({ includeLabels: false, includeOverlays: false });
        if (p.x >= b.x1 && p.x <= b.x2 && p.y >= b.y1 && p.y <= b.y2) { const a = b.w * b.h; if (a < area) { area = a; best = n; } }
      });
      return best;
    }
    cy.on('select', 'edge', (ev) => { if (!ev.target.id().startsWith('__')) show(ev.target); });
    cy.on('unselect', 'edge', (ev) => { if (ev.target === edge && !drag) { clear(); edge = null; } });
    cy.on('remove', 'edge', (ev) => { if (ev.target === edge && !drag) { edge = null; clear(); } });
    cy.on('position', 'node', (ev) => { if (edge && !drag && !ev.target.hasClass('kit-ep') && (ev.target === edge.source() || ev.target === edge.target() || ev.target.isParent())) place(); });
    cy.on('zoom', () => { const z = size(); cy.nodes('.kit-ep').forEach((n) => n.data(z)); });
    cy.on('grab', 'node.kit-ep', (ev) => {
      if (!edge) return;
      const end = ev.target.data('end');
      const fixed = end === 'source' ? edge.target() : edge.source();
      drag = { end, handle: ev.target, fixed, over: null };
      edge.addClass('kit-ep-moving');
      cy.add({ group: 'edges', data: Object.assign({}, ED, end === 'source' ? { id: '__kit_ep_ghost', source: ev.target.id(), target: fixed.id() } : { id: '__kit_ep_ghost', source: fixed.id(), target: ev.target.id() }), classes: 'kit-ep-ghost', selectable: false });
    });
    cy.on('drag', 'node.kit-ep', (ev) => {
      if (!drag) return;
      const n = nodeAt(ev.target.position());
      if (drag.over && drag.over !== n) drag.over.removeClass('kit-ep-hover kit-ep-bad');
      drag.over = n;
      if (n) {
        const cur = drag.end === 'source' ? edge.source() : edge.target();
        const ok = n === cur || !o.accept || o.accept(edge, drag.end, n) === true;
        n.addClass(ok ? 'kit-ep-hover' : 'kit-ep-bad');
      }
    });
    cy.on('free', 'node.kit-ep', (ev) => {
      if (!drag || !edge) { clear(); return; }
      const d = drag; const n = nodeAt(ev.target.position()); const e = edge;
      const cur = d.end === 'source' ? e.source() : e.target();
      clear();
      show(e);
      if (!n || n === cur) return;
      if (o.accept) { const ok = o.accept(e, d.end, n); if (ok !== true) { toast(typeof ok === 'string' ? ok : 'That end cannot go there.', { kind: 'err' }); return; } }
      o.onDrop(e, d.end, n);
    });
    return { clear: () => { clear(); edge = null; }, refresh: () => { if (edge) show(edge); }, get edge() { return edge; } };
  }

  // ------------------------------------------------------------------ overlap removal
  /**
   * separate(cy, nodes, {pad, pinned: Set of ids, animate}) — fcose's repulsion does not guarantee that
   * no two nodes overlap (two nodes with the same neighbours can land on one spot). This pushes every
   * overlapping pair of leaf boxes apart along the axis that needs the smaller move, half each (all of it
   * for the one that is not pinned), until nothing overlaps or 60 rounds pass.
   */
  function separate(cy, nodes, opts) {
    const o = Object.assign({ pad: 14, pinned: new Set(), animate: false }, opts || {});
    const list = nodes.filter((n) => !n.isParent() && n.visible()).map((n) => {
      const p = n.position(); const w = n.outerWidth() + o.pad; const h = n.outerHeight() + o.pad;
      return { n, x: p.x, y: p.y, w, h, fixed: o.pinned.has(n.id()) };
    });
    for (let round = 0; round < 60; round++) {
      let moved = false;
      for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
        const a = list[i]; const b = list[j];
        if (a.fixed && b.fixed) continue;
        const ox = (a.w + b.w) / 2 - Math.abs(a.x - b.x); const oy = (a.h + b.h) / 2 - Math.abs(a.y - b.y);
        if (ox <= 0 || oy <= 0) continue;
        moved = true;
        const alongX = ox < oy;
        const sign = alongX ? (a.x < b.x || (a.x === b.x && i < j) ? -1 : 1) : (a.y < b.y || (a.y === b.y && i < j) ? -1 : 1);
        const d = alongX ? ox : oy; const sa = a.fixed ? 0 : b.fixed ? 1 : 0.5; const sb = 1 - sa;
        if (alongX) { a.x += sign * d * sa; b.x -= sign * d * sb; } else { a.y += sign * d * sa; b.y -= sign * d * sb; }
      }
      if (!moved) break;
    }
    list.forEach((e) => {
      const p = e.n.position(); if (Math.abs(p.x - e.x) < 0.5 && Math.abs(p.y - e.y) < 0.5) return;
      if (o.animate) e.n.animate({ position: { x: e.x, y: e.y } }, { duration: 180 }); else e.n.position({ x: e.x, y: e.y });
    });
  }

  // ------------------------------------------------------------------ pinned nodes
  /**
   * pins(cy, {key() -> localStorage key, or null when pins are off}) (decided 2026-10-08)
   * A pinned node stays where it is: it cannot be dragged, never follows a dragged neighbour, and every
   * layout and settle leaves it in place. Pins are remembered per key (the graph and its layout) as
   * {id: {x, y}} in this browser. P toggles the selection (the hosts bind the key).
   */
  function pins(cy, opts) {
    const o = opts || {};
    cy.style().selector('node.kit-pinned').style({ 'border-width': 3, 'border-style': 'double', 'border-color': '#facc15' }).update();
    const read = () => { const k = o.key && o.key(); if (!k) return {}; try { return JSON.parse(localStorage.getItem(k) || '{}') || {}; } catch (e) { return {}; } };
    const write = (m) => { const k = o.key && o.key(); if (!k) return; try { localStorage.setItem(k, JSON.stringify(m)); } catch (e) { /* storage blocked */ } };
    const api = {
      /** ids pinned under the current key */
      ids: () => new Set(Object.keys(read())),
      has: (n) => n.hasClass('kit-pinned'),
      /** put every pinned node of the current key back where it was pinned, and mark it */
      apply() {
        const m = read();
        cy.batch(() => {
          cy.nodes('.kit-pinned').forEach((n) => { if (!m[n.id()]) { n.removeClass('kit-pinned'); n.grabify(); } });
          Object.entries(m).forEach(([id, p]) => { const n = cy.getElementById(id); if (n.empty() || n.isParent()) return; n.position(p); n.addClass('kit-pinned'); n.ungrabify(); });
        });
      },
      /** fcose fixedNodeConstraint entries for the pinned nodes in view */
      constraints() { return Object.entries(read()).filter(([id]) => cy.getElementById(id).nonempty()).map(([id, p]) => ({ nodeId: id, position: { x: p.x, y: p.y } })); },
      /** pin or unpin: all of `nodes` unpinned -> pin them where they are, else unpin them. Returns 'pinned' | 'unpinned' | null. */
      toggle(nodes) {
        const list = nodes.filter((n) => !n.isParent() && !n.id().startsWith('__'));
        if (list.empty() || !(o.key && o.key())) return null;
        const m = read(); const pin = list.some((n) => !m[n.id()]);
        list.forEach((n) => { if (pin) { m[n.id()] = Object.assign({}, n.position()); n.addClass('kit-pinned'); n.ungrabify(); } else { delete m[n.id()]; n.removeClass('kit-pinned'); n.grabify(); } });
        write(m);
        return pin ? 'pinned' : 'unpinned';
      },
    };
    return api;
  }

  // ------------------------------------------------------------------ drag with neighbours, then settle (Cytoscape + fcose)
  /**
   * springDrag(cy, {enabled() -> bool, skip(node) -> bool, layout() -> fcose options, hops, falloff, maxFollow, onMove(nodes), onSettled()})
   * For the force layouts (decided 2026-10-08). While a node is dragged, the nodes near it follow part of
   * the way: falloff^hop of the move (default 0.5 for a neighbour, 0.25 two hops out), as if tied by springs;
   * at most maxFollow (40) of them, the fewest hops away first, then the closest, so a hub does not haul
   * the whole drawing. Followers move once per animation frame.
   * On release, fcose runs again from the current positions (randomize: false) with the dropped node pinned,
   * animated, so the springs (edges) and the repulsion between nodes find a new balance around it; then
   * `separate` pushes apart any boxes still overlapping.
   * Compound parents move with their children (Cytoscape's own); skip() nodes (helpers, nodes another rule
   * places) neither follow nor take part in the settle. A tap — no real move — changes nothing.
   */
  function springDrag(cy, opts) {
    const o = Object.assign({ hops: 2, falloff: 0.5, minMove: 4, maxFollow: 40 }, opts || {});
    const skipped = (n) => n.hasClass('kit-ep') || n.id().startsWith('__') || (o.skip && o.skip(n));
    const isPinned = (n) => n.hasClass('kit-pinned');
    let drag = null; let running = null;
    cy.on('grab', 'node', (ev) => {
      const n = ev.target;
      if (skipped(n) || (o.enabled && !o.enabled())) { drag = null; return; }
      if (running) { running.stop(); running = null; }
      // every node grabbed together (and what they carry) is moved by Cytoscape; the rest may follow
      const held = cy.nodes(':grabbed').union(n);
      const carried = held.union(held.descendants());
      const weight = new Map();
      let ring = held; const seen = new Set(carried.map((x) => x.id()));
      held.ancestors().forEach((a) => seen.add(a.id()));
      for (let hop = 1; hop <= o.hops; hop++) {
        const next = ring.union(ring.descendants()).neighborhood('node').filter((x) => !seen.has(x.id()) && !skipped(x) && x.visible());
        if (next.empty()) break;
        const w = Math.pow(o.falloff, hop);
        next.forEach((x) => {
          seen.add(x.id());
          // a compound follows by moving its leaves; a leaf inside a followed compound keeps the compound's weight
          (x.isParent() ? x.descendants().filter((d) => !d.isParent() && !skipped(d)) : x).forEach((leaf) => { if (!weight.has(leaf.id()) && !isPinned(leaf)) weight.set(leaf.id(), w); });
        });
        ring = next;
      }
      carried.forEach((x) => weight.delete(x.id()));
      // a hub would drag the whole drawing: only the nearest maxFollow follow (fewest hops, then closest)
      if (weight.size > o.maxFollow) {
        const p0 = n.position(); const dist = (id) => { const q = cy.getElementById(id).position(); return Math.hypot(q.x - p0.x, q.y - p0.y); };
        [...weight.entries()].sort((a, b) => b[1] - a[1] || dist(a[0]) - dist(b[0])).slice(o.maxFollow).forEach(([id]) => weight.delete(id));
      }
      const els = [...weight.keys()].map((id) => cy.getElementById(id)).filter((el) => el.nonempty());
      drag = { node: n, at: Object.assign({}, n.position()), from: Object.assign({}, n.position()), weight, els: cy.collection(els), dx: 0, dy: 0, frame: 0 };
    });
    // followers move at most once per animation frame, by everything the node moved since the last one
    const follow = (d) => {
      d.frame = 0;
      const dx = d.dx; const dy = d.dy; d.dx = 0; d.dy = 0;
      if (!dx && !dy) return;
      cy.batch(() => d.els.forEach((el) => { const w = d.weight.get(el.id()); const q = el.position(); el.position({ x: q.x + dx * w, y: q.y + dy * w }); }));
      if (o.onMove && d.els.nonempty()) o.onMove(d.els);
    };
    cy.on('drag', 'node', (ev) => {
      const d = drag;
      if (!d || ev.target !== d.node) return;
      const p = d.node.position(); d.dx += p.x - d.at.x; d.dy += p.y - d.at.y;
      d.at = Object.assign({}, p);
      if (!d.frame) d.frame = requestAnimationFrame(() => { if (drag === d) follow(d); });
    });
    cy.on('free', 'node', (ev) => {
      const d = drag; drag = null;
      if (!d || ev.target !== d.node) return;
      if (d.frame) { cancelAnimationFrame(d.frame); follow(d); }
      const p = d.node.position();
      if (Math.hypot(p.x - d.from.x, p.y - d.from.y) < o.minMove) return;
      const dropped = (d.node.isParent() ? d.node.descendants().filter((x) => !x.isParent()) : d.node).filter((x) => !skipped(x));
      const nodes = cy.nodes().filter((x) => !skipped(x) && x.visible());
      running = forceLayout(cy, nodes, o.layout ? o.layout() : { name: 'fcose' }, dropped.union(nodes.filter(isPinned)), {
        randomize: false, animate: true, animationDuration: 450,
        done: () => { running = null; if (o.onSettled) setTimeout(o.onSettled, 200); },
      });
    });
    return { get settling() { return !!running; } };
  }

  /**
   * forceLayout(cy, nodes, fcoseOptions, fixed, {randomize, animate, animationDuration, done})
   * fcose over `nodes` (and the edges among them) with every node in `fixed` held where it is, then
   * `separate` (fixed nodes stay put). With label-inclusive sizes fcose holds the centre of node + label,
   * not the node: the constraint names that box, so the node itself does not move. Returns the layout.
   */
  function forceLayout(cy, nodes, fcose, fixed, opts) {
    const o = Object.assign({ randomize: false, animate: false }, opts || {});
    const lo = Object.assign({}, fcose);
    const at = (x) => {
      const q = x.position();
      if (!lo.nodeDimensionsIncludeLabels) return { x: q.x, y: q.y };
      const bb = x.boundingBox({ includeLabels: true, includeOverlays: false });
      return { x: (bb.x1 + bb.x2) / 2, y: (bb.y1 + bb.y2) / 2 };
    };
    const held = fixed.filter((x) => !x.isParent() && nodes.contains(x));
    const eles = nodes.union(nodes.edgesWith(nodes));
    const extra = { randomize: o.randomize, animate: o.animate, fit: false };
    if (o.animate) extra.animationDuration = o.animationDuration || 450;
    if (held.nonempty()) extra.fixedNodeConstraint = held.map((x) => ({ nodeId: x.id(), position: at(x) }));
    const lay = eles.layout(Object.assign(lo, extra));
    const ids = new Set(held.map((x) => x.id()));
    lay.one('layoutstop', () => { separate(cy, nodes, { pinned: ids, animate: o.animate }); if (o.done) o.done(); });
    lay.run();
    return lay;
  }

  /** True while the user types in a field (shortcuts must not fire). */
  const typing = () => { const a = document.activeElement; return !!a && (a.isContentEditable || /^(TEXTAREA|SELECT)$/.test(a.tagName) || (a.tagName === 'INPUT' && !/^(checkbox|radio|button|submit|reset|range|color)$/i.test(a.type || ''))); };
  /** A modal dialog is open (shortcuts must not reach the page under it). */
  const dialogOpen = () => !!document.querySelector('dialog[open]');

  window.IterKit = { HELPER_NODE, HELPER_EDGE, esc, md, toast, modal, ask, confirm: confirmBox, choose, configure, menu, closeMenu, help, endpointHandles, springDrag, separate, forceLayout, pins, typing, dialogOpen, MOD, isMac,
    clip: { project: null, settings: null } };
}());
