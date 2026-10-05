import { api, streamInto } from './api.js';
import { store, on, emit, scope, chapterLabel, reload } from './store.js';
import { h, toast, confirmBox, promptBox, debounce, today, fmtWords, fmtTime, countWords, themeButton, moreButton, icon, pushLayer } from './ui.js';
import { renderPanels } from './panels.js';
import { openSettings, openBookSettings, openExport, openVersions, openStats, openPlanner, openVolume, openHandbook, openBatchFinalize, openBatchDraft, openPalette, openSkills, openReplace } from './dialogs.js';
import { attachAssist, pref } from './assist.js';
import { libraryButton } from './stylelib.js';
import { openReader } from './reader.js';
import { openTrends } from './trends.js';

let els = {};
let dirty = false;
let saving = null;
let assist = null;
let drawerLayer = null;
const autosave = debounce(() => save().catch(() => {}), 1500);

export function renderEditor(root) {
  root.innerHTML = '';
  els = {};
  dirty = false;
  const layout = h('div', { class: 'editor-layout' + (localStorage.getItem('focusMode') === '1' ? ' focus' : '') });
  const sidebar = h('aside', { class: 'sidebar' });
  const main = h('main', { class: 'main' });
  const panel = h('aside', { class: 'panel' });
  layout.append(sidebar, main, panel, h('div', { class: 'drawer-mask', onclick: closeDrawers }));
  els.layout = layout;
  root.append(h('div', { class: 'shell' }, topBar(layout), layout));
  buildMain(main);
  renderSidebar(sidebar);
  renderPanels(panel);
  panel.prepend(h('div', { class: 'drawer-head' }, h('b', null, 'AI 助手'), h('button', { class: 'icon-btn', title: '收起', onclick: closeDrawers }, icon('close'))));
  scope.add(() => { drawerLayer?.(); drawerLayer = null; });

  scope.add(on('chapters-changed', () => { renderSidebar(sidebar); updateNum(); fillVolumes(store.chapter?.volume_id); }));
  scope.add(on('entries-changed', () => assist?.setEntities()));
  scope.add(() => { assist?.destroy(); assist = null; });
  scope.add(on('book-changed', () => {
    els.bookTitle.textContent = store.book.title;
    document.title = `${store.book.title} - AI 小说工坊`;
    updateCount();
  }));
  const onKey = (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      save({ force: true }).then(() => toast('已保存')).catch(() => {});
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      openPalette();
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'h' && !document.querySelector('.modal')) {
      e.preventDefault();
      openReplace({ find: store.editor?.selection().text.trim() || '' });
    } else if (e.key === 'Escape' && layout.matches('.show-side, .show-panel')) {
      closeDrawers();
    }
  };
  document.addEventListener('keydown', onKey);
  scope.add(() => { document.removeEventListener('keydown', onKey); autosave.cancel(); });

  const last = Number(localStorage.getItem('lastChapter:' + store.book.id));
  const target = store.chapters.find((c) => c.id === last) || store.chapters[0];
  if (target) openChapter(target.id);
  else showEmpty();
}

