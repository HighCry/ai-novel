// 信息节奏：秘密台账和揭示计划（埋种子 → 给线索 → 揭开）、设定进展，以及角色知识和知情表。
// 设计见 docs/叙事逻辑架构方案.md 第二、三期。
import { api } from './api.js';
import { store, chapterLabel } from './store.js';
import { h, toast, modal, field, busy, confirmBox } from './ui.js';

export const STEP = { seed: '埋种子', clue: '给线索', reveal: '揭开' };
const GAPS = [['', '（未定）'], ['好奇', '好奇：读者知道这里有谜'], ['惊奇', '惊奇：不能让读者提前察觉'], ['悬念', '悬念：读者知道危险，结果延后']];
const PLAN_KEYS = [['seed_at', 'seed'], ['clue_at', 'clue'], ['reveal_at', 'reveal']];
const SOURCES = ['本来就知道', '亲历', '被告知', '推断'];
/** 模型给的「是否误会」可能是 true、"true"、"是" */
const isMisread = (v) => v === true || ['true', '是', '误会', 'yes', '1'].includes(String(v ?? '').trim());

const select = (options, value, onchange) => h('select', { onchange: (e) => onchange(e.target.value) },
  options.map(([v, label]) => h('option', { value: v, selected: String(v) === String(value ?? '') }, label)));

/** 章号：12、"12"、"第12章" 都认，不是正整数时为 null */
const toNum = (v) => {
  const m = /\d+/.exec(String(v ?? ''));
  const n = m ? Number(m[0]) : NaN;
  return n > 0 ? n : null;
};
const terms = (v) => (Array.isArray(v) ? v.join('、') : (v || ''));
/** 和后端 split_terms 一样按顿号、逗号、分号、斜杠、竖线或空白拆词 */
const splitTerms = (v) => [...new Set(String(v || '').split(/[,，、;；/|\s]+/).map((t) => t.trim()).filter(Boolean))];
/** 步骤名：模型可能写英文，也可能写「种子」「揭开」，和后端 normalize_step 认的写法一致 */
const stepKey = (s) => ({ seed: 'seed', 种子: 'seed', 埋种子: 'seed', clue: 'clue', 线索: 'clue', 给线索: 'clue', reveal: 'reveal', 揭开: 'reveal', 揭示: 'reveal', 揭晓: 'reveal', 揭真相: 'reveal' })[String(s ?? '').trim()];
const planText = (r) => PLAN_KEYS.filter(([k]) => r[k]).map(([k, s]) => `第${r[k]}章${STEP[s]}`).join('、');
/** 揭示计划草稿里的知情：「姜沉雪（开篇前）、陈渊（第2章，误会）」 */
const knowsText = (s) => (s.knows || []).filter((k) => k && String(k.who || '').trim()).map((k) => {
  const n = toNum(k.chapter);
  return `${String(k.who).trim()}（${n ? `第${n}章` : '开篇前'}${isMisread(k.misread) ? '，误会' : ''}）`;
}).join('、');
const entryNames = (ids) => (ids || []).map((id) => store.entries.find((e) => e.id === id)?.name).filter(Boolean).join('、');
const entryIds = (text) => [...new Set(text.split(/[,，、;；\s]+/).map((n) => n.trim()).filter(Boolean)
  .map((n) => store.entries.find((e) => e.name === n || (e.aliases || '').split(/[,，、\s]+/).includes(n))?.id)
  .filter((id) => id != null))];

// ---------------- 信息节奏面板 ----------------

