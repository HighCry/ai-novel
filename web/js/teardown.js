import { api } from './api.js';
import { store, modelReady } from './store.js';
import { h, toast, modal, confirmBox, pickFile, readFile, fmtWords } from './ui.js';

const SVG = 'http://www.w3.org/2000/svg';
const svg = (tag, attrs = {}, ...kids) => {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  el.append(...kids);
  return el;
};
const pct = (v) => (v == null ? '—' : `${Math.round(v * 100)}%`);
const fixed = (v, d = 1) => (v == null ? '—' : Number(v).toFixed(d));

/** 张力曲线：每条线一本书，按章节序号对齐 */
function curveChart(series, n) {
  const W = 680;
  const H = 170;
  const pad = 26;
  const x = (i) => pad + (n <= 1 ? 0 : ((i - 1) * (W - pad * 2)) / (n - 1));
  const y = (t) => H - pad - (t / 10) * (H - pad * 2);
  const root = svg('svg', { viewBox: `0 0 ${W} ${H}`, class: 'td-chart' });
  for (const t of [0, 5, 10]) {
    root.append(svg('line', { x1: pad, x2: W - pad, y1: y(t), y2: y(t), class: 'td-grid' }), svg('text', { x: 4, y: y(t) + 4, class: 'td-axis' }, String(t)));
  }
  for (const s of series) {
    let seg = [];
    const flush = () => {
      if (seg.length > 1) root.append(svg('polyline', { points: seg.join(' '), class: 'td-line', style: `stroke: ${s.color}` }));
      seg = [];
    };
    for (const p of s.points) {
      if (p.tension == null) { flush(); continue; }
      seg.push(`${x(p.n)},${y(p.tension)}`);
      root.append(svg('circle', { cx: x(p.n), cy: y(p.tension), r: 3, style: `fill: ${s.color}` }, svg('title', {}, `${s.label} 第${p.n}章：张力 ${p.tension}${p.hook && p.hook !== '无' ? ' · 钩子：' + p.hook : ''}`)));
    }
    flush();
  }
  return root;
}

function counts(map) {
  const list = Object.entries(map || {}).sort((a, b) => b[1] - a[1]);
  return list.length ? list.map(([k, v]) => `${k} ${v}`).join('、') : '—';
}

