import { api } from './api.js';
import { store } from './store.js';
import { h, toast, modal, field, confirmBox } from './ui.js';

const num = (v) => (v === '' || v == null ? null : Number(v));

/** 「第3天傍晚（故事第 3 天）」 */
function describe(time, day) {
  const t = (time || '').trim();
  if (day == null) return t;
  return t ? `${t}（故事第 ${day} 天）` : `故事第 ${day} 天`;
}

/** 时间线：每章结束时的故事时间、关键事件，以及正文里定下的时限 */
export async function openTimeline() {
  const m = modal({ title: '时间线', wide: 'xl', body: h('div', { class: 'boot' }, '加载中…'), actions: [{ label: '关闭', onClick: (c) => c() }] });
  let data = { chapters: [], events: [], today: null };
  const label = (id) => data.chapters.find((c) => c.id === id)?.label || '';

  async function load() {
    try {
      data = await api.get(`/books/${store.book.id}/timeline`);
    } catch (e) {
      m.body.replaceChildren(h('div', { class: 'empty' }, '出错了：' + e.message));
      return;
    }
    draw();
  }

  async function saveChapter(c, patch) {
    try {
      await api.patch(`/chapters/${c.id}`, patch);
      Object.assign(c, patch);
      if (store.chapter?.id === c.id) Object.assign(store.chapter, patch);
      data.today = [...data.chapters].reverse().find((x) => x.story_day != null)?.story_day ?? null;
      drawHead();
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  async function setDone(e, done) {
    const last = data.chapters.filter((c) => c.words > 0).pop();
    try {
      await api.patch(`/events/${e.id}`, done ? { status: 'done', done_chapter_id: store.chapter?.id ?? last?.id ?? null } : { status: 'open', done_chapter_id: null });
      await load();
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  function editEvent(ev) {
    const e = { kind: 'event', title: '', who: '', story_time: '', day: null, detail: '', status: 'open', chapter_id: store.chapter?.id ?? null, done_chapter_id: null, ...ev };
    const isNew = !e.id;
    const deadline = e.kind === 'deadline';
    const chapterSelect = (value, onchange) => h('select', { onchange: (x) => onchange(num(x.target.value)) },
      [['', '（不关联）'], ...data.chapters.map((c) => [c.id, c.label])].map(([v, t]) => h('option', { value: v, selected: String(v) === String(value ?? '') }, t)));
    const save = async (close) => {
      if (!e.title.trim()) return toast('内容不能为空', 'warn');
      try {
        if (isNew) await api.post(`/books/${store.book.id}/events`, e);
        else await api.patch(`/events/${e.id}`, e);
        close();
        await load();
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
          if (!(await confirmBox(`删除「${e.title}」？`, { okText: '删除', danger: true }))) return;
          await api.del(`/events/${e.id}`);
          close();
          await load();
        },
      });
    }
    actions.push({ label: '取消', onClick: (c) => c() }, { label: '保存', class: 'primary', onClick: save });
    modal({
      title: `${isNew ? '添加' : '编辑'}${deadline ? '时限' : '事件'}`,
      body: h('div', null,
        field(deadline ? '时限' : '事件', h('input', { value: e.title, placeholder: deadline ? '如：三日内交出一百斤灵铁' : '一句话写清发生了什么', oninput: (x) => { e.title = x.target.value; } })),
        h('div', { class: 'grid2' },
          field('相关人物', h('input', { value: e.who, placeholder: '顿号分隔', oninput: (x) => { e.who = x.target.value; } })),
          field(deadline ? '定于哪一章' : '发生在哪一章', chapterSelect(e.chapter_id, (v) => { e.chapter_id = v; }))),
        h('div', { class: 'grid2' },
          field(deadline ? '到期时间' : '发生时间', h('input', { value: e.story_time, placeholder: deadline ? '如：第6天、宗门大比当天' : '如：第3天夜里', oninput: (x) => { e.story_time = x.target.value; } })),
          field('故事第几天', h('input', { type: 'number', min: 1, value: e.day ?? '', oninput: (x) => { e.day = num(x.target.value); } }), deadline ? '填了才能算还剩几天、有没有过期' : '')),
        deadline
          ? h('div', { class: 'grid2' },
            field('状态', h('select', { onchange: (x) => { e.status = x.target.value; } }, [['open', '未了结'], ['done', '已了结']].map(([v, t]) => h('option', { value: v, selected: e.status === v }, t)))),
            field('在哪一章了结', chapterSelect(e.done_chapter_id, (v) => { e.done_chapter_id = v; })))
          : null,
        field('备注', h('textarea', { rows: 2, value: e.detail, oninput: (x) => { e.detail = x.target.value; } }))),
      actions,
    });
  }

  const head = h('div', { class: 'tl-head' });
  function drawHead() {
    const open = data.events.filter((e) => e.kind === 'deadline' && e.status !== 'done');
    const overdue = data.today == null ? [] : open.filter((e) => e.day != null && e.day < data.today);
    const now = [...data.chapters].reverse().find((c) => c.story_day != null || (c.story_time || '').trim());
    head.replaceChildren(...[
      h('span', null, now ? `写到${now.label}：${describe(now.story_time, now.story_day)}` : '还没有记录故事时间。定稿时 AI 会顺带记下每章结束时是故事第几天，也可以在下面手动填。'),
      open.length ? h('span', { class: overdue.length ? 'warn-text' : 'muted' }, ` · 未了结的时限 ${open.length} 条${overdue.length ? `，已过期 ${overdue.length} 条` : ''}`) : null,
    ].filter(Boolean));
  }

  function deadlineRow(e) {
    const done = e.status === 'done';
    let badge = '到期日未定';
    let cls = 'badge';
    if (done) badge = `已了结${e.done_chapter_id ? ' · ' + label(e.done_chapter_id) : ''}`;
    else if (e.day != null && data.today != null) {
      const left = e.day - data.today;
      badge = left < 0 ? `已过期 ${-left} 天` : left === 0 ? '今天到期' : `还剩 ${left} 天`;
      cls = left < 0 ? 'badge red' : left <= 1 ? 'badge yellow' : 'badge green';
    } else if (e.day != null) badge = `第 ${e.day} 天到期`;
    return h('div', { class: 'tl-deadline' + (done ? ' done' : '') },
      h('span', { class: cls }, badge),
      h('div', { class: 'grow' },
        h('b', null, e.title),
        h('div', { class: 'muted small' }, [e.who && `相关：${e.who}`, describe(e.story_time, e.day) && `到期：${describe(e.story_time, e.day)}`, e.chapter_id && `定于${label(e.chapter_id)}`].filter(Boolean).join(' · '))),
      h('button', { class: 'mini', onclick: () => setDone(e, !done) }, done ? '改回未了结' : '了结'),
      h('button', { class: 'mini', onclick: () => editEvent(e) }, '改'));
  }

  function chapterRow(c) {
    const events = data.events.filter((e) => e.kind === 'event' && e.chapter_id === c.id);
    const setDeadlines = data.events.filter((e) => e.kind === 'deadline' && e.chapter_id === c.id);
    return h('tr', null,
      h('td', { class: 'tl-ch' }, c.label),
      h('td', null, h('input', { value: c.story_time || '', placeholder: '如：第3天傍晚', onchange: (x) => saveChapter(c, { story_time: x.target.value.trim() }) })),
      h('td', null, h('input', { type: 'number', min: 1, class: 'num', value: c.story_day ?? '', onchange: (x) => saveChapter(c, { story_day: num(x.target.value) }) })),
      h('td', null,
        events.map((e) => h('button', { class: 'chip', title: [e.who && `相关：${e.who}`, e.story_time, e.detail].filter(Boolean).join('\n'), onclick: () => editEvent(e) }, e.title)),
        setDeadlines.map((e) => h('button', { class: 'chip tl-dl', title: '本章定下的时限', onclick: () => editEvent(e) }, '⏳ ' + e.title)),
        h('button', { class: 'chip add', onclick: () => editEvent({ kind: 'event', chapter_id: c.id, day: c.story_day }) }, '＋')));
  }

  function draw() {
    drawHead();
    const deadlines = data.events.filter((e) => e.kind === 'deadline').sort((a, b) => (a.status === 'done') - (b.status === 'done') || (a.day ?? 1e9) - (b.day ?? 1e9));
    m.body.replaceChildren(h('div', { class: 'timeline' },
      head,
      h('div', { class: 'row' }, h('h4', null, '时限'), h('span', { class: 'spacer' }), h('button', { class: 'btn', onclick: () => editEvent({ kind: 'deadline' }) }, '＋ 时限')),
      deadlines.length ? deadlines.map(deadlineRow) : h('div', { class: 'empty' }, '正文里定下的期限、约定、倒计时会列在这里：到期还没交代，体检会提醒。'),
      h('h4', null, '各章时间和事件'),
      data.chapters.length
        ? h('table', { class: 'table tl-table' },
          h('tr', null, h('th', null, '章节'), h('th', null, '结束时'), h('th', null, '第几天'), h('th', null, '关键事件')),
          data.chapters.map(chapterRow))
        : h('div', { class: 'empty' }, '还没有章节'),
      h('p', { class: 'hint' }, '「第几天」从故事开始算，第一章开始那天是第 1 天；回忆、插叙按主线的时间填。写作时 AI 会看到上一章结束时的时间和还没了结的时限。')));
  }

  load();
}