function topBar(layout) {
  els.bookTitle = h('button', { class: 'book-title', title: '作品设定', onclick: () => openBookSettings() }, store.book.title);
  const actions = h('div', { class: 'top-actions' },
    h('button', { class: 'ghost mobile-only', onclick: () => openPalette() }, '搜索章节和设定'),
    h('button', { class: 'ghost', onclick: () => openBookSettings() }, '作品设定'),
    h('button', { class: 'ghost', title: '整屏阅读正文，左右方向键翻章', onclick: () => openReader() }, '阅读'),
    h('button', { class: 'ghost', onclick: () => openExport() }, '导出'),
    h('button', { class: 'ghost', onclick: () => openStats() }, '统计'),
    h('button', { class: 'ghost', title: '今天的热梗、起点和番茄榜单、读者偏好预测', onclick: () => openTrends() }, '热点'),
    h('button', { class: 'ghost', title: '爽点、钩子、冲突、润色等写作手册', onclick: () => openHandbook() }, '手册'),
    h('button', { class: 'ghost', title: '去 AI 味等写作技能：写正文时自动遵守，也能用来修订', onclick: () => openSkills() }, '技能'),
    libraryButton(),
    h('button', {
      class: 'ghost desk-only',
      title: '隐藏两侧面板，专心写作',
      onclick: () => localStorage.setItem('focusMode', layout.classList.toggle('focus') ? '1' : '0'),
    }, '专注'),
    themeButton(),
    h('button', { class: 'ghost', onclick: () => openSettings() }, '设置'));
  const drawer = (name, other) => () => {
    layout.classList.remove(other);
    layout.classList.toggle(name);
    syncDrawerLayer();
  };
  return h('header', { class: 'topbar' },
    h('button', { class: 'ghost', title: '回到书架', onclick: () => emit('go-shelf') }, h('span', { class: 'desk-only' }, '← 书架'), h('span', { class: 'mobile-only' }, icon('back'))),
    els.bookTitle,
    h('div', { class: 'spacer' }),
    actions,
    h('button', { class: 'ghost mobile-only tool', onclick: drawer('show-side', 'show-panel') }, icon('list'), '目录'),
    h('button', { class: 'ghost mobile-only tool', onclick: drawer('show-panel', 'show-side') }, icon('spark'), '助手'),
    moreButton(actions));
}

function syncDrawerLayer() {
  const open = !!els.layout?.matches('.show-side, .show-panel');
  if (open && !drawerLayer) {
    drawerLayer = pushLayer(closeDrawers);
  } else if (!open && drawerLayer) {
    drawerLayer();
    drawerLayer = null;
  }
}

function closeDrawers() {
  els.layout?.classList.remove('show-side', 'show-panel');
  syncDrawerLayer();
}

