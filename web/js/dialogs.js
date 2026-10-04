import { api, streamInto } from './api.js';
import { store, emit, reload, chapterLabel } from './store.js';
import { h, toast, modal, field, busy, confirmBox, copyText, download, fmtTime, fmtWords, readFile, pickFile, KIND, CHAR_FIELDS, ROLES, renderMarkdown, cleanAi, countWords, pushLayer } from './ui.js';
import { GENRES } from './wizard.js';
import { openLibrary, openSaveToLibrary } from './stylelib.js';

let genreProfiles = null;
export async function loadGenres() {
  if (!genreProfiles) genreProfiles = await api.get('/genres').catch(() => []);
  return genreProfiles;
}

export function matchGenre(text, profiles) {
  const t = (text || '').trim();
  if (!t || !profiles) return null;
  return profiles.find((g) => g.genre_name === t)
    || profiles.filter((g) => { const core = g.genre_name.replace(/流$/, ''); return core && (t.includes(core) || core.includes(t)); })
      .sort((a, b) => b.genre_name.length - a.genre_name.length)[0]
    || null;
}

function genreInput(value, onChange) {
  const hint = h('div', { class: 'hint genre-hint' });
  const list = h('datalist', { id: 'genre-profiles' });
  const input = h('input', { value, list: 'genre-profiles', placeholder: '从 37 个题材模板里选，也可以自己写', oninput: (e) => { onChange(e.target.value); show(); } });
  const show = () => {
    const g = matchGenre(input.value, genreProfiles);
    hint.textContent = g ? `匹配题材模板「${g.genre_name}」：${g.core_tone.slice(0, 80)}${g.core_tone.length > 80 ? '…' : ''}` : '没有匹配到题材模板，AI 会按通用网文写法处理。';
  };
  loadGenres().then((profiles) => {
    const names = [...new Set([...profiles.map((g) => g.genre_name), ...GENRES])];
    names.forEach((n) => list.append(h('option', { value: n })));
    show();
  });
  return h('div', null, input, list, hint);
}

const PRESETS = [
  ['DeepSeek', 'https://api.deepseek.com/v1'],
  ['Kimi（月之暗面）', 'https://api.moonshot.cn/v1'],
  ['通义千问（阿里云百炼）', 'https://dashscope.aliyuncs.com/compatible-mode/v1'],
  ['智谱 GLM', 'https://open.bigmodel.cn/api/paas/v4'],
  ['豆包（火山方舟）', 'https://ark.cn-beijing.volces.com/api/v3'],
  ['硅基流动', 'https://api.siliconflow.cn/v1'],
  ['OpenRouter', 'https://openrouter.ai/api/v1'],
  ['OpenAI', 'https://api.openai.com/v1'],
  ['Google Gemini', 'https://generativelanguage.googleapis.com/v1beta/openai'],
  ['Ollama（本机）', 'http://127.0.0.1:11434/v1'],
  ['本机 sub2api（8080 端口）', 'http://127.0.0.1:8080/v1'],
  ['自定义', ''],
];

const PLATFORMS = [['fanqie', '番茄小说'], ['qidian', '起点中文网'], ['', '其他']];
const TASK_NAMES = {
  continue: '续写', write_chapter: '写整章', expand: '扩写', shorten: '缩写', rewrite: '改写', polish: '润色', free: '问编辑',
  chat: '角色对话', world: '世界观', outline: '总纲', synopsis: '简介', volume_outline: '卷纲', ideas: '开书方案', characters: '人物设计',
  chapter_outlines: '章纲规划', summarize: '摘要', extract: '提取设定', check: '一致性检查', review: '审稿', volume_summary: '卷摘要',
};

const select = (options, value, onchange) => h('select', { onchange: (e) => onchange(e.target.value) },
  options.map(([v, label]) => h('option', { value: v, selected: String(v) === String(value ?? '') }, label)));

// ---------------- 设置 ----------------

export function openSettings() {
  const s = structuredClone(store.settings || {});
  s.providers = s.providers || [];
  s.writer = s.writer || { provider_id: '', model: '', temperature: 0.85, max_tokens: 8192 };
  s.analyst = s.analyst || { provider_id: '', model: '', temperature: 0.3, max_tokens: 4096 };
  s.extra_cliches = s.extra_cliches || [];
  s.embedding = s.embedding || { provider_id: '', model: '' };
  s.library_refs = s.library_refs ?? 2;
  s.use_style_guide = s.use_style_guide ?? true;
  const prefs = { ghostAuto: localStorage.getItem('ghostAuto') || '0', ghostDelay: localStorage.getItem('ghostDelay') || '2500', ghostWords: localStorage.getItem('ghostWords') || '50' };
  const models = new Set();
  const datalist = h('datalist', { id: 'model-options' });
  const provList = h('div', { class: 'prov-list' });
  const roles = h('div', { class: 'roles' });

  const refreshDatalist = () => {
    datalist.innerHTML = '';
    [...models].sort().forEach((m) => datalist.append(h('option', { value: m })));
  };

  async function test(p, result, btn) {
    const model = [s.writer, s.analyst].find((r) => r.provider_id === p.id && r.model)?.model || '';
    result.innerHTML = '';
    await busy(btn, async () => {
      const r = await api.post('/settings/test', { provider: p, model });
      const line = (ok, text) => result.append(h('div', { class: ok === null ? 'muted' : ok ? 'ok' : 'err' }, (ok === null ? '' : ok ? '✓ ' : '✗ ') + text));
      if (r.models) {
        r.models.forEach((m) => models.add(m));
        refreshDatalist();
        line(true, `获取到 ${r.models.length} 个模型，填写模型时可以直接选`);
      } else {
        line(false, '获取模型列表失败（有些接口不支持，不影响使用）：' + r.models_error);
      }
      if (!model) line(null, '在下面选好模型后再测一次，可以验证对话是否正常。');
      else if (r.chat_ok) line(true, `${model} 回复「${r.reply}」，用时 ${(r.latency_ms / 1000).toFixed(1)} 秒`);
      else line(false, `${model} 调用失败：${r.chat_error}`);
    }, '测试中…');
  }

  function renderProviders() {
    provList.innerHTML = '';
    if (!s.providers.length) provList.append(h('div', { class: 'empty' }, '还没有接口。从下面选一个预设添加，填上 API Key 就能用。'));
    s.providers.forEach((p, i) => {
      const result = h('div', { class: 'test-result' });
      const testBtn = h('button', { class: 'mini', onclick: () => test(p, result, testBtn) }, '测试连接');
      provList.append(h('div', { class: 'prov-card' },
        h('div', { class: 'row' },
          h('input', { value: p.name, placeholder: '名称', class: 'grow', oninput: (e) => { p.name = e.target.value; renderRoles(); } }),
          testBtn,
          h('button', { class: 'mini danger', onclick: () => { s.providers.splice(i, 1); renderProviders(); renderRoles(); } }, '删除')),
        h('div', { class: 'grid2' },
          field('接口地址', h('input', { value: p.base_url, placeholder: 'https://api.example.com/v1', oninput: (e) => { p.base_url = e.target.value; } }), '填到 /v1 这一级'),
          field('API Key', h('input', { value: p.api_key, type: 'password', autocomplete: 'off', placeholder: 'sk-...（本地模型可留空）', oninput: (e) => { p.api_key = e.target.value; } }))),
        result));
    });
  }

  function roleCard(key, label, hint) {
    const r = s[key];
    if (!s.providers.some((p) => p.id === r.provider_id)) r.provider_id = s.providers[0]?.id || '';
    return h('div', { class: 'role-card' },
      h('div', { class: 'role-head' }, h('b', null, label), h('span', { class: 'hint' }, hint)),
      h('div', { class: 'grid4' },
        field('接口', select(s.providers.map((p) => [p.id, p.name || p.base_url || '未命名接口']), r.provider_id, (v) => { r.provider_id = v; })),
        field('模型', h('input', { value: r.model, list: 'model-options', placeholder: '如 deepseek-chat', oninput: (e) => { r.model = e.target.value.trim(); } })),
        field('温度', h('input', { type: 'number', step: 0.05, min: 0, max: 2, value: r.temperature, oninput: (e) => { r.temperature = Number(e.target.value); } })),
        field('最大输出', h('input', { type: 'number', min: 0, step: 512, value: r.max_tokens, oninput: (e) => { r.max_tokens = Number(e.target.value); } }), '0 = 不限制')),
      h('div', { class: 'grid4' },
        field('输入单价', h('input', { type: 'number', min: 0, step: 0.1, value: r.input_price || 0, oninput: (e) => { r.input_price = Number(e.target.value); } }), '元 / 百万 token'),
        field('输出单价', h('input', { type: 'number', min: 0, step: 0.1, value: r.output_price || 0, oninput: (e) => { r.output_price = Number(e.target.value); } }), '填了才会在统计里算费用')));
  }

  function renderRoles() {
    roles.innerHTML = '';
    roles.append(
      roleCard('writer', '写作模型', '写正文、续写、润色、开书策划，选文笔最好的。'),
      roleCard('analyst', '分析模型', '摘要、提取设定、一致性检查、审稿；可以选便宜快速的，不填就用写作模型。'));
  }

  const preset = select(PRESETS.map(([name], i) => [i, name]), 0, () => {});
  const addBtn = h('button', {
    class: 'btn',
    onclick: () => {
      const [name, url] = PRESETS[Number(preset.value)];
      s.providers.push({ id: `p${Date.now()}`, name: name === '自定义' ? '' : name, base_url: url, api_key: '' });
      renderProviders();
      renderRoles();
    },
  }, '＋ 添加接口');

  const cliches = h('textarea', { rows: 3, value: s.extra_cliches.join('\n'), placeholder: '一行一个，比如你自己容易写多的口头禅' });
  const body = h('div', { class: 'settings' },
    h('h4', null, '模型接口'),
    h('p', { class: 'hint' }, '支持所有 OpenAI 兼容接口。密钥只保存在本机的数据库里。'),
    provList,
    h('div', { class: 'row' }, preset, addBtn),
    h('h4', null, '模型分工'),
    datalist,
    roles,
    h('h4', null, '写作与检查'),
    h('div', { class: 'grid4' },
      field('上下文预算（字）', h('input', { type: 'number', min: 3000, step: 1000, value: s.context_budget, oninput: (e) => { s.context_budget = Number(e.target.value); } }), '每次生成时注入的设定和前情上限'),
      field('单章最少字数', h('input', { type: 'number', min: 0, step: 100, value: s.min_chapter_words, oninput: (e) => { s.min_chapter_words = Number(e.target.value); } }), '投稿检查用'),
      field('单章最多字数', h('input', { type: 'number', min: 0, step: 100, value: s.max_chapter_words, oninput: (e) => { s.max_chapter_words = Number(e.target.value); } })),
      field('JSON 模式', h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: s.json_mode, onchange: (e) => { s.json_mode = e.target.checked; } }), '接口支持 response_format 时开启'))),
    field('自定义禁用词', cliches, '文字质量检查时会额外标出这些词'),
    h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: !!s.stream_usage, onchange: (e) => { s.stream_usage = e.target.checked; } }), '流式输出时请求真实 token 用量（OpenAI、DeepSeek 等支持；接口报错就关掉，改用字数估算）'),
    h('h4', null, '文风库'),
    h('p', { class: 'hint' }, '写正文时（续写、写整章、润色、灰字、斜杠指令）自动带上文风指南，并从文风库里按场景检索几段范文给 AI 参考。'),
    h('div', { class: 'grid4' },
      field('每次参考范文', select([[0, '不参考'], [1, '1 段'], [2, '2 段'], [3, '3 段'], [4, '4 段']], s.library_refs, (v) => { s.library_refs = Number(v); }), '段数越多越像范文，也越费 token'),
      field('文风指南', h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: s.use_style_guide, onchange: (e) => { s.use_style_guide = e.target.checked; } }), '写作时带上')),
      field('向量接口（可选）', select([['', '不用向量检索'], ...s.providers.map((p) => [p.id, p.name || p.base_url || '未命名接口'])], s.embedding.model ? s.embedding.provider_id : '', (v) => { s.embedding.provider_id = v; if (!v) s.embedding.model = ''; })),
      field('向量模型', h('input', { value: s.embedding.model, placeholder: '如 BAAI/bge-m3', oninput: (e) => { s.embedding.model = e.target.value.trim(); if (s.embedding.model && !s.embedding.provider_id) s.embedding.provider_id = s.providers[0]?.id || ''; } }), '不填就只用关键词和标签检索')),
    h('div', { class: 'row' }, h('button', { class: 'btn', onclick: () => openLibrary() }, '打开文风库…'), h('span', { class: 'hint' }, '需要接口支持 /v1/embeddings（硅基流动、Ollama 等）；填好后在文风库里点「生成向量」。')),
    h('h4', null, '编辑器'),
    h('div', { class: 'grid4' },
      field('自动灰字续写', select([['0', '关闭（按 Ctrl+Enter 才出）'], ['1', '停顿后自动出']], prefs.ghostAuto, (v) => { prefs.ghostAuto = v; }), '自动模式每次停顿都会调用模型'),
      field('停顿多久', select([['1500', '1.5 秒'], ['2500', '2.5 秒'], ['4000', '4 秒'], ['6000', '6 秒']], prefs.ghostDelay, (v) => { prefs.ghostDelay = v; })),
      field('灰字长度', select([['30', '短：一句话'], ['50', '中：一两句'], ['120', '长：一小段']], prefs.ghostWords, (v) => { prefs.ghostWords = v; }))),
    h('h4', null, '提示词'),
    h('div', { class: 'row' },
      h('button', { class: 'btn', onclick: () => openPrompts() }, '编辑提示词模板…'),
      h('span', { class: 'hint' }, '所有 AI 任务的提示词都能改，改坏了可以一键恢复默认。')));

  renderProviders();
  renderRoles();
  modal({
    title: '设置',
    body,
    wide: 'xl',
    actions: [
      { label: '取消', onClick: (c) => c() },
      {
        label: '保存',
        class: 'primary',
        onClick: async (close) => {
          s.extra_cliches = cliches.value.split('\n').map((x) => x.trim()).filter(Boolean);
          Object.entries(prefs).forEach(([k, v]) => localStorage.setItem(k, v));
          try {
            store.settings = await api.put('/settings', s);
            emit('settings-changed');
            toast('设置已保存');
            close();
          } catch (e) {
            toast(e.message, 'error');
          }
        },
      },
    ],
  });
}

