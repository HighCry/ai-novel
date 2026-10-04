// 文风库：收藏范文 → AI 分析写法 → 提炼文风指南；写作时自动检索范文、带上指南。

import { api } from './api.js';
import { store, modelReady as ready } from './store.js';
import { h, toast, modal, field, busy, confirmBox, fmtTime, fmtWords, readFile, pickFile } from './ui.js';

export const STYLE_TAGS = ['开篇', '章末钩子', '对话', '打斗', '环境', '心理', '感情', '爽点', '悬念', '日常', '转场', '人物描写', '搞笑', '群像'];

function tagPicker(selected) {
  const set = new Set(selected || []);
  const el = h('div', { class: 'chips' });
  const draw = () => {
    el.innerHTML = '';
    STYLE_TAGS.forEach((t) => el.append(h('button', { class: 'chip' + (set.has(t) ? ' on' : ''), onclick: (e) => { e.preventDefault(); set.has(t) ? set.delete(t) : set.add(t); draw(); } }, t)));
  };
  draw();
  el.value = () => [...set];
  return el;
}

const tagList = (tags) => (tags || []).map((t) => h('span', { class: 'tag accent' }, t));

/** 把一段文字收藏进文风库（编辑器选区工具条、粘贴范文都用它） */
export function openSaveToLibrary({ content = '', book_id = null, source = '', title = '', genre = '' } = {}, onSaved) {
  const text = h('textarea', { rows: 8, value: content, placeholder: '粘贴你觉得写得好的段落：自己的得意之作、喜欢的作者的片段都可以。' });
  const titleIn = h('input', { value: title, placeholder: '标题（可留空，自动取开头）' });
  const sourceIn = h('input', { value: source, placeholder: '出处或作者' });
  const genreIn = h('input', { value: genre, placeholder: '题材，如 玄幻、都市' });
  const tags = tagPicker([]);
  const note = h('textarea', { rows: 2, placeholder: '为什么觉得它好？比如“打斗干脆”“对话有火药味”。AI 分析时会参考。' });
  const analyze = h('input', { type: 'checkbox', checked: ready() });
  modal({
    title: '收藏到文风库',
    wide: true,
    body: h('div', null,
      field('范文', text, book_id ? '来自本书的片段不会被当成照抄' : '只用来学写法，AI 写作时不会照抄原句'),
      h('div', { class: 'grid3' }, field('标题', titleIn), field('出处', sourceIn), field('题材', genreIn)),
      field('场景标签', tags, '写作时会优先参考同类场景的范文'),
      field('收藏理由', note),
      h('label', { class: 'check' }, analyze, '保存后让 AI 分析它好在哪里（会用分析模型）')),
    actions: [
      { label: '取消', onClick: (c) => c() },
      {
        label: '收藏',
        class: 'primary',
        onClick: async (close) => {
          try {
            const item = await api.post('/library', { content: text.value, title: titleIn.value, source: sourceIn.value, genre: genreIn.value, tags: tags.value(), note: note.value, book_id });
            close();
            toast('已收藏到文风库');
            onSaved?.(item);
            if (analyze.checked) {
              api.post(`/library/${item.id}/analyze`, {})
                .then((it) => { toast(`《${it.title}》分析完成`); onSaved?.(it); })
                .catch((e) => toast('分析失败：' + e.message, 'error', 5000));
            }
          } catch (e) {
            toast(e.message, 'error');
          }
        },
      },
    ],
  });
}

// ---------------- 文风库主界面 ----------------

export function openLibrary(tab = 'items') {
  const body = h('div', { class: 'lib' });
  const tabs = h('div', { class: 'tabs lib-tabs' });
  const pane = h('div', { class: 'lib-pane' });
  body.append(tabs, pane);
  const TABS = [['items', '范文'], ['guide', '文风指南'], ['learn', '学习记录'], ['search', '检索测试']];
  let current = tab;
  const show = (id) => {
    current = id;
    tabs.innerHTML = '';
    TABS.forEach(([k, label]) => tabs.append(h('button', { class: 'tab' + (k === id ? ' active' : ''), onclick: () => show(k) }, label)));
    pane.innerHTML = '';
    ({ items: itemsTab, guide: guideTab, learn: learnTab, search: searchTab })[id](pane, show);
  };
  modal({ title: '文风库', body, wide: 'xl', actions: [{ label: '关闭', onClick: (c) => c() }] });
  show(current);
}