function buildMain(main) {
  els.num = h('span', { class: 'ch-num' });
  els.title = h('input', { class: 'ch-title', placeholder: '章节标题', oninput: markDirty });
  els.volume = h('select', { class: 'ch-select', title: '所属卷', onchange: () => { markDirty(); save().catch(() => {}); } });
  els.status = h('select', { class: 'ch-select', title: '状态', onchange: () => { markDirty(); save().catch(() => {}); } },
    h('option', { value: 'draft' }, '草稿'), h('option', { value: 'done' }, '定稿'));
  els.outline = h('textarea', {
    class: 'ch-outline',
    rows: 3,
    placeholder: '本章章纲：这一章发生什么、冲突是什么、结尾留什么钩子。AI「续写」和「写整章」都会按这里来。',
    oninput: markDirty,
  });
  els.outlineBox = h('details', { class: 'outline-box' }, h('summary', null, '本章章纲'), els.outline);
  els.beats = h('textarea', {
    class: 'ch-outline',
    rows: 5,
    placeholder: '本章节拍：把这一章拆成 5～8 个场景，每行一个。可以让 AI 先规划，你改完再点右侧「写整章」，AI 会按节拍逐个写。',
    oninput: markDirty,
  });
  const beatsBtn = h('button', {
    class: 'mini',
    onclick: async (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (!store.chapter) return;
      try { await save(); } catch { return; }
      els.beatsBox.open = true;
      await streamInto(els.beats, { task: 'beats', book_id: store.book.id, chapter_id: store.chapter.id, finale: !!store.finale }, beatsBtn);
    },
  }, 'AI 规划节拍');
  els.beatsBox = h('details', { class: 'outline-box' }, h('summary', null, '本章节拍 ', beatsBtn), els.beats);
  scope.add(on('run-beats', () => beatsBtn.click()));
  els.text = h('textarea', {
    class: 'ch-text',
    spellcheck: false,
    placeholder: matchMedia('(pointer: coarse)').matches
      ? '开始写作……\n\n在新的一行打 / 或 、：描写、对话、打斗、起名等指令；打 @ 插入设定里的名字\n选中一段文字：润色、扩写、精简，或收藏到文风库'
      : '开始写作……\n\nCtrl+Enter：AI 在光标处给出灰色的续写，Tab 采纳，Esc 取消\n在新的一行打 / 或 、：描写、对话、打斗、起名等指令；打 @ 插入设定里的名字\n选中一段文字：润色、扩写、精简，或收藏到文风库\nCtrl+S 保存，Ctrl+K 搜索命令和章节',
    oninput: () => { markDirty(); updateCount(); },
    onselect: updateSel,
    onkeyup: updateSel,
    onmouseup: updateSel,
  });
  els.wrap = h('div', { class: 'text-wrap' }, els.text);
  const highlight = h('button', {
    class: 'ghost small' + (pref('highlight', '1') !== '0' ? ' on' : ''),
    title: '在正文里高亮设定库里的人名、地名等，鼠标停在上面可以看设定',
    onclick: () => {
      const on = pref('highlight', '1') === '0';
      localStorage.setItem('highlight', on ? '1' : '0');
      highlight.classList.toggle('on', on);
      assist?.refresh();
    },
  }, '高亮设定');
  els.count = h('span');
  els.aiCount = h('span', { class: 'desk-only' });
  els.sel = h('span', { class: 'muted' });
  els.saved = h('span', { class: 'saved' });
  els.paper = h('div', { class: 'paper' },
    h('div', { class: 'ch-header' },
      els.num, els.title, els.volume, els.status,
      highlight,
      h('button', { class: 'ghost small', title: '给当前内容存一个快照', onclick: snapshot }, '快照'),
      h('button', { class: 'ghost small', title: '查看和恢复历史版本', onclick: () => openVersions() }, '历史'),
      h('button', { class: 'ghost small desk-only', title: '在光标处拆成两章：光标后面的内容移到紧跟着的新章节', onclick: splitHere }, '拆章'),
      h('button', { class: 'ghost small desk-only', title: '把下一章接到这一章后面', onclick: mergeNext }, '并下章')),
    els.outlineBox,
    els.beatsBox,
    els.wrap);
  els.empty = h('div', { class: 'empty big', hidden: true },
    h('p', null, '这本书还没有章节。'),
    h('div', { class: 'row center' },
      h('button', { class: 'btn primary', onclick: () => addChapter() }, '＋ 新建第一章'),
      h('button', { class: 'btn', onclick: () => openPlanner() }, '✦ 让 AI 规划章纲')));
  const ghostBtn = h('button', {
    class: 'ghost small mobile-only pill',
    title: '在光标处让 AI 给出灰色的续写',
    onclick: () => { els.text.focus(); assist?.triggerGhost(); },
  }, icon('spark'), '续写一句');
  main.append(els.empty, els.paper, h('div', { class: 'statusbar' }, els.count, els.aiCount, els.sel, h('div', { class: 'spacer' }), ghostBtn, els.saved));
  store.editor = editorApi;
  assist = attachAssist(els.text, els.wrap);
}

function markDirty() {
  if (!store.chapter) return;
  dirty = true;
  els.saved.textContent = '未保存';
  autosave();
}

async function save({ snapshot, force } = {}) {
  const ch = store.chapter;
  if (!ch || (!dirty && !snapshot && !force)) return;
  if (saving) await saving.catch(() => {});
  const body = {
    title: els.title.value,
    outline: els.outline.value,
    beats: els.beats.value,
    content: els.text.value,
    status: els.status.value,
    volume_id: els.volume.value ? Number(els.volume.value) : null,
    client_day: today(),
  };
  if (snapshot) body.snapshot = snapshot;
  dirty = false;
  autosave.cancel();
  els.saved.textContent = '保存中…';
  saving = (async () => {
    try {
      const updated = await api.patch(`/chapters/${ch.id}`, body);
      if (store.chapter?.id === updated.id) store.chapter = updated;
      syncMeta(updated);
      els.saved.textContent = '已保存 ' + new Date().toTimeString().slice(0, 5);
    } catch (e) {
      dirty = true;
      els.saved.textContent = '保存失败';
      toast('保存失败：' + e.message, 'error', 5000);
      throw e;
    } finally {
      saving = null;
    }
  })();
  return saving;
}