/** 拆书 / 对标爆款：导入对标作品，AI 拆解前 N 章的结构，和自己的书对比。只学结构，原文不进写作提示词 */
export async function openTeardown() {
  const m = modal({ title: '拆书对标', wide: 'xl', body: h('div', { class: 'boot' }, '加载中…'), actions: [{ label: '关闭', onClick: (c) => c() }] });
  let books = [];
  let current = null;
  let detail = null;
  let compare = null;
  let running = null;
  let limit = Number(localStorage.getItem('teardownLimit')) || 30;

  async function loadList(selectId) {
    try {
      books = await api.get('/refbooks');
    } catch (e) {
      m.body.replaceChildren(h('div', { class: 'empty' }, '出错了：' + e.message));
      return;
    }
    current = books.find((b) => b.id === selectId) || books.find((b) => b.id === current?.id) || books[0] || null;
    await loadBook();
  }

  async function loadBook() {
    detail = null;
    compare = null;
    if (current) {
      try {
        [detail, compare] = await Promise.all([
          api.get(`/refbooks/${current.id}`),
          api.get(`/refbooks/${current.id}/compare?${new URLSearchParams({ limit, ...(store.book ? { book_id: store.book.id } : {}) })}`),
        ]);
        current = detail.book;
      } catch (e) {
        toast(e.message, 'error');
      }
    }
    draw();
  }

  async function importBook() {
    const file = await pickFile('.txt,.epub,.docx');
    if (!file) return;
    toast(`正在导入《${file.name}》…`);
    try {
      const b = await api.post('/refbooks/import', { filename: file.name, data: await readFile(file), genre: store.book?.genre || '' });
      toast(`已导入《${b.title}》：${b.chapters} 章，${fmtWords(b.words)} 字`);
      await loadList(b.id);
    } catch (e) {
      toast(e.message, 'error', 5000);
    }
  }

  async function runAnalysis() {
    if (!modelReady()) return toast('请先在设置里配置模型', 'warn');
    const todo = detail.chapters.slice(0, limit).filter((c) => !c.analyzed);
    if (!todo.length) return toast(`前 ${limit} 章都拆解过了`);
    const chars = todo.reduce((a, c) => a + Math.min(c.words, 9000), 0);
    if (!(await confirmBox(`用 AI 拆解 ${todo.length} 章？大约发送 ${fmtWords(chars)} 字给分析模型，一章一章来，可以随时停止。`, { okText: '开始拆解' }))) return;
    running = { done: 0, total: todo.length, stop: false };
    draw();
    // 接口偶尔抽风（502、超时），每章失败后隔几秒重试一次，还不行再停
    const analyze = (c) => api.post(`/refchapters/${c.id}/analyze`, {});
    for (const c of todo) {
      if (running.stop) break;
      try {
        c.analysis = await analyze(c).catch(() => new Promise((r) => setTimeout(r, 3000)).then(() => analyze(c)));
        c.analyzed = true;
      } catch (e) {
        toast(`${c.label}：${e.message}`, 'error', 6000);
        break;
      }
      running.done += 1;
      draw();
    }
    running = null;
    await loadBook();
  }

  function compareCards() {
    if (!compare) return null;
    const r = compare.ref;
    const mine = compare.mine;
    const card = (label, a, b) => h('div', { class: 'stat big' },
      h('div', { class: 'stat-label' }, label),
      h('div', { class: 'td-pair' }, h('span', { class: 'td-ref' }, a), mine ? h('span', { class: 'td-mine' }, b) : null));
    return h('div', null,
      h('div', { class: 'row small' },
        h('span', { class: 'td-ref' }, `■ 《${current.title}》前 ${r.chapters} 章`),
        mine ? h('span', { class: 'td-mine' }, `■ 《${compare.book_title}》前 ${mine.chapters} 章`) : h('span', { class: 'muted' }, '（在作品里打开时会和你的书对比）')),
      h('div', { class: 'stats-grid five' },
        card('平均章长', fmtWords(Math.round(r.avg_words)), mine && fmtWords(Math.round(mine.avg_words))),
        card('对话占比', pct(r.dialogue), mine && pct(mine.dialogue)),
        card('平均张力', fixed(r.avg_tension), mine && fixed(mine.avg_tension)),
        card('每章爽点', fixed(r.cool_per_chapter), mine && fixed(mine.cool_per_chapter)),
        card('章末钩子', pct(r.hook_rate), mine && pct(mine.hook_rate))),
      compare.notes.length ? h('ul', { class: 'td-notes' }, compare.notes.map((n) => h('li', null, n))) : null,
      r.analyzed || mine?.analyzed
        ? h('div', null,
          h('h4', null, '张力曲线'),
          curveChart([
            { label: '对标', color: 'var(--accent)', points: r.curve },
            ...(mine ? [{ label: '我的', color: 'var(--green)', points: mine.curve }] : []),
          ], Math.max(r.curve.length, mine?.curve.length || 0)),
          h('div', { class: 'small' }, '钩子类型：对标 ', counts(r.hooks), mine ? h('span', null, '；我的 ', counts(mine.hooks)) : null),
          h('div', { class: 'small' }, '爽点类型：对标 ', counts(r.cool_types), mine ? h('span', null, '；我的 ', counts(mine.cool_types)) : null))
        : null);
  }

  function chapterTable() {
    const rows = detail.chapters.slice(0, Math.max(limit, detail.chapters.filter((c) => c.analyzed).length));
    return h('table', { class: 'table td-table' },
      h('tr', null, ['章节', '字数', '对话', '开头', '张力', '爽点', '章末钩子', '概要和手法'].map((t) => h('th', null, t))),
      rows.map((c) => {
        const a = c.analysis || {};
        const cools = (a.cool_points || []).map((p) => `${p.type}${p.level ? '（' + p.level + '）' : ''}`).join('、');
        return h('tr', null,
          h('td', { class: 'nowrap' }, c.label),
          h('td', null, c.words),
          h('td', null, pct(c.stats?.dialogue)),
          h('td', { class: 'small' }, a.opening || ''),
          h('td', null, a.tension ?? (c.analyzed ? '—' : '')),
          h('td', { class: 'small' }, cools),
          h('td', { class: 'small' }, a.hook ? `${a.hook.type || ''}${a.hook.desc ? '：' + a.hook.desc : ''}` : ''),
          h('td', { class: 'small' },
            a.summary || (c.analyzed ? '' : h('span', { class: 'muted' }, '未拆解')),
            (a.techniques || []).length ? h('ul', { class: 'td-tech' }, a.techniques.map((t) => h('li', null, t))) : null,
            (a.new_elements || []).length ? h('div', { class: 'muted' }, '新登场：' + a.new_elements.join('、')) : null));
      }));
  }

  function draw() {
    const picker = h('select', { onchange: (e) => { current = books.find((b) => b.id === Number(e.target.value)); loadBook(); } },
      books.map((b) => h('option', { value: b.id, selected: b.id === current?.id }, `${b.title}（${b.chapters} 章）`)));
    const top = h('div', { class: 'row' },
      books.length ? picker : null,
      h('button', { class: 'btn', onclick: importBook }, '导入对标书'),
      current ? h('button', {
        class: 'btn danger',
        onclick: async () => {
          if (!(await confirmBox(`删除对标作品《${current.title}》和它的拆解结果？`, { okText: '删除', danger: true }))) return;
          await api.del(`/refbooks/${current.id}`);
          current = null;
          await loadList();
        },
      }, '删除') : null,
      h('span', { class: 'spacer' }),
      h('span', { class: 'muted small' }, '只学结构：对标书的原文只用来分析，不会进写作提示词。'));
    if (!current || !detail) {
      m.body.replaceChildren(h('div', { class: 'teardown' }, top, h('div', { class: 'empty' },
        '导入一本同类型的爆款（txt、epub、docx，按「第X章」自动拆章），AI 拆解前几十章的开头、张力、爽点和章末钩子，再和你的书并排对比，看节奏差在哪里。')));
      return;
    }
    const limitInput = h('input', { type: 'number', min: 1, max: 100, value: limit, class: 'num', onchange: (e) => { limit = Math.min(100, Math.max(1, Number(e.target.value) || 30)); localStorage.setItem('teardownLimit', limit); loadBook(); } });
    const progress = running
      ? h('span', null, `正在拆解 ${running.done}/${running.total}…`, h('button', { class: 'mini', onclick: () => { running.stop = true; } }, '停止'))
      : h('button', { class: 'btn primary', onclick: runAnalysis }, `AI 拆解前 ${limit} 章`);
    m.body.replaceChildren(h('div', { class: 'teardown' },
      top,
      h('div', { class: 'row' },
        h('b', null, `《${current.title}》`),
        h('span', { class: 'muted' }, `${current.chapters} 章 · ${fmtWords(current.words)} 字 · 已拆解 ${current.analyzed} 章`),
        h('span', { class: 'spacer' }),
        h('label', { class: 'inline' }, '对比前 ', limitInput, ' 章'),
        progress),
      compareCards(),
      h('h4', null, '逐章拆解'),
      chapterTable()));
  }

  loadList();
}
