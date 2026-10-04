// 编辑器助手：设定名高亮和悬停卡片、灰字续写、斜杠指令、选区工具条。
// textarea 不能给部分文字上色，所以在它下面垫一层排版完全相同的镜像层，
// 高亮和灰字画在镜像层上，透过透明背景的 textarea 显示出来。

import { api, stream } from './api.js';
import { store, emit, modelReady, chapterLabel } from './store.js';
import { h, toast, promptBox, cleanAi, countWords, KIND, keywords } from './ui.js';
import { openEntry } from './dialogs.js';
import { openSaveToLibrary } from './stylelib.js';

export const COMMANDS = [
  { id: 'more', name: '续写一段', keys: 'xx xuxie 续写 往下', desc: '顺着光标往下写一小段', task: 'ghost', words: 200 },
  { id: 'scene', name: '环境描写', keys: 'hj huanjing 环境 景色 氛围', desc: '用感官细节烘托此刻的气氛', goal: '写一段环境或场景描写，用具体的感官细节烘托此刻的气氛，不推进剧情', tags: ['环境'], words: 150 },
  { id: 'look', name: '人物描写', keys: 'rw renwu 外貌 神态 人物', desc: '外貌、神态或小动作', goal: '写一段当前出场人物的外貌、神态或小动作，突出性格', tags: ['人物描写'], words: 120 },
  { id: 'action', name: '动作打斗', keys: 'dz dongzuo dd dadou 打斗 动作 战斗', desc: '短句为主，每个动作都有结果', goal: '写一段动作或打斗，短句为主，每个动作都要有结果和对手的反应', tags: ['打斗'], words: 250 },
  { id: 'inner', name: '心理活动', keys: 'xl xinli 心理 内心', desc: '贴着处境写，不说教', goal: '写一段主角此刻的心理活动，贴着处境写，不说教，不总结', tags: ['心理'], words: 120 },
  { id: 'talk', name: '对话', keys: 'dh duihua 对话 对白', desc: '推进情节，符合各人性格', goal: '写一段推进情节的人物对话，每句话都符合说话人的性格，夹少量动作和神态', tags: ['对话'], words: 250 },
  { id: 'cool', name: '爽点', keys: 'sd shuangdian 爽 打脸 反转', desc: '打脸、反转或亮出实力', goal: '写一个小爽点：打脸、反转或主角亮出实力，铺垫要短，兑现要狠', tags: ['爽点'], words: 300 },
  { id: 'cut', name: '转场', keys: 'zc zhuanchang 转场 过渡', desc: '一两句切到下一个场景', goal: '写一两句简短的场景或时间过渡，自然地切到下一个场景', tags: ['转场'], words: 60 },
  { id: 'hook', name: '章末钩子', keys: 'gz gouzi 钩子 结尾 悬念', desc: '留下悬念，让人想看下一章', goal: '写这一章的结尾钩子：留下悬念或危机，让读者想立刻看下一章', tags: ['章末钩子'], words: 150 },
  { id: 'polish', name: '润色上一段', keys: 'rs runse 润色', desc: '只改表达，不改情节', kind: 'paragraph', task: 'polish' },
  { id: 'expand', name: '扩写上一段', keys: 'kx kuoxie 扩写', desc: '补动作、对话和细节', kind: 'paragraph', task: 'expand' },
  { id: 'names', name: '起名', keys: 'qm qiming 起名 名字', desc: '人物、地点、功法、势力', kind: 'names' },
  { id: 'entity', name: '插入设定名', keys: 'sd sheding rm renming 人名 设定 @', desc: '从设定库里选一个名字', kind: 'entity' },
  { id: 'beats', name: '规划本章节拍', keys: 'jp jiepai 节拍', desc: '把本章拆成场景节拍', kind: 'beats' },
];

const MOVE_KEYS = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'];
const STYLE_PROPS = ['fontFamily', 'fontSize', 'fontWeight', 'fontStyle', 'lineHeight', 'letterSpacing', 'wordSpacing', 'textIndent', 'tabSize',
  'paddingTop', 'paddingLeft', 'paddingBottom', 'borderTopWidth', 'borderLeftWidth', 'borderRightWidth', 'borderBottomWidth'];

