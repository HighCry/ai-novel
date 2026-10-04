/** 创建元素：h('div', { class, onclick, ... }, 子节点...) */
export function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null || v === false) continue;
      if (k === 'class') el.className = v;
      else if (k === 'style' && typeof v === 'object') Object.assign(el.style, v);
      else if (k === 'dataset') Object.assign(el.dataset, v);
      else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
      else if (k in el && k !== 'list' && k !== 'form') el[k] = v;
      else el.setAttribute(k, v === true ? '' : v);
    }
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

export function toast(msg, type = 'info', ms = 2800) {
  const el = h('div', { class: 'toast ' + type }, msg);
  document.getElementById('toasts').append(el);
  setTimeout(() => el.classList.add('hide'), ms);
  setTimeout(() => el.remove(), ms + 400);
}

/** 弹窗：actions = [{ label, class, onClick(close) }] */
export function modal({ title, body, actions = [], wide = false, onClose } = {}) {
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    overlay.remove();
    document.removeEventListener('keydown', onKey);
    onClose?.();
  };
  const onKey = (e) => { if (e.key === 'Escape' && overlay === document.querySelector('.overlay:last-of-type')) close(); };
  const foot = h('div', { class: 'modal-foot' });
  const setActions = (list) => {
    foot.innerHTML = '';
    for (const a of list) foot.append(h('button', { class: 'btn ' + (a.class || ''), onclick: () => a.onClick(close) }, a.label));
    foot.hidden = !list.length;
  };
  setActions(actions);
  const bodyEl = h('div', { class: 'modal-body' }, body);
  const box = h('div', { class: 'modal' + (wide === 'xl' ? ' xl' : wide ? ' wide' : '') },
    h('div', { class: 'modal-head' }, h('h3', null, title), h('button', { class: 'icon-btn', title: '关闭（Esc）', onclick: close }, '✕')),
    bodyEl, foot);
  const overlay = h('div', { class: 'overlay' }, box);
  document.body.append(overlay);
  document.addEventListener('keydown', onKey);
  return { close, box, body: bodyEl, setActions };
}

export function confirmBox(message, { okText = '确定', danger = false } = {}) {
  return new Promise((resolve) => {
    let answered = false;
    const m = modal({
      title: '请确认',
      body: h('p', { class: 'confirm-text' }, message),
      actions: [
        { label: '取消', onClick: (close) => close() },
        { label: okText, class: danger ? 'danger' : 'primary', onClick: (close) => { answered = true; close(); resolve(true); } },
      ],
      onClose: () => { if (!answered) resolve(false); },
    });
    m.box.querySelector('.modal-foot .btn:last-child').focus();
  });
}

export function promptBox(title, { value = '', placeholder = '', okText = '确定' } = {}) {
  return new Promise((resolve) => {
    let answered = false;
    const input = h('input', { value, placeholder });
    const submit = (close) => { answered = true; close(); resolve(input.value.trim()); };
    const m = modal({
      title,
      body: input,
      actions: [{ label: '取消', onClick: (c) => c() }, { label: okText, class: 'primary', onClick: submit }],
      onClose: () => { if (!answered) resolve(null); },
    });
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') submit(m.close); });
    input.focus();
    input.select();
  });
}

/** 按钮执行异步任务期间禁用并显示进度文字 */
export async function busy(btn, fn, text = '处理中…') {
  const old = btn.textContent;
  btn.disabled = true;
  btn.textContent = text;
  try {
    return await fn();
  } catch (e) {
    toast(e.message, 'error', 5000);
  } finally {
    btn.disabled = false;
    btn.textContent = old;
  }
}

export function field(label, input, hint) {
  return h('label', { class: 'field' }, h('span', { class: 'field-label' }, label), input, hint ? h('span', { class: 'hint' }, hint) : null);
}