// ---------------- 作品设定 ----------------

export function openBookSettings() {
  const b = structuredClone(store.book);
  const fields = () => ({ title: b.title, genre: b.genre, platform: b.platform, logline: b.logline, worldview: b.worldview, outline: b.outline });
  const text = (key, rows, placeholder) => h('textarea', { rows, value: b[key], placeholder, oninput: (e) => { b[key] = e.target.value; } });
  const synopsis = text('synopsis', 5, '平台展示用的作品简介');
  const world = text('worldview', 18, '世界观：时代地图、力量体系、势力、金手指规则……');
  const outline = text('outline', 18, '总纲：主线、结局、分卷安排……');
  const genBtn = (label, task, target) => {
    const btn = h('button', { class: 'btn', onclick: () => streamInto(target, { task, book_id: b.id, fields: fields() }, btn) }, label);
    return btn;
  };
  const pages = {
    basic: h('div', null,
      h('div', { class: 'grid2' },
        field('书名', h('input', { value: b.title, oninput: (e) => { b.title = e.target.value; } })),
        field('题材', genreInput(b.genre, (v) => { b.genre = v; }))),
      h('div', { class: 'grid2' },
        field('目标平台', select(PLATFORMS, b.platform, (v) => { b.platform = v; })),
        field('单章目标字数', h('input', { type: 'number', min: 500, step: 100, value: b.target_words, oninput: (e) => { b.target_words = Number(e.target.value); } }), '写整章时的默认长度')),
      field('一句话梗概', text('logline', 2, '主角是谁、想要什么、最大的阻碍')),
      field('作品简介', h('div', null, synopsis, genBtn('AI 写 3 版简介', 'synopsis', synopsis)))),
    world: h('div', null, h('div', { class: 'row' }, genBtn('AI 生成世界观', 'world', world), h('span', { class: 'hint' }, '世界观会注入每次写作的上下文')), world),
    outline: h('div', null, h('div', { class: 'row' }, genBtn('AI 生成总纲', 'outline', outline), h('span', { class: 'hint' }, '总纲决定全书走向')), outline),
    style: (() => {
      const guide = text('style_guide', 6, '比如：第三人称限知视角；语言简洁口语化，少用成语；对话占一半以上；每章至少一个爽点。');
      const sample = text('style_sample', 12, '贴一段你自己写得满意的文字（或你想要的风格），AI 会模仿它的句式和节奏。');
      const extract = h('button', {
        class: 'btn',
        onclick: () => {
          if ((b.style_sample || '').trim().length < 200) return toast('先在下面贴一段至少 200 字的样章', 'warn');
          streamInto(guide, { task: 'style_profile', book_id: b.id, selection: b.style_sample }, extract);
        },
      }, '从样章提取文风');
      return h('div', null,
        field('文风要求', h('div', null, guide, h('div', { class: 'row' }, extract, h('span', { class: 'hint' }, '分析样章的视角、句长、对话比例和用词，写成文风要求'))), '会加进每次写作的系统提示'),
        field('样章（模仿语感）', sample, '建议 500-1500 字'));
    })(),
  };
  const tabs = [['basic', '基本信息'], ['world', '世界观'], ['outline', '总纲'], ['style', '文风']];
  const content = h('div', { class: 'tab-content' }, pages.basic);
  const bar = h('div', { class: 'seg' }, tabs.map(([k, label], i) => h('button', {
    class: 'seg-btn' + (i === 0 ? ' on' : ''),
    onclick: (e) => { bar.querySelectorAll('.seg-btn').forEach((x) => x.classList.remove('on')); e.currentTarget.classList.add('on'); content.innerHTML = ''; content.append(pages[k]); },
  }, label)));
  modal({
    title: '作品设定',
    body: h('div', null, bar, content),
    wide: 'xl',
    actions: [
      {
        label: '删除作品',
        class: 'danger left',
        onClick: async (close) => {
          if (!(await confirmBox(`确定删除《${store.book.title}》？所有内容都会删除，无法恢复。`, { okText: '删除', danger: true }))) return;
          await api.del(`/books/${b.id}`);
          close();
          emit('go-shelf');
          toast('已删除');
        },
      },
      { label: '取消', onClick: (c) => c() },
      {
        label: '保存',
        class: 'primary',
        onClick: async (close) => {
          try {
            store.book = await api.patch(`/books/${b.id}`, b);
            emit('book-changed');
            toast('已保存');
            close();
          } catch (e) {
            toast(e.message, 'error');
          }
        },
      },
    ],
  });
}