const esc = (s) => s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
/** 原生 append 会把 null 变成文字 "null"，这里先过滤掉 */
const put = (el, ...kids) => el.append(...kids.filter((k) => k != null && k !== false));
const clip = (s, n) => (s.length > n ? s.slice(0, n) + '…' : s);
export const pref = (key, fallback) => localStorage.getItem(key) ?? fallback;

/** 清理灰字：去掉思考过程和开场白；模型把光标前的字重复了一遍时去掉重复部分 */
export function ghostClean(raw, before) {
  let t = (raw || '').replace(/<think>[\s\S]*?(?:<\/think>|$)/g, '');
  const newPara = /^[ \t]*\n/.test(t) && before && !before.endsWith('\n');
  t = cleanAi(t);
  if (!t) return '';
  const tail = before.slice(-24);
  for (let n = Math.min(tail.length, t.length - 1); n >= 4; n--) {
    if (t.startsWith(tail.slice(-n))) { t = t.slice(n); break; }
  }
  return (newPara ? '\n' : '') + t.replace(/^[ \t]+/, '');
}

function trimToSentence(t) {
  const m = t.match(/^[\s\S]*[。！？…!?”」]/);
  return m && m[0].length >= t.length * 0.4 ? m[0] : t;
}

/** 光标所在段落；光标在空行上时取上一段 */
export function paragraphAt(v, pos) {
  const ps = v.lastIndexOf('\n', pos - 1) + 1;
  let pe = v.indexOf('\n', pos);
  if (pe < 0) pe = v.length;
  if (v.slice(ps, pe).trim()) return [ps, pe];
  let e = ps - 1;
  while (e > 0) {
    const s = v.lastIndexOf('\n', e - 1) + 1;
    if (v.slice(s, e).trim()) return [s, e];
    e = s - 1;
  }
  return null;
}