function syncMeta(ch) {
  const m = store.chapters.find((x) => x.id === ch.id);
  if (!m) return;
  const structural = m.title !== ch.title || m.status !== ch.status || m.volume_id !== ch.volume_id;
  Object.assign(m, {
    title: ch.title, status: ch.status, volume_id: ch.volume_id, word_count: ch.word_count,
    ai_chars: ch.ai_chars, has_outline: !!ch.outline.trim(), has_summary: !!ch.summary.trim(),
  });
  if (structural) {
    reload(['chapters']).then(() => emit('chapters-changed'));
  } else {
    const wc = document.querySelector(`.ch-row[data-id="${ch.id}"] .wc`);
    if (wc) wc.textContent = ch.word_count ? fmtWords(ch.word_count) : '';
  }
}

async function openChapter(id) {
  try { await save(); } catch { return; }
  let ch;
  try { ch = await api.get(`/chapters/${id}`); } catch (e) { toast(e.message, 'error'); return; }
  store.chapter = ch;
  localStorage.setItem('lastChapter:' + store.book.id, id);
  els.empty.hidden = true;
  els.paper.hidden = false;
  els.title.value = ch.title;
  els.outline.value = ch.outline;
  els.beats.value = ch.beats || '';
  // 手机屏幕矮：已有正文时把章纲、节拍收起来，先露出正文
  const compact = matchMedia('(max-width: 760px)').matches && !!ch.content.trim();
  els.beatsBox.open = !compact && !!(ch.beats || '').trim();
  els.text.value = ch.content;
  els.status.value = ch.status === 'done' ? 'done' : 'draft';
  fillVolumes(ch.volume_id);
  els.outlineBox.open = !compact && (!!ch.outline.trim() || !ch.content.trim());
  dirty = false;
  autosave.cancel();
  els.saved.textContent = '已保存';
  updateNum();
  updateCount();
  updateSel();
  document.querySelectorAll('.ch-row').forEach((r) => r.classList.toggle('active', Number(r.dataset.id) === id));
  els.text.scrollTop = 0;
  closeDrawers();
  assist?.refresh();
  emit('chapter-loaded', ch);
}

function showEmpty() {
  store.chapter = null;
  els.paper.hidden = true;
  els.empty.hidden = false;
  els.count.textContent = '';
  els.saved.textContent = '';
  emit('chapter-loaded', null);
}

function fillVolumes(selected) {
  if (!els.volume) return;
  els.volume.innerHTML = '';
  els.volume.append(h('option', { value: '' }, '不分卷'), ...store.volumes.map((v) => h('option', { value: v.id, selected: v.id === selected }, v.title)));
  els.volume.hidden = !store.volumes.length;
}

function updateNum() {
  const m = store.chapter && store.chapters.find((x) => x.id === store.chapter.id);
  els.num.textContent = m?.number != null ? `第${m.number}章` : '';
}

function updateCount() {
  if (!store.chapter) return;
  const n = countWords(els.text.value);
  const target = store.book.target_words || 0;
  const ai = store.chapter.ai_chars || 0;
  els.count.textContent = `本章 ${n} 字` + (target ? ` / 目标 ${target}` : '');
  els.aiCount.textContent = ai ? `采纳 AI ${ai} 字` : '';
  els.count.classList.toggle('reached', !!target && n >= target);
}

function updateSel() {
  const t = els.text;
  const n = t.selectionEnd > t.selectionStart ? countWords(t.value.slice(t.selectionStart, t.selectionEnd)) : 0;
  els.sel.textContent = n ? `选中 ${n} 字` : '';
}

/** 用镜像元素算出选区所在高度，让文本框滚动到选区附近 */
function scrollToSelection() {
  const t = els.text;
  const style = getComputedStyle(t);
  const mirror = document.createElement('div');
  for (const p of ['fontFamily', 'fontSize', 'fontWeight', 'lineHeight', 'letterSpacing', 'paddingTop', 'paddingLeft', 'paddingRight', 'textIndent']) {
    mirror.style[p] = style[p];
  }
  Object.assign(mirror.style, { position: 'absolute', visibility: 'hidden', whiteSpace: 'pre-wrap', overflowWrap: 'break-word', boxSizing: 'border-box', width: t.clientWidth + 'px', top: '0', left: '-9999px' });
  mirror.textContent = t.value.slice(0, t.selectionStart);
  const marker = document.createElement('span');
  marker.textContent = '|';
  mirror.append(marker);
  document.body.append(mirror);
  const top = marker.offsetTop;
  mirror.remove();
  t.scrollTop = Math.max(0, top - t.clientHeight / 3);
}