// ---------------- 卷 ----------------

export function openVolume(vol) {
  const v = structuredClone(vol);
  const outline = h('textarea', { rows: 10, value: v.outline, placeholder: '卷纲：本卷起止状态、关键事件、新人物、伏笔、高潮', oninput: (e) => { v.outline = e.target.value; } });
  const summary = h('textarea', { rows: 6, value: v.summary, placeholder: '卷摘要：本卷写完后生成，后面的卷会用它回顾前情', oninput: (e) => { v.summary = e.target.value; } });
  const genOutline = h('button', { class: 'btn', onclick: () => streamInto(outline, { task: 'volume_outline', book_id: store.book.id, volume_id: v.id }, genOutline) }, 'AI 细化卷纲');
  const genSummary = h('button', {
    class: 'btn',
    onclick: (e) => busy(e.currentTarget, async () => {
      const r = await api.post('/ai/json', { task: 'volume_summary', book_id: store.book.id, volume_id: v.id });
      summary.value = r.text;
      v.summary = r.text;
    }, '生成中…'),
  }, 'AI 生成卷摘要');
  modal({
    title: '编辑卷',
    wide: true,
    body: h('div', null,
      field('卷名', h('input', { value: v.title, oninput: (e) => { v.title = e.target.value; } })),
      field('卷纲', h('div', null, outline, genOutline)),
      field('卷摘要', h('div', null, summary, genSummary), '本卷章节都定稿后再生成效果最好')),
    actions: [
      {
        label: '删除这一卷',
        class: 'danger left',
        onClick: async (close) => {
          if (!(await confirmBox('删除这一卷？卷里的章节会保留，变成“不分卷”。', { okText: '删除', danger: true }))) return;
          await api.del(`/volumes/${v.id}`);
          await reload(['volumes', 'chapters']);
          emit('chapters-changed');
          close();
        },
      },
      { label: '取消', onClick: (c) => c() },
      {
        label: '保存',
        class: 'primary',
        onClick: async (close) => {
          await api.patch(`/volumes/${v.id}`, v);
          await reload(['volumes']);
          emit('chapters-changed');
          toast('已保存');
          close();
        },
      },
    ],
  });
}

// ---------------- 导出 ----------------

export function openExport() {
  const n = store.chapters.length;
  const o = { format: 'fanqie', from: 1, to: n, indent: true, blank_line: true, numerals: '' };
  const options = h('div', { class: 'grid4' },
    field('起始章节序号', h('input', { type: 'number', min: 1, max: n, value: 1, oninput: (e) => { o.from = Number(e.target.value); } })),
    field('结束章节序号', h('input', { type: 'number', min: 1, max: n, value: n, oninput: (e) => { o.to = Number(e.target.value); } })),
    field('段首缩进', h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.indent = e.target.checked; } }), '两个全角空格')),
    field('段间空行', h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.blank_line = e.target.checked; } }), '段落之间空一行')));
  const formats = [['fanqie', '番茄投稿 TXT', '第1章 标题'], ['qidian', '起点投稿 TXT', '第一章 标题'], ['txt', '普通 TXT', ''], ['md', 'Markdown', ''], ['epub', 'EPUB 电子书', '手机阅读器可直接打开']];
  const fmtBar = h('div', { class: 'format-list' }, formats.map(([v, label, hint]) => h('label', { class: 'format-item' },
    h('input', { type: 'radio', name: 'fmt', value: v, checked: v === o.format, onchange: () => { o.format = v; } }),
    h('b', null, label), hint ? h('span', { class: 'muted small' }, hint) : null)));
  const query = (extra = {}) => {
    const p = new URLSearchParams({ format: o.format, indent: o.indent, blank_line: o.blank_line, ...extra });
    if (o.numerals) p.set('numerals', o.numerals);
    return p.toString();
  };
  modal({
    title: '导出',
    wide: true,
    body: h('div', null,
      fmtBar,
      options,
      h('p', { class: 'hint' }, '投稿前建议先在右侧「检查 → 全书投稿检查」里过一遍：空章、字数、半角标点、残留的 AI 回复语都会标出来。番茄要求如实声明 AI 使用情况，可以在「统计」里查看每章采纳的 AI 字数。')),
    actions: [
      {
        label: '复制当前章（投稿格式）',
        onClick: async () => {
          if (!store.chapter) return toast('请先打开一个章节', 'warn');
          await store.editor.save();
          const fmt = o.format === 'qidian' ? 'qidian' : 'fanqie';
          const blob = await api.blob(`/books/${store.book.id}/export?${new URLSearchParams({ format: fmt, chapter_id: store.chapter.id, indent: o.indent, blank_line: o.blank_line })}`);
          copyText(await blob.text());
        },
      },
      {
        label: '导出文件',
        class: 'primary',
        onClick: async (close) => {
          try {
            await store.editor?.save();
            const blob = await api.blob(`/books/${store.book.id}/export?${query({ from: o.from, to: o.to })}`);
            const ext = { epub: 'epub', md: 'md' }[o.format] || 'txt';
            download(blob, `${store.book.title}.${ext}`);
            close();
          } catch (e) {
            toast(e.message, 'error');
          }
        },
      },
    ],
  });
}

// ---------------- 历史版本 ----------------

export async function openVersions() {
  const ch = store.chapter;
  if (!ch) return;
  await store.editor.save().catch(() => {});
  const list = await api.get(`/chapters/${ch.id}/versions`);
  const preview = h('textarea', { class: 'preview', readOnly: true, placeholder: '点左侧的版本查看内容' });
  let chosen = null;
  const items = h('div', { class: 'version-list' });
  if (!list.length) items.append(h('div', { class: 'empty' }, '还没有历史版本。AI 修改正文、恢复旧版本、手动点「快照」时都会自动保存。'));
  for (const v of list) {
    items.append(h('div', {
      class: 'version-item',
      onclick: async (e) => {
        items.querySelectorAll('.version-item').forEach((x) => x.classList.remove('on'));
        e.currentTarget.classList.add('on');
        chosen = await api.get(`/versions/${v.id}`);
        preview.value = chosen.content;
      },
    }, h('b', null, fmtTime(v.created_at)), h('div', { class: 'small' }, v.note), h('div', { class: 'muted small' }, `${v.word_count} 字`)));
  }
  modal({
    title: `历史版本 · ${chapterLabel(store.chapters.find((c) => c.id === ch.id))}`,
    wide: 'xl',
    body: h('div', { class: 'versions' }, items, preview),
    actions: [
      { label: '关闭', onClick: (c) => c() },
      {
        label: '恢复这个版本',
        class: 'primary',
        onClick: async (close) => {
          if (!chosen) return toast('先选一个版本', 'warn');
          if (!(await confirmBox('用这个版本替换当前正文？当前内容会先自动存一个快照。'))) return;
          await api.post(`/versions/${chosen.id}/restore`);
          await store.editor.reloadChapter();
          close();
          toast('已恢复');
        },
      },
    ],
  });
}

// ---------------- 统计 ----------------

