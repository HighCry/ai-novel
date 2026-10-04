import { api } from './api.js';
import { store, on, scope, reload } from './store.js';
import { toast, syncThemeColor, closeTopLayer } from './ui.js';
import { renderShelf } from './shelf.js';
import { renderEditor } from './editor.js';

const root = document.getElementById('app');

// 安卓 App 里网页铺满全屏（画到状态栏、导航栏下面），由原生壳告诉我们这两条栏有多高；之后变化时原生会直接改这两个变量
if (window.AiNovelAndroid) {
  document.documentElement.classList.add('in-app');
  try {
    const { top, bottom } = JSON.parse(window.AiNovelAndroid.insets());
    document.documentElement.style.setProperty('--safe-top', top + 'px');
    document.documentElement.style.setProperty('--safe-bottom', bottom + 'px');
  } catch { /* 拿不到就用 env() 的默认值 */ }
}

export function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem('theme', theme);
  syncThemeColor();
}
applyTheme(localStorage.getItem('theme') || 'light');

document.addEventListener('click', (e) => {
  if (e.target.closest('.more-btn')) return;
  for (const menu of document.querySelectorAll('.top-actions.open')) {
    if (!menu.contains(e.target) || e.target.closest('button')) menu.classList.remove('open');
  }
});

document.addEventListener('pointerdown', (e) => {
  const btn = e.target.closest('.btn.primary');
  if (!btn || btn.disabled) return;
  const r = btn.getBoundingClientRect();
  btn.style.setProperty('--x', `${e.clientX - r.left}px`);
  btn.style.setProperty('--y', `${e.clientY - r.top}px`);
  btn.classList.remove('ink');
  void btn.offsetWidth;
  btn.classList.add('ink');
});

// 本机（桌面版、浏览器版）不需要；从手机等远程地址打开时才注册，用来「添加到主屏幕」
if ('serviceWorker' in navigator && window.isSecureContext && !/^(127\.|localhost$|\[::1\]$)/.test(location.hostname)) {
  navigator.serviceWorker.register('/sw.js').catch(() => {});
}

async function leaveCurrent() {
  try { await store.editor?.flush(); } catch { /* 保存失败时编辑器自己会提示 */ }
  scope.clear();
  store.editor = null;
  store.chapter = null;
}

async function showShelf() {
  await leaveCurrent();
  store.book = null;
  history.replaceState(null, '', '#/');
  document.title = 'AI 小说工坊';
  renderShelf(root);
}

async function openBook(id) {
  await leaveCurrent();
  try {
    store.book = await api.get(`/books/${id}`);
    store.chats = {};
    await reload();
    history.replaceState(null, '', `#/book/${id}`);
    document.title = `${store.book.title} - AI 小说工坊`;
    renderEditor(root);
  } catch (e) {
    toast(e.message, 'error');
    showShelf();
  }
}

async function boot() {
  try {
    store.settings = await api.get('/settings');
  } catch (e) {
    toast('连不上后端服务：' + e.message, 'error', 8000);
  }
  on('open-book', openBook);
  on('go-shelf', showShelf);
  // 给桌面版窗口调用：关窗口前保存正在编辑的内容，导出完成后显示提示
  window.__aiNovelFlush = () => store.editor?.flush();
  window.__aiNovelToast = toast;
  // 给安卓返回键调用：先收菜单、关最上层浮层，再从书里回书架；返回 false 表示已在书架，由原生退到后台
  window.__aiNovelBack = () => {
    const menu = document.querySelector('.top-actions.open');
    if (menu) {
      menu.classList.remove('open');
      return true;
    }
    if (closeTopLayer()) return true;
    if (store.book) {
      showShelf();
      return true;
    }
    return false;
  };
  window.addEventListener('beforeunload', (e) => {
    if (store.editor?.isDirty()) {
      store.editor.flush();
      e.preventDefault();
    }
  });
  const m = location.hash.match(/^#\/book\/(\d+)/);
  if (m) openBook(Number(m[1]));
  else showShelf();
}

boot();