export async function openReveals() {
  const bid = store.book.id;
  let data = { reveals: [], written: 0, chapters: 0, cast: [] };
  let zoom = 'all';
  let view = 'bars';
  const body = h('div', { class: 'rv-view' });
  const load = async () => {
    try { data = await api.get(`/books/${bid}/reveals`); } catch (e) { toast(e.message, 'error'); }
    draw();
  };
  const seg = (options, current, pick) => {
    const el = h('div', { class: 'seg' }, options.map(([v, label]) => h('button', {
      class: 'seg-btn' + (v === current ? ' on' : ''),
      onclick: (e) => { el.querySelectorAll('.seg-btn').forEach((b) => b.classList.remove('on')); e.currentTarget.classList.add('on'); pick(v); draw(); },
    }, label)));
    return el;
  };
  const zoomSeg = seg([['all', '全书'], ['near', '当前前后']], zoom, (v) => { zoom = v; });
  const viewSeg = seg([['bars', '时间条'], ['who', '知情表']], view, (v) => { view = v; });

  function draw() {
    body.innerHTML = '';
    const list = data.reveals;
    const written = data.written || 0;
    body.append(
      h('div', { class: 'row' },
        h('button', { class: 'btn primary', onclick: () => openRevealEditor({}, load) }, '＋ 新建秘密'),
        h('button', { class: 'btn', onclick: () => openRevealPlan(list.filter((r) => r.status !== 'dropped').length, load) }, '一键生成揭示计划'),
        h('span', { class: 'grow' }),
        list.length ? viewSeg : null,
        list.length && view === 'bars' ? zoomSeg : null));
    if (!list.length) {
      body.append(h('div', { class: 'empty' }, '还没有揭示计划。点「一键生成揭示计划」，AI 会读世界观、总纲、卷纲和已写章节，把读者暂时不该知道的真相拆成一条条秘密，排好埋种子、给线索、揭开的章节；你审过再采纳。'));
      return;
    }
    if (view === 'who') {
      drawWho(list);
      return;
    }
    body.append(h('p', { class: 'hint' }, '每条秘密一根时间条：空心圆是计划（绿：埋种子，黄：给线索，红：揭开），实心圆是定稿时记下的实际进度，竖线是现在写到的位置；拖动空心圆可以改计划章节。写某一章时，AI 只拿到本章要埋的种子、要给的线索和要揭开的真相；还没揭开的只告诉它哪些词不能写（禁区），不给真相。'));
    const planned = list.flatMap((r) => [r.seed_at, r.clue_at, r.reveal_at, ...r.events.map((e) => e.number)]).filter((n) => n);
    const [lo, hi] = zoom === 'near' ? [Math.max(1, written - 10), written + 30] : [1, Math.max(10, written, data.chapters || 0, ...planned)];
    const x = (n) => `${Math.min(100, Math.max(0, ((n - lo) / Math.max(1, hi - lo)) * 100))}%`;
    const inside = (n) => n >= lo && n <= hi;

    const plannedMark = (r, key, step, n) => {
      const mark = h('span', { class: `rv-mark plan ${step}`, style: { left: x(n) }, title: `计划第${n}章${STEP[step]}（拖动可以改）` });
      mark.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        const bar = mark.parentElement;
        const rect = bar.getBoundingClientRect();
        const tip = h('span', { class: 'rv-tip', style: { left: x(n) } }, `第${n}章`);
        bar.append(tip);
        let target = n;
        const move = (e) => {
          const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
          target = Math.max(1, Math.round(lo + ratio * (hi - lo)));
          mark.style.left = x(target);
          tip.style.left = x(target);
          tip.textContent = `第${target}章`;
        };
        const up = async () => {
          window.removeEventListener('pointermove', move);
          window.removeEventListener('pointerup', up);
          tip.remove();
          if (target === n) return;
          try {
            await api.patch(`/reveals/${r.id}`, { [key]: target });
            toast(`「${r.title}」改成第${target}章${STEP[step]}`);
          } catch (err) {
            toast(err.message, 'error');
          }
          load();
        };
        window.addEventListener('pointermove', move);
        window.addEventListener('pointerup', up);
      });
      return mark;
    };

    const row = (r) => {
      const bar = h('div', { class: 'rv-bar' });
      if (inside(written)) bar.append(h('span', { class: 'rv-now', style: { left: x(written) }, title: `写到第${written}章` }));
      for (const [key, step] of PLAN_KEYS) if (r[key] && inside(r[key])) bar.append(plannedMark(r, key, step, r[key]));
      for (const e of r.events) {
        if (e.number && inside(e.number)) bar.append(h('span', { class: `rv-mark done ${e.step}`, style: { left: x(e.number) }, title: `第${e.number}章${STEP[e.step] || ''}${e.quote ? '：' + e.quote : ''}` }));
      }
      const done = r.events.map((e) => `第${e.number ?? '?'}章${STEP[e.step] || ''}`).join('、');
      return h('div', { class: 'rv-row' + (r.status === 'dropped' ? ' dropped' : '') },
        h('div', { class: 'rv-head', title: '点击编辑', onclick: () => openRevealEditor(r, load) },
          h('b', null, r.title),
          r.gap ? h('span', { class: 'tag' }, r.gap) : null,
          h('span', { class: 'muted small' }, planText(r) || '还没排计划'),
          done ? h('span', { class: 'small' }, `· 已做：${done}`) : null),
        bar);
    };

    const groups = [
      ['进行中：埋过种子，还没揭开', (r) => r.status !== 'dropped' && r.events.length && !r.events.some((e) => e.step === 'reveal')],
      ['还没开始', (r) => r.status !== 'dropped' && !r.events.length],
      ['已经揭开', (r) => r.status !== 'dropped' && r.events.some((e) => e.step === 'reveal')],
      ['已放弃', (r) => r.status === 'dropped'],
    ];
    body.append(h('div', { class: 'rv-axis muted small' }, h('span', null, `第${lo}章`), h('span', null, `写到第${written}章`), h('span', null, `第${hi}章`)));
    for (const [label, test] of groups) {
      const items = list.filter(test);
      if (items.length) body.append(h('div', { class: 'group-title' }, `${label}（${items.length}）`), ...items.map(row));
    }
  }

  /** 知情表：每条秘密一行，主角、重要配角、反派各一列；关联到主角的秘密是底牌，排在最前面 */
  function drawWho(list) {
    const cast = data.cast || [];
    body.append(h('p', { class: 'hint' }, '每格是这个人写到现在对这条秘密知道多少：知道、误会，或者还不知道（—）。写某一章时，AI 只按这一章之前的记录写人物的言行，视角人物不知道的不会替他说破。「底牌」是关联到主角的秘密，一眼看出谁还不知道主角的底牌。点格子记一笔或修改；定稿时 AI 也会找出本章谁知道了什么，你确认后记在这里。'));
    if (!cast.length) {
      body.append(h('div', { class: 'empty' }, '设定库里还没有标成主角、重要配角、反派或常驻的人物。编辑人物设定、选好角色后，这里会列出来；在某条秘密的编辑框里给任何人物记过知情，他也会出现在这里。'));
      return;
    }
    const heroes = new Set(cast.filter((p) => p.hero).map((p) => p.id));
    const isCard = (r) => (r.entry_ids || []).some((id) => heroes.has(id));
    const active = list.filter((r) => r.status !== 'dropped');
    const rows = [...active.filter(isCard), ...active.filter((r) => !isCard(r))];
    const latest = (r, pid) => r.knows.filter((k) => k.entry_id === pid).at(-1);
    const cell = (r, p) => {
      const k = latest(r, p.id);
      const tip = k ? `${k.when}${k.note ? '：' + k.note : ''}` : '还不知道，点击记一笔';
      return h('td', { class: 'who-cell' + (k ? (k.misread ? ' misread' : ' knows') : ''), title: tip, onclick: () => editKnowledge(r, p, load) },
        k ? (k.misread ? '误会' : '知道') : '—',
        k ? h('div', { class: 'small muted' }, k.when) : null);
    };
    body.append(h('div', { class: 'who-wrap' }, h('table', { class: 'who-table' },
      h('thead', null, h('tr', null, h('th', null, '秘密'), ...cast.map((p) => h('th', { title: p.role || '' }, p.name)))),
      h('tbody', null, rows.map((r) => h('tr', null,
        h('th', { class: 'who-secret', title: '点击编辑这条秘密', onclick: () => openRevealEditor(r, load) }, isCard(r) ? h('span', { class: 'tag' }, '底牌') : null, r.title),
        ...cast.map((p) => cell(r, p))))))));
  }

  draw();
  modal({ title: '信息节奏：秘密什么时候埋、什么时候揭', wide: 'xl', body, actions: [{ label: '完成', class: 'primary', onClick: (c) => c() }] });
  load();
}