export function attachAssist(ta, wrap) {
  const back = h('div', { class: 'backdrop', 'aria-hidden': 'true' });
  const hover = h('div', { class: 'ent-card', hidden: true });
  const card = h('div', { class: 'ghost-card', hidden: true });
  const menu = h('div', { class: 'slash-menu', hidden: true });
  const bubble = h('div', { class: 'sel-bubble', hidden: true });
  wrap.prepend(back);
  wrap.append(hover, card, menu, bubble);

  let entityRe = null;
  let entityMap = new Map();
  let ghost = null;
  let slash = null;
  let composing = false;
  let internal = false;
  let autoTimer = null;
  let frame = 0;
  let style = null;
  let extraPad = 0;
  let basePad = 0;

  const highlightOn = () => pref('highlight', '1') !== '0';

  // ---------- 镜像层 ----------

  function syncStyle() {
    const cs = getComputedStyle(ta);
    for (const p of STYLE_PROPS) back.style[p] = cs[p];
    style = { pr: parseFloat(cs.paddingRight) || 0, bl: parseFloat(cs.borderLeftWidth) || 0, br: parseFloat(cs.borderRightWidth) || 0, lh: parseFloat(cs.lineHeight) || 34 };
  }

  /** 灰字不是真正的文字，textarea 滚不到它下面：临时加底部留白并滚动，让灰字和提示完整露出来 */
  function ensureRoom(bottom) {
    const over = bottom - wrap.clientHeight + 8;
    if (over <= 0) return false;
    if (!extraPad) basePad = parseFloat(getComputedStyle(ta).paddingBottom) || 0;
    extraPad += over;
    ta.style.paddingBottom = basePad + extraPad + 'px';
    syncStyle();
    ta.scrollTop += over;
    return true;
  }

  function resetRoom() {
    if (!extraPad) return;
    extraPad = 0;
    ta.style.paddingBottom = '';
    syncStyle();
  }

  function marked(text) {
    if (!entityRe || !highlightOn()) return esc(text);
    let out = '';
    let last = 0;
    for (const m of text.matchAll(entityRe)) {
      const e = entityMap.get(m[0]);
      out += esc(text.slice(last, m.index)) + `<mark class="ent k-${e.kind}" data-id="${e.id}">${esc(m[0])}</mark>`;
      last = m.index + m[0].length;
    }
    return out + esc(text.slice(last));
  }

  const ghostInline = () => ghost && !composing && ghost.inline;

  function render(markAt = null) {
    if (!style) syncStyle();
    const scrollbar = ta.offsetWidth - ta.clientWidth - style.bl - style.br;
    back.style.paddingRight = style.pr + Math.max(0, scrollbar) + 'px';
    const v = ta.value;
    const cuts = [];
    if (markAt != null) cuts.push([markAt, 0, '<span class="caret-mark"></span>']);
    if (ghostInline()) cuts.push([ghost.pos, 1, `<span class="ghost-text">${esc(ghost.text || '…')}</span>`]);
    cuts.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    let html = '';
    let last = 0;
    for (const [p, , span] of cuts) {
      html += marked(v.slice(last, p)) + span;
      last = p;
    }
    html += marked(v.slice(last));
    back.innerHTML = html + (v.endsWith('\n') || !v ? ' ' : '');
    back.scrollTop = ta.scrollTop;
  }

  /** 某个字符位置在 wrap 里的坐标 */
  function caretXY(pos) {
    render(pos);
    const m = back.querySelector('.caret-mark');
    const xy = m ? { x: m.offsetLeft, y: m.offsetTop - ta.scrollTop } : { x: 0, y: 0 };
    back.querySelector('.caret-mark')?.remove();
    return xy;
  }

  /** 放在一行文字下方，放不下就放到上方 */
  function place(el, x, lineTop) {
    el.style.left = '0px';
    el.style.top = '0px';
    const w = el.offsetWidth;
    const hgt = el.offsetHeight;
    const left = Math.max(8, Math.min(x, wrap.clientWidth - w - 8));
    let top = lineTop + style.lh;
    if (top + hgt > wrap.clientHeight - 4 && lineTop - hgt - 4 > 0) top = lineTop - hgt - 4;
    el.style.left = left + 'px';
    el.style.top = Math.max(4, top) + 'px';
  }

  function setEntities() {
    entityMap = new Map();
    for (const e of store.entries || []) {
      for (const k of keywords(e)) if (k.length >= 2 && !entityMap.has(k)) entityMap.set(k, e);
    }
    const keys = [...entityMap.keys()].sort((a, b) => b.length - a.length).map((k) => k.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    entityRe = keys.length ? new RegExp(keys.join('|'), 'g') : null;
    render();
  }

  // ---------- 编辑 ----------

  /** 用 execCommand 插入，浏览器的撤销（Ctrl+Z）仍然有效 */
  function replaceText(start, end, text) {
    internal = true;
    try {
      ta.focus();
      ta.setSelectionRange(start, end);
      const ok = text ? document.execCommand('insertText', false, text) : document.execCommand('delete');
      if (!ok) {
        ta.setRangeText(text, start, end, 'end');
        ta.dispatchEvent(new Event('input', { bubbles: true }));
      }
    } finally {
      internal = false;
    }
  }

  // ---------- 设定名悬停卡片 ----------

  const markAt = (x, y) => document.elementsFromPoint(x, y).find((el) => el.classList?.contains('ent'));
  const entryOf = (m) => store.entries.find((e) => e.id === Number(m.dataset.id));

  function showHover(x, y) {
    const m = entityRe && highlightOn() ? markAt(x, y) : null;
    const e = m && entryOf(m);
    if (!e) { hover.hidden = true; return; }
    if (hover.dataset.id !== String(e.id) || hover.hidden) {
      hover.dataset.id = e.id;
      hover.innerHTML = '';
      put(hover,
        h('div', { class: 'ent-head' }, h('b', null, e.name), h('span', { class: 'tag k-' + e.kind }, KIND[e.kind] || '设定'), e.role ? h('span', { class: 'tag' }, e.role) : null),
        e.aliases ? h('div', { class: 'muted small' }, '又称：' + e.aliases) : null,
        e.state ? h('div', { class: 'ent-state' }, clip(e.state, 140)) : null,
        e.description ? h('div', { class: 'small' }, clip(e.description, 160)) : null,
        h('div', { class: 'hint' }, 'Ctrl + 点击打开设定'));
    }
    hover.hidden = false;
    const wr = wrap.getBoundingClientRect();
    const r = m.getBoundingClientRect();
    hover.style.left = Math.max(8, Math.min(r.left - wr.left, wrap.clientWidth - hover.offsetWidth - 8)) + 'px';
    const below = r.bottom - wr.top + 6;
    hover.style.top = (below + hover.offsetHeight > wrap.clientHeight ? r.top - wr.top - hover.offsetHeight - 6 : below) + 'px';
  }

  // ---------- 灰字 ----------

  function startGhost(body, { mode = 'insert', start, end, label = '灰字续写' } = {}) {
    if (!store.chapter) return toast('请先打开一个章节', 'warn');
    if (!modelReady()) return toast('请先在设置里配置模型', 'warn');
    closeGhost();
    closeMenu();
    hideBubble();
    const v = ta.value;
    const pos = mode === 'insert' ? (start ?? ta.selectionEnd) : start;
    const stop = mode === 'insert' ? pos : end;
    const g = {
      mode, label, pos, start, end, full: '', text: '', offset: 0, taken: '', done: false, refs: [], guide: false,
      inline: mode === 'insert' && !v.slice(pos).trim(),
      lenAfter: v.length - pos,
      before: v.slice(0, mode === 'insert' ? pos : start),
      origin: { before: v.slice(Math.max(0, (mode === 'insert' ? pos : start) - 30), mode === 'insert' ? pos : start), after: v.slice(stop, stop + 30) },
      body: { ...body, book_id: store.book.id, chapter_id: store.chapter.id },
      ctrl: new AbortController(),
    };
    if (mode === 'insert') Object.assign(g.body, { before: v.slice(0, pos), after: v.slice(pos) });
    ghost = g;
    if (mode === 'insert' && ta.selectionStart !== pos) ta.setSelectionRange(pos, pos);
    update();
    const limit = (body.words || 60) * 3 + 60;
    const apply = (full) => {
      g.full = g.mode === 'insert' ? ghostClean(full, g.before) : cleanAi(full);
      g.text = g.full.slice(g.offset);
    };
    stream(g.body, {
      signal: g.ctrl.signal,
      onMeta: (m) => { g.refs = m.refs || []; g.guide = !!m.guide; },
      onDelta: (_, full) => {
        if (ghost !== g) return;
        apply(full);
        if (g.full.length > limit) g.ctrl.abort();
        update();
      },
    }).then((full) => {
      if (ghost !== g) return;
      apply(full);
      finish(g);
    }).catch((e) => {
      if (ghost !== g) return;
      if (e.name === 'AbortError') {
        g.full = trimToSentence(g.full);
        g.text = g.full.slice(g.offset);
        finish(g);
      } else {
        toast(e.message, 'error', 6000);
        closeGhost();
      }
    });
  }

  function finish(g) {
    g.done = true;
    if (!g.text && !g.taken) {
      toast('AI 这次没有给出内容，可以再按一次 Ctrl+Enter', 'warn');
      closeGhost();
      return;
    }
    if (!g.text) { closeGhost(); return; }
    update();
  }

  function update() {
    render();
    const g = ghost;
    if (!g || composing) { card.hidden = true; return; }
    card.innerHTML = '';
    const refs = g.refs.length ? '参考：' + g.refs.map((r) => `《${r.title}》`).join('') : '';
    const btn = (label, fn, cls = '') => h('button', { class: 'mini ' + cls, onmousedown: (e) => { e.preventDefault(); fn(); } }, label);
    if (g.inline) {
      card.className = 'ghost-card hint-only';
      // 手机键盘没有 Tab / Esc / Ctrl，换成能点的按钮
      const hints = matchMedia('(pointer: coarse)').matches
        ? [
          g.text ? btn(g.done ? '采纳' : '先采纳', () => acceptGhost(false), 'primary') : h('span', null, 'AI 思考中…'),
          g.done ? btn('逐句', () => acceptGhost(true)) : null,
          g.done ? btn('换一个', regenerate) : null,
          btn('取消', closeGhost),
        ]
        : [h('span', null, g.done ? 'Tab 采纳 · Ctrl+→ 逐句 · Esc 取消 · Ctrl+Enter 换一个' : g.text ? '生成中…  Tab 先采纳已有部分' : 'AI 思考中…  Esc 取消')];
      put(card,
        ...hints,
        refs ? h('span', { class: 'muted' }, refs) : null,
        g.guide ? h('span', { class: 'muted' }, '已用文风指南') : null);
      card.hidden = false;
      const lastRect = () => {
        const rects = back.querySelector('.ghost-text')?.getClientRects() || [];
        return rects[rects.length - 1];
      };
      const wr = wrap.getBoundingClientRect();
      let last = lastRect();
      if (last && ensureRoom(last.top - wr.top + style.lh + card.offsetHeight + 4)) {
        render();
        last = lastRect();
      }
      if (last) place(card, last.left - wr.left, last.top - wr.top);
      return;
    }
    card.className = 'ghost-card';
    card.append(
      h('div', { class: 'ghost-head' }, h('b', null, g.label), h('span', { class: 'muted small' }, g.done ? '' : g.text ? '生成中…' : 'AI 思考中…'), h('span', { class: 'spacer' }), refs ? h('span', { class: 'muted small' }, refs) : null),
      h('div', { class: 'ghost-body' }, g.text || '…'),
      h('div', { class: 'ghost-actions' },
        btn(g.mode === 'replace' ? '替换 Tab' : '插入 Tab', () => acceptGhost(false), 'primary'),
        g.mode === 'insert' ? btn('逐句 Ctrl+→', () => acceptGhost(true)) : null,
        btn('换一个', regenerate),
        btn('取消 Esc', closeGhost)));
    card.hidden = false;
    const at = caretXY(g.mode === 'insert' ? g.pos : g.end);
    place(card, at.x, at.y);
  }

  function acceptGhost(partial) {
    const g = ghost;
    if (!g || !g.text) return;
    let piece = g.text;
    if (partial && g.mode === 'insert') {
      const m = g.text.match(/^[\s\S]*?[，。！？；：…!?;:,—](?:[”’」』"]+)?/);
      if (m) piece = m[0];
    }
    if (g.mode === 'insert') {
      replaceText(g.pos, g.pos, piece);
      g.pos += piece.length;
      g.offset += piece.length;
      g.taken += piece;
      g.text = g.full.slice(g.offset);
    } else {
      replaceText(g.start, g.end, piece);
      g.taken = piece;
      g.text = '';
    }
    if (!partial || !g.text) {
      if (!g.done) g.ctrl.abort();
      closeGhost();
    } else {
      update();
    }
  }

  function record(g) {
    const text = g.taken.trim();
    if (!text) return;
    const n = countWords(text);
    api.post(`/chapters/${g.body.chapter_id}/ai_accept`, { chars: n, text, task: g.body.task, before: g.origin.before, after: g.origin.after })
      .then(() => {
        if (store.chapter?.id !== g.body.chapter_id) return;
        store.chapter.ai_chars = (store.chapter.ai_chars || 0) + n;
        store.editor?.refreshCount();
      })
      .catch(() => {});
  }

  function closeGhost() {
    const g = ghost;
    if (!g) return;
    ghost = null;
    if (!g.done) g.ctrl.abort();
    record(g);
    card.hidden = true;
    resetRoom();
    render();
  }

  function regenerate() {
    const g = ghost;
    if (!g) return;
    const body = { ...g.body, variant: (g.body.variant || 0) + 1 };
    const opts = { mode: g.mode, start: g.mode === 'insert' ? g.pos : g.start, end: g.end, label: g.label };
    closeGhost();
    startGhost(body, opts);
  }

  function triggerGhost() {
    startGhost({ task: 'ghost', words: Number(pref('ghostWords', '50')) || 50 }, { mode: 'insert' });
  }

  // 用户自己打的字和灰字开头一致时，灰字往后退，不打断
  function followTyping() {
    const g = ghost;
    if (!g) return;
    const v = ta.value;
    const caret = ta.selectionStart;
    if (g.mode !== 'insert' || caret !== ta.selectionEnd || v.length - caret !== g.lenAfter || caret < g.pos) return closeGhost();
    if (caret === g.pos) return;
    const typed = v.slice(g.pos, caret);
    if (!g.text.startsWith(typed)) return closeGhost();
    g.pos = caret;
    g.offset += typed.length;
    g.text = g.full.slice(g.offset);
    if (!g.text && g.done) closeGhost();
  }

  function scheduleAuto() {
    clearTimeout(autoTimer);
    if (pref('ghostAuto', '0') !== '1') return;
    autoTimer = setTimeout(() => {
      if (ghost || slash || composing || document.activeElement !== ta || !store.chapter || !modelReady()) return;
      const v = ta.value;
      const p = ta.selectionEnd;
      if (ta.selectionStart !== p || (p < v.length && v[p] !== '\n')) return;
      if (v.slice(v.lastIndexOf('\n', p - 1) + 1, p).trim().length < 6) return;
      triggerGhost();
    }, Number(pref('ghostDelay', '2500')) || 2500);
  }

  // ---------- 斜杠指令 ----------

  function openMenu(pos, mode = 'commands') {
    hideBubble();
    slash = { pos, char: ta.value[pos], query: '', extra: '', index: 0, mode };
    fillMenu();
  }

  function closeMenu() {
    slash = null;
    menu.hidden = true;
  }

  function items() {
    if (slash.mode === 'entities') {
      const q = slash.query.trim();
      return store.entries.filter((e) => !q || keywords(e).some((k) => k.includes(q))).slice(0, 8);
    }
    const [head = '', ...rest] = slash.query.trim().split(/\s+/);
    const q = head.toLowerCase();
    slash.extra = rest.join(' ');
    return COMMANDS.filter((c) => !q || c.name.includes(q) || c.keys.split(' ').some((k) => k.startsWith(q)));
  }

  function fillMenu() {
    const list = items();
    if (!list.length && slash.query.trim().length >= 3) return closeMenu();
    slash.index = Math.max(0, Math.min(slash.index, list.length - 1));
    menu.innerHTML = '';
    const row = (title, desc, i) => h('div', {
      class: 'slash-item' + (i === slash.index ? ' on' : ''),
      onmousedown: (e) => { e.preventDefault(); slash.index = i; choose(); },
    }, h('b', null, title), h('span', { class: 'muted small' }, desc));
    if (!list.length) menu.append(h('div', { class: 'muted small slash-empty' }, slash.mode === 'entities' ? '设定库里没有匹配的名字' : '没有匹配的指令'));
    if (slash.mode === 'entities') list.forEach((e, i) => menu.append(row(e.name, (KIND[e.kind] || '设定') + (e.role ? ' · ' + e.role : ''), i)));
    else list.forEach((c, i) => menu.append(row(c.name, c.desc, i)));
    menu.append(h('div', { class: 'slash-foot' }, slash.mode === 'entities' ? '↑↓ 选择 · Enter 插入 · Esc 关闭' : '↑↓ 选择 · Enter 执行 · Esc 关闭 · 指令后空一格可以写要求'));
    menu.hidden = false;
    const at = caretXY(ta.selectionEnd);
    place(menu, at.x, at.y);
    menu.querySelector('.slash-item.on')?.scrollIntoView({ block: 'nearest' });
  }

  function checkSlash() {
    const v = ta.value;
    const caret = ta.selectionStart;
    if (slash) {
      if (caret <= slash.pos || v[slash.pos] !== slash.char || v.slice(slash.pos, caret).includes('\n') || caret - slash.pos > 30) return closeMenu();
      slash.query = v.slice(slash.pos + 1, caret);
      return fillMenu();
    }
    if (caret !== ta.selectionEnd) return;
    const ch = v[caret - 1];
    if (ch === '/' || ch === '、') {
      const lineStart = v.lastIndexOf('\n', caret - 2) + 1;
      if (/^[\s\u3000]*$/.test(v.slice(lineStart, caret - 1))) openMenu(caret - 1);
    } else if (ch === '@' && !/[A-Za-z0-9._-]/.test(v[caret - 2] || '') && store.entries.length) {
      openMenu(caret - 1, 'entities');
    }
  }

  /** 删掉“/指令”这几个字，返回原来斜杠的位置 */
  function removeTyped() {
    const caret = ta.selectionStart;
    replaceText(slash.pos, caret, '');
    return slash.pos;
  }

  async function choose() {
    const list = items();
    const item = list[slash.index];
    if (!item) return closeMenu();
    if (slash.mode === 'entities') {
      const pos = slash.pos;
      const caret = ta.selectionStart;
      closeMenu();
      replaceText(pos, caret, item.name);
      return;
    }
    const extra = slash.extra;
    if (item.kind === 'entity') {
      const pos = removeTyped();
      closeMenu();
      replaceText(pos, pos, '@');
      openMenu(pos, 'entities');
      return;
    }
    const pos = removeTyped();
    closeMenu();
    runCommand(item, extra, pos);
  }

  async function runCommand(c, extra, pos) {
    const v = ta.value;
    if (c.kind === 'beats') return emit('run-beats');
    if (c.kind === 'names') return names(extra, pos);
    if (c.kind === 'paragraph') {
      const range = paragraphAt(v, pos);
      if (!range) return toast('上面还没有可以处理的段落', 'warn');
      const [ps, pe] = range;
      return startGhost({ task: c.task, selection: v.slice(ps, pe), before: v.slice(0, ps), after: v.slice(pe), instruction: extra }, { mode: 'replace', start: ps, end: pe, label: c.name });
    }
    startGhost({ task: c.task || 'insert', goal: c.goal || '', words: c.words, tags: c.tags || [], instruction: extra }, { mode: 'insert', start: pos, label: c.name });
  }

  async function names(extra, pos) {
    const goal = extra || await promptBox('给什么起名？', { placeholder: '比如：主角的师父 / 一个隐世宗门 / 一门剑法' });
    ta.focus();
    if (!goal) return;
    if (!modelReady()) return toast('请先在设置里配置模型', 'warn');
    card.className = 'ghost-card';
    card.innerHTML = '';
    card.append(h('div', { class: 'ghost-head' }, h('b', null, '起名：' + goal), h('span', { class: 'muted small' }, '生成中…')));
    card.hidden = false;
    let at = caretXY(pos);
    place(card, at.x, at.y);
    try {
      const r = await api.post('/ai/json', { task: 'names', book_id: store.book.id, chapter_id: store.chapter?.id, goal });
      const list = Array.isArray(r.names) ? r.names.filter((n) => n && n.name) : [];
      card.innerHTML = '';
      card.append(
        h('div', { class: 'ghost-head' }, h('b', null, '起名：' + goal), h('span', { class: 'muted small' }, '点名字插入到光标处')),
        h('div', { class: 'name-list' }, list.map((n) => h('button', {
          class: 'name-pick',
          title: n.note || '',
          onmousedown: (e) => { e.preventDefault(); card.hidden = true; replaceText(pos, pos, n.name); },
        }, h('b', null, n.name), n.note ? h('span', { class: 'muted small' }, n.note) : null))),
        h('div', { class: 'ghost-actions' }, h('button', { class: 'mini', onmousedown: (e) => { e.preventDefault(); card.hidden = true; } }, '关闭')));
      at = caretXY(pos);
      place(card, at.x, at.y);
    } catch (e) {
      card.hidden = true;
      toast(e.message, 'error', 5000);
    }
  }

  // ---------- 选区工具条 ----------

  function hideBubble() {
    bubble.hidden = true;
  }

  function showBubble() {
    if (composing || ghost || slash || document.activeElement !== ta) return hideBubble();
    const s = ta.selectionStart;
    const e = ta.selectionEnd;
    const text = ta.value.slice(s, e);
    if (e - s < 2 || !text.trim()) return hideBubble();
    const v = ta.value;
    const run = (task, label, instruction = '') => startGhost({ task, selection: text, before: v.slice(0, s), after: v.slice(e), instruction }, { mode: 'replace', start: s, end: e, label });
    const btn = (label, fn) => h('button', { class: 'mini', onmousedown: (ev) => { ev.preventDefault(); fn(); } }, label);
    bubble.innerHTML = '';
    bubble.append(
      btn('润色', () => run('polish', '润色')),
      btn('扩写', () => run('expand', '扩写')),
      btn('精简', () => run('shorten', '精简')),
      btn('改写…', async () => {
        const goal = await promptBox('怎么改？', { placeholder: '比如：改成主角视角 / 语气更狠一点 / 加一句对话（可留空）' });
        if (goal === null) return ta.focus();
        run('rewrite', '改写', goal);
      }),
      h('span', { class: 'sep' }),
      btn('收藏到文风库', () => {
        const m = store.chapters.find((c) => c.id === store.chapter?.id);
        openSaveToLibrary({ content: text, book_id: store.book.id, source: store.book.title, title: m ? chapterLabel(m) : '', genre: store.book.genre });
      }));
    bubble.hidden = false;
    const at = caretXY(s);
    bubble.style.left = '0px';
    const left = Math.max(8, Math.min(at.x, wrap.clientWidth - bubble.offsetWidth - 8));
    const top = at.y - bubble.offsetHeight - 4;
    bubble.style.left = left + 'px';
    bubble.style.top = (top < 4 ? at.y + style.lh + 4 : top) + 'px';
  }

  // ---------- 事件 ----------

  const onInput = (e) => {
    if (internal) { render(); return; }
    if (e.isComposing || composing) { render(); return; }
    hideBubble();
    followTyping();
    checkSlash();
    scheduleAuto();
    if (ghost) update(); else render();
  };

  const onKeydown = (e) => {
    if (e.isComposing || e.keyCode === 229) return;
    const mod = e.ctrlKey || e.metaKey;
    if (slash && !menu.hidden) {
      const n = items().length;
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        if (n) slash.index = (slash.index + (e.key === 'ArrowDown' ? 1 : -1) + n) % n;
        fillMenu();
        return;
      }
      if ((e.key === 'Enter' || e.key === 'Tab') && n) { e.preventDefault(); choose(); return; }
      if (e.key === 'Escape') { e.preventDefault(); closeMenu(); return; }
    }
    if (ghost) {
      if (e.key === 'Tab' && !e.shiftKey) { e.preventDefault(); acceptGhost(false); return; }
      if (mod && e.key === 'ArrowRight' && ghost.mode === 'insert') { e.preventDefault(); acceptGhost(true); return; }
      if (e.key === 'Escape') { e.preventDefault(); closeGhost(); return; }
      if (mod && e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); regenerate(); return; }
      if (MOVE_KEYS.includes(e.key)) closeGhost();
    }
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      if (e.shiftKey) emit('ai-run', 'continue');
      else triggerGhost();
      return;
    }
    if (mod && (e.key === '/' || e.code === 'Slash')) {
      e.preventDefault();
      const p = ta.selectionStart;
      replaceText(p, ta.selectionEnd, '/');
      openMenu(p);
      return;
    }
    if (e.key === 'Escape') hideBubble();
  };

  const onMousedown = (e) => {
    hover.hidden = true;
    if (ghost) closeGhost();
    if (slash) closeMenu();
    hideBubble();
    if ((e.ctrlKey || e.metaKey) && entityRe) {
      const m = markAt(e.clientX, e.clientY);
      const ent = m && entryOf(m);
      if (ent) { e.preventDefault(); openEntry(ent); }
    }
  };

  const onMousemove = (e) => {
    if (frame) return;
    const { clientX, clientY } = e;
    frame = requestAnimationFrame(() => { frame = 0; showHover(clientX, clientY); });
  };

  const onScroll = () => {
    back.scrollTop = ta.scrollTop;
    hover.hidden = true;
    hideBubble();
    if (slash) fillMenu();
    else if (ghost) update();
  };

  const listeners = [
    ['input', onInput],
    ['keydown', onKeydown],
    ['mousedown', onMousedown],
    ['mousemove', onMousemove],
    ['mouseleave', () => { hover.hidden = true; }],
    ['scroll', onScroll],
    ['mouseup', () => setTimeout(showBubble, 0)],
    ['keyup', (e) => { if (e.shiftKey || e.key === 'Shift') showBubble(); }],
    ['compositionstart', () => { composing = true; hideBubble(); if (ghost) update(); }],
    ['compositionend', () => { composing = false; followTyping(); checkSlash(); scheduleAuto(); if (ghost) update(); else render(); }],
    ['blur', () => { hover.hidden = true; setTimeout(() => { if (document.activeElement !== ta) hideBubble(); }, 150); }],
  ];
  listeners.forEach(([ev, fn]) => ta.addEventListener(ev, fn));
  const ro = new ResizeObserver(() => { syncStyle(); render(); hideBubble(); if (ghost) update(); });
  ro.observe(ta);
  setEntities();

  return {
    /** 程序修改了正文（切换章节、AI 面板写入）后调用 */
    refresh() {
      if (ghost) closeGhost();
      if (slash) closeMenu();
      hideBubble();
      render();
    },
    setEntities,
    triggerGhost,
    busy: () => !!(ghost || slash),
    destroy() {
      clearTimeout(autoTimer);
      if (ghost) closeGhost();
      ro.disconnect();
      listeners.forEach(([ev, fn]) => ta.removeEventListener(ev, fn));
    },
  };
}