export function debounce(fn, ms) {
  let t;
  const wrapped = (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
  wrapped.cancel = () => clearTimeout(t);
  return wrapped;
}

export function today() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

export function fmtWords(n) {
  n = n || 0;
  return n >= 10000 ? (n / 10000).toFixed(n >= 100000 ? 0 : 1) + ' 万' : String(n);
}

export function fmtTime(ts) {
  const d = new Date(ts * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  if (d.toDateString() === now.toDateString()) return '今天 ' + hm;
  const md = `${d.getMonth() + 1}月${d.getDate()}日`;
  return d.getFullYear() === now.getFullYear() ? `${md} ${hm}` : `${d.getFullYear()}年${md}`;
}

export function countWords(s) {
  return (s || '').replace(/\s/g, '').length;
}

/** 中文正文里的英文双引号按段成对换成“” */
export function normalizeQuotes(text) {
  if (!text.includes('"') || !/[\u4e00-\u9fff]/.test(text)) return text;
  return text.split('\n').map((line) => {
    let open = true;
    return line.replace(/"/g, () => { const q = open ? '“' : '”'; open = !open; return q; });
  }).join('\n');
}

/** 去掉 AI 输出开头的开场白、大标题、分隔线和代码块标记，并规范引号 */
export function cleanAi(t) {
  const text = (t || '').replace(/^\s*```[a-zA-Z]*\s*\n?/, '').replace(/\n?```\s*$/, '');
  const lines = text.split('\n');
  const junk = (line) => {
    const s = line.trim();
    if (!s) return true;
    if (/^#\s/.test(s) || /^[-*_=]{3,}$/.test(s)) return true;
    if (s.length <= 40 && /^\*\*[^*]+\*\*[：:]?$/.test(s)) return true;
    return s.length < 40 && /^(好的|以下是|下面是|当然|没问题|续写如下|正文如下)/.test(s);
  };
  while (lines.length && junk(lines[0])) lines.shift();
  return normalizeQuotes(lines.join('\n').replace(/\n{3,}/g, '\n\n').trim());
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = h('textarea', { value: text, style: { position: 'fixed', opacity: '0' } });
    document.body.append(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
  toast('已复制到剪贴板');
}

export function readFile(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result);
    r.onerror = () => reject(new Error('读取文件失败'));
    r.readAsDataURL(file);
  });
}

export function download(blob, filename) {
  const a = h('a', { href: URL.createObjectURL(blob), download: filename });
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 2000);
}

export function pickFile(accept) {
  return new Promise((resolve) => {
    const input = h('input', { type: 'file', accept, style: { display: 'none' } });
    input.addEventListener('change', () => { resolve(input.files[0] || null); input.remove(); });
    document.body.append(input);
    input.click();
  });
}

export function themeButton() {
  const icon = () => (document.documentElement.dataset.theme === 'dark' ? '☀ 浅色' : '☾ 深色');
  const btn = h('button', {
    class: 'ghost',
    title: '切换深色/浅色主题',
    onclick: () => {
      const next = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
      document.documentElement.dataset.theme = next;
      localStorage.setItem('theme', next);
      syncThemeColor();
      btn.textContent = icon();
    },
  }, icon());
  return btn;
}

/** 手机状态栏的颜色跟着主题走 */
export function syncThemeColor() {
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.content = getComputedStyle(document.documentElement).getPropertyValue('--surface').trim() || '#fffdf8';
}

/** 手机上顶栏放不下，把次要按钮收进「⋯」菜单（点菜单外或点了菜单里的按钮就收起，见 app.js） */
export function moreButton(menu) {
  return h('button', { class: 'ghost mobile-only more-btn', title: '更多', onclick: () => menu.classList.toggle('open') }, '⋯');
}

/** 按分句做 LCS 对比，返回 [{type:'eq',text}|{type:'chg',del,ins,on}]，用于修订模式逐条采纳 */
export function diffClauses(a, b) {
  const tok = (s) => (s.match(/[^，。！？；：、…\n“”]*[，。！？；：、…\n“”]?/g) || []).filter(Boolean);
  const A = tok(a);
  const B = tok(b);
  const n = A.length;
  const m = B.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  }
  const out = [];
  const add = (kind, text) => {
    const last = out[out.length - 1];
    if (kind === 'eq') {
      if (last && last.type === 'eq') last.text += text;
      else out.push({ type: 'eq', text });
    } else if (last && last.type === 'chg') {
      last[kind] += text;
    } else {
      out.push({ type: 'chg', del: kind === 'del' ? text : '', ins: kind === 'ins' ? text : '', on: true });
    }
  };
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) { add('eq', A[i]); i++; j++; } else if (dp[i + 1][j] >= dp[i][j + 1]) { add('del', A[i]); i++; } else { add('ins', B[j]); j++; }
  }
  while (i < n) add('del', A[i++]);
  while (j < m) add('ins', B[j++]);
  return out;
}

export function applyDiff(parts) {
  return parts.map((p) => (p.type === 'eq' ? p.text : p.on ? p.ins : p.del)).join('');
}

/** 简易 Markdown 渲染：标题、列表、表格、引用、代码块、粗体 */
export function renderMarkdown(md) {
  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  const inline = (s) => esc(s).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`([^`]+)`/g, '<code>$1</code>');
  let html = '';
  let list = false;
  let table = false;
  let code = false;
  const close = () => {
    if (list) { html += '</ul>'; list = false; }
    if (table) { html += '</table>'; table = false; }
  };
  for (const raw of md.split('\n')) {
    const line = raw.trimEnd();
    if (line.trim().startsWith('```')) {
      close();
      html += code ? '</pre>' : '<pre>';
      code = !code;
      continue;
    }
    if (code) { html += esc(line) + '\n'; continue; }
    if (/^\s*\|.*\|\s*$/.test(line)) {
      if (/^\s*\|[\s:|-]+\|\s*$/.test(line)) continue;
      if (!table) { close(); html += '<table class="table">'; table = true; }
      html += '<tr>' + line.trim().slice(1, -1).split('|').map((c) => `<td>${inline(c.trim())}</td>`).join('') + '</tr>';
      continue;
    }
    const head = line.match(/^(#{1,4})\s+(.*)/);
    if (head) { close(); html += `<h${head[1].length + 2}>${inline(head[2])}</h${head[1].length + 2}>`; continue; }
    const item = line.match(/^\s*(?:[-*+]|\d+[.、])\s+(.*)/);
    if (item) {
      if (table) { html += '</table>'; table = false; }
      if (!list) { html += '<ul>'; list = true; }
      html += `<li>${inline(item[1])}</li>`;
      continue;
    }
    close();
    if (/^\s*>/.test(line)) html += `<blockquote>${inline(line.replace(/^\s*>\s?/, ''))}</blockquote>`;
    else if (/^\s*-{3,}\s*$/.test(line)) html += '<hr>';
    else if (line.trim()) html += `<p>${inline(line)}</p>`;
  }
  close();
  if (code) html += '</pre>';
  return html;
}

export const CHAR_FIELDS = [
  ['location', '位置'], ['power', '实力'], ['body', '身体'], ['mind', '心理'],
  ['items', '关键物品'], ['recent', '近期经历'], ['knows', '知道的秘密'], ['unaware', '还不知道的事'],
];
export const ROLES = ['主角', '重要配角', '配角', '反派', '路人'];

export const KIND = { character: '人物', faction: '势力', location: '地点', item: '物品', concept: '设定', other: '其他' };

export function keywords(entry) {
  return [entry.name, ...(entry.aliases || '').split(/[,，、;；/|\s]+/)].map((s) => s.trim()).filter((s) => s.length >= 2 || s === entry.name);
}
