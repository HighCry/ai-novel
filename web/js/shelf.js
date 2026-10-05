import { api } from './api.js';
import { emit, modelReady, on, scope } from './store.js';
import { h, toast, confirmBox, fmtWords, fmtTime, themeButton, moreButton } from './ui.js';
import { openWizard } from './wizard.js';
import { openSettings, openImport, openHandbook, openSkills } from './dialogs.js';
import { libraryButton } from './stylelib.js';
import { openTeardown } from './teardown.js';

const PLATFORM = { fanqie: '番茄', qidian: '起点' };

export async function renderShelf(root) {
  scope.clear();
  scope.add(on('settings-changed', () => renderShelf(root)));
  root.innerHTML = '';
  const grid = h('div', { class: 'book-grid' }, h('div', { class: 'boot' }, '加载中……'));
  const banner = modelReady()
    ? null
    : h('div', { class: 'notice big' },
      h('div', null, h('b', null, '第一步：配置模型。'), ' 支持所有 OpenAI 兼容接口：DeepSeek、Kimi、通义千问、智谱、豆包、硅基流动、OpenRouter、本机 Ollama 或 sub2api 等。'),
      h('button', { class: 'btn primary', onclick: () => openSettings() }, '去设置'));
  const actions = h('div', { class: 'top-actions' },
    h('button', { class: 'ghost', title: '爽点、钩子、冲突、润色等写作手册', onclick: () => openHandbook() }, '写作手册'),
    h('button', { class: 'ghost', title: '去 AI 味等写作技能：写正文时自动遵守，也能用来修订', onclick: () => openSkills() }, '写作技能'),
    libraryButton(),
    h('button', { class: 'ghost', title: '导入同类爆款，拆解开头、张力、爽点和钩子，只学结构', onclick: () => openTeardown() }, '拆书'),
    themeButton(),
    h('button', { class: 'ghost', onclick: () => openSettings() }, '设置'));
  root.append(h('div', { class: 'shell' },
    h('header', { class: 'topbar' },
      h('div', { class: 'brand' }, h('span', { class: 'logo' }, '墨'), 'AI 小说工坊'),
      h('div', { class: 'spacer' }),
      actions,
      moreButton(actions)),
    h('div', { class: 'shelf' },
      h('div', { class: 'shelf-head' },
        h('div', null,
          h('h1', null, '我的作品'),
          h('p', { class: 'muted' }, '开新书、续写旧稿。AI 记住设定、人物和伏笔，剧情由你掌控。')),
        h('div', { class: 'row' },
          h('button', { class: 'btn primary big', onclick: () => openWizard() }, '＋ 开新书'),
          h('button', { class: 'btn big', onclick: () => openImport() }, '导入旧稿续写'))),
      banner,
      grid)));
  try {
    const books = await api.get('/books');
    grid.innerHTML = '';
    if (!books.length) {
      grid.append(h('div', { class: 'empty big' }, '还没有作品。点「开新书」让 AI 陪你从一个创意走到第一卷章纲，或者导入已经写了一部分的稿子（支持 txt / docx / epub，自动识别章节和分卷）。'));
    }
    books.forEach((b, i) => {
      const card = bookCard(b, root);
      card.style.setProperty('--i', String(Math.min(i, 12)));
      grid.append(card);
    });
  } catch (e) {
    grid.innerHTML = '';
    grid.append(h('div', { class: 'empty' }, '加载失败：' + e.message));
  }
}

function bookCard(b, root) {
  const remove = async (e) => {
    e.stopPropagation();
    const ok = await confirmBox(`确定删除《${b.title}》吗？所有章节、设定、伏笔和历史版本都会一起删除，无法恢复。`, { okText: '删除', danger: true });
    if (!ok) return;
    await api.del(`/books/${b.id}`);
    toast('已删除');
    renderShelf(root);
  };
  return h('div', { class: 'book-card', onclick: () => emit('open-book', b.id) },
    h('div', { class: 'book-cover' }, (b.title || '书').slice(0, 1)),
    h('div', { class: 'book-info' },
      h('h3', null, b.title),
      h('div', { class: 'tags' },
        b.genre ? h('span', { class: 'tag' }, b.genre) : null,
        PLATFORM[b.platform] ? h('span', { class: 'tag accent' }, PLATFORM[b.platform]) : null),
      h('p', { class: 'logline' }, b.logline || '（还没有一句话梗概）'),
      h('div', { class: 'muted small' }, `${b.chapter_count} 章 · ${fmtWords(b.word_count)} 字 · ${fmtTime(b.updated_at)}`)),
    h('button', { class: 'icon-btn card-del', title: '删除作品', onclick: remove }, '✕'));
}
