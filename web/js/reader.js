import { api } from './api.js';
import { store, chapterLabel } from './store.js';
import { h, toast } from './ui.js';

/** 阅读模式：整屏看正文，上一章 / 下一章翻页 */
export async function openReader() {
  try { await store.editor?.flush(); } catch { /* 保存失败时编辑器自己会提示 */ }
  const list = store.chapters;
  if (!list.length) {
    toast('这本书还没有章节');
    return;
  }
  let index = Math.max(0, list.findIndex((c) => c.id === store.chapter?.id));
  let size = Number(localStorage.getItem('readerSize')) || 19;
  let loading = 0;

  const title = h('div', { class: 'reader-title' });
  const body = h('div', { class: 'reader-body' });
  const pos = h('span', { class: 'muted small' });
  const prev = h('button', { class: 'btn', onclick: () => go(index - 1) }, '‹ 上一章');
  const next = h('button', { class: 'btn', onclick: () => go(index + 1) }, '下一章 ›');
  const setSize = (d) => {
    size = Math.min(28, Math.max(14, size + d));
    body.style.setProperty('--reader-size', size + 'px');
    localStorage.setItem('readerSize', String(size));
  };
  const close = () => {
    el.remove();
    document.removeEventListener('keydown', onKey);
  };
  const onKey = (e) => {
    if (e.key === 'Escape') close();
    else if (e.key === 'ArrowLeft') go(index - 1);
    else if (e.key === 'ArrowRight') go(index + 1);
  };
  const el = h('div', { class: 'reader' },
    h('div', { class: 'reader-head' },
      h('button', { class: 'icon-btn', title: '关闭（Esc）', onclick: close }, '✕'),
      title,
      h('button', { class: 'mini', title: '字小一点', onclick: () => setSize(-1) }, 'A−'),
      h('button', { class: 'mini', title: '字大一点', onclick: () => setSize(1) }, 'A+')),
    body,
    h('div', { class: 'reader-foot' }, prev, pos, next));
  document.body.append(el);
  document.addEventListener('keydown', onKey);
  setSize(0);
  go(index);

  async function go(i) {
    if (i < 0 || i >= list.length) {
      toast(i < 0 ? '已经是第一章了' : '已经是最后一章了');
      return;
    }
    const ticket = ++loading;
    let ch;
    try {
      ch = await api.get(`/chapters/${list[i].id}`);
    } catch (e) {
      toast(e.message, 'error');
      return;
    }
    if (ticket !== loading) return;
    index = i;
    const label = chapterLabel(list[i]);
    const paras = (ch.content || '').split(/\n+/).map((s) => s.replace(/^[\s\u3000]+/, '')).filter(Boolean);
    title.textContent = `${store.book.title} · ${label}`;
    pos.textContent = `${i + 1} / ${list.length}`;
    prev.disabled = i === 0;
    next.disabled = i === list.length - 1;
    body.replaceChildren(h('article', { class: 'reader-page' },
      h('h2', null, label),
      paras.length ? paras.map((p) => h('p', null, p)) : h('p', { class: 'muted' }, '（这一章还没有正文）')));
    body.scrollTop = 0;
  }
}
