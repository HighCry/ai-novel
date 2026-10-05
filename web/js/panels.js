import { api, stream } from './api.js';
import { store, on, emit, scope, chapterLabel, modelReady } from './store.js';
import { h, toast, busy, cleanAi, copyText, countWords, KIND, keywords, diffClauses, applyDiff } from './ui.js';
import { openSettings, openEntry, openThread, openFinalize, openContext, openTavernImport, openVolume, openPreview, openRelations, openSkills } from './dialogs.js';
import { openReveals, openRevealEditor } from './reveals.js';

const TABS = [['ai', 'AI 写作'], ['bible', '设定库'], ['threads', '伏笔'], ['check', '检查'], ['memory', '记忆'], ['chat', '对话']];
const TASK_LABEL = { continue: '续写', write_chapter: '整章初稿', expand: '扩写', shorten: '缩写', rewrite: '改写', polish: '润色', deslop: '去 AI 味', proofread: '校对' };
const SELECT_TASKS = ['expand', 'shorten', 'rewrite', 'polish'];
/** 只改原文、不写新内容的任务：可以整章做，结果直接进修订模式，采纳后不算 AI 字数 */
const REVISE_TASKS = ['deslop', 'proofread'];

let container = null;
let current = localStorage.getItem('panelTab') || 'ai';
let aiEl = null;
let chatEl = null;
let checkCache = null;

export function renderPanels(el) {
  container = el;
  aiEl = null;
  chatEl = null;
  checkCache = null;
  if (!TABS.some(([id]) => id === current)) current = 'ai';
  el.append(
    h('div', { class: 'tabs' }, TABS.map(([id, label]) => h('button', { class: 'tab' + (id === current ? ' active' : ''), dataset: { tab: id }, onclick: () => switchTab(id) }, label))),
    h('div', { class: 'panel-body' }));
  draw();
  scope.add(on('chapter-loaded', () => { if (current !== 'ai' && current !== 'chat') draw(); }));
  scope.add(on('entries-changed', () => { if (current === 'bible') draw(); chatEl?.refresh(); }));
  scope.add(on('threads-changed', () => { if (current === 'threads') draw(); }));
  scope.add(on('switch-tab', switchTab));
  scope.add(on('ai-run', (t) => {
    switchTab('ai');
    const { task, ...extra } = typeof t === 'string' ? { task: t } : t;
    aiEl?.run(task, extra);
  }));
  scope.add(on('settings-changed', () => aiEl?.refreshNotice()));
  scope.add(() => { aiEl = null; chatEl = null; checkCache = null; container = null; });
}

function switchTab(id) {
  current = id;
  localStorage.setItem('panelTab', id);
  container?.querySelectorAll('.tab').forEach((b) => b.classList.toggle('active', b.dataset.tab === id));
  draw();
}

function draw() {
  if (!container) return;
  const body = container.querySelector('.panel-body');
  body.innerHTML = '';
  ({ ai: aiPanel, bible: biblePanel, threads: threadsPanel, check: checkPanel, memory: memoryPanel, chat: chatPanel })[current](body);
}

const empty = (text) => h('div', { class: 'empty' }, text);
const groupTitle = (text) => h('div', { class: 'group-title' }, text);

// ---------------- AI 写作 ----------------

function aiPanel(body) {
  if (!aiEl) aiEl = buildAi();
  body.append(aiEl);
}

