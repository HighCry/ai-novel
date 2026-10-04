import { api, streamInto } from './api.js';
import { emit, modelReady } from './store.js';
import { h, toast, modal, field, busy, promptBox } from './ui.js';
import { openSettings, loadGenres, matchGenre } from './dialogs.js';

export const GENRES = ['都市脑洞', '都市高武', '都市日常', '玄幻', '仙侠修真', '历史穿越', '科幻末世', '悬疑灵异', '游戏竞技', '体育', '无限流', '系统流', '重生逆袭', '年代文', '种田', '古代言情', '现代言情', '幻想言情', '宫斗宅斗', '快穿'];
const STEPS = ['创意', '选方案', '世界观', '人物', '总纲', '第一卷章纲'];

export function openWizard() {
  if (!modelReady()) {
    toast('开书向导需要先配置模型', 'warn');
    openSettings();
    return;
  }
  const d = {
    platform: 'fanqie', genre: '玄幻', idea: '', protagonist: '',
    ideas: [], title: '', logline: '',
    worldview: '', characters: [], outline: '',
    count: 10, outlines: [], bookId: null, volumeId: null,
  };
  let step = 0;
  const m = modal({ title: '开新书', body: h('div'), wide: 'xl' });

  const fields = () => ({
    title: d.title, genre: d.genre, platform: d.platform, idea: d.idea, protagonist: d.protagonist,
    logline: d.logline, worldview: d.worldview, outline: d.outline,
    characters: d.characters.map((c) => `· ${c.name}（${c.role || '人物'}）：${c.description}`).join('\n'),
  });

  const go = (n) => { step = n; render(); };

  async function createBook() {
    if (d.bookId) return d.bookId;
    if (!d.title.trim()) {
      const t = await promptBox('给这本书起个名字', { placeholder: '书名' });
      if (!t) throw new Error('需要书名才能创建作品');
      d.title = t;
    }
    const book = await api.post('/books', {
      title: d.title, genre: d.genre, platform: d.platform, logline: d.logline,
      worldview: d.worldview, outline: d.outline,
    });
    const vol = await api.post(`/books/${book.id}/volumes`, { title: '第一卷' });
    for (const c of d.characters.filter((c) => c.name?.trim())) {
      await api.post(`/books/${book.id}/entries`, {
        kind: 'character', name: c.name, aliases: c.aliases || '',
        description: [c.role, c.description].filter(Boolean).join('，'),
        immutable: c.immutable || '', state: c.state || '',
        always_include: c.role === '主角',
      });
    }
    d.bookId = book.id;
    d.volumeId = vol.id;
    return book.id;
  }

  async function finish() {
    const id = await createBook();
    for (const o of d.outlines.filter((o) => o.checked)) {
      await api.post(`/books/${id}/chapters`, { title: o.title, outline: o.outline, volume_id: d.volumeId });
    }
    m.close();
    toast('作品已创建，开始写作吧');
    emit('open-book', id);
  }

  function nav(next, nextLabel = '下一步') {
    const list = [];
    if (step > 0) list.push({ label: '上一步', onClick: () => go(step - 1) });
    if (step >= 1) list.push({ label: '跳过剩下的，直接建书', onClick: (close) => busyFinish(close) });
    if (next) list.push({ label: nextLabel, class: 'primary', onClick: next });
    m.setActions(list);
  }

  async function busyFinish() {
    try { await finish(); } catch (e) { toast(e.message, 'error', 5000); }
  }

  const stepBar = () => h('div', { class: 'steps' }, STEPS.map((s, i) => h('span', { class: 'step' + (i === step ? ' on' : i < step ? ' done' : '') }, `${i + 1}. ${s}`)));

  function render() {
    m.body.innerHTML = '';
    m.body.append(stepBar());
    [stepIdea, stepPick, stepWorld, stepCharacters, stepOutline, stepChapters][step]();
  }

  function stepIdea() {
    const platform = h('div', { class: 'seg' }, [['fanqie', '番茄小说'], ['qidian', '起点中文网'], ['', '其他平台']].map(([v, l]) =>
      h('button', { class: 'seg-btn' + (d.platform === v ? ' on' : ''), onclick: (e) => { d.platform = v; platform.querySelectorAll('.seg-btn').forEach((b) => b.classList.remove('on')); e.target.classList.add('on'); } }, l)));
    const genreHint = h('span', { class: 'hint' });
    const showGenre = (profiles) => {
      const g = matchGenre(d.genre, profiles);
      genreHint.textContent = g ? `题材模板「${g.genre_name}」：${g.core_tone.slice(0, 70)}…` : '';
    };
    const datalist = h('datalist', { id: 'genre-list' }, GENRES.map((g) => h('option', { value: g })));
    let profilesCache = [];
    const genre = h('input', { value: d.genre, list: 'genre-list', oninput: (e) => { d.genre = e.target.value; showGenre(profilesCache); } });
    loadGenres().then((profiles) => {
      profilesCache = profiles;
      profiles.filter((p) => !GENRES.includes(p.genre_name)).forEach((p) => datalist.append(h('option', { value: p.genre_name })));
      showGenre(profiles);
    });
    const idea = h('textarea', { rows: 4, value: d.idea, placeholder: '一句话或几句话说说你想写什么。比如：外卖员送餐时绑定了“差评返还系统”，每收到一个差评就返还十倍的能力。', oninput: (e) => { d.idea = e.target.value; } });
    const hero = h('textarea', { rows: 2, value: d.protagonist, placeholder: '（可选）主角是什么样的人？性格、身份、处境。', oninput: (e) => { d.protagonist = e.target.value; } });
    m.body.append(
      field('目标平台', platform, '番茄节奏更快、爽点更密；起点更看重设定和剧情逻辑。'),
      field('题材', h('div', null, genre, datalist, genreHint), '可以从列表选（含 37 个题材模板），也可以自己写，比如“都市 + 鉴宝”'),
      field('核心创意', idea),
      field('主角设定', hero));
    nav(async () => {
      if (!d.idea.trim()) return toast('先写一句核心创意', 'warn');
      const btn = m.box.querySelector('.modal-foot .primary');
      await busy(btn, async () => {
        const r = await api.post('/ai/json', { task: 'ideas', fields: fields() });
        d.ideas = r.ideas || [];
        if (!d.ideas.length) throw new Error('模型没有给出方案，请重试');
        go(1);
      }, 'AI 正在构思…');
    }, 'AI 出 3 个开书方案 →');
  }

  function stepPick() {
    const title = h('input', { value: d.title, placeholder: '书名', oninput: (e) => { d.title = e.target.value; } });
    const logline = h('textarea', { rows: 2, value: d.logline, placeholder: '一句话梗概', oninput: (e) => { d.logline = e.target.value; } });
    const cards = h('div', { class: 'idea-grid' }, d.ideas.map((it) => h('div', {
      class: 'idea-card' + (d.title === it.title ? ' on' : ''),
      onclick: (e) => {
        d.title = it.title; d.logline = it.logline;
        title.value = d.title; logline.value = d.logline;
        cards.querySelectorAll('.idea-card').forEach((c) => c.classList.remove('on'));
        e.currentTarget.classList.add('on');
      },
    },
    h('h4', null, it.title),
    h('p', null, it.logline),
    it.selling_points ? h('p', { class: 'small' }, h('b', null, '卖点：'), it.selling_points) : null,
    it.opening ? h('p', { class: 'small muted' }, h('b', null, '开篇：'), it.opening) : null)));
    const again = h('button', { class: 'btn', onclick: (e) => busy(e.target, async () => {
      const r = await api.post('/ai/json', { task: 'ideas', fields: fields() });
      d.ideas = r.ideas || d.ideas;
      render();
    }, '重新构思中…') }, '换一批');
    m.body.append(h('p', { class: 'hint' }, '点选一个方案，或者在下面直接修改书名和梗概。'), cards, again, field('书名', title), field('一句话梗概', logline));
    nav(() => {
      if (!d.title.trim()) return toast('选一个方案或填上书名', 'warn');
      go(2);
    });
  }

  function stepWorld() {
    const ta = h('textarea', { rows: 16, value: d.worldview, placeholder: '世界观：时代地图、力量体系、势力、金手指规则……可以让 AI 生成后再改。', oninput: (e) => { d.worldview = e.target.value; } });
    const gen = h('button', { class: 'btn primary', onclick: () => streamInto(ta, { task: 'world', fields: fields() }, gen) }, 'AI 生成世界观');
    m.body.append(h('div', { class: 'row' }, gen, h('span', { class: 'hint' }, '只写会影响剧情的设定，生成后可以随意修改。')), ta);
    nav(() => go(3));
  }

  function stepCharacters() {
    const list = h('div', { class: 'char-list' });
    const draw = () => {
      list.innerHTML = '';
      if (!d.characters.length) list.append(h('div', { class: 'empty' }, '还没有人物。可以让 AI 设计，也可以手动添加。'));
      d.characters.forEach((c, i) => list.append(h('div', { class: 'char-card' },
        h('div', { class: 'row' },
          h('input', { value: c.name, placeholder: '姓名', class: 'grow', oninput: (e) => { c.name = e.target.value; } }),
          h('input', { value: c.role || '', placeholder: '主角/配角/反派', class: 'narrow', oninput: (e) => { c.role = e.target.value; } }),
          h('button', { class: 'icon-btn', title: '删除', onclick: () => { d.characters.splice(i, 1); draw(); } }, '✕')),
        h('textarea', { rows: 2, value: c.description || '', placeholder: '身份、性格、动机、和主角的关系', oninput: (e) => { c.description = e.target.value; } }),
        h('input', { value: c.immutable || '', placeholder: '不能写崩的核心特征', oninput: (e) => { c.immutable = e.target.value; } }),
        h('input', { value: c.state || '', placeholder: '开篇状态：位置、实力、处境', oninput: (e) => { c.state = e.target.value; } }))));
    };
    const gen = h('button', { class: 'btn primary', onclick: (e) => busy(e.target, async () => {
      const r = await api.post('/ai/json', { task: 'characters', fields: fields() });
      d.characters = r.characters || [];
      draw();
    }, 'AI 设计中…') }, 'AI 设计主要人物');
    const add = h('button', { class: 'btn', onclick: () => { d.characters.push({ name: '', role: '配角' }); draw(); } }, '＋ 手动添加');
    m.body.append(h('div', { class: 'row' }, gen, add, h('span', { class: 'hint' }, '人物会存进设定库，主角设为常驻（每次写作都带上）。')), list);
    draw();
    nav(() => go(4));
  }

  function stepOutline() {
    const ta = h('textarea', { rows: 16, value: d.outline, placeholder: '总纲：主线、结局、按卷拆分……', oninput: (e) => { d.outline = e.target.value; } });
    const gen = h('button', { class: 'btn primary', onclick: () => streamInto(ta, { task: 'outline', fields: fields() }, gen) }, 'AI 生成总纲');
    m.body.append(h('div', { class: 'row' }, gen, h('span', { class: 'hint' }, '总纲决定全书走向，后面每章写作都会参考。')), ta);
    nav(() => go(5), '下一步：规划章纲');
  }

  function stepChapters() {
    const list = h('div', { class: 'outline-list' });
    const draw = () => {
      list.innerHTML = '';
      if (!d.outlines.length) list.append(h('div', { class: 'empty' }, '点「生成章纲」，AI 会按总纲规划第一卷前几章，你可以逐条修改或取消勾选。'));
      d.outlines.forEach((o, i) => list.append(h('div', { class: 'outline-item' },
        h('input', { type: 'checkbox', checked: o.checked, onchange: (e) => { o.checked = e.target.checked; } }),
        h('div', { class: 'grow' },
          h('input', { value: o.title, oninput: (e) => { o.title = e.target.value; }, placeholder: `第${i + 1}章标题` }),
          h('textarea', { rows: 2, value: o.outline, oninput: (e) => { o.outline = e.target.value; } })))));
    };
    const count = h('input', { type: 'number', min: 3, max: 30, value: d.count, class: 'num', oninput: (e) => { d.count = +e.target.value; } });
    const gen = h('button', { class: 'btn primary', onclick: (e) => busy(e.target, async () => {
      const id = await createBook();
      const r = await api.post('/ai/json', { task: 'chapter_outlines', book_id: id, volume_id: d.volumeId, count: d.count });
      d.outlines = (r.chapters || []).map((c) => ({ ...c, checked: true }));
      draw();
    }, '建书并规划中…') }, '生成章纲');
    m.body.append(h('div', { class: 'row' }, h('label', null, '规划前 ', count, ' 章'), gen, h('span', { class: 'hint' }, '这一步会先创建作品。')), list);
    draw();
    m.setActions([
      { label: '上一步', onClick: () => go(4) },
      { label: '完成，开始写作', class: 'primary', onClick: () => busyFinish() },
    ]);
  }

  render();
}