function itemsTab(pane) {
  let all = [];
  let query = '';
  let tag = '';
  const list = h('div', { class: 'lib-list' });
  const head = h('div', { class: 'lib-summary muted small' });
  const search = h('input', { placeholder: '搜索标题、出处、内容', oninput: (e) => { query = e.target.value.trim(); draw(); } });
  const tagSel = h('select', { onchange: (e) => { tag = e.target.value; draw(); } }, h('option', { value: '' }, '全部标签'), STYLE_TAGS.map((t) => h('option', { value: t }, t)));
  const analyzeAll = h('button', { class: 'btn', onclick: () => runAnalyzeAll() }, '分析全部');
  const embedBtn = h('button', { class: 'btn', hidden: !store.settings?.embedding?.model, onclick: () => runEmbed() }, '生成向量');

  async function load() {
    all = await api.get('/library');
    draw();
  }

  function draw() {
    const items = all.filter((i) => (!tag || i.tags.includes(tag)) && (!query || [i.title, i.source, i.genre, i.content, i.note].join(' ').includes(query)));
    const pending = all.filter((i) => !i.analysis).length;
    analyzeAll.textContent = pending ? `分析全部（${pending} 篇未分析）` : '全部已分析';
    analyzeAll.disabled = !pending;
    head.textContent = all.length ? `共 ${all.length} 篇 · ${fmtWords(all.reduce((a, i) => a + i.word_count, 0))} 字 · 写作时按场景自动检索，停用的范文不参与` : '';
    list.innerHTML = '';
    if (!all.length) {
      list.append(h('div', { class: 'empty' },
        h('p', null, '文风库还是空的。'),
        h('p', null, '把你觉得写得好的段落收藏进来：自己的得意章节、喜欢的作者的片段都行。AI 会分析每篇好在哪里，写作时按场景挑几段当参考，再把这些心得提炼成你的「文风指南」。'),
        h('p', null, '在编辑器里选中一段文字，点浮出来的「收藏到文风库」也可以。')));
      return;
    }
    if (!items.length) list.append(h('div', { class: 'empty' }, '没有匹配的范文'));
    for (const i of items) {
      const enabled = h('input', {
        type: 'checkbox',
        checked: i.enabled,
        title: '停用后写作时不参考它，也不参与照抄检测',
        onclick: (e) => e.stopPropagation(),
        onchange: async (e) => { await api.patch(`/library/${i.id}`, { enabled: e.target.checked }).catch((err) => toast(err.message, 'error')); i.enabled = e.target.checked; row.classList.toggle('off', !i.enabled); },
      });
      const row = h('div', { class: 'lib-row' + (i.enabled ? '' : ' off'), onclick: () => detail(i.id) },
        enabled,
        h('div', { class: 'grow' },
          h('div', { class: 'lib-title' }, h('b', null, i.title), i.source ? h('span', { class: 'muted small' }, ' · ' + i.source) : null, i.genre ? h('span', { class: 'tag' }, i.genre) : null, ...tagList(i.tags)),
          h('div', { class: 'muted small lib-preview' }, i.content.replace(/\s+/g, ' ').slice(0, 90))),
        h('span', { class: 'muted small nowrap' }, fmtWords(i.word_count) + ' 字'),
        h('span', { class: 'tag' + (i.analysis ? ' ok' : '') }, i.analysis ? '已分析' : '未分析'));
      list.append(row);
    }
  }

  async function runAnalyzeAll() {
    if (!ready()) return toast('请先在设置里配置模型', 'warn');
    const todo = all.filter((i) => !i.analysis);
    await busy(analyzeAll, async () => {
      let n = 0;
      for (const i of todo) {
        analyzeAll.textContent = `分析中 ${++n}/${todo.length}…`;
        try { Object.assign(i, await api.post(`/library/${i.id}/analyze`, {})); } catch (e) { toast(`《${i.title}》分析失败：${e.message}`, 'error', 5000); }
      }
    }, '分析中…');
    draw();
  }

  async function runEmbed() {
    await busy(embedBtn, async () => {
      let total = 0;
      for (;;) {
        const r = await api.post('/library/embed', {});
        total += r.done;
        embedBtn.textContent = `生成中… 剩 ${r.remaining} 段`;
        if (r.remaining <= 0 || !r.done) break;
      }
      toast(total ? `已生成 ${total} 段向量` : '所有片段都已有向量');
    }, '生成中…');
  }

  async function importFile() {
    const file = await pickFile('.txt,.md,.docx,.epub');
    if (!file) return;
    const genre = h('input', { placeholder: '题材（可留空）' });
    const split = h('input', { type: 'checkbox', checked: true });
    modal({
      title: '导入范文：' + file.name,
      body: h('div', null, field('题材', genre), h('label', { class: 'check' }, split, '识别到章节标题时，每章存成一篇（方便检索开篇和章末钩子）')),
      actions: [
        { label: '取消', onClick: (c) => c() },
        {
          label: '导入',
          class: 'primary',
          onClick: async (close) => {
            try {
              const data = await readFile(file);
              const r = await api.post('/library/import', { filename: file.name, data, genre: genre.value.trim(), split: split.checked });
              close();
              toast(`已导入 ${r.created} 篇`);
              load();
            } catch (e) {
              toast(e.message, 'error', 5000);
            }
          },
        },
      ],
    });
  }

  async function detail(id) {
    const d = await api.get(`/library/${id}`);
    const it = d.item;
    const titleIn = h('input', { value: it.title });
    const sourceIn = h('input', { value: it.source });
    const genreIn = h('input', { value: it.genre });
    const tags = tagPicker(it.tags);
    const note = h('textarea', { rows: 2, value: it.note, placeholder: '收藏理由' });
    const content = h('textarea', { rows: 10, value: it.content, class: 'lib-content' });
    const analysis = h('div', { class: 'out-text answer lib-analysis' }, it.analysis || '还没有分析。');
    const analyzeBtn = h('button', {
      class: 'btn',
      onclick: () => busy(analyzeBtn, async () => {
        if (!ready()) throw new Error('请先在设置里配置模型');
        const r = await api.post(`/library/${id}/analyze`, {});
        analysis.textContent = r.analysis;
        Object.assign(all.find((x) => x.id === id) || {}, r);
        toast('分析完成');
      }, 'AI 分析中…'),
    }, it.analysis ? '重新分析' : 'AI 分析');
    pane.innerHTML = '';
    pane.append(
      h('div', { class: 'row' }, h('button', { class: 'btn', onclick: () => { pane.innerHTML = ''; itemsTab(pane); } }, '← 返回列表'), h('span', { class: 'spacer' }), analyzeBtn,
        h('button', {
          class: 'btn danger',
          onclick: async () => {
            if (!(await confirmBox(`删除范文《${it.title}》？`, { okText: '删除', danger: true }))) return;
            await api.del(`/library/${id}`);
            toast('已删除');
            pane.innerHTML = '';
            itemsTab(pane);
          },
        }, '删除')),
      h('div', { class: 'grid3' }, field('标题', titleIn), field('出处', sourceIn), field('题材', genreIn)),
      field('场景标签', tags),
      field('收藏理由', note),
      h('div', { class: 'grid2 lib-detail' },
        field('范文', content, d.stats || ''),
        h('div', null, h('div', { class: 'field-label' }, '写法分析'), analysis,
          h('div', { class: 'field-label', style: { marginTop: '10px' } }, `检索片段（${d.chunks.length}）`),
          h('div', { class: 'lib-chunks' }, d.chunks.map((c) => h('div', { class: 'lib-chunk' }, h('div', null, ...tagList(c.tags), c.embedded ? h('span', { class: 'tag' }, '有向量') : null), h('div', { class: 'small' }, c.text.slice(0, 120) + (c.text.length > 120 ? '…' : ''))))))),
      h('div', { class: 'row end' }, h('button', {
        class: 'btn primary',
        onclick: async (e) => {
          await busy(e.currentTarget, async () => {
            const r = await api.patch(`/library/${id}`, { title: titleIn.value, source: sourceIn.value, genre: genreIn.value, tags: tags.value(), note: note.value, content: content.value });
            Object.assign(all.find((x) => x.id === id) || {}, r);
            toast('已保存');
          }, '保存中…');
        },
      }, '保存修改')));
  }

  pane.append(
    h('div', { class: 'row' }, search, tagSel,
      h('button', { class: 'btn primary', onclick: () => openSaveToLibrary({ genre: store.book?.genre || '' }, () => load()) }, '＋ 粘贴范文'),
      h('button', { class: 'btn', onclick: importFile }, '导入文件'),
      analyzeAll, embedBtn),
    head,
    list);
  load().catch((e) => toast(e.message, 'error'));
}