function buildAi() {
  const notice = h('div', { class: 'notice' }, '还没有配置模型，', h('a', { href: '#', onclick: (e) => { e.preventDefault(); openSettings(); } }, '去设置'), '。');
  const instruction = h('textarea', { rows: 2, placeholder: '补充要求（可选）：比如“多写对话”“这里埋个伏笔”“节奏快一点”' });
  const chips = ['多写对话', '加强冲突', '更爽一点', '节奏紧凑', '多些细节', '章末留钩子', '加点幽默'].map((t) =>
    h('button', { class: 'chip', onclick: () => { instruction.value = (instruction.value.trim() ? instruction.value.trim() + '；' : '') + t; } }, t));
  const words = h('input', { type: 'number', min: 100, max: 8000, step: 100, value: 800, class: 'num', title: '续写的目标字数' });
  const variants = h('select', { title: '同时生成几个版本对比挑选' }, [1, 2, 3].map((n) => h('option', { value: n }, n === 1 ? '1 个版本' : `${n} 个版本对比`)));
  const outputs = h('div', { class: 'outputs' });
  const skillSel = h('select', { class: 'skill-select', title: '去 AI 味时按哪个写作技能修订', onchange: () => localStorage.setItem('reviseSkill', skillSel.value) });
  const fillSkills = async () => {
    let list;
    try { list = await api.get('/skills'); } catch { return; }
    const keep = localStorage.getItem('reviseSkill') || 'deslop';
    skillSel.replaceChildren(...list.map((s) => h('option', { value: s.id, selected: s.id === keep }, s.name)));
  };
  fillSkills();
  scope.add(on('skills-changed', fillSkills));
  let lastTask = 'continue';
  const run = (task, extra = {}) => {
    lastTask = task;
    return generate(task, {
      instruction: instruction.value.trim(),
      words: Number(words.value) || 800,
      variants: Number(variants.value) || 1,
      outputs,
      skill: skillSel.value || 'deslop',
      skillName: skillSel.selectedOptions[0]?.textContent || '',
      ...extra,
    });
  };
  const finale = h('label', { class: 'check', title: '写整章和规划节拍时按全书最后一章处理，回收所有未回收的伏笔' },
    h('input', { type: 'checkbox', checked: !!store.finale, onchange: (e) => { store.finale = e.target.checked; } }), '这是大结局');
  const previewLink = h('a', {
    href: '#',
    class: 'small',
    onclick: (e) => {
      e.preventDefault();
      if (!store.chapter) return toast('请先打开一个章节', 'warn');
      const sel = store.editor.selection();
      const body = { task: lastTask, book_id: store.book.id, chapter_id: store.chapter.id, instruction: instruction.value.trim(), words: Number(words.value) || 800, finale: !!store.finale };
      if (SELECT_TASKS.includes(lastTask)) Object.assign(body, { selection: sel.text || '（这里是选中的文字）', before: sel.before, after: sel.after });
      if (REVISE_TASKS.includes(lastTask)) Object.assign(body, sel.text ? { selection: sel.text, before: sel.before, after: sel.after } : { selection: store.editor.text() }, { skill: skillSel.value });
      if (lastTask === 'continue') Object.assign(body, { before: store.editor.text().slice(0, sel.end), after: store.editor.text().slice(sel.end) });
      openPreview(body);
    },
  }, '查看将要发送的提示词');
  const el = h('div', { class: 'ai-panel' },
    notice,
    groupTitle('写作'),
    h('div', { class: 'btn-grid' },
      h('button', { class: 'btn primary', title: '从光标处接着写（Ctrl+Enter）', onclick: () => run('continue') }, '续写'),
      h('button', { class: 'btn', title: '按本章章纲写出整章初稿', onclick: () => run('write_chapter') }, '写整章')),
    groupTitle('选中正文里的一段后'),
    h('div', { class: 'btn-grid four' }, SELECT_TASKS.map((t) => h('button', { class: 'btn', onclick: () => run(t) }, TASK_LABEL[t]))),
    groupTitle('校对（只改错别字、病句、标点）'),
    h('div', { class: 'btn-grid' },
      h('button', { class: 'btn', title: '逐段找错别字、病句和标点错误，不润色；改动可以逐句选择', onclick: () => run('proofread') }, '整章校对'),
      h('button', { class: 'btn', title: '只校对正文里选中的段落', onclick: () => run('proofread', { selectionOnly: true }) }, '选中段校对')),
    groupTitle('去 AI 味'),
    h('div', { class: 'btn-grid' },
      h('button', { class: 'btn', title: '按写作技能和本地检测结果修订整章，改动可以逐句选择', onclick: () => run('deslop') }, '整章去 AI 味'),
      h('button', { class: 'btn', title: '只修订正文里选中的段落', onclick: () => run('deslop', { selectionOnly: true }) }, '选中段去 AI 味')),
    h('div', { class: 'row' },
      h('label', { class: 'inline' }, '按技能 ', skillSel),
      h('span', { class: 'spacer' }),
      h('a', { href: '#', class: 'small', onclick: (e) => { e.preventDefault(); openSkills(); } }, '管理写作技能')),
    instruction,
    h('div', { class: 'chips' }, chips),
    h('div', { class: 'row' }, h('label', { class: 'inline' }, '续写字数 ', words), variants),
    h('div', { class: 'row' }, finale, h('span', { class: 'spacer' }), previewLink),
    outputs,
    groupTitle('问问编辑'),
    askBox());
  el.run = run;
  el.refreshNotice = () => { notice.hidden = modelReady(); };
  el.refreshNotice();
  return el;
}

async function generate(task, { instruction, words, variants, outputs, skill, skillName, selectionOnly }) {
  const ed = store.editor;
  const ch = store.chapter;
  if (!ch || !ed) return toast('请先打开一个章节', 'warn');
  if (!modelReady()) { toast('请先在设置里配置模型', 'warn'); openSettings(); return; }
  let sel = ed.selection();
  const revising = REVISE_TASKS.includes(task);
  const needSel = SELECT_TASKS.includes(task) || (revising && selectionOnly);
  if (needSel && !sel.text.trim()) return toast('请先在正文里选中一段文字', 'warn');
  if (revising && !selectionOnly) {
    const all = ed.text();
    if (!all.trim()) return toast('这一章还没有正文', 'warn');
    sel = { start: 0, end: all.length, text: all, before: '', after: '' };
  }
  try { await ed.save(); } catch { return; }
  const body = { task, book_id: store.book.id, chapter_id: ch.id, instruction, finale: !!store.finale };
  if (task === 'continue') {
    body.before = ed.text().slice(0, sel.end);
    body.after = ed.text().slice(sel.end);
    body.words = words;
  } else if (needSel || revising) {
    Object.assign(body, { selection: sel.text, before: sel.before, after: sel.after });
  }
  if (task === 'deslop') body.skill = skill;
  outputs.querySelectorAll('.out-card.finished').forEach((c) => c.remove());
  for (let i = 0; i < variants; i++) runVariant({ ...body, variant: i }, sel, outputs, i, variants, task === 'deslop' ? skillName : '');
}