function setText(value, selStart, selEnd) {
  closeDrawers();
  els.text.value = value;
  markDirty();
  updateCount();
  els.text.focus();
  els.text.setSelectionRange(selStart, selEnd);
  scrollToSelection();
  updateSel();
  assist?.refresh();
}

const editorApi = {
  flush: () => save(),
  save: (opts) => save(opts),
  isDirty: () => dirty,
  text: () => els.text?.value ?? '',
  outline: () => els.outline?.value ?? '',
  selection() {
    const t = els.text;
    const v = t.value;
    const s = t.selectionStart;
    const e = t.selectionEnd;
    return { start: s, end: e, text: v.slice(s, e), before: v.slice(0, s), after: v.slice(e) };
  },
  replaceRange(start, end, text, expected) {
    const v = els.text.value;
    if (expected != null && v.slice(start, end) !== expected) {
      const i = v.indexOf(expected);
      if (i < 0) {
        toast('原来选中的文字已经改动过，找不到替换位置。可以复制结果后手动粘贴。', 'warn', 5000);
        return false;
      }
      start = i;
      end = i + expected.length;
    }
    setText(v.slice(0, start) + text + v.slice(end), start, start + text.length);
    return true;
  },
  insertAt(pos, text) {
    const v = els.text.value;
    pos = Math.min(pos, v.length);
    const pre = pos > 0 && v[pos - 1] !== '\n' ? '\n' : '';
    const post = pos < v.length && v[pos] !== '\n' ? '\n' : '';
    setText(v.slice(0, pos) + pre + text + post + v.slice(pos), pos + pre.length, pos + pre.length + text.length);
    return true;
  },
  append(text) {
    const v = els.text.value.replace(/\s+$/, '');
    const pre = v ? '\n' : '';
    setText(v + pre + text, v.length + pre.length, v.length + pre.length + text.length);
    return true;
  },
  replaceAll(text) {
    setText(text, 0, 0);
    els.text.scrollTop = 0;
    return true;
  },
  select(start, end) {
    els.text.focus();
    els.text.setSelectionRange(start, end);
    scrollToSelection();
    updateSel();
  },
  find: (str) => els.text.value.indexOf(str),
  open: (id) => openChapter(id),
  reloadChapter: () => store.chapter && openChapter(store.chapter.id),
  refreshCount: updateCount,
  setStatus(status) {
    els.status.value = status;
    markDirty();
    return save();
  },
};

function renderSidebar(el) {
  el.innerHTML = '';
  const volMap = new Map(store.volumes.map((v) => [v.id, v]));
  const tree = h('div', { class: 'tree' });
  let prev;
  for (const m of store.chapters) {
    const vid = m.volume_id && volMap.has(m.volume_id) ? m.volume_id : null;
    if (vid !== prev) {
      if (vid) tree.append(volumeRow(volMap.get(vid)));
      else if (store.volumes.length) tree.append(h('div', { class: 'vol-row plain' }, h('span', { class: 'vol-name' }, '未分卷')));
      prev = vid;
    }
    tree.append(chapterRow(m));
  }
  for (const v of store.volumes) {
    if (!store.chapters.some((c) => c.volume_id === v.id)) tree.append(volumeRow(v));
  }
  if (!store.chapters.length && !store.volumes.length) tree.append(h('div', { class: 'empty' }, '还没有章节'));
  const total = store.chapters.reduce((a, c) => a + c.word_count, 0);
  el.append(
    h('div', { class: 'side-head' },
      h('b', null, '目录'),
      h('div', { class: 'row tight' },
        h('button', { class: 'mini', title: '新建一卷', onclick: addVolume }, '＋卷'),
        h('button', { class: 'mini primary', title: '在最后一卷末尾新建章节', onclick: () => addChapter() }, '＋章'),
        h('button', { class: 'icon-btn mobile-only drawer-close', title: '收起', onclick: closeDrawers }, icon('close')))),
    tree,
    h('div', { class: 'side-foot' },
      h('button', { class: 'btn block', onclick: () => openPlanner() }, '✦ AI 规划后续章纲'),
      h('div', { class: 'row tight' },
        h('button', { class: 'btn grow', title: '给有章纲、没正文的章节依次规划节拍并起草', onclick: () => openBatchDraft() }, '批量起草'),
        h('button', { class: 'btn grow', title: '一次给多章生成摘要、提取设定和张力，适合导入旧稿后建档', onclick: () => openBatchFinalize() }, '批量定稿')),
      h('div', { class: 'muted small center desk-only' }, 'Ctrl+K 搜索命令和章节'),
      h('div', { class: 'muted small center' }, `${store.chapters.length} 章 · ${fmtWords(total)} 字`)));
}