function guideTab(pane, show) {
  let genre = '';
  const text = h('textarea', { rows: 16, class: 'guide-text', placeholder: '还没有文风指南。可以自己写，也可以点「AI 提炼」从范文和你的修改习惯里总结。' });
  const meta = h('div', { class: 'muted small' });
  const info = h('div', { class: 'lib-summary' });
  const rhythm = h('div', { class: 'muted small' });
  const versions = h('div', { class: 'guide-versions' });
  const genreSel = h('select', { onchange: (e) => { genre = e.target.value; load(); } });
  const instruction = h('input', { class: 'grow', placeholder: '这次想特别强调的（可留空），如“对话要更狠”“少写心理”' });
  const distillBtn = h('button', {
    class: 'btn primary',
    onclick: () => busy(distillBtn, async () => {
      if (!ready()) throw new Error('请先在设置里配置模型');
      await api.post('/library/guide/distill', { genre, instruction: instruction.value.trim() });
      instruction.value = '';
      toast('文风指南已更新');
      await load();
    }, 'AI 提炼中…'),
  }, 'AI 提炼 / 更新');
  const saveBtn = h('button', {
    class: 'btn',
    onclick: () => busy(saveBtn, async () => {
      await api.put('/library/guide', { genre, content: text.value });
      toast('已保存为新版本');
      await load();
    }, '保存中…'),
  }, '保存修改');

  async function load() {
    const s = await api.get('/library/guide?genre=' + encodeURIComponent(genre));
    const keep = genreSel.value;
    genreSel.innerHTML = '';
    genreSel.append(h('option', { value: '' }, '通用指南'), ...s.genres.map((g) => h('option', { value: g }, g + '专用')));
    genreSel.value = keep && [...genreSel.options].some((o) => o.value === keep) ? keep : genre;
    text.value = s.current?.content || '';
    meta.textContent = s.current ? `当前版本：${fmtTime(s.current.created_at)} · ${s.current.note}` : '';
    const c = s.counts;
    const st = store.settings || {};
    info.innerHTML = '';
    info.append(h('span', null,
      h('span', null, `范文 ${c.items} 篇（已分析 ${c.analyzed}）· ${fmtWords(c.words)} 字 · ${c.chunks} 个检索片段${c.embed_model ? ` · 向量 ${c.embedded}/${c.chunks}` : ''}`),
      s.feedback_pending ? h('a', { href: '#', onclick: (e) => { e.preventDefault(); show('learn'); } }, ` · 新的修改样本 ${s.feedback_pending} 条`) : null),
      h('div', { class: 'muted small' }, `写作时：${st.use_style_guide === false ? '不使用文风指南' : '自动带上文风指南（本书题材有专用指南时用专用的）'}；${st.library_refs ? `每次参考 ${st.library_refs} 段范文（灰字续写最多 2 段）` : '不参考范文'}。可在「设置 → 文风库」里调整。`));
    rhythm.textContent = s.rhythm ? '范文统计：' + s.rhythm : '';
    versions.innerHTML = '';
    const mine = s.versions.filter((v) => v.genre === genre);
    if (!mine.length) versions.append(h('div', { class: 'muted small' }, '还没有历史版本'));
    mine.forEach((v, i) => versions.append(h('div', { class: 'guide-version' + (i === 0 ? ' on' : '') },
      h('div', { class: 'grow' }, h('div', null, fmtTime(v.created_at)), h('div', { class: 'muted small' }, `${v.note} · ${v.content.length} 字`)),
      h('button', { class: 'mini', onclick: () => { text.value = v.content; meta.textContent = `正在查看 ${fmtTime(v.created_at)} 的版本（点「保存修改」或「恢复」才会生效）`; } }, '查看'),
      i === 0 ? null : h('button', { class: 'mini', onclick: async () => { await api.post(`/library/guide/${v.id}/restore`, {}); toast('已恢复'); load(); } }, '恢复'))));
  }

  pane.append(
    h('div', { class: 'row' }, field('指南', genreSel), h('div', { class: 'grow' }, info)),
    h('div', { class: 'grid-guide' },
      h('div', null, text, meta, h('div', { class: 'row' }, instruction, distillBtn, saveBtn)),
      h('div', null, h('div', { class: 'field-label' }, '历史版本'), versions, h('div', { class: 'field-label', style: { marginTop: '12px' } }, '节奏数据'), rhythm,
        h('p', { class: 'hint' }, 'AI 提炼时会读：每篇范文的分析、范文的句长和对话占比，以及你对 AI 文字的修改。范文越多、修改越多，指南越像你。'))));
  load().catch((e) => toast(e.message, 'error'));
}