function runVariant(body, sel, outputs, i, n, note = '') {
  const ctrl = new AbortController();
  const textEl = h('div', { class: 'out-text' });
  const status = h('span', { class: 'out-status' }, '连接中…');
  const stopBtn = h('button', { class: 'mini', onclick: () => ctrl.abort() }, '停止');
  const actions = h('div', { class: 'out-actions' });
  const refsEl = h('div', { class: 'out-refs', hidden: true });
  const warnEl = h('div', { class: 'out-warn', hidden: true });
  const title = (n > 1 ? `版本 ${i + 1} · ` : '') + TASK_LABEL[body.task] + (note ? ` · ${note}` : '');
  const card = h('div', { class: 'out-card' }, h('div', { class: 'out-head' }, h('b', null, title), status, stopBtn), refsEl, warnEl, textEl, actions);
  outputs.prepend(card);
  let thinking = 0;
  let revise = null;
  stream(body, {
    signal: ctrl.signal,
    onMeta: (m) => {
      const parts = [];
      if (m.refs?.length) parts.push('参考范文：' + m.refs.map((r) => `《${r.title}》`).join(''));
      if (m.guide) parts.push('已用文风指南');
      refsEl.textContent = parts.join(' · ');
      refsEl.hidden = !parts.length;
      revise = m.revise || null;
    },
    onDelta: (_, full) => {
      textEl.textContent = full;
      textEl.scrollTop = textEl.scrollHeight;
      status.textContent = `生成中 · ${countWords(full)} 字`;
    },
    onThinking: (c) => { thinking += c; status.textContent = `模型思考中 · ${thinking} 字`; },
  }).then((full) => {
    status.textContent = `完成 · ${countWords(cleanAi(full))} 字`;
    finish();
  }).catch((e) => {
    if (e.name === 'AbortError') {
      status.textContent = '已停止';
      finish();
    } else {
      status.textContent = '失败';
      textEl.textContent = e.message;
      card.classList.add('error');
      actions.append(h('button', { class: 'mini', onclick: () => card.remove() }, '关闭'));
    }
  }).finally(() => stopBtn.remove());

  let diffParts = null;
  function showDiff(on) {
    if (!on) {
      if (diffParts) textEl.textContent = applyDiff(diffParts);
      diffParts = null;
      textEl.contentEditable = 'true';
      textEl.classList.remove('diff');
      return;
    }
    diffParts = diffClauses(sel.text, cleanAi(textEl.innerText));
    textEl.contentEditable = 'false';
    textEl.classList.add('diff');
    const draw = () => {
      textEl.innerHTML = '';
      for (const p of diffParts) {
        if (p.type === 'eq') { textEl.append(p.text); continue; }
        const chg = h('span', { class: 'chg' + (p.on ? ' on' : ''), title: p.on ? '已采纳修改，点击改回原文' : '保留原文，点击采纳修改', onclick: () => { p.on = !p.on; draw(); } },
          p.del ? h('del', null, p.del) : null, p.ins ? h('ins', null, p.ins) : null);
        textEl.append(chg);
      }
      checkLoss();
    };
    draw();
  }

  function finish() {
    card.classList.add('finished');
    textEl.contentEditable = 'true';
    textEl.title = '可以先在这里修改，再放进正文';
    const ed = () => store.editor;
    const end = () => ed().text().length;
    // 每个按钮：[文字, 放进正文的操作, 会被替换的范围（用来记录采纳位置，供学习作者的修改）]
    const btns = [];
    if (SELECT_TASKS.includes(body.task) || REVISE_TASKS.includes(body.task)) {
      const whole = REVISE_TASKS.includes(body.task) && !sel.before && !sel.after;
      btns.push([whole ? '替换全文' : '替换选中', () => ed().replaceRange(sel.start, sel.end, get(), sel.text), () => [sel.start, sel.end]]);
      const toggle = h('button', { class: 'mini', title: '按句对比原文和修改，逐条决定采纳哪些', onclick: () => { showDiff(!diffParts); toggle.textContent = diffParts ? '退出修订' : '修订模式'; } }, '修订模式');
      actions.append(toggle);
      // 去 AI 味、校对直接进修订模式：改了哪些一目了然，不想要的点一下改回原文
      if (REVISE_TASKS.includes(body.task) && textEl.innerText.trim()) {
        showDiff(true);
        toggle.textContent = '退出修订';
        if (body.task === 'deslop') compareAi();
        else {
          const n = diffParts.filter((p) => p.type !== 'eq').length;
          status.textContent = n ? `完成 · ${n} 处修改，点一下可以改回原文` : '完成 · 没有发现要改的地方';
        }
      }
    }
    if (body.task === 'continue') btns.push(['插入到光标处', () => ed().insertAt(sel.end, get()), () => [sel.end, sel.end]]);
    if (body.task === 'write_chapter') {
      btns.push([store.editor.text().trim() ? '替换全文' : '放进正文', () => ed().replaceAll(get()), () => [0, end()]]);
      if (store.editor.text().trim()) btns.push(['追加到末尾', () => ed().append(get()), () => [end(), end()]]);
    }
    if (body.task !== 'write_chapter' && body.task !== 'continue' && !SELECT_TASKS.includes(body.task)) btns.push(['追加到末尾', () => ed().append(get()), () => [end(), end()]]);
    actions.append(
      ...btns.map(([label, fn, range], k) => h('button', { class: 'mini' + (k === 0 ? ' primary' : ''), onclick: () => accept(fn, range) }, label)),
      h('button', { class: 'mini', onclick: () => copyText(get()) }, '复制'),
      h('button', { class: 'mini', onclick: () => card.remove() }, '丢弃'));
  }

  function get() { return diffParts ? applyDiff(diffParts) : cleanAi(textEl.innerText); }

  /** 去 AI 味：按当前采纳的修改算删减比例，核对设定词有没有被改掉；返回删减百分比 */
  function checkLoss() {
    if (!revise) return null;
    const after = get();
    const len = (s) => s.replace(/\s/g, '').length;
    const cut = Math.round((1 - len(after) / Math.max(1, len(sel.text))) * 100);
    const lost = (revise.keep || []).filter((t) => sel.text.includes(t) && !after.includes(t));
    const limit = parseInt(revise.max_cut, 10) || 25;
    const warns = [];
    if (cut > limit) warns.push(`删减了 ${cut}%，超过这一档 ${limit}% 的上限，可能删掉了有用的信息。`);
    if (cut < -5) warns.push(`修改稿比原文长了 ${-cut}%，可能补写了原文没有的情节、细节或台词。`);
    if (lost.length) warns.push(`这些设定词在修改稿里不见了：${lost.join('、')}。`);
    warnEl.textContent = warns.length ? warns.join('') + '在修订模式里点一下对应的修改，就能改回原文。' : '';
    warnEl.hidden = !warns.length;
    return cut;
  }

  async function compareAi() {
    const cut = checkLoss();
    try {
      const [a, b] = await Promise.all([api.post('/lint', { text: sel.text }), api.post('/lint', { text: get() })]);
      status.textContent = `AI 味 ${a.ai_level} ${a.ai_density.toFixed(1)} → ${b.ai_level} ${b.ai_density.toFixed(1)}` + (cut == null ? '' : cut >= 0 ? ` · 删减 ${cut}%` : ` · 增加 ${-cut}%`);
    } catch { /* 对比失败不影响结果本身 */ }
  }

  async function accept(apply, range) {
    if (store.chapter?.id !== body.chapter_id) return toast('当前打开的不是生成时的章节，可以复制后手动粘贴', 'warn', 4000);
    const text = get();
    if (!text) return;
    try { await store.editor.save(); } catch { return; }
    const v = store.editor.text();
    const [s, e] = range();
    const where = { before: v.slice(Math.max(0, s - 30), s), after: v.slice(e, e + 30) };
    if (!apply()) return;
    try { await store.editor.save({ snapshot: `AI ${TASK_LABEL[body.task]}前` }); } catch { return; }
    // 去 AI 味、校对只是改原文，不算新采纳的 AI 字数，也不拿来学习作者的改法
    if (!REVISE_TASKS.includes(body.task)) {
      api.post(`/chapters/${body.chapter_id}/ai_accept`, { chars: countWords(text), text, task: body.task, ...where })
        .then(() => { store.chapter.ai_chars = (store.chapter.ai_chars || 0) + countWords(text); store.editor.refreshCount(); })
        .catch(() => {});
    }
    outputs.querySelectorAll('.out-card.finished').forEach((c) => c.remove());
    toast('已放进正文（修改前的内容已存快照，可在「历史」恢复）');
  }
}