export async function openStats() {
  const s = await api.get(`/books/${store.book.id}/stats`);
  const byDay = new Map(s.daily.map((d) => [d.day, d.words]));
  const days = [];
  for (let i = 29; i >= 0; i--) {
    const d = new Date(Date.now() - i * 86400000);
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    days.push([key, byDay.get(key) || 0]);
  }
  const max = Math.max(1, ...days.map(([, w]) => w));
  const total30 = days.reduce((a, [, w]) => a + w, 0);
  const card = (label, value, sub) => h('div', { class: 'stat big' }, h('div', { class: 'stat-value' }, value), h('div', { class: 'stat-label' }, label), sub ? h('div', { class: 'muted small' }, sub) : null);
  modal({
    title: '写作统计',
    wide: 'xl',
    body: h('div', { class: 'stats' },
      h('div', { class: 'stats-grid four' },
        card('总字数', fmtWords(s.words)),
        card('章节', s.chapters, `已定稿 ${s.done} 章`),
        card('近 30 天日均', Math.round(total30 / 30), `共 ${fmtWords(total30)} 字`),
        card('采纳的 AI 字数', fmtWords(s.ai_chars), `约占正文 ${(s.ai_ratio * 100).toFixed(1)}%`)),
      h('h4', null, '近 30 天码字'),
      h('div', { class: 'day-chart' }, days.map(([day, w]) => h('div', { class: 'day-col', title: `${day}：${w} 字` }, h('div', { class: 'day-bar', style: { height: (w / max) * 100 + '%' } })))),
      h('h4', null, '张力曲线'),
      s.curve.length
        ? h('div', null,
          h('div', { class: 'tension-chart' }, s.curve.map((c) => {
            const t = Number(c.tension) || 0;
            const tip = `${chapterLabel(c)}：张力 ${t}${c.emotion ? ' · ' + c.emotion : ''}\n钩子：${c.hook || '无'}\n爽点：${(c.cool_points || []).map((p) => p.type).join('、') || '无'}`;
            return h('div', { class: 'tension-col', title: tip, onclick: () => store.editor?.open(c.id) },
              h('div', { class: 'tension-bar ' + (t >= 7 ? 'good' : t >= 4 ? 'mid' : 'bad'), style: { height: t * 10 + '%' } }),
              h('div', { class: 'tension-label' }, c.number ?? '·'));
          })),
          h('p', { class: 'hint' }, '每根柱子是一章，定稿时自动打分；连续几章偏低（黄、红）容易流失读者。点击柱子跳到对应章节。'))
        : h('div', { class: 'empty' }, '还没有数据。章节定稿时会顺带分析张力、爽点和章末钩子。'),
      h('h4', null, 'AI 使用记录'),
      h('p', { class: 'small' },
        `累计 token：输入 ${fmtWords(s.tokens.prompt)}，输出 ${fmtWords(s.tokens.completion)}${s.tokens.estimated ? '（部分按字数估算）' : ''}`,
        s.cost > 0 ? ` · 费用约 ¥${s.cost}` : ' · 在设置里填写模型单价后可以计算费用'),
      h('p', { class: 'hint' }, '番茄小说要求作者如实勾选 AI 使用情况，起点反对用 AI 替代真人完成核心创作。下面记录了每次调用 AI 的情况，以及你把 AI 生成内容放进正文时的字数（按采纳那一刻统计，之后的修改不计入），可以作为如实声明的依据。'),
      s.ai_usage.length
        ? h('table', { class: 'table' },
          h('tr', null, h('th', null, '用途'), h('th', null, '次数'), h('th', null, '发送字数'), h('th', null, '生成字数')),
          s.ai_usage.map((u) => h('tr', null, h('td', null, TASK_NAMES[u.task] || u.task), h('td', null, u.count), h('td', null, fmtWords(u.input_chars)), h('td', null, fmtWords(u.output_chars)))))
        : h('div', { class: 'empty' }, '还没有调用过 AI'),
      s.ai_chapters.length ? h('h4', null, '各章采纳的 AI 字数') : null,
      s.ai_chapters.length
        ? h('table', { class: 'table' },
          h('tr', null, h('th', null, '章节'), h('th', null, '正文字数'), h('th', null, '采纳 AI 字数'), h('th', null, '占比')),
          s.ai_chapters.map((c) => h('tr', null,
            h('td', null, chapterLabel(c)), h('td', null, c.words), h('td', null, c.ai_chars),
            h('td', null, c.words ? Math.min(100, (c.ai_chars / c.words) * 100).toFixed(0) + '%' : '—'))))
        : null),
    actions: [{ label: '关闭', onClick: (c) => c() }],
  });
}

// ---------------- 章纲规划 ----------------

export function openPlanner() {
  const lastVol = store.chapters.length ? store.chapters[store.chapters.length - 1].volume_id : store.volumes[store.volumes.length - 1]?.id;
  const o = { count: 10, volume_id: lastVol || '', instruction: '' };
  let planned = [];
  const list = h('div', { class: 'outline-list' });
  const draw = () => {
    list.innerHTML = '';
    if (!planned.length) list.append(h('div', { class: 'empty' }, 'AI 会参考总纲、卷纲、前情和未回收的伏笔，接着最后一章往下规划。'));
    planned.forEach((p) => list.append(h('div', { class: 'outline-item' },
      h('input', { type: 'checkbox', checked: p.checked, onchange: (e) => { p.checked = e.target.checked; } }),
      h('div', { class: 'grow' },
        h('input', { value: p.title, oninput: (e) => { p.title = e.target.value; } }),
        h('textarea', { rows: 2, value: p.outline, oninput: (e) => { p.outline = e.target.value; } })))));
  };
  const genBtn = h('button', {
    class: 'btn primary',
    onclick: () => busy(genBtn, async () => {
      const r = await api.post('/ai/json', { task: 'chapter_outlines', book_id: store.book.id, volume_id: o.volume_id ? Number(o.volume_id) : null, count: o.count, instruction: o.instruction });
      planned = (r.chapters || []).map((c) => ({ ...c, checked: true }));
      if (!planned.length) throw new Error('模型没有返回章纲，请重试');
      draw();
    }, '规划中…'),
  }, '生成章纲');
  const simOut = h('div', { class: 'out-text answer', hidden: true });
  const simBtn = h('button', {
    class: 'btn',
    onclick: () => busy(simBtn, async () => {
      const r = await api.post('/ai/json', { task: 'simulate', book_id: store.book.id, instruction: o.instruction });
      simOut.hidden = false;
      simOut.textContent = r.text;
      simOut.append(h('div', { class: 'row' }, h('button', { class: 'mini', onclick: () => copyText(r.text) }, '复制推演结果'), h('span', { class: 'hint' }, '可以把喜欢的走向贴进「补充要求」再生成章纲')));
    }, '推演中…'),
  }, '人物推演');
  modal({
    title: 'AI 规划后续章纲',
    wide: 'xl',
    body: h('div', null,
      h('div', { class: 'grid4' },
        field('规划章数', h('input', { type: 'number', min: 1, max: 50, value: o.count, oninput: (e) => { o.count = Number(e.target.value); } })),
        field('放进哪一卷', select([['', '不分卷'], ...store.volumes.map((v) => [v.id, v.title])], o.volume_id, (v) => { o.volume_id = v; })),
        field('补充要求', h('input', { placeholder: '比如：这几章安排主角进城，结尾和反派第一次交手', oninput: (e) => { o.instruction = e.target.value; } }))),
      h('div', { class: 'row' }, genBtn, simBtn, h('span', { class: 'hint' }, '不知道接下来写什么？先让主要人物按各自的目标推演一下。')),
      simOut,
      list),
    actions: [
      { label: '取消', onClick: (c) => c() },
      {
        label: '添加选中的章节',
        class: 'primary',
        onClick: async (close) => {
          const chosen = planned.filter((p) => p.checked);
          if (!chosen.length) return toast('没有选中的章纲', 'warn');
          let firstId = null;
          for (const p of chosen) {
            const ch = await api.post(`/books/${store.book.id}/chapters`, { title: p.title, outline: p.outline, volume_id: o.volume_id ? Number(o.volume_id) : null });
            firstId = firstId || ch.id;
          }
          await reload(['chapters']);
          emit('chapters-changed');
          close();
          toast(`已添加 ${chosen.length} 章`);
          if (firstId) store.editor.open(firstId);
        },
      },
    ],
  });
}

// ---------------- 定稿 ----------------