function chapterRow(m) {
  const stop = (fn) => (e) => { e.stopPropagation(); fn(); };
  const dot = m.status === 'done' ? 's-done' : m.word_count ? 's-draft' : 's-none';
  return h('div', {
    class: 'ch-row' + (store.chapter?.id === m.id ? ' active' : '') + (m.published_at ? ' published' : ''),
    dataset: { id: m.id },
    title: `${chapterLabel(m)}${m.has_summary ? '（已有摘要）' : ''}${m.published_at ? `，${fmtTime(m.published_at)}发布` : ''}`,
    onclick: () => openChapter(m.id),
  },
  h('span', { class: 'dot ' + dot }),
  h('span', { class: 'ch-name' }, chapterLabel(m)),
  m.published_at ? h('span', { class: 'pub-tag' }, '已发') : null,
  h('span', { class: 'wc' }, m.word_count ? fmtWords(m.word_count) : m.has_outline ? '有纲' : ''),
  h('span', { class: 'row-actions' },
    m.word_count || m.published_at
      ? h('button', {
        class: 'icon-btn',
        title: m.published_at ? '取消发布（这一章和后面的章节）' : '标成已发布（这一章和前面的章节）',
        onclick: stop(() => publish(m)),
      }, m.published_at ? '撤' : '发')
      : null,
    h('button', { class: 'icon-btn', title: '上移', onclick: stop(() => move(m.id, -1)) }, '↑'),
    h('button', { class: 'icon-btn', title: '下移', onclick: stop(() => move(m.id, 1)) }, '↓'),
    h('button', { class: 'icon-btn', title: '删除', onclick: stop(() => removeChapter(m)) }, '✕')));
}

async function splitHere() {
  if (!store.chapter) return;
  const at = els.text.selectionStart;
  const v = els.text.value;
  if (!v.slice(0, at).trim() || !v.slice(at).trim()) return toast('先把光标放在要拆开的地方（正文中间）', 'warn');
  if (!(await confirmBox(`在光标处拆成两章？光标后面的 ${countWords(v.slice(at))} 字会移到紧跟着的新章节，拆之前自动存快照。`, { okText: '拆章' }))) return;
  try { await save({ force: true }); } catch { return; }
  try {
    const ch = await api.post(`/chapters/${store.chapter.id}/split`, { at });
    await reload(['chapters']);
    emit('chapters-changed');
    await openChapter(ch.id);
    toast('已拆成两章，新章节在后面，记得起个标题');
    els.title.focus();
  } catch (e) {
    toast(e.message, 'error');
  }
}

async function mergeNext() {
  if (!store.chapter) return;
  const i = store.chapters.findIndex((c) => c.id === store.chapter.id);
  const next = store.chapters[i + 1];
  if (!next) return toast('已经是最后一章了', 'warn');
  if (!(await confirmBox(`把「${chapterLabel(next)}」接到这一章后面，然后删掉它？它的历史版本、设定状态和揭示进度都会转到这一章，两章原文都会先存快照。`, { okText: '合并', danger: true }))) return;
  try { await save({ force: true }); } catch { return; }
  try {
    const id = store.chapter.id;
    await api.post(`/chapters/${id}/merge_next`);
    await reload(['chapters']);
    emit('chapters-changed');
    await openChapter(id);
    toast('已合并；章末变了，需要的话重新定稿分析张力和钩子');
  } catch (e) {
    toast(e.message, 'error');
  }
}