function askBox() {
  const q = h('textarea', { rows: 2, placeholder: '比如：这一章的冲突怎么写更有张力？给我三个反转思路。' });
  const out = h('div', { class: 'out-text answer', hidden: true });
  const btn = h('button', {
    class: 'btn',
    onclick: async () => {
      if (!q.value.trim()) return toast('先写下你的问题', 'warn');
      if (!store.chapter) return toast('请先打开一个章节', 'warn');
      try { await store.editor.save(); } catch { return; }
      out.hidden = false;
      out.textContent = '思考中…';
      await busy(btn, async () => {
        await stream({ task: 'free', book_id: store.book.id, chapter_id: store.chapter.id, instruction: q.value.trim() }, {
          onDelta: (_, full) => { out.textContent = full; },
        });
      }, '回答中…');
    },
  }, '提问');
  return h('div', { class: 'ask' }, q, btn, out);
}

// ---------------- 设定库 ----------------

function biblePanel(body) {
  let kind = 'all';
  let query = '';
  const content = store.editor?.text() || '';
  const list = h('div', { class: 'entry-list' });
  const render = () => {
    list.innerHTML = '';
    const items = store.entries.filter((e) => (kind === 'all' || e.kind === kind)
      && (!query || [e.name, e.aliases, e.description, e.state].join(' ').includes(query)));
    if (!items.length) {
      list.append(empty(store.entries.length ? '没有匹配的设定' : '设定库还是空的。人物、地点、物品、势力、修炼体系都记在这里，AI 写作时会自动带上本章出场的条目。'));
    }
    for (const e of items) {
      const present = keywords(e).some((k) => content.includes(k));
      list.append(h('div', { class: 'entry-card' + (present ? ' present' : ''), onclick: () => openEntry(e) },
        h('div', { class: 'entry-head' },
          h('span', { class: 'tag k-' + e.kind }, KIND[e.kind] || '其他'),
          h('b', null, e.name),
          e.role ? h('span', { class: 'muted small' }, e.role) : null,
          e.always_include ? h('span', { class: 'badge' }, '常驻') : null,
          e.visibility ? h('span', { class: 'badge gray', title: '可见性：写到这里之前 AI 看不到这条设定' }, e.visibility) : null,
          e.secret ? h('span', { class: 'badge gray', title: '有作者底牌，任何时候都不发给 AI' }, '底牌') : null,
          present ? h('span', { class: 'badge green' }, '本章出场') : null),
        e.aliases ? h('div', { class: 'muted small' }, '又称：' + e.aliases) : null,
        e.state ? h('div', { class: 'entry-state' }, e.state) : null,
        e.description ? h('div', { class: 'entry-desc' }, e.description.length > 90 ? e.description.slice(0, 90) + '…' : e.description) : null));
    }
  };
  const kinds = h('div', { class: 'chips' }, [['all', '全部'], ...Object.entries(KIND)].map(([k, label]) => h('button', {
    class: 'chip' + (k === kind ? ' on' : ''),
    onclick: (ev) => {
      kind = k;
      kinds.querySelectorAll('.chip').forEach((c) => c.classList.remove('on'));
      ev.currentTarget.classList.add('on');
      render();
    },
  }, label)));
  body.append(
    h('div', { class: 'row' },
      h('button', { class: 'btn primary', onclick: () => openEntry({ kind: kind === 'all' ? 'character' : kind }) }, '＋ 新建设定'),
      h('button', { class: 'btn', onclick: () => openRelations() }, '人物关系图'),
      h('button', { class: 'btn', onclick: () => openTavernImport() }, '导入酒馆卡')),
    h('input', { class: 'search', placeholder: '搜索名称、别名、描述', oninput: (e) => { query = e.target.value.trim(); render(); } }),
    kinds,
    list);
  render();
}