export async function openFinalize() {
  const ch = store.chapter;
  if (!ch) return;
  if (!store.editor.text().trim()) return toast('这一章还没有正文', 'warn');
  try { await store.editor.save(); } catch { return; }
  const m = modal({ title: '定稿本章', wide: 'xl', body: h('div', { class: 'boot' }, 'AI 正在阅读本章：生成摘要、找出人物状态变化和新伏笔……') });
  let summary;
  let ext;
  let tension = null;
  try {
    [summary, ext, tension] = await Promise.all([
      api.post('/ai/json', { task: 'summarize', book_id: store.book.id, chapter_id: ch.id }),
      api.post('/ai/json', { task: 'extract', book_id: store.book.id, chapter_id: ch.id }),
      api.post('/ai/json', { task: 'tension', book_id: store.book.id, chapter_id: ch.id }).catch(() => null),
    ]);
  } catch (e) {
    m.body.innerHTML = '';
    m.body.append(h('div', { class: 'empty' }, '出错了：' + e.message));
    m.setActions([{ label: '关闭', onClick: (c) => c() }]);
    return;
  }
  store.chapter.summary = summary.text;
  const entries = (ext.entries || []).filter((e) => e.name).map((e) => ({ ...e, checked: true }));
  const threadsNew = (ext.threads_new || []).filter((t) => t.title).map((t) => ({ ...t, checked: true }));
  const threadsResolved = (ext.threads_resolved || []).filter((t) => store.threads.some((x) => x.id === t.id && x.status === 'open')).map((t) => ({ ...t, checked: true }));
  const threadsProgressed = (ext.threads_progressed || []).filter((t) => store.threads.some((x) => x.id === t.id && x.status === 'open'));
  const existing = (name) => store.entries.find((e) => e.name === name || (e.aliases || '').split(/[,，、\s]+/).includes(name));

  const summaryBox = h('textarea', { rows: 5, value: summary.text });
  const entryRows = entries.map((e) => {
    const old = existing(e.name);
    return h('div', { class: 'change-item' },
      h('input', { type: 'checkbox', checked: true, onchange: (ev) => { e.checked = ev.target.checked; } }),
      h('div', { class: 'grow' },
        h('div', { class: 'entry-head' }, h('span', { class: 'badge ' + (old ? '' : 'green') }, old ? '更新' : '新增'), h('span', { class: 'tag k-' + (old?.kind || e.kind) }, KIND[old?.kind || e.kind] || '其他'), h('b', null, e.name)),
        old?.state ? h('div', { class: 'muted small' }, '原状态：' + old.state) : null,
        !old && e.description ? h('textarea', { rows: 2, value: e.description, oninput: (ev) => { e.description = ev.target.value; } }) : null,
        stateEditor(e)));
  });
  function stateEditor(e) {
    const f = e.fields || {};
    const keys = CHAR_FIELDS.filter(([k]) => (f[k] || '').trim());
    if (!keys.length) return h('input', { value: e.state || '', placeholder: '最新状态', oninput: (ev) => { e.state = ev.target.value; } });
    return h('div', { class: 'grid2' }, keys.map(([k, label]) => h('label', { class: 'inline-field' }, h('span', null, label), h('input', { value: f[k], oninput: (ev) => { f[k] = ev.target.value; } }))));
  }
  const tensionBox = tension && typeof tension.tension === 'number'
    ? h('div', { class: 'tension-box' },
      h('span', { class: 'score ' + (tension.tension >= 7 ? 'good' : tension.tension >= 4 ? 'mid' : 'bad') }, tension.tension),
      h('div', null,
        h('div', null, h('b', null, '张力 '), `${tension.emotion || ''}${tension.comment ? '：' + tension.comment : ''}`),
        h('div', { class: 'small' }, '爽点：', (tension.cool_points || []).length ? tension.cool_points.map((p) => `${p.type}（${p.level}）`).join('、') : '无'),
        h('div', { class: 'small' }, '章末钩子：', tension.hook?.type || '无', tension.hook?.desc ? ` · ${tension.hook.desc}` : '')))
    : null;
  const threadRows = threadsNew.map((t) => h('div', { class: 'change-item' },
    h('input', { type: 'checkbox', checked: true, onchange: (ev) => { t.checked = ev.target.checked; } }),
    h('div', { class: 'grow' },
      h('div', { class: 'entry-head' }, h('span', { class: 'badge green' }, '新伏笔')),
      h('input', { value: t.title, oninput: (ev) => { t.title = ev.target.value; } }),
      h('textarea', { rows: 2, value: t.detail || '', oninput: (ev) => { t.detail = ev.target.value; } }))));
  const resolvedRows = threadsResolved.map((t) => h('div', { class: 'change-item' },
    h('input', { type: 'checkbox', checked: true, onchange: (ev) => { t.checked = ev.target.checked; } }),
    h('div', { class: 'grow' },
      h('div', { class: 'entry-head' }, h('span', { class: 'badge' }, '回收'), h('b', null, store.threads.find((x) => x.id === t.id)?.title)),
      t.note ? h('div', { class: 'small' }, t.note) : null)));

  m.body.innerHTML = '';
  m.body.append(h('div', null,
    h('p', { class: 'hint' }, '核对 AI 找出的变化，取消勾选不准确的，也可以直接修改，然后点「应用」。'),
    tensionBox,
    field('本章摘要（已保存）', summaryBox),
    h('h4', null, `设定变化（${entries.length}）`),
    entries.length ? entryRows : h('div', { class: 'empty' }, '没有发现设定变化'),
    h('h4', null, `伏笔（新埋 ${threadsNew.length}，回收 ${threadsResolved.length}）`),
    threadsNew.length || threadsResolved.length ? [threadRows, resolvedRows] : h('div', { class: 'empty' }, '没有发现新伏笔或回收')));
  m.setActions([
    { label: '取消', onClick: (c) => c() },
    {
      label: '应用并标记为定稿',
      class: 'primary',
      onClick: async (close) => {
        try {
          if (summaryBox.value !== summary.text) {
            await api.patch(`/chapters/${ch.id}`, { summary: summaryBox.value });
            store.chapter.summary = summaryBox.value;
          }
          const r = await api.post(`/books/${store.book.id}/apply_updates`, {
            chapter_id: ch.id,
            entries: entries.filter((e) => e.checked),
            threads_new: threadsNew.filter((t) => t.checked),
            threads_progressed: threadsProgressed,
            threads_resolved: threadsResolved.filter((t) => t.checked),
            relations: ext.relations || [],
          });
          await store.editor.setStatus('done');
          await reload(['entries', 'threads', 'chapters']);
          emit('entries-changed');
          emit('threads-changed');
          emit('chapters-changed');
          emit('chapter-loaded', store.chapter);
          close();
          toast(`已定稿：新增设定 ${r.created}，更新 ${r.updated}，新伏笔 ${r.threads_added}，回收 ${r.threads_resolved}`, 'info', 4500);
        } catch (e) {
          toast(e.message, 'error');
        }
      },
    },
  ]);
}

// ---------------- 提示词模板、预览 ----------------

export async function openPrompts() {
  const r = await api.get('/prompts');
  const templates = r.templates;
  let current = templates[0];
  const list = h('div', { class: 'prompt-list' });
  const editor = h('textarea', { class: 'prompt-editor', spellcheck: false });
  const info = h('div', { class: 'prompt-info' });
  const insert = (text) => {
    const s = editor.selectionStart;
    editor.setRangeText(text, s, editor.selectionEnd, 'end');
    editor.focus();
  };
  const drawList = () => {
    list.innerHTML = '';
    let group = '';
    for (const t of templates) {
      if (t.group !== group) { group = t.group; list.append(h('div', { class: 'group-title' }, group)); }
      list.append(h('div', { class: 'prompt-item' + (t === current ? ' on' : ''), onclick: () => choose(t) }, t.name, t.custom ? h('span', { class: 'badge' }, '已修改') : null));
    }
  };
  const drawInfo = () => {
    info.innerHTML = '';
    info.append(
      h('div', null, h('b', null, current.name), ` · ${current.description}`),
      current.vars.length ? h('div', { class: 'var-list' }, '变量（点击插入）：', current.vars.map((v) => h('button', { class: 'chip', title: v.desc, onclick: () => insert(`{{${v.name}}}`) }, v.name))) : '',
      h('div', { class: 'var-list' }, '写作手册（点击插入全文）：', r.craft.map((c) => h('button', { class: 'chip', title: `插入「${c.title}」全文，会明显增加字数`, onclick: () => insert(`{{craft:${c.id}}}`) }, c.title))));
  };
  const choose = (t) => {
    if (current && editor.value !== current.text && !confirm('当前模板有未保存的修改，放弃修改吗？')) return;
    current = t;
    editor.value = t.text;
    drawList();
    drawInfo();
  };
  current = null;
  choose(templates[0]);
  modal({
    title: '提示词模板',
    wide: 'xl',
    body: h('div', { class: 'prompts' }, list, h('div', { class: 'prompt-main' }, info, editor)),
    actions: [
      {
        label: '恢复默认',
        class: 'left',
        onClick: async () => {
          await api.put(`/prompts/${current.id}`, { text: '' });
          current.text = current.default;
          current.custom = false;
          editor.value = current.default;
          drawList();
          toast('已恢复默认');
        },
      },
      { label: '关闭', onClick: (c) => { if (editor.value !== current.text && !confirm('有未保存的修改，确定关闭吗？')) return; c(); } },
      {
        label: '保存',
        class: 'primary',
        onClick: async () => {
          const res = await api.put(`/prompts/${current.id}`, { text: editor.value });
          current.text = editor.value;
          current.custom = res.custom;
          drawList();
          toast(res.custom ? '已保存，之后的 AI 调用会用这个版本' : '和默认一样，已按默认处理');
        },
      },
    ],
  });
}

export async function openPreview(body) {
  let r;
  try { r = await api.post('/ai/preview', body); } catch (e) { toast(e.message, 'error'); return; }
  const roleName = { system: '系统提示', user: '用户消息', assistant: '助手消息' };
  modal({
    title: `将要发送的提示词 · ${r.chars} 字（约 ${r.estimated_tokens} token）`,
    wide: 'xl',
    body: h('div', { class: 'context-view' },
      h('p', { class: 'hint' }, `模型：${r.model}。内容可以在「设置 → 编辑提示词模板」里调整。`),
      r.messages.map((m) => h('details', { open: true }, h('summary', null, `${roleName[m.role] || m.role}（${m.content.length} 字）`), h('pre', null, m.content)))),
    actions: [
      { label: '复制全部', onClick: () => copyText(r.messages.map((m) => `【${roleName[m.role] || m.role}】\n${m.content}`).join('\n\n')) },
      { label: '关闭', onClick: (c) => c() },
    ],
  });
}

// ---------------- 写作手册 ----------------

export async function openHandbook(id) {
  const docs = await api.get('/craft');
  const content = h('div', { class: 'handbook-content' });
  const nav = h('div', { class: 'handbook-nav' });
  const show = async (docId) => {
    const r = await api.get(`/craft/${docId}`);
    nav.querySelectorAll('.handbook-item').forEach((b) => b.classList.toggle('on', b.dataset.id === docId));
    content.innerHTML = renderMarkdown(r.text);
    content.prepend(h('p', { class: 'hint' }, `来源：${r.meta.source}（${r.meta.license}），详见 THIRD_PARTY_NOTICES.md。在提示词模板里写 {{craft:${docId}}} 可以把这篇手册放进提示词。`));
    content.scrollTop = 0;
  };
  docs.forEach((d) => nav.append(h('button', { class: 'handbook-item', dataset: { id: d.id }, onclick: () => show(d.id) }, d.title, h('span', { class: 'muted small' }, d.source))));
  modal({ title: '写作手册', wide: 'xl', body: h('div', { class: 'handbook' }, nav, content), actions: [{ label: '关闭', onClick: (c) => c() }] });
  show(id || docs[0].id);
}

