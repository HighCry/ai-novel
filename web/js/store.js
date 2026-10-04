import { api } from './api.js';

export const store = {
  settings: null,
  book: null,
  volumes: [],
  chapters: [],
  chapter: null,
  entries: [],
  threads: [],
  /** 编辑器对外接口，由 editor.js 提供 */
  editor: null,
  /** 角色对话记录：entry id → messages */
  chats: {},
  chatTarget: null,
};

const bus = new EventTarget();
export const emit = (name, detail) => bus.dispatchEvent(new CustomEvent(name, { detail }));
export function on(name, fn) {
  const handler = (e) => fn(e.detail);
  bus.addEventListener(name, handler);
  return () => bus.removeEventListener(name, handler);
}

/** 当前视图注册的监听，切换视图时统一清理 */
export const scope = {
  list: [],
  add(fn) { this.list.push(fn); },
  clear() { this.list.splice(0).forEach((f) => f()); },
};

export function chapterLabel(m) {
  if (!m) return '';
  if (m.number == null) return m.title || '（无标题）';
  return `第${m.number}章` + (m.title ? ' ' + m.title : '');
}

export function modelReady() {
  const s = store.settings;
  return !!(s && s.providers?.length && (s.writer?.model || s.analyst?.model));
}

export async function reload(parts = ['chapters', 'volumes', 'entries', 'threads']) {
  const id = store.book.id;
  const paths = { chapters: 'chapters', volumes: 'volumes', entries: 'entries', threads: 'threads' };
  const results = await Promise.all(parts.map((p) => api.get(`/books/${id}/${paths[p]}`)));
  parts.forEach((p, i) => { store[p] = results[i]; });
}