async function publish(m) {
  const published = !m.published_at;
  try {
    const r = await api.post(`/books/${store.book.id}/publish`, { chapter_id: m.id, published });
    await reload(['chapters']);
    emit('chapters-changed');
    const drafts = store.chapters.filter((c) => c.word_count > 0 && !c.published_at);
    const words = drafts.reduce((a, c) => a + c.word_count, 0);
    toast(`${published ? '标成已发布' : '取消发布'} ${r.changed} 章；存稿 ${drafts.length} 章 ${fmtWords(words)} 字`);
  } catch (e) {
    toast(e.message, 'error');
  }
}

function volumeRow(v) {
  return h('div', { class: 'vol-row' },
    h('span', { class: 'vol-name', title: '编辑卷名、卷纲和卷摘要', onclick: () => openVolume(v) }, v.title),
    h('span', { class: 'row-actions always' },
      h('button', { class: 'icon-btn', title: '在本卷末尾新建章节', onclick: () => addChapter(v.id) }, '＋')));
}

async function addVolume() {
  const title = await promptBox('新建卷', { placeholder: `第${store.volumes.length + 1}卷 卷名（可留空）` });
  if (title === null) return;
  await api.post(`/books/${store.book.id}/volumes`, { title });
  await reload(['volumes']);
  emit('chapters-changed');
  toast('已新建，之后用「＋章」新建的章节会放进这一卷');
}

async function addChapter(volumeId = store.volumes.length ? store.volumes[store.volumes.length - 1].id : null) {
  try { await save(); } catch { return; }
  const ch = await api.post(`/books/${store.book.id}/chapters`, { volume_id: volumeId });
  if (volumeId) {
    const ids = store.chapters.map((c) => c.id);
    let last = -1;
    store.chapters.forEach((c, i) => { if (c.volume_id === volumeId) last = i; });
    if (last >= 0 && last < ids.length - 1) {
      ids.splice(last + 1, 0, ch.id);
      await api.post(`/books/${store.book.id}/chapters/reorder`, ids);
    }
  }
  await reload(['chapters']);
  emit('chapters-changed');
  await openChapter(ch.id);
  els.title.focus();
}

async function move(id, dir) {
  const ids = store.chapters.map((c) => c.id);
  const i = ids.indexOf(id);
  const j = i + dir;
  if (j < 0 || j >= ids.length) return;
  [ids[i], ids[j]] = [ids[j], ids[i]];
  await api.post(`/books/${store.book.id}/chapters/reorder`, ids);
  await reload(['chapters']);
  emit('chapters-changed');
}

async function removeChapter(m) {
  const ok = await confirmBox(`删除「${chapterLabel(m)}」？正文和它的历史版本都会删除。`, { okText: '删除', danger: true });
  if (!ok) return;
  const wasCurrent = store.chapter?.id === m.id;
  if (wasCurrent) { dirty = false; autosave.cancel(); }
  await api.del(`/chapters/${m.id}`);
  const idx = store.chapters.findIndex((c) => c.id === m.id);
  await reload(['chapters']);
  emit('chapters-changed');
  if (wasCurrent) {
    const next = store.chapters[Math.min(idx, store.chapters.length - 1)];
    if (next) openChapter(next.id);
    else showEmpty();
  }
  toast('已删除');
}

async function snapshot() {
  if (!store.chapter) return;
  try { await save(); } catch { return; }
  const note = await promptBox('保存快照', { value: '手动快照', placeholder: '备注' });
  if (note === null) return;
  await api.post(`/chapters/${store.chapter.id}/versions`, { note: note || '手动快照' });
  toast('快照已保存，可以在「历史」里查看和恢复');
}