function learnTab(pane, show) {
  const box = h('div', { class: 'learn-list' });
  pane.append(
    h('p', { class: 'hint' }, '你采纳 AI 的文字之后又改过的地方，会在提炼文风指南时告诉 AI：你把什么改成了什么、删掉了什么。这是 AI 了解你口味最直接的方式。'),
    box);
  api.get('/library/feedback').then((f) => {
    box.innerHTML = '';
    if (!f.changed.length && !f.deleted.length) {
      box.append(h('div', { class: 'empty' }, '还没有修改样本。用灰字续写、斜杠指令或右侧 AI 面板写作，采纳后按你的习惯改一改，这里就会出现记录。'));
      return;
    }
    box.append(h('div', { class: 'row' },
      h('span', null, `改过 ${f.changed.length} 处，删掉 ${f.deleted.length} 句` + (f.pending ? `，其中 ${f.pending} 条采纳记录还没学过` : '')),
      h('span', { class: 'spacer' }),
      h('button', { class: 'btn primary', onclick: () => show('guide') }, '去更新文风指南')));
    f.changed.forEach((p) => box.append(h('div', { class: 'learn-pair' },
      h('div', null, h('span', { class: 'tag' }, 'AI'), h('del', null, p.ai)),
      h('div', null, h('span', { class: 'tag accent' }, '你'), h('ins', null, p.final)))));
    if (f.deleted.length) {
      box.append(h('div', { class: 'field-label', style: { marginTop: '10px' } }, '整句删掉的'));
      f.deleted.forEach((d) => box.append(h('div', { class: 'learn-pair' }, h('del', null, d))));
    }
  }).catch((e) => toast(e.message, 'error'));
}