// ---------------- 批量定稿 ----------------

export function openBatchFinalize() {
  const n = store.chapters.length;
  if (!n) return toast('还没有章节', 'warn');
  const o = { from: 1, to: n, skipDone: true, autoApply: true, tension: true };
  const bar = h('div', { class: 'progress' }, h('div', { class: 'progress-fill' }));
  const log = h('div', { class: 'batch-log' });
  let cancelled = false;
  let running = false;
  const run = async (btn) => {
    if (running) return;
    running = true;
    cancelled = false;
    btn.disabled = true;
    log.innerHTML = '';
    const targets = store.chapters.slice(o.from - 1, o.to).filter((c) => c.word_count > 0 && !(o.skipDone && c.status === 'done' && c.has_summary));
    if (!targets.length) log.append(h('div', null, '范围内没有需要处理的章节（空章和已定稿且有摘要的章节会跳过）。'));
    let done = 0;
    for (const c of targets) {
      if (cancelled) { log.append(h('div', { class: 'warn-text' }, '已停止')); break; }
      const line = h('div', null, `${chapterLabel(c)}：处理中…`);
      log.append(line);
      log.scrollTop = log.scrollHeight;
      try {
        const base = { book_id: store.book.id, chapter_id: c.id };
        const [, ext] = await Promise.all([
          api.post('/ai/json', { task: 'summarize', ...base }),
          api.post('/ai/json', { task: 'extract', ...base }),
          o.tension ? api.post('/ai/json', { task: 'tension', ...base }).catch(() => null) : null,
        ]);
        let note = '已生成摘要';
        if (o.autoApply) {
          const r = await api.post(`/books/${store.book.id}/apply_updates`, { chapter_id: c.id, entries: ext.entries || [], threads_new: ext.threads_new || [], threads_progressed: ext.threads_progressed || [], threads_resolved: ext.threads_resolved || [], relations: ext.relations || [] });
          await api.patch(`/chapters/${c.id}`, { status: 'done' });
          note += `，设定新增 ${r.created}、更新 ${r.updated}，伏笔新增 ${r.threads_added}、回收 ${r.threads_resolved}`;
        }
        line.textContent = `${chapterLabel(c)}：${note}`;
      } catch (e) {
        line.textContent = `${chapterLabel(c)}：失败（${e.message}）`;
        line.classList.add('warn-text');
      }
      done += 1;
      bar.firstChild.style.width = `${(done / targets.length) * 100}%`;
    }
    await reload(['entries', 'threads', 'chapters']);
    emit('entries-changed');
    emit('threads-changed');
    emit('chapters-changed');
    if (store.chapter) store.editor.reloadChapter();
    running = false;
    btn.disabled = false;
    toast(`批量处理完成：${done} 章`);
  };
  const startBtn = h('button', { class: 'btn primary', onclick: (e) => run(e.currentTarget) }, '开始');
  modal({
    title: '批量定稿 / 建档',
    wide: true,
    body: h('div', null,
      h('p', { class: 'hint' }, '按顺序逐章生成摘要、提取人物状态和伏笔、分析张力。适合导入旧稿后一次性建立设定库和前情摘要。每章大约调用 3 次分析模型。'),
      h('div', { class: 'grid4' },
        field('从第几个章节', h('input', { type: 'number', min: 1, max: n, value: 1, oninput: (e) => { o.from = Math.max(1, Number(e.target.value)); } })),
        field('到第几个章节', h('input', { type: 'number', min: 1, max: n, value: n, oninput: (e) => { o.to = Math.min(n, Number(e.target.value)); } }))),
      h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.skipDone = e.target.checked; } }), '跳过已定稿且有摘要的章节'),
      h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.autoApply = e.target.checked; } }), '自动把设定变化写回设定库，并标记为定稿（不逐章确认）'),
      h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.tension = e.target.checked; } }), '同时分析张力和爽点（用于张力曲线和节奏检查）'),
      h('div', { class: 'row' }, startBtn, h('button', { class: 'btn', onclick: () => { cancelled = true; } }, '停止')),
      bar,
      log),
    actions: [{ label: '关闭', onClick: (c) => { cancelled = true; c(); } }],
  });
}

// ---------------- 人物关系图 ----------------

const SVG_NS = 'http://www.w3.org/2000/svg';
function svgEl(tag, attrs = {}, text) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v);
  }
  if (text != null) el.textContent = text;
  return el;
}

export async function openRelations() {
  let rels = await api.get(`/books/${store.book.id}/relations`);
  const people = store.entries.filter((e) => e.kind === 'character' || e.kind === 'faction');
  const W = 900;
  const H = 480;
  const svg = svgEl('svg', { viewBox: `0 0 ${W} ${H}`, class: 'rel-graph' });
  const list = h('div', { class: 'rel-list' });
  const name = (id) => store.entries.find((e) => e.id === id)?.name || '？';
  const reload = async () => { rels = await api.get(`/books/${store.book.id}/relations`); draw(); };

  function draw() {
    const nodes = people.map((e, i) => ({ e, x: W / 2 + Math.cos(i * 2.39) * (80 + i * 12), y: H / 2 + Math.sin(i * 2.39) * (60 + i * 10) }));
    const at = new Map(nodes.map((n) => [n.e.id, n]));
    const edges = rels.filter((r) => at.has(r.a_id) && at.has(r.b_id));
    for (let step = 0; step < 400; step++) {
      for (const n of nodes) { n.fx = (W / 2 - n.x) * 0.01; n.fy = (H / 2 - n.y) * 0.01; }
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          const a = nodes[i]; const b = nodes[j];
          const dx = a.x - b.x; const dy = a.y - b.y;
          const d2 = Math.max(dx * dx + dy * dy, 100); const d = Math.sqrt(d2); const f = 18000 / d2;
          a.fx += (f * dx) / d; a.fy += (f * dy) / d; b.fx -= (f * dx) / d; b.fy -= (f * dy) / d;
        }
      }
      for (const r of edges) {
        const a = at.get(r.a_id); const b = at.get(r.b_id);
        const dx = b.x - a.x; const dy = b.y - a.y; const d = Math.max(Math.hypot(dx, dy), 1); const f = (d - 170) * 0.03;
        a.fx += (f * dx) / d; a.fy += (f * dy) / d; b.fx -= (f * dx) / d; b.fy -= (f * dy) / d;
      }
      for (const n of nodes) {
        n.x = Math.min(W - 50, Math.max(50, n.x + n.fx * 0.4));
        n.y = Math.min(H - 35, Math.max(35, n.y + n.fy * 0.4));
      }
    }
    svg.innerHTML = '';
    for (const r of edges) {
      const a = at.get(r.a_id); const b = at.get(r.b_id);
      svg.append(svgEl('line', { x1: a.x, y1: a.y, x2: b.x, y2: b.y, class: 'rel-edge' + (r.status === 'ended' ? ' ended' : '') }));
      svg.append(svgEl('text', { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 - 4, class: 'rel-label', onclick: () => editRelation(r) }, r.kind));
    }
    for (const n of nodes) {
      const major = n.e.role === '主角' || n.e.always_include;
      const g = svgEl('g', { class: 'rel-node ' + n.e.kind + (major ? ' major' : ''), onclick: () => openEntry(n.e) });
      g.append(svgEl('circle', { cx: n.x, cy: n.y, r: major ? 30 : 23 }), svgEl('text', { x: n.x, y: n.y + 5, 'text-anchor': 'middle' }, n.e.name.slice(0, 4)));
      svg.append(g);
    }
    list.innerHTML = '';
    if (!rels.length) list.append(h('div', { class: 'empty' }, '还没有关系。定稿时 AI 会自动提取，也可以在上面手动添加。'));
    for (const r of rels) {
      list.append(h('div', { class: 'rel-row' + (r.status === 'ended' ? ' ended' : '') },
        h('b', null, `${name(r.a_id)} — ${name(r.b_id)}`), h('span', { class: 'tag' }, r.kind), h('span', { class: 'muted small grow' }, r.detail || ''),
        r.status === 'ended' ? h('span', { class: 'badge' }, '已结束') : null,
        h('button', { class: 'mini', onclick: () => editRelation(r) }, '编辑'),
        h('button', { class: 'mini danger', onclick: async () => { await api.del(`/relations/${r.id}`); reload(); } }, '删除')));
    }
  }

  function editRelation(r) {
    const x = { ...r };
    modal({
      title: `${name(r.a_id)} 与 ${name(r.b_id)}`,
      body: h('div', null,
        field('关系', h('input', { value: x.kind, oninput: (e) => { x.kind = e.target.value; } })),
        field('说明', h('input', { value: x.detail, oninput: (e) => { x.detail = e.target.value; } })),
        field('状态', select([['active', '进行中'], ['ended', '已结束（决裂、死亡、分手等）']], x.status, (v) => { x.status = v; }))),
      actions: [{ label: '取消', onClick: (c) => c() }, { label: '保存', class: 'primary', onClick: async (c) => { await api.patch(`/relations/${r.id}`, x); c(); reload(); } }],
    });
  }

  const opts = people.map((e) => [e.id, e.name]);
  const form = { a_id: opts[0]?.[0], b_id: opts[1]?.[0], kind: '', detail: '' };
  const add = h('div', { class: 'row rel-form' },
    select(opts, form.a_id, (v) => { form.a_id = Number(v); }),
    h('span', null, '与'),
    select(opts, form.b_id, (v) => { form.b_id = Number(v); }),
    h('input', { placeholder: '关系，如：师徒、宿敌', class: 'grow', oninput: (e) => { form.kind = e.target.value; } }),
    h('input', { placeholder: '说明（可选）', class: 'grow', oninput: (e) => { form.detail = e.target.value; } }),
    h('button', {
      class: 'btn primary',
      onclick: async () => {
        try {
          await api.post(`/books/${store.book.id}/relations`, { ...form, a_id: Number(form.a_id), b_id: Number(form.b_id), since_chapter_id: store.chapter?.id ?? null });
          reload();
        } catch (e) { toast(e.message, 'error'); }
      },
    }, '添加'));
  modal({
    title: '人物关系',
    wide: 'xl',
    body: h('div', null,
      people.length < 2 ? h('div', { class: 'empty' }, '设定库里至少要有两个人物或势力。') : svg,
      h('p', { class: 'hint' }, '点人物打开设定，点连线上的文字编辑关系；虚线表示关系已结束。写作时会自动带上出场人物之间的关系。'),
      people.length >= 2 ? add : null,
      list),
    actions: [{ label: '关闭', onClick: (c) => c() }],
  });
  draw();
}