// ---------------- 角色知识 ----------------

const chapterOptions = () => [['', '开篇前就知道'], ...store.chapters.filter((c) => c.number != null).map((c) => [c.id, chapterLabel(c)])];

/** 记一条角色知识的表单：person 不给时可以选人；保存后调 onSaved */
function knowledgeForm(reveal, person, onSaved) {
  const people = store.entries.filter((e) => e.kind === 'character');
  const k = { entry_id: person?.id ?? people[0]?.id ?? '', chapter_id: store.chapter?.id ?? '', source: '亲历', misread: false, note: '', quote: '' };
  return h('div', { class: 'know-form' },
    h('div', { class: 'row' },
      person ? h('b', null, person.name) : select(people.map((e) => [e.id, e.name]), k.entry_id, (v) => { k.entry_id = v; }),
      select(chapterOptions(), k.chapter_id, (v) => { k.chapter_id = v; }),
      select(SOURCES.map((s) => [s, s]), k.source, (v) => { k.source = v; }),
      h('label', { class: 'check', title: '他以为知道了，信的却是错的说法' }, h('input', { type: 'checkbox', onchange: (e) => { k.misread = e.target.checked; } }), '误会')),
    h('div', { class: 'row' },
      h('input', { class: 'grow', placeholder: '他知道了什么；误会时写他以为的说法（可不填）', oninput: (e) => { k.note = e.target.value; } }),
      h('input', { class: 'grow', placeholder: '原文依据（可不填）', oninput: (e) => { k.quote = e.target.value; } }),
      h('button', {
        class: 'mini primary',
        onclick: async () => {
          if (!k.entry_id) return toast('设定库里还没有人物', 'warn');
          try {
            await api.post(`/books/${store.book.id}/knowledge`, { ...k, reveal_id: reveal.id, entry_id: Number(k.entry_id), chapter_id: k.chapter_id ? Number(k.chapter_id) : null });
            onSaved();
          } catch (err) { toast(err.message, 'error'); }
        },
      }, '记一笔')));
}

/** 一条知情记录：谁、知道还是误会、什么时候怎么知道的、内容；带删除 */
function knowledgeRow(k, withName, onChanged) {
  return h('div', { class: 'rv-event' },
    h('b', null, `${withName ? k.name + '：' : ''}${k.misread ? '误会' : '知道'}（${k.when}）`),
    h('span', { class: 'grow' }, k.note || k.quote || ''),
    h('button', { class: 'mini', onclick: async () => { await api.del(`/knowledge/${k.id}`); onChanged(); } }, '删除'));
}