// ---------------- 伏笔 ----------------

function threadsPanel(body) {
  let filter = 'open';
  const curNum = store.chapter && store.chapters.find((c) => c.id === store.chapter.id)?.number;
  const list = h('div');
  const setStatus = async (t, status) => {
    await api.patch(`/threads/${t.id}`, { status, resolved_chapter_id: status === 'resolved' ? store.chapter?.id ?? null : null });
    store.threads = await api.get(`/books/${store.book.id}/threads`);
    render();
    toast(status === 'resolved' ? '已标记回收' : '已重新打开');
  };
  const render = () => {
    list.innerHTML = '';
    const items = store.threads.filter((t) => filter === 'all' || (filter === 'open' ? t.status === 'open' : t.status !== 'open'));
    if (!items.length) list.append(empty(filter === 'open' ? '没有未回收的伏笔。定稿章节时，AI 会自动把新埋下的伏笔记到这里。' : '没有记录'));
    for (const t of items) {
      const planted = store.chapters.find((c) => c.id === t.planted_chapter_id);
      const resolved = store.chapters.find((c) => c.id === t.resolved_chapter_id);
      const age = planted?.number != null && curNum != null ? curNum - planted.number : null;
      const overdue = t.status === 'open' && age >= 30;
      list.append(h('div', { class: 'thread-card ' + t.status + (overdue ? ' overdue' : '') },
        h('div', { class: 'entry-head' },
          h('b', null, t.title),
          h('span', { class: 'badge ' + (t.status === 'open' ? '' : 'green') }, t.status === 'open' ? '未回收' : t.status === 'resolved' ? '已回收' : '已放弃')),
        t.detail ? h('div', { class: 'entry-desc pre' }, t.detail) : null,
        h('div', { class: 'muted small' },
          planted ? `埋于 ${chapterLabel(planted)}` : '未关联章节',
          resolved ? ` · 回收于 ${chapterLabel(resolved)}` : '',
          overdue ? h('span', { class: 'warn-text' }, ` · 已过 ${age} 章，考虑回收`) : ''),
        h('div', { class: 'out-actions' },
          t.status === 'open'
            ? h('button', { class: 'mini primary', onclick: () => setStatus(t, 'resolved') }, '在本章回收')
            : h('button', { class: 'mini', onclick: () => setStatus(t, 'open') }, '重新打开'),
          h('button', { class: 'mini', onclick: () => openThread(t) }, '编辑'))));
    }
  };
  const seg = h('div', { class: 'seg' }, [['open', '未回收'], ['done', '已回收'], ['all', '全部']].map(([v, label]) => h('button', {
    class: 'seg-btn' + (v === filter ? ' on' : ''),
    onclick: (e) => { filter = v; seg.querySelectorAll('.seg-btn').forEach((b) => b.classList.remove('on')); e.currentTarget.classList.add('on'); render(); },
  }, label)));
  body.append(h('div', { class: 'row' }, h('button', { class: 'btn primary', onclick: () => openThread({ planted_chapter_id: store.chapter?.id ?? null }) }, '＋ 记一个伏笔'), seg), list);
  render();
}

// ---------------- 检查 ----------------