// ---------------- 批量起草 ----------------

export function openBatchDraft() {
  const pending = store.chapters.filter((c) => c.has_outline && c.word_count === 0);
  if (!pending.length) return toast('没有“有章纲、没正文”的章节。先用「AI 规划后续章纲」规划几章。', 'warn', 4000);
  const o = { beats: true, words: store.book.target_words || 2500, chosen: new Set(pending.map((c) => c.id)) };
  const bar = h('div', { class: 'progress' }, h('div', { class: 'progress-fill' }));
  const log = h('div', { class: 'batch-log' });
  let cancelled = false;
  let running = false;
  const run = async (btn) => {
    if (running) return;
    running = true;
    cancelled = false;
    btn.disabled = true;
    const targets = pending.filter((c) => o.chosen.has(c.id));
    let done = 0;
    for (const c of targets) {
      if (cancelled) { log.append(h('div', { class: 'warn-text' }, '已停止')); break; }
      const line = h('div', null, `${chapterLabel(c)}：${o.beats ? '规划节拍…' : '起草中…'}`);
      log.append(line);
      log.scrollTop = log.scrollHeight;
      try {
        const base = { book_id: store.book.id, chapter_id: c.id };
        if (o.beats) {
          const b = await api.post('/ai/json', { task: 'beats', ...base });
          await api.patch(`/chapters/${c.id}`, { beats: cleanAi(b.text) });
          line.textContent = `${chapterLabel(c)}：起草中…`;
        }
        const r = await api.post('/ai/json', { task: 'write_chapter', ...base, words: o.words });
        const text = cleanAi(r.text);
        await api.patch(`/chapters/${c.id}`, { content: text, status: 'draft', snapshot: 'AI 批量起草前' });
        await api.post(`/chapters/${c.id}/ai_accept`, { chars: countWords(text) });
        line.textContent = `${chapterLabel(c)}：已起草 ${countWords(text)} 字`;
      } catch (e) {
        line.textContent = `${chapterLabel(c)}：失败（${e.message}）`;
        line.classList.add('warn-text');
      }
      done += 1;
      bar.firstChild.style.width = `${(done / targets.length) * 100}%`;
    }
    await reload(['chapters']);
    emit('chapters-changed');
    if (store.chapter) store.editor.reloadChapter();
    running = false;
    btn.disabled = false;
    toast(`批量起草完成：${done} 章。请逐章审改，满意后再定稿。`, 'info', 5000);
  };
  const startBtn = h('button', { class: 'btn primary', onclick: (e) => run(e.currentTarget) }, '开始起草');
  modal({
    title: '批量起草',
    wide: true,
    body: h('div', null,
      h('p', { class: 'hint' }, '按顺序给“有章纲、没正文”的章节先规划节拍、再写出初稿，状态保持为草稿。起草的字数会计入「采纳的 AI 字数」。初稿只是起点：请逐章审改，满意后再定稿。'),
      h('div', { class: 'chapter-picks' }, pending.map((c) => h('label', { class: 'check' },
        h('input', { type: 'checkbox', checked: true, onchange: (e) => { if (e.target.checked) o.chosen.add(c.id); else o.chosen.delete(c.id); } }), chapterLabel(c)))),
      h('div', { class: 'grid4' },
        field('每章字数', h('input', { type: 'number', min: 500, step: 100, value: o.words, oninput: (e) => { o.words = Number(e.target.value); } }))),
      h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: true, onchange: (e) => { o.beats = e.target.checked; } }), '先规划节拍再写（更贴章纲，多一次调用）'),
      h('div', { class: 'row' }, startBtn, h('button', { class: 'btn', onclick: () => { cancelled = true; } }, '停止')),
      bar,
      log),
    actions: [{ label: '关闭', onClick: (c) => { cancelled = true; c(); } }],
  });
}

// ---------------- 命令面板（Ctrl+K） ----------------

export function openPalette() {
  if (document.querySelector('.palette')) return;
  const commands = [
    ['续写（从光标处）', () => emit('ai-run', 'continue')],
    ['写整章', () => emit('ai-run', 'write_chapter')],
    ['规划本章节拍', () => emit('run-beats')],
    ['定稿本章', () => openFinalize()],
    ['连续性体检', () => emit('switch-tab', 'check')],
    ['AI 规划后续章纲', () => openPlanner()],
    ['批量起草', () => openBatchDraft()],
    ['批量定稿 / 建档', () => openBatchFinalize()],
    ['人物关系图', () => openRelations()],
    ['文风库', () => openLibrary()],
    ['文风指南', () => openLibrary('guide')],
    ['收藏范文', () => openSaveToLibrary({ genre: store.book?.genre || '' })],
    ['作品设定', () => openBookSettings()],
    ['导出', () => openExport()],
    ['统计', () => openStats()],
    ['写作手册', () => openHandbook()],
    ['提示词模板', () => openPrompts()],
    ['设置', () => openSettings()],
  ].map(([label, run]) => ({ label, hint: '命令', run }));
  const items = [
    ...commands,
    ...store.chapters.map((c) => ({ label: chapterLabel(c), hint: '章节', run: () => store.editor?.open(c.id) })),
    ...store.entries.map((e) => ({ label: e.name, hint: KIND[e.kind] || '设定', run: () => openEntry(e) })),
  ];
  let filtered = items;
  let active = 0;
  const list = h('div', { class: 'palette-list' });
  const close = () => { unlayer(); overlay.remove(); };
  const pick = (it) => { close(); it.run(); };
  const draw = () => {
    list.innerHTML = '';
    filtered.slice(0, 40).forEach((it, i) => list.append(h('div', { class: 'palette-item' + (i === active ? ' on' : ''), onmousedown: (e) => { e.preventDefault(); pick(it); } }, h('span', null, it.label), h('span', { class: 'muted small' }, it.hint))));
  };
  const input = h('input', {
    placeholder: '搜索命令、章节、设定……',
    oninput: (e) => {
      const q = e.target.value.trim().toLowerCase();
      filtered = q ? items.filter((it) => it.label.toLowerCase().includes(q)) : items;
      active = 0;
      draw();
    },
    onkeydown: (e) => {
      if (e.key === 'ArrowDown') { active = Math.min(active + 1, Math.min(filtered.length, 40) - 1); draw(); e.preventDefault(); }
      else if (e.key === 'ArrowUp') { active = Math.max(active - 1, 0); draw(); e.preventDefault(); }
      else if (e.key === 'Enter' && filtered[active]) pick(filtered[active]);
      else if (e.key === 'Escape') close();
    },
    onblur: () => setTimeout(close, 120),
  });
  const overlay = h('div', { class: 'palette' }, h('div', { class: 'palette-box' }, input, list));
  document.body.append(overlay);
  const unlayer = pushLayer(close);
  draw();
  input.focus();
}

// ---------------- 上下文预览 ----------------

export async function openContext() {
  const ch = store.chapter;
  await store.editor?.save().catch(() => {});
  const r = await api.get(`/books/${store.book.id}/context${ch ? `?chapter_id=${ch.id}` : ''}`);
  modal({
    title: `AI 写作时看到的上下文 · ${r.chars} 字（预算 ${r.budget} 字）`,
    wide: 'xl',
    body: h('div', { class: 'context-view' },
      h('p', { class: 'hint' }, '除了这些，生成时还会带上本章章纲、光标前的正文和你的补充要求。内容太多会被截断，可以在设置里调整预算。'),
      r.sections.map((s, i) => h('details', { open: i < 3 }, h('summary', null, `${s.title}（${s.body.length} 字）`), h('pre', null, s.body)))),
    actions: [{ label: '关闭', onClick: (c) => c() }],
  });
}