/** 知情表里点一格：这个人对这条秘密的全部记录（按先后），可以删，也可以再记一笔 */
function editKnowledge(reveal, person, onDone) {
  const box = h('div');
  let rows = reveal.knows.filter((k) => k.entry_id === person.id);
  const refresh = async () => {
    const d = await api.get(`/books/${store.book.id}/reveals`);
    reveal.knows = d.reveals.find((x) => x.id === reveal.id)?.knows || [];
    rows = reveal.knows.filter((k) => k.entry_id === person.id);
    draw();
    onDone?.();
  };
  const draw = () => {
    box.innerHTML = '';
    box.append(
      h('p', { class: 'hint' }, '同一个人可以记好几笔，比如先误会、后来才知道真相；写某一章时用这一章之前最新的一笔。在哪一章知道的，写那一章时还算不知道，从下一章起才算知道。'),
      rows.length ? h('div', null, rows.map((k) => knowledgeRow(k, false, refresh))) : h('div', { class: 'empty' }, `还没有记录：${person.name}还不知道这件事。`),
      h('div', { class: 'field-label' }, '记一笔'),
      knowledgeForm(reveal, person, refresh));
  };
  draw();
  modal({ title: `${person.name} · 「${reveal.title}」`, wide: true, body: box, actions: [{ label: '完成', class: 'primary', onClick: (c) => c() }] });
}

// ---------------- 编辑一条秘密 ----------------

export function openRevealEditor(reveal, onDone) {
  const r = { title: '', truth: '', misread: '', gap: '', terms: '', exceptions: '', entry_ids: [], seed_at: null, clue_at: null, reveal_at: null, seed_note: '', clue_note: '', payoff: '', status: 'active', events: [], knows: [], ...reveal };
  const isNew = !r.id;
  let names = entryNames(r.entry_ids);
  const input = (key, placeholder) => h('input', { value: r[key] ?? '', placeholder, oninput: (e) => { r[key] = e.target.value; } });
  const area = (key, rows, placeholder) => h('textarea', { rows, value: r[key] ?? '', placeholder, oninput: (e) => { r[key] = e.target.value; } });
  const chapter = (key) => h('input', { type: 'number', min: 1, value: r[key] ?? '', placeholder: '第几章', oninput: (e) => { r[key] = toNum(e.target.value); } });
  const events = h('div', { class: 'rv-events' });
  const knows = h('div', { class: 'rv-events' });
  const refresh = async () => {
    const d = await api.get(`/books/${store.book.id}/reveals`);
    const fresh = d.reveals.find((x) => x.id === r.id);
    r.events = fresh?.events || [];
    r.knows = fresh?.knows || [];
    drawEvents();
    drawKnows();
    onDone?.();
  };
  const drawKnows = () => {
    knows.innerHTML = '';
    if (isNew) return;
    knows.append(h('div', { class: 'field-label' }, `谁知道（${r.knows.length}）`));
    if (!r.knows.length) knows.append(h('div', { class: 'hint' }, '还没有记录。开篇前就知道的人（秘密本来就是他的、幕后知情的人）在这里记上；之后定稿时 AI 会找出本章谁知道了什么。写某一章时，AI 按这些记录写人物知道什么、不知道什么。'));
    for (const k of r.knows) knows.append(knowledgeRow(k, true, refresh));
    knows.append(knowledgeForm(r, null, refresh));
  };
  drawKnows();
  const drawEvents = () => {
    events.innerHTML = '';
    if (isNew) return;
    events.append(h('div', { class: 'field-label' }, `实际进度（${r.events.length}）`));
    if (!r.events.length) events.append(h('div', { class: 'hint' }, '还没有记录。定稿时 AI 会找出本章对这条秘密做了哪一步，你确认后记在这里；也可以手动记。'));
    for (const e of r.events) {
      events.append(h('div', { class: 'rv-event' },
        h('b', null, `第${e.number ?? '?'}章${STEP[e.step] || ''}`),
        h('span', { class: 'grow' }, e.quote || e.note || ''),
        h('button', { class: 'mini', onclick: async () => { await api.del(`/reveal_events/${e.id}`); refresh(); } }, '删除')));
    }
    const add = { chapter_id: store.chapter?.id ?? '', step: 'clue', quote: '' };
    events.append(h('div', { class: 'row' },
      select([['', '选章节'], ...store.chapters.filter((c) => c.number != null).map((c) => [c.id, chapterLabel(c)])], add.chapter_id, (v) => { add.chapter_id = v; }),
      select(Object.entries(STEP), add.step, (v) => { add.step = v; }),
      h('input', { class: 'grow', placeholder: '原文依据（可不填）', oninput: (e) => { add.quote = e.target.value; } }),
      h('button', {
        class: 'mini primary',
        onclick: async () => {
          if (!add.chapter_id) return toast('先选章节', 'warn');
          try { await api.post(`/reveals/${r.id}/events`, { ...add, chapter_id: Number(add.chapter_id) }); refresh(); } catch (err) { toast(err.message, 'error'); }
        },
      }, '记一笔')));
  };
  drawEvents();
  const save = async (close) => {
    if (!r.title.trim()) return toast('写个标题：话题就行，不写答案', 'warn');
    const payload = { ...r, entry_ids: entryIds(names) };
    delete payload.events;
    delete payload.knows;
    try {
      if (isNew) await api.post(`/books/${store.book.id}/reveals`, payload);
      else await api.patch(`/reveals/${r.id}`, payload);
      close();
      onDone?.();
    } catch (err) {
      toast(err.message, 'error');
    }
  };
  const actions = [];
  if (!isNew) {
    actions.push({
      label: '删除',
      class: 'danger left',
      onClick: async (close) => {
        if (!(await confirmBox(`删除秘密「${r.title}」和它的进度记录？`, { okText: '删除', danger: true }))) return;
        await api.del(`/reveals/${r.id}`);
        close();
        onDone?.();
      },
    });
  }
  actions.push({ label: '取消', onClick: (c) => c() }, { label: '保存', class: 'primary', onClick: save });
  modal({
    title: isNew ? '新建秘密' : `编辑秘密 · ${r.title}`,
    wide: true,
    body: h('div', null,
      h('div', { class: 'grid3' },
        field('标题', input('title', '天轨的来历'), '写成话题，不写答案：标题会出现在写正文时的禁区里'),
        field('缺口类型', select(GAPS, r.gap, (v) => { r.gap = v; })),
        field('状态', select([['active', '在用'], ['dropped', '放弃（不再进提示词和体检）']], r.status, (v) => { r.status = v; }))),
      field('真相', area('truth', 3, '作者层：只给规划用，写正文时只在揭开的那一章给 AI')),
      field('读者揭开前会怎么想', input('misread', '可不填：读者最容易相信的表面解释')),
      h('div', { class: 'grid2' },
        field('泄露词', input('terms', '伪神、铸天者'), '揭开前正文里写出来就算说破，体检会查'),
        field('不算泄露的说法', input('exceptions', '天轨护道盟'), '含泄露词的普通称呼')),
      h('div', { class: 'grid3' },
        field('第几章埋种子', chapter('seed_at')),
        field('第几章给线索', chapter('clue_at')),
        field('第几章揭开', chapter('reveal_at'))),
      h('div', { class: 'grid2' },
        field('种子长什么样', input('seed_note', '只写读者看得到的：矿奴说天上的光每年暗一次')),
        field('线索长什么样', input('clue_note', '锁链上刻着铸造的纹路'))),
      h('div', { class: 'grid2' },
        field('揭开后改变什么', input('payoff', '人物、关系、资源、世界、对手')),
        field('关联设定', h('input', { value: names, placeholder: '设定名称，用顿号分隔', oninput: (e) => { names = e.target.value; } }), '关联到主角的秘密在知情表里算主角的底牌')),
      events,
      knows),
    actions,
  });
}