function checkPanel(body) {
  const ch = store.chapter;
  if (!checkCache || checkCache.chapterId !== ch?.id) checkCache = { chapterId: ch?.id, el: h('div', { class: 'check-out' }) };
  const out = checkCache.el;
  const needChapter = (fn) => async (e) => {
    const btn = e.currentTarget;
    if (!store.chapter) return toast('请先打开一个章节', 'warn');
    try { await store.editor.save(); } catch { return; }
    await busy(btn, () => fn(out), '检查中…');
  };
  body.append(
    h('div', { class: 'btn-col' },
      h('button', { class: 'btn', onclick: (e) => busy(e.currentTarget, () => runContinuity(out), '体检中…') }, '连续性体检：伏笔、人物缺席、节奏（本地，免费）'),
      h('button', { class: 'btn', onclick: () => openReveals() }, '信息节奏：秘密什么时候埋种子、给线索、揭开'),
      h('button', { class: 'btn', onclick: needChapter(runLint) }, '文字质量：套话、AI 腔、跨章重复（本地，免费）'),
      h('button', { class: 'btn', onclick: needChapter(runConsistency) }, '设定一致性检查（AI）'),
      h('button', { class: 'btn', onclick: needChapter(runReview) }, '编辑审稿打分（AI）'),
      h('button', { class: 'btn', onclick: needChapter((o) => runText(o, 'first_read', '冷读者反馈：一个没看过设定的读者怎么看这一章')) }, '冷读者反馈（AI）'),
      h('button', { class: 'btn', onclick: needChapter((o) => runText(o, 'revision_plan', '修订计划', o.dataset.feedback || '')) }, '生成修订计划（AI，会参考下面已有的检查结果）'),
      h('button', { class: 'btn', onclick: (e) => busy(e.currentTarget, () => runSubmission(out), '检查中…') }, '全书投稿检查')),
    out);
}

function locate(text) {
  const i = store.editor.find(text);
  if (i >= 0) store.editor.select(i, i + text.length);
  else toast('正文里没找到这段原文（可能已经改过了）', 'warn');
}

async function runContinuity(out) {
  const r = await api.get(`/books/${store.book.id}/continuity${store.chapter ? `?chapter_id=${store.chapter.id}` : ''}`);
  out.innerHTML = '';
  const list = r.findings;
  const critical = list.filter((f) => f.level === 'critical').length;
  out.append(h('div', { class: 'score-line' }, h('b', null, list.length ? `发现 ${list.length} 处需要留意（${critical} 处严重）` : '没有发现连续性问题')));
  if (!list.some((f) => f.kind === '张力偏低' || f.kind === '爽点断档')) {
    out.append(h('p', { class: 'hint' }, '节奏检查需要先对章节做「定稿」，定稿时会顺带分析张力和爽点。'));
  }
  if (!store.entries.some((e) => e.visibility)) {
    out.append(h('p', { class: 'hint' }, '给后期才登场或揭晓的设定设上可见性（编辑设定 → 可见性），体检就能查出提前写出来的地方。'));
  }
  if (!list.some((f) => f.reveal_id)) {
    out.append(h('p', { class: 'hint' }, '做一份揭示计划（上面的「信息节奏」），体检还会查泄露词提前出现、线索断档、揭开前没唤醒和揭示逾期。'));
  }
  const openReveal = async (id) => {
    const r = (await api.get(`/books/${store.book.id}/reveals`)).reveals.find((x) => x.id === id);
    if (r) openRevealEditor(r);
  };
  for (const f of list) {
    const target = async () => {
      if (f.chapter_id) {
        await store.editor.open(f.chapter_id);
        if (f.quote) locate(f.quote.replace(/……$/, ''));
      } else if (f.reveal_id) openReveal(f.reveal_id);
      else if (f.entry_id) { const e = store.entries.find((x) => x.id === f.entry_id); if (e) openEntry(e); } else if (f.thread_id) { const t = store.threads.find((x) => x.id === f.thread_id); if (t) openThread(t); }
    };
    out.append(h('div', { class: 'issue clickable ' + (f.level === 'critical' ? 'high' : f.level === 'warn' ? 'medium' : 'low'), onclick: target },
      h('div', { class: 'issue-head' }, h('span', { class: 'tag' }, f.kind)),
      h('div', { class: 'small' }, f.message),
      f.quote ? h('blockquote', null, f.quote) : null));
  }
}

async function runLint(out) {
  const r = await api.post('/lint', { text: store.editor.text(), book_id: store.book.id, chapter_id: store.chapter.id });
  out.innerHTML = '';
  const s = r.stats;
  out.append(
    h('div', { class: 'score-line' }, h('span', { class: 'score ' + (r.score >= 85 ? 'good' : r.score >= 65 ? 'mid' : 'bad') }, r.score), h('span', null, '文字质量分（本地规则估算）')),
    h('div', { class: 'score-line' },
      h('span', { class: 'ai-level ' + (r.ai_level === '轻度' ? 'good' : r.ai_level === '中度' ? 'mid' : 'bad') }, `AI 味${r.ai_level}`),
      h('span', { class: 'grow small muted' }, `每千字约 ${r.ai_density.toFixed(1)} 处套话和模板句`),
      h('button', { class: 'mini', title: 'AI 逐段找错别字、病句和标点错误，不润色', onclick: () => emit('ai-run', 'proofread') }, '校对'),
      h('button', { class: 'mini primary', title: '按写作技能修订整章，改动可以逐句选择', onclick: () => emit('ai-run', 'deslop') }, '去 AI 味')),
    h('div', { class: 'stats-grid' },
      stat('字数', s.chars), stat('段落', s.paragraphs), stat('对话占比', Math.round(s.dialogue_ratio * 100) + '%'),
      stat('平均句长', s.avg_sentence.toFixed(0)), stat('最长段落', s.max_paragraph), stat('长段落', s.long_paragraphs)));
  if (!r.issues.length) out.append(empty('没有发现明显问题'));
  if (r.issues.some((i) => i.kind === '标点')) {
    const fix = h('button', {
      class: 'btn primary',
      onclick: () => busy(fix, async () => {
        const res = await api.post('/tools/normalize', { text: store.editor.text() });
        if (!res.changed) return toast('没有需要修正的标点');
        await store.editor.save();
        store.editor.replaceAll(res.text);
        await store.editor.save({ snapshot: '修正标点前' });
        toast(`已修正 ${res.changed} 处标点（修改前已存快照）`);
        runLint(out);
      }, '修正中…'),
    }, '一键修正标点：英文引号、半角标点、省略号');
    out.append(fix);
  }
  for (const issue of r.issues) {
    let k = 0;
    out.append(h('div', {
      class: 'issue ' + issue.severity + (issue.positions.length ? ' clickable' : ''),
      title: issue.positions.length ? '点击依次定位到正文' : '',
      onclick: () => {
        if (!issue.positions.length) return;
        const [a, b] = issue.positions[k % issue.positions.length];
        k += 1;
        store.editor.select(a, b);
      },
    },
    h('div', { class: 'issue-head' }, h('span', { class: 'tag' }, issue.kind), h('b', null, issue.text), issue.count > 1 ? h('span', { class: 'muted' }, `×${issue.count}`) : null),
    h('div', { class: 'small' }, issue.suggestion)));
  }
}