// ---------------- 设定条目 ----------------

export function openEntry(entry) {
  const e = { kind: 'character', name: '', aliases: '', description: '', state: '', immutable: '', always_include: false, role: '', ...entry };
  e.fields = { ...(entry.fields || {}) };
  const isNew = !e.id;
  const save = async (close) => {
    if (!e.name.trim()) return toast('名称不能为空', 'warn');
    try {
      if (isNew) await api.post(`/books/${store.book.id}/entries`, e);
      else await api.patch(`/entries/${e.id}`, { ...e, as_of_chapter_id: store.chapter?.id ?? null });
      await reload(['entries']);
      emit('entries-changed');
      close();
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
        if (!(await confirmBox(`删除设定「${e.name}」？`, { okText: '删除', danger: true }))) return;
        await api.del(`/entries/${e.id}`);
        await reload(['entries']);
        emit('entries-changed');
        close();
      },
    });
    if (e.kind === 'character') {
      actions.push({ label: '和 TA 对话', onClick: (close) => { store.chatTarget = e.id; close(); emit('switch-tab', 'chat'); } });
    }
  }
  actions.push({ label: '取消', onClick: (c) => c() }, { label: '保存', class: 'primary', onClick: save });
  modal({
    title: isNew ? '新建设定' : `编辑设定 · ${e.name}`,
    wide: true,
    body: (() => {
      const stateBox = h('div');
      const scopeHint = store.chapter
        ? `修改从「${chapterLabel(store.chapters.find((c) => c.id === store.chapter.id))}」开始生效，之前的章节仍按原来的状态写。`
        : '没有打开章节，修改的是初始状态。';
      const drawState = () => {
        stateBox.innerHTML = '';
        if (e.kind === 'character') {
          stateBox.append(
            h('div', { class: 'field-label' }, '当前状态'),
            h('p', { class: 'hint' }, `${scopeHint}定稿时 AI 会按章节自动更新这些字段。`),
            h('div', { class: 'grid2' }, CHAR_FIELDS.map(([k, label]) => field(label, h('input', { value: e.fields[k] || '', oninput: (ev) => { e.fields[k] = ev.target.value; } })))));
        } else {
          stateBox.append(field('当前状态', h('textarea', { rows: 3, value: e.state, oninput: (ev) => { e.state = ev.target.value; } }), scopeHint));
        }
      };
      drawState();
      const history = h('div', { class: 'state-history' });
      if (!isNew) {
        api.get(`/entries/${e.id}/states`).then((states) => {
          if (!states.length) return;
          const when = (s) => (s.chapter_id == null ? '初始' : `${chapterLabel(store.chapters.find((c) => c.id === s.chapter_id)) || '已删除的章节'}${s.phase === 'start' ? '起' : '末'}`);
          history.append(h('details', null, h('summary', null, `状态变化记录（${states.length}）`),
            states.map((s) => h('div', { class: 'state-row' }, h('b', null, when(s)), h('span', null, renderState(s))))));
        }).catch(() => {});
      }
      return h('div', null,
        h('div', { class: 'grid3' },
          field('类别', select(Object.entries(KIND), e.kind, (v) => { e.kind = v; drawState(); })),
          field('名称', h('input', { value: e.name, oninput: (ev) => { e.name = ev.target.value; } })),
          field('定位', select([['', '（不设置）'], ...ROLES.map((r) => [r, r])], e.role, (v) => { e.role = v; }), '主角、重要配角、反派会做缺席检查')),
        field('别名 / 称呼', h('input', { value: e.aliases, placeholder: '用顿号分隔，比如：凡哥、林少', oninput: (ev) => { e.aliases = ev.target.value; } }), '正文、章纲里出现名称或别名时，这条设定会自动带进 AI 的上下文'),
        field('设定描述', h('textarea', { rows: 4, value: e.description, oninput: (ev) => { e.description = ev.target.value; } })),
        field('不可改变的特征', h('input', { value: e.immutable, placeholder: '写崩就会被读者骂的东西：性格底线、身世、口头禅', oninput: (ev) => { e.immutable = ev.target.value; } })),
        stateBox,
        history,
        h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: e.always_include, onchange: (ev) => { e.always_include = ev.target.checked; } }), '常驻：每次写作都带上（适合主角、核心体系）'));
    })(),
    actions,
  });
}

function renderState(s) {
  const f = s.fields || {};
  const parts = CHAR_FIELDS.filter(([k]) => (f[k] || '').trim()).map(([k, l]) => `${l}：${f[k]}`);
  return parts.length ? parts.join('；') : s.state;
}

// ---------------- 伏笔 ----------------

export function openThread(thread) {
  const t = { title: '', detail: '', status: 'open', planted_chapter_id: null, resolved_chapter_id: null, target_chapter: null, ...thread };
  const isNew = !t.id;
  const chapterOptions = [['', '（不关联）'], ...store.chapters.map((c) => [c.id, chapterLabel(c)])];
  const actions = [];
  if (!isNew) {
    actions.push({
      label: '删除',
      class: 'danger left',
      onClick: async (close) => {
        if (!(await confirmBox('删除这个伏笔？', { okText: '删除', danger: true }))) return;
        await api.del(`/threads/${t.id}`);
        await reload(['threads']);
        emit('threads-changed');
        close();
      },
    });
  }
  actions.push({ label: '取消', onClick: (c) => c() }, {
    label: '保存',
    class: 'primary',
    onClick: async (close) => {
      if (!t.title.trim()) return toast('写一下伏笔是什么', 'warn');
      if (isNew) await api.post(`/books/${store.book.id}/threads`, t);
      else await api.patch(`/threads/${t.id}`, t);
      await reload(['threads']);
      emit('threads-changed');
      close();
    },
  });
  modal({
    title: isNew ? '记一个伏笔' : '编辑伏笔',
    wide: true,
    body: h('div', null,
      field('伏笔', h('input', { value: t.title, placeholder: '比如：黑色玉佩的来历', oninput: (e) => { t.title = e.target.value; } })),
      field('细节和回收方向', h('textarea', { rows: 4, value: t.detail, oninput: (e) => { t.detail = e.target.value; } })),
      h('div', { class: 'grid3' },
        field('埋在哪一章', select(chapterOptions, t.planted_chapter_id, (v) => { t.planted_chapter_id = v ? Number(v) : null; })),
        field('计划第几章回收', h('input', { type: 'number', min: 1, value: t.target_chapter ?? '', placeholder: '可不填', oninput: (e) => { t.target_chapter = e.target.value ? Number(e.target.value) : null; } }), '过期会在连续性体检里提醒'),
        field('状态', select([['open', '未回收'], ['resolved', '已回收'], ['dropped', '已放弃']], t.status, (v) => { t.status = v; })))),
    actions,
  });
}

// ---------------- 导入 ----------------

export async function openTavernImport() {
  const file = await pickFile('.png,.json');
  if (!file) return;
  try {
    const data = await readFile(file);
    const r = await api.post(`/books/${store.book.id}/import/tavern`, { filename: file.name, data });
    await reload(['entries']);
    emit('entries-changed');
    toast(`已导入：新增 ${r.created} 条，更新 ${r.updated} 条（${r.names.slice(0, 5).join('、')}${r.names.length > 5 ? '…' : ''}）`, 'info', 4500);
  } catch (e) {
    toast(e.message, 'error', 5000);
  }
}

export function openImport() {
  let file = null;
  const o = { title: '', genre: '' };
  const fileLabel = h('span', { class: 'muted' }, '未选择文件');
  const choose = h('button', {
    class: 'btn',
    onclick: async () => {
      file = await pickFile('.txt,.docx,.epub');
      if (file) fileLabel.textContent = `${file.name}（${(file.size / 1024).toFixed(0)} KB）`;
    },
  }, '选择文件');
  modal({
    title: '导入旧稿续写',
    wide: true,
    body: h('div', null,
      h('p', { class: 'hint' }, '支持 txt（自动识别 UTF-8 / GBK）、docx、epub。会按“第X章”“第X卷”自动拆分章节和分卷；开头的简介会放进作品简介。导入后建议对最近几章点「定稿」，让 AI 建立设定库和前情摘要。'),
      h('div', { class: 'row' }, choose, fileLabel),
      h('div', { class: 'grid2' },
        field('书名（可选）', h('input', { placeholder: '默认用文件名', oninput: (e) => { o.title = e.target.value; } })),
        field('题材（可选）', h('input', { list: 'genre-options-import', oninput: (e) => { o.genre = e.target.value; } }))),
      h('datalist', { id: 'genre-options-import' }, GENRES.map((g) => h('option', { value: g })))),
    actions: [
      { label: '取消', onClick: (c) => c() },
      {
        label: '导入',
        class: 'primary',
        onClick: async (close) => {
          if (!file) return toast('先选择文件', 'warn');
          const btn = document.querySelector('.modal-foot .primary');
          await busy(btn, async () => {
            const data = await readFile(file);
            const r = await api.post('/import/novel', { filename: file.name, data, title: o.title, genre: o.genre });
            close();
            toast(`导入完成：${r.chapters} 章，${fmtWords(r.words)} 字`, 'info', 4000);
            emit('open-book', r.book_id);
          }, '导入中…');
        },
      },
    ],
  });
}