// ---------------- 一键生成揭示计划 ----------------

export function openRevealPlan(existing, onDone) {
  const o = { include_secrets: false, replace: false, instruction: '' };
  let drafts = [];
  const list = h('div', { class: 'rv-drafts' });
  const request = () => ({ task: 'reveal_plan', book_id: store.book.id, instruction: o.instruction, fields: { include_secrets: o.include_secrets } });
  /** 服务端标的 seen 里，泄露词还没被删掉的那些：揭开之前的已写章节里已经写出来了 */
  const earlyOf = (s) => (s.seen || []).filter((x) => splitTerms(s.terms).includes(x.term));
  const drawList = () => {
    list.innerHTML = '';
    if (!drafts.length) return;
    const flagged = drafts.filter((s) => earlyOf(s).length).length;
    list.append(h('p', { class: 'hint' }, `AI 拆出 ${drafts.length} 条秘密。取消勾选不要的，也可以直接改；「已写章节里做过」会记成实际进度，「知情」会记进知情表。`,
      flagged ? h('span', { class: 'warn-text' }, `其中 ${flagged} 条的泄露词在已写章节里已经出现过，看黄色提示。`) : null));
    for (const s of drafts) {
      const chapter = (key) => h('input', { type: 'number', min: 1, value: s[key] ?? '', class: 'gate-num', oninput: (e) => { s[key] = toNum(e.target.value); } });
      const done = (s.done || []).filter((d) => d && stepKey(d.step));
      const early = earlyOf(s);
      list.append(h('div', { class: 'change-item' },
        h('input', { type: 'checkbox', checked: s.checked, onchange: (e) => { s.checked = e.target.checked; } }),
        h('div', { class: 'grow' },
          h('div', { class: 'row' },
            h('input', { class: 'grow', value: s.title, oninput: (e) => { s.title = e.target.value; } }),
            select(GAPS, s.gap, (v) => { s.gap = v; })),
          h('textarea', { rows: 2, value: s.truth || '', placeholder: '真相', oninput: (e) => { s.truth = e.target.value; } }),
          h('div', { class: 'row small' },
            '埋种子', chapter('seed_at'), '给线索', chapter('clue_at'), '揭开', chapter('reveal_at'),
            h('input', { class: 'grow', value: s.terms, placeholder: '泄露词', title: '泄露词', oninput: (e) => { s.terms = e.target.value; } })),
          early.length ? h('div', { class: 'out-warn' },
            `揭开之前的已写章节里已经写出来了：${early.map((x) => `「${x.term}」第${x.chapter}章`).join('、')}。读者早就见过的名字别当泄露词：体检会误报，写正文时还会被列进禁区；是有意留的线索就留着。`,
            h('button', { class: 'mini', style: { marginLeft: '6px' }, onclick: () => { s.terms = splitTerms(s.terms).filter((t) => !early.some((x) => x.term === t)).join('、'); drawList(); } }, '去掉这些词')) : null,
          s.seed_note || s.clue_note ? h('div', { class: 'small muted' }, [s.seed_note && `种子：${s.seed_note}`, s.clue_note && `线索：${s.clue_note}`].filter(Boolean).join('　')) : null,
          done.length ? h('div', { class: 'small' }, '已写章节里做过：', done.map((d) => `第${toNum(d.chapter) ?? '?'}章${STEP[stepKey(d.step)]}`).join('、')) : null,
          knowsText(s) ? h('div', { class: 'small' }, '知情：', knowsText(s)) : null)));
    }
  };
  const gen = h('button', {
    class: 'btn primary',
    onclick: () => busy(gen, async () => {
      const r = await api.post('/ai/json', request());
      drafts = (r.secrets || []).filter((s) => s && s.title).map((s) => ({
        ...s,
        terms: terms(s.terms),
        exceptions: terms(s.exceptions),
        entries: terms(s.entries),
        seed_at: toNum(s.seed_at),
        clue_at: toNum(s.clue_at),
        reveal_at: toNum(s.reveal_at),
        checked: true,
      }));
      if (!drafts.length) toast('AI 没有拆出秘密，换个说法再试试', 'warn');
      drawList();
    }, 'AI 正在读设定和章节…').then(() => { if (drafts.length) gen.textContent = '重新生成'; }),
  }, '生成');
  const preview = h('button', { class: 'btn', onclick: async () => { const { openPreview } = await import('./dialogs.js'); openPreview(request()); } }, '查看提示词');
  modal({
    title: '一键生成揭示计划',
    wide: 'xl',
    body: h('div', null,
      h('p', { class: 'hint' }, 'AI 读世界观、总纲、卷纲（对 AI 隐藏的节不给）、设定库和已写章节的摘要，按“先写真相、再想读者会怎么误会、再定三步计划”拆出秘密。结果是草稿，你审过再采纳；采纳后写正文时 AI 会按计划拿到投放清单和禁区。'),
      field('补充要求', h('input', { placeholder: '比如：姜沉雪的身份分两次揭开；第一卷只揭开到逆命司', oninput: (e) => { o.instruction = e.target.value; } })),
      h('label', { class: 'check' }, h('input', { type: 'checkbox', onchange: (e) => { o.include_secrets = e.target.checked; } }), '把设定里的「作者底牌」也给 AI 参考（只用于这次生成；平时写作仍然不给）'),
      existing ? h('label', { class: 'check' }, h('input', { type: 'checkbox', onchange: (e) => { o.replace = e.target.checked; } }), `整份替换：先删掉现有的 ${existing} 条秘密和它们的进度（不勾时同标题的更新，其余新增）`) : null,
      h('div', { class: 'row' }, gen, preview),
      list),
    actions: [
      { label: '取消', onClick: (c) => c() },
      {
        label: '采纳选中的',
        class: 'primary',
        onClick: async (close) => {
          const chosen = drafts.filter((s) => s.checked && (s.title || '').trim());
          if (!chosen.length) return toast('没有选中的秘密', 'warn');
          if (o.replace && !(await confirmBox(`会删掉现有的 ${existing} 条秘密和它们的进度记录，换成选中的 ${chosen.length} 条。确定吗？`, { okText: '整份替换', danger: true }))) return;
          try {
            const r = await api.post(`/books/${store.book.id}/reveals/plan`, { replace: o.replace, secrets: chosen.map(({ checked, seen, ...s }) => s) });
            close();
            toast(`已采纳：新建 ${r.created}、更新 ${r.updated} 条秘密，记下已写章节里的进度 ${r.events} 步、知情 ${r.knows || 0} 条`, 'info', 4500);
            onDone?.();
          } catch (err) {
            toast(err.message, 'error');
          }
        },
      },
    ],
  });
}