async function runText(out, task, title, feedback = '') {
  const previous = feedback || out.innerText.slice(0, 3000);
  const r = await api.post('/ai/json', { task, book_id: store.book.id, chapter_id: store.chapter.id, instruction: task === 'revision_plan' ? previous : '' });
  out.innerHTML = '';
  out.dataset.feedback = '';
  out.append(h('div', { class: 'score-line' }, h('b', null, title)), h('div', { class: 'out-text answer' }, r.text), h('button', { class: 'mini', onclick: () => copyText(r.text) }, '复制'));
}

function stat(label, value) {
  return h('div', { class: 'stat' }, h('div', { class: 'stat-value' }, value), h('div', { class: 'stat-label' }, label));
}

async function runConsistency(out) {
  const r = await api.post('/ai/json', { task: 'check', book_id: store.book.id, chapter_id: store.chapter.id });
  out.innerHTML = '';
  const issues = r.issues || [];
  out.append(h('div', { class: 'score-line' }, h('b', null, issues.length ? `发现 ${issues.length} 处可能的矛盾` : '没有发现和设定、前情矛盾的地方')));
  for (const it of issues) {
    out.append(h('div', { class: 'issue ' + (it.severity === 'high' ? 'high' : it.severity === 'low' ? 'low' : 'medium') + (it.quote ? ' clickable' : ''), onclick: () => it.quote && locate(it.quote) },
      h('div', { class: 'issue-head' }, h('span', { class: 'tag' }, it.type || '问题')),
      it.quote ? h('blockquote', null, it.quote) : null,
      h('div', { class: 'small' }, it.problem),
      it.suggestion ? h('div', { class: 'small muted' }, '建议：' + it.suggestion) : null));
  }
}

const SCORE_LABEL = { hook: '开头吸引力', pacing: '节奏', payoff: '爽点/情绪', character: '人物', ending: '章末钩子', readability: '可读性' };

async function runReview(out) {
  const r = await api.post('/ai/json', { task: 'review', book_id: store.book.id, chapter_id: store.chapter.id });
  out.innerHTML = '';
  const scores = r.scores || {};
  out.append(h('div', { class: 'check-out' },
    r.summary ? h('div', { class: 'score-line' }, h('b', null, r.summary)) : null,
    h('div', { class: 'bars' }, Object.entries(SCORE_LABEL).map(([k, label]) => {
      const v = Math.max(0, Math.min(10, Number(scores[k]) || 0));
      return h('div', { class: 'bar-row' }, h('span', { class: 'bar-label' }, label), h('span', { class: 'bar' }, h('span', { class: 'bar-fill ' + (v >= 8 ? 'good' : v >= 6 ? 'mid' : 'bad'), style: { width: v * 10 + '%' } })), h('span', { class: 'bar-value' }, v));
    })),
    (r.strengths || []).length ? h('div', null, groupTitle('优点'), h('ul', null, r.strengths.map((s) => h('li', null, s)))) : null,
    (r.problems || []).length ? groupTitle('问题和改法') : null,
    (r.problems || []).map((p) => h('div', { class: 'issue medium' + (p.quote ? ' clickable' : ''), onclick: () => p.quote && locate(p.quote) },
      p.quote ? h('blockquote', null, p.quote) : null,
      h('div', { class: 'small' }, p.problem),
      p.suggestion ? h('div', { class: 'small muted' }, '改法：' + p.suggestion) : null))));
}

async function runSubmission(out) {
  const r = await api.get(`/books/${store.book.id}/check`);
  out.innerHTML = '';
  const errors = r.items.filter((i) => i.level === 'error').length;
  out.append(h('div', { class: 'score-line' }, h('b', null, r.items.length ? `${r.chapters} 章里有 ${r.items.length} 条提示（${errors} 条需要处理）` : `${r.chapters} 章全部通过检查`)));
  for (const it of r.items) {
    out.append(h('div', { class: 'issue clickable ' + (it.level === 'error' ? 'high' : 'low'), onclick: () => store.editor.open(it.chapter_id) },
      h('div', { class: 'issue-head' }, h('b', null, it.label || '（无标题）')),
      h('div', { class: 'small' }, it.message)));
  }
}