function searchTab(pane) {
  const text = h('textarea', { rows: 3, placeholder: '输入一段正文或场景描述，看看写作时会参考哪些范文。比如：两人在雨夜的屋顶交手' });
  const tags = tagPicker([]);
  const genre = h('input', { value: store.book?.genre || '', placeholder: '题材' });
  const out = h('div', { class: 'lib-list' });
  const btn = h('button', {
    class: 'btn primary',
    onclick: () => busy(btn, async () => {
      const r = await api.post('/library/search', { text: text.value, tags: tags.value(), genre: genre.value.trim(), k: 5 });
      out.innerHTML = '';
      out.append(h('div', { class: 'muted small' }, `检索标签：${r.tags.join('、') || '无'} · ${r.vector ? '已用向量检索' : '关键词检索'}${r.vector_error ? '（向量检索失败：' + r.vector_error + '）' : ''}`));
      if (!r.hits.length) out.append(h('div', { class: 'empty' }, '文风库里还没有范文'));
      r.hits.forEach((hit, i) => out.append(h('div', { class: 'lib-chunk' },
        h('div', null, h('b', null, `${i + 1}. 《${hit.title}》`), h('span', { class: 'muted small' }, ` 得分 ${hit.score.toFixed(2)} · ${hit.why.join('、') || '最近收藏'}`), ...tagList(hit.tags)),
        h('div', { class: 'small pre' }, hit.text))));
    }, '检索中…'),
  }, '检索');
  pane.append(text, field('场景标签', tags), h('div', { class: 'row' }, field('题材', genre), btn), out);
}

export function libraryButton(cls = 'ghost') {
  return h('button', { class: cls, title: '收藏范文、提炼文风指南，让 AI 学你的写法', onclick: () => openLibrary() }, '文风库');
}