// ---------------- 设定进展 ----------------

/** 设定编辑里的「设定进展」：写到第几章（第几卷）起给描述补一段或整段替换 */
export function progressionEditor(entry) {
  const box = h('div', { class: 'prog-box' });
  let list = [];
  const curNum = store.chapters.find((c) => c.id === store.chapter?.id)?.number || 1;
  const row = (p) => {
    const m = /^第(\d+)([卷章])起$/.exec(p.gate || '');
    let unit = m ? (m[2] === '卷' ? 'vol' : 'ch') : 'ch';
    let n = m ? Number(m[1]) : curNum;
    const save = async () => {
      if (!(p.text || '').trim()) return toast('写一下进展的内容', 'warn');
      const body = { gate: unit === 'vol' ? `第${n}卷起` : `第${n}章起`, mode: p.mode, text: p.text };
      try {
        Object.assign(p, p.id ? await api.patch(`/progressions/${p.id}`, body) : await api.post(`/entries/${entry.id}/progressions`, body));
        toast('设定进展已保存');
        draw();
      } catch (err) {
        toast(err.message, 'error');
      }
    };
    const remove = async () => {
      if (p.id) await api.del(`/progressions/${p.id}`);
      list = list.filter((x) => x !== p);
      draw();
    };
    return h('div', { class: 'prog-row' },
      select([['ch', '从第 N 章起'], ['vol', '从第 N 卷起']], unit, (v) => { unit = v; }),
      h('input', { type: 'number', min: 1, value: n, class: 'gate-num', oninput: (e) => { n = Math.max(1, Number(e.target.value) || 1); } }),
      select([['add', '补充'], ['replace', '替换']], p.mode, (v) => { p.mode = v; }),
      h('input', { class: 'grow', value: p.text, placeholder: '比如：她其实是逆命司的暗桩', oninput: (e) => { p.text = e.target.value; } }),
      h('button', { class: 'mini primary', onclick: save }, p.id ? '保存' : '添加'),
      h('button', { class: 'mini', onclick: remove }, '删除'));
  };
  const draw = () => {
    box.innerHTML = '';
    box.append(
      h('div', { class: 'field-label' }, '设定进展'),
      h('p', { class: 'hint' }, '描述里只写读者一开始就能知道的；后面才揭开的写成进展，写到那一章（或那一卷）起 AI 才看得到。「补充」接在描述后面，「替换」从那里起整段换掉。'),
      ...list.map(row),
      h('button', { class: 'mini', onclick: () => { list.push({ gate: '', mode: 'add', text: '' }); draw(); } }, '＋ 加一条进展'));
  };
  draw();
  api.get(`/entries/${entry.id}/progressions`).then((ps) => { list = ps; draw(); }).catch(() => {});
  return box;
}