// ---------------- 记忆 ----------------

function memoryPanel(body) {
  const ch = store.chapter;
  if (!ch) { body.append(empty('先打开一个章节')); return; }
  const summary = h('textarea', {
    rows: 7,
    value: ch.summary,
    placeholder: '本章摘要：写后面章节时，AI 靠它回顾前情。可以手写，也可以让 AI 生成。',
    onchange: async () => {
      await api.patch(`/chapters/${ch.id}`, { summary: summary.value });
      store.chapter.summary = summary.value;
      toast('摘要已保存');
    },
  });
  const genSummary = h('button', {
    class: 'btn',
    onclick: (e) => busy(e.currentTarget, async () => {
      await store.editor.save();
      const r = await api.post('/ai/json', { task: 'summarize', book_id: store.book.id, chapter_id: ch.id });
      summary.value = r.text;
      store.chapter.summary = r.text;
      const m = store.chapters.find((c) => c.id === ch.id);
      if (m) m.has_summary = true;
      toast('摘要已生成并保存');
    }, '生成中…'),
  }, 'AI 生成摘要');
  const vol = store.volumes.find((v) => v.id === ch.volume_id);
  body.append(h('div', null,
    h('p', { class: 'hint' }, '长篇不崩的关键：每章写完点一次「定稿」，让 AI 记住发生了什么、人物有什么变化、埋了哪些伏笔。'),
    h('button', { class: 'btn primary block', onclick: () => openFinalize() }, '定稿本章：生成摘要，更新设定库和伏笔'),
    groupTitle('本章摘要'),
    summary,
    genSummary,
    vol ? groupTitle(`所属卷：${vol.title}`) : null,
    vol ? h('button', { class: 'btn', onclick: () => openVolume(vol) }, '编辑卷纲和卷摘要') : null,
    groupTitle('上下文'),
    h('p', { class: 'hint' }, 'AI 写这一章时会看到：作品信息、世界观、大纲、前情提要、本章出场的设定、未回收的伏笔、检索到的相关前文、上一章结尾。'),
    h('button', { class: 'btn', onclick: () => openContext() }, '查看 AI 能看到的上下文')));
}

// ---------------- 角色对话 ----------------

function chatPanel(body) {
  if (!chatEl) chatEl = buildChat();
  body.append(chatEl);
  chatEl.refresh();
}

function buildChat() {
  const select = h('select', { onchange: () => { store.chatTarget = Number(select.value) || null; renderLog(); } });
  const log = h('div', { class: 'chat-log' });
  const input = h('textarea', {
    rows: 2,
    placeholder: '对 TA 说点什么……（Enter 发送，Shift+Enter 换行）',
    onkeydown: (e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } },
  });
  const sendBtn = h('button', { class: 'btn primary', onclick: () => send() }, '发送');
  const msgs = () => {
    const id = Number(select.value);
    if (!id) return [];
    store.chats[id] = store.chats[id] || [];
    return store.chats[id];
  };
  function renderLog() {
    log.innerHTML = '';
    const list = msgs();
    if (!list.length) log.append(empty('和人物聊聊，试试 TA 的性格、口头禅和说话方式。人物设定来自设定库，聊天内容不会写进正文。'));
    for (const m of list) log.append(h('div', { class: 'bubble ' + m.role }, m.content));
    log.scrollTop = log.scrollHeight;
  }
  async function send() {
    const id = Number(select.value);
    const text = input.value.trim();
    if (!id) return toast('先在设定库里建一个人物', 'warn');
    if (!text) return;
    const list = msgs();
    list.push({ role: 'user', content: text });
    input.value = '';
    renderLog();
    const bubble = h('div', { class: 'bubble assistant' }, '……');
    log.append(bubble);
    sendBtn.disabled = true;
    try {
      const reply = await stream({ task: 'chat', book_id: store.book.id, entry_id: id, messages: list }, {
        onDelta: (_, full) => { bubble.textContent = full; log.scrollTop = log.scrollHeight; },
      });
      list.push({ role: 'assistant', content: cleanAi(reply) });
    } catch (e) {
      bubble.textContent = '出错了：' + e.message;
      bubble.classList.add('error');
      list.pop();
    } finally {
      sendBtn.disabled = false;
    }
  }
  const el = h('div', { class: 'chat-panel' },
    h('p', { class: 'hint' }, '酒馆式角色对话：用来试人物的性格和台词。'),
    select, log, input,
    h('div', { class: 'row' }, sendBtn, h('button', { class: 'btn', onclick: () => { store.chats[Number(select.value)] = []; renderLog(); } }, '清空对话')));
  el.refresh = () => {
    const chars = store.entries.filter((e) => e.kind === 'character');
    select.innerHTML = '';
    if (!chars.length) select.append(h('option', { value: '' }, '设定库里还没有人物'));
    for (const c of chars) select.append(h('option', { value: c.id }, c.name));
    const want = store.chatTarget && chars.some((c) => c.id === store.chatTarget) ? store.chatTarget : chars[0]?.id;
    if (want) select.value = String(want);
    renderLog();
  };
  return el;
}