// ---------------- 定稿时确认揭示进度 ----------------

/** 定稿确认框里的「信息揭示」：AI 找出的本章揭示进度默认勾上，计划外的新秘密默认不勾 */
export function revealReview(ext, plan) {
  const steps = (ext.reveals || []).map((s) => ({ ...s, step: stepKey(s?.step), checked: true })).filter((s) => s.step && plan.some((r) => r.id === s.id));
  const fresh = (ext.reveals_new || []).filter((s) => (s.title || '').trim() && !plan.some((r) => r.title === s.title)).map((s) => ({ ...s, checked: false }));
  const check = (item) => h('input', { type: 'checkbox', checked: item.checked, onchange: (e) => { item.checked = e.target.checked; } });
  const el = steps.length || fresh.length ? h('div', null,
    h('h4', null, `信息揭示（${steps.length}${fresh.length ? `，计划外 ${fresh.length}` : ''}）`),
    steps.map((s) => h('div', { class: 'change-item' }, check(s),
      h('div', { class: 'grow' },
        h('div', { class: 'entry-head' }, h('span', { class: 'badge' + (s.step === 'reveal' ? ' green' : '') }, STEP[s.step]), h('b', null, plan.find((r) => r.id === s.id)?.title)),
        s.quote ? h('blockquote', null, s.quote) : null,
        s.note ? h('div', { class: 'small muted' }, s.note) : null))),
    fresh.map((s) => h('div', { class: 'change-item' }, check(s),
      h('div', { class: 'grow' },
        h('div', { class: 'entry-head' }, h('span', { class: 'badge gray' }, '计划外的新秘密'), h('span', { class: 'hint' }, '确实要藏到后面才揭开的真相再勾上')),
        h('input', { value: s.title, oninput: (e) => { s.title = e.target.value; } }),
        h('textarea', { rows: 2, value: s.truth || '', placeholder: '真相', oninput: (e) => { s.truth = e.target.value; } }),
        s.quote ? h('blockquote', null, s.quote) : null)))) : null;
  return { el, picked: () => ({ reveals: steps.filter((s) => s.checked), reveals_new: fresh.filter((s) => s.checked) }) };
}

// ---------------- 逻辑审校结果 ----------------

const LOGIC_TIP = { 超前揭示: '还没到揭开的时候被写破', 角色越知: '人物知道了他这时还不知道的事', 视角越权: '旁白替视角人物说出了他不知道的', 因果缺口: '行动依赖还没交代的信息或资源', 状态矛盾: '和设定、前情里的状态对不上' };
const severity = (s) => (s === 'high' ? 'high' : s === 'low' ? 'low' : 'medium');

/** 逻辑审校的问题列表；给了 locate、replace 时可以点原文定位，或者一键把问题句换成改好的句子 */
export function logicIssues(r, { stale = false, locate, replace } = {}) {
  const issues = r?.issues || [];
  const box = h('div', { class: 'logic-out' },
    h('div', { class: 'score-line' }, h('b', null, issues.length ? `逻辑审校发现 ${issues.length} 处问题` : '逻辑审校没有发现信息越界')),
    stale ? h('p', { class: 'hint warn-text' }, '这是正文改动之前的结果，可能已经过时；改完可以再审一次。') : null);
  for (const it of issues) {
    const quote = String(it.quote || '').trim();
    const why = [it.time, it.presence, it.source].map((x) => String(x || '').trim()).filter(Boolean);
    const canReplace = replace && quote && it.rewrite && it.found !== false;
    box.append(h('div', { class: 'issue ' + severity(it.severity) + (locate && quote ? ' clickable' : ''), title: locate && quote ? '点击定位到正文' : '', onclick: () => locate && quote && locate(quote) },
      h('div', { class: 'issue-head' },
        h('span', { class: 'tag', title: LOGIC_TIP[it.type] || '' }, it.type || '问题'),
        it.who ? h('span', { class: 'small muted' }, it.who) : null,
        it.secret ? h('span', { class: 'small muted' }, `「${it.secret}」`) : null),
      quote ? h('blockquote', null, quote) : null,
      it.found === false ? h('div', { class: 'small warn-text' }, '正文里没找到这句原文，可能是模型改了字，核对后再改。') : null,
      it.problem ? h('div', { class: 'small' }, it.problem) : null,
      why.length ? h('div', { class: 'small muted' }, why.join('；')) : null,
      it.fix ? h('div', { class: 'small muted' }, '改法：' + it.fix) : null,
      it.rewrite ? h('div', { class: 'small logic-rewrite' }, '改成：', h('span', null, it.rewrite),
        canReplace ? h('button', { class: 'mini primary', onclick: (e) => { e.stopPropagation(); replace(quote, it.rewrite); } }, '替换这句') : null) : null));
  }
  return box;
}

/** 定稿确认框里的「人物知情」：AI 找出的本章谁知道（或误会）了哪个秘密，对得上计划和设定库的默认勾上；
 *  正文里人物越界知道的事提取出来也像事实，所以逻辑审校判成「角色越知」的同一人同一秘密默认不勾。flag 用新的审校结果再判一次 */
export function knowledgeReview(ext, plan, logic) {
  const person = (name) => store.entries.find((e) => e.name === name || (e.aliases || '').split(/[,，、\s]+/).includes(name));
  const items = (ext.knowledge || [])
    .filter((k) => k && plan.some((r) => r.id === k.id) && person(String(k.who || '').trim()))
    .map((k) => ({ ...k, who: String(k.who).trim(), misread: isMisread(k.misread), title: plan.find((r) => r.id === k.id)?.title || '', checked: true }));
  const flaggedBy = (k, issues) => issues.find((i) => /角色越知/.test(i.type || '') && String(i.who || '').includes(k.who)
    && (!i.secret || String(i.secret).includes(k.title) || k.title.includes(String(i.secret))));
  const el = items.length ? h('div') : null;
  const draw = () => {
    el.innerHTML = '';
    el.append(h('h4', null, `人物知情（${items.length}）`), ...items.map((k) => h('div', { class: 'change-item' },
      h('input', { type: 'checkbox', checked: k.checked, onchange: (e) => { k.checked = e.target.checked; } }),
      h('div', { class: 'grow' },
        h('div', { class: 'entry-head' },
          h('span', { class: 'badge' + (k.misread ? ' yellow' : ' green') }, k.misread ? '误会' : '知道了'),
          h('b', null, k.who), h('span', { class: 'muted' }, '·'), h('b', null, k.title),
          k.source ? h('span', { class: 'muted small' }, k.source) : null),
        k.issue ? h('div', { class: 'small warn-text' }, `逻辑审校认为这是越界：${String(k.issue.problem || '他没有获知来源').trim().replace(/[。.！!；;]+$/, '')}。默认不记，确认他确实有来源再勾上。`) : null,
        k.note ? h('div', { class: 'small' }, k.note) : null,
        k.quote ? h('blockquote', null, k.quote) : null))));
  };
  const flag = (r) => {
    if (!el) return;
    for (const k of items) {
      k.issue = flaggedBy(k, r?.issues || []);
      k.checked = !k.issue;
    }
    draw();
  };
  if (el) flag(logic);
  return { el, flag, picked: () => items.filter((k) => k.checked).map(({ checked, issue, title, ...k }) => k) };
}
