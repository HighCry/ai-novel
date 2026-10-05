import { api } from './api.js';
import { store, modelReady } from './store.js';
import { h, toast, modal, busy, fmtTime, copyText } from './ui.js';

const PLATFORM = { fanqie: '番茄', qidian: '起点' };
const SOURCES = ['微博', '抖音', 'B站', '百度'];
const QIDIAN_KINDS = [['yuepiao', '月票榜'], ['hotsales', '畅销榜'], ['readindex', '阅读指数榜'], ['newfans', '书友榜'], ['newauthor', '新人榜']];
/** 还没拿到番茄分类表时，男频、女频各用哪个分类 */
const DEFAULT_CAT = { 1: '258', 0: '1139' };

const now = () => Math.floor(Date.now() / 1000);
const heat = (n) => (!n ? '' : n >= 10000 ? `${(n / 10000).toFixed(n >= 1e6 ? 0 : 1)}万` : String(n));
const when = (ts) => (ts ? `${fmtTime(ts)} 抓取` : '');
const text = (v) => (v == null ? '' : typeof v === 'object' ? Object.values(v).join('：') : String(v));
const list = (items) => (items?.length ? h('ul', { class: 'tr-list' }, items.map((t) => h('li', null, text(t)))) : h('div', { class: 'muted small' }, '无'));
const seg = (options, value, onPick) => h('div', { class: 'seg' }, options.map(([v, label]) =>
  h('button', { class: 'seg-btn' + (v === value ? ' on' : ''), onclick: () => onPick(v) }, label)));
/** AI 结果按书存在本机：重新打开能看到上次的，不用再花一次调用 */
const loadSaved = (key) => { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } };
const keep = (key, v) => localStorage.setItem(key, JSON.stringify(v));
/** 原生 replaceChildren 不展开数组、会把 null 写成文字，这里按 h() 的规则来 */
const fill = (el, ...kids) => el.replaceChildren(...kids.flat(Infinity).filter((k) => k != null && k !== false));

const memeCard = (m) => h('div', { class: 'tr-meme' },
  h('div', { class: 'row tight' }, h('b', null, m.word), m.from && m.from !== m.word ? h('span', { class: 'tag' }, m.from) : null),
  m.meaning ? h('div', null, h('span', { class: 'muted' }, '意思：'), m.meaning) : null,
  m.scene ? h('div', null, h('span', { class: 'muted' }, '用法：'), m.scene) : null,
  m.example ? h('div', { class: 'tr-example' }, h('span', null, m.example), h('button', { class: 'mini', onclick: () => copyText(m.example) }, '复制')) : null,
  m.risk ? h('div', { class: 'small tr-risk' }, '注意：' + m.risk) : null);

/** 热点：从今日热搜里挑能写进书的梗、看起点和番茄的榜单、按榜单预测读者偏好 */
export function openTrends(tab = 'memes') {
  const tabs = h('div', { class: 'tabs lib-tabs' });
  const pane = h('div');
  modal({ title: '热点：热梗、榜单、读者偏好', body: h('div', { class: 'lib' }, tabs, pane), wide: 'xl', actions: [{ label: '关闭', onClick: (c) => c() }] });

  // 在作品里打开就看这本书；在书架上打开可以选
  let books = store.book ? [store.book] : null;
  let book = store.book || null;
  let hot = null;
  let source = '';
  const rk = { platform: book?.platform === 'fanqie' ? 'fanqie' : 'qidian', kind: 'yuepiao', gender: 1, list: 2, category: DEFAULT_CAT[1], ...loadSaved('trendsRank') };
  let fanqieCats = [];
  let rankSeq = 0;

  const TABS = [['memes', '热梗'], ['rank', '榜单'], ['preference', '读者偏好']];
  async function show(id) {
    tab = id;
    tabs.replaceChildren(...TABS.map(([k, label]) => h('button', { class: 'tab' + (k === id ? ' active' : ''), onclick: () => show(k) }, label)));
    if (id !== 'rank' && !books) {
      pane.replaceChildren(h('div', { class: 'boot' }, '加载中…'));
      books = await api.get('/books').catch((e) => { toast(e.message, 'error'); return []; });
      book = books[0] || null;
      if (tab !== id) return;
    }
    ({ memes: drawMemes, rank: drawRank, preference: drawPreference })[id]();
  }

  function bookBar(...extra) {
    const picker = store.book
      ? h('b', null, `《${book.title}》`)
      : h('select', { onchange: (e) => { book = books.find((b) => b.id === Number(e.target.value)); show(tab); } },
        books.map((b) => h('option', { value: b.id, selected: b.id === book.id }, `《${b.title}》`)));
    const meta = [PLATFORM[book.platform] || '其他平台', book.genre].filter(Boolean).join(' · ');
    return h('div', { class: 'row' }, picker, h('span', { class: 'muted small' }, meta), h('span', { class: 'spacer' }), extra);
  }

  function drawMemes() {
    const key = book && `trendsMemes:${book.id}`;
    const result = h('div', { class: 'tr-memes' });
    const side = h('div');
    const runBtn = h('button', { class: 'btn primary', onclick: () => busy(runBtn, run, 'AI 正在挑…') }, 'AI 挑能写进这本书的梗');
    pane.replaceChildren(
      book ? bookBar(runBtn) : h('div', { class: 'notice' }, '还没有作品：先开一本书，AI 才能按它的题材和风格挑梗。右边的热搜可以先看看。'),
      h('div', { class: 'tr-split' }, result, side));

    async function run() {
      if (!modelReady()) return toast('请先在设置里配置模型', 'warn');
      const r = await api.post('/trends/memes', { book_id: book.id });
      const v = { memes: r.memes || [], fetched_at: r.fetched_at, at: now() };
      keep(key, v);
      drawResult(v);
    }

    function drawResult(v) {
      if (!v) {
        result.replaceChildren(h('div', { class: 'empty' }, '从微博、抖音、B站、百度的热搜里挑出能写进这本书的流行语和梗：说明意思和适合的场景，按这本书的风格给一句示例，并提醒时效和风险。真实人物、明星八卦和新闻事件不会挑。'));
        return;
      }
      fill(result,
        h('div', { class: 'muted small' }, `${fmtTime(v.at)} 挑的，热搜${when(v.fetched_at)}`),
        v.memes.length ? v.memes.map(memeCard) : h('div', { class: 'empty' }, '今天的热搜里没有适合这本书的梗，过几个小时热搜换了再试试。'));
    }

    async function loadHot(refresh = false) {
      side.replaceChildren(h('div', { class: 'boot' }, '正在抓取热搜…'));
      try {
        hot = await api.get('/trends/hot' + (refresh ? '?refresh=true' : ''));
      } catch (e) {
        side.replaceChildren(h('div', { class: 'empty' }, '热搜暂时拿不到：' + e.message));
        return;
      }
      drawHot();
    }

    function drawHot() {
      const items = hot.items.filter((x) => !source || x.source === source);
      fill(side,
        h('div', { class: 'row tight' },
          seg([['', '全部'], ...SOURCES.map((s) => [s, s])], source, (v) => { source = v; drawHot(); }),
          h('span', { class: 'spacer' }),
          h('button', { class: 'mini', title: '热搜缓存 6 小时，点这里马上重新抓', onclick: () => loadHot(true) }, '刷新')),
        hot.fetched_at ? h('div', { class: 'muted small' }, `${when(hot.fetched_at)}，共 ${hot.items.length} 条，已按敏感词去掉高风险的`) : null,
        hot.errors?.length ? h('div', { class: 'notice small' }, '有的来源没抓到：' + hot.errors.join('；')) : null,
        items.length
          ? h('div', { class: 'tr-hot-list' }, items.map((x, i) => h('div', null,
            source ? h('span', { class: 'muted small tr-n' }, i + 1) : h('span', { class: 'tag' }, x.source),
            h('span', { class: 'tr-word' }, x.word),
            x.tag ? h('span', { class: 'badge' }, x.tag) : null,
            h('span', { class: 'muted small' }, heat(x.heat)))))
          : h('div', { class: 'empty' }, '没有内容'));
    }

    drawResult(key && loadSaved(key));
    if (hot) drawHot();
    else loadHot();
  }

  function drawRank() {
    const fanqie = rk.platform === 'fanqie';
    const pick = (patch) => {
      Object.assign(rk, patch);
      localStorage.setItem('trendsRank', JSON.stringify(rk));
      drawRank();
    };
    const catSel = h('select', { onchange: (e) => pick({ category: e.target.value }) });
    const fillCats = () => {
      const own = fanqieCats.filter((c) => c.gender === rk.gender);
      catSel.replaceChildren(...own.map((c) => h('option', { value: c.id, selected: c.id === rk.category }, c.name)));
      catSel.hidden = !own.length;
    };
    const stamp = h('span', { class: 'muted small' });
    const out = h('div');
    // [类别, 值]：只看这个题材、书名词或卖点标签的书
    let filter = null;
    pane.replaceChildren(
      h('div', { class: 'row' },
        seg([['qidian', '起点'], ['fanqie', '番茄']], rk.platform, (v) => pick({ platform: v })),
        seg([[1, '男频'], [0, '女频']], rk.gender, (g) => {
          const own = fanqieCats.filter((c) => c.gender === g);
          pick({ gender: g, category: own.some((c) => c.id === rk.category) ? rk.category : own[0]?.id || DEFAULT_CAT[g] });
        }),
        fanqie
          ? [seg([[2, '阅读榜'], [1, '新书榜']], rk.list, (l) => pick({ list: l })), catSel]
          : h('select', { onchange: (e) => pick({ kind: e.target.value }) }, QIDIAN_KINDS.map(([k, label]) => h('option', { value: k, selected: k === rk.kind }, label))),
        h('span', { class: 'spacer' }),
        stamp,
        h('button', { class: 'mini', title: '榜单缓存 6 小时，点这里马上重新抓', onclick: () => load(true) }, '重新抓取')),
      out);
    fillCats();

    async function load(refresh = false) {
      const seq = ++rankSeq;
      stamp.textContent = '';
      filter = null;
      out.replaceChildren(h('div', { class: 'boot' }, fanqie ? '正在抓取番茄榜单前 50 本……番茄的书名是加密字体，要对照书页学字，第一次可能要十几秒' : '正在抓取起点榜单前 60 本……'));
      const q = fanqie ? { platform: 'fanqie', gender: rk.gender, list: rk.list, category: rk.category } : { platform: 'qidian', kind: rk.kind, gender: rk.gender };
      if (refresh) q.refresh = true;
      let r;
      try {
        r = await api.get('/trends/rank?' + new URLSearchParams(q));
      } catch (e) {
        if (seq === rankSeq) out.replaceChildren(h('div', { class: 'empty' }, e.message));
        return;
      }
      if (seq !== rankSeq) return;
      if (r.categories) {
        fanqieCats = r.categories;
        fillCats();
      }
      stamp.textContent = when(r.fetched_at);
      drawTable(r);
    }

    function drawTable(r) {
      const stats = r.stats || {};
      const all = r.books || [];
      const hit = ([kind, v], b) => (kind === 'cat' ? b.category.split('·').pop() === v : kind === 'word' ? b.title.includes(v) : (b.tags || []).includes(v));
      const books = filter ? all.filter((b) => hit(filter, b)) : all;
      const toggle = (kind, v) => {
        filter = filter?.[0] === kind && filter[1] === v ? null : [kind, v];
        drawTable(r);
      };
      const group = (title, kind, pairs, none) => h('div', null,
        h('h4', null, title),
        h('div', { class: 'tags' }, pairs?.length
          ? pairs.map(([v, n]) => h('button', {
            class: 'chip' + (filter?.[0] === kind && filter[1] === v ? ' on' : ''),
            title: '只看这些书，再点一次看全部',
            onclick: () => toggle(kind, v),
          }, v, h('span', { class: 'tr-count' }, n)))
          : h('span', { class: 'muted small' }, none)));
      // 起点畅销榜、阅读指数榜、新人榜没有热度数字
      const metric = all.some((b) => b.metric);
      fill(out,
        r.limited ? h('div', { class: 'notice' }, `${r.limited}。现在显示的是上次抓到的榜单。`) : null,
        fanqie && r.undecoded ? h('div', { class: 'notice' }, `书名和作者里还有 ${r.undecoded} 个字没认出来，显示成 □。${r.paused
          ? `番茄暂时限制了书页访问，认字先停一下，${r.paused} 分钟后再打开会接着认。`
          : '番茄用的是加密字体，为了不被限流每次只对照一部分书页学字，过十分钟再打开会认出更多。'}`) : null,
        h('div', { class: 'muted small' }, `前 ${stats.count ?? all.length} 本的统计，数字是出现在几本书里；点一下只看这些书。`),
        h('div', { class: 'tr-stats' },
          // 番茄一次只看一个分类，题材分布没有意义
          fanqie ? null : group('题材分布', 'cat', stats.categories, '—'),
          group('书名高频词', 'word', stats.keywords, '书名里没有重复出现的词'),
          group('卖点标签', 'tag', stats.tags, fanqie ? '简介里没有重复出现的卖点标签' : '起点的简介里很少写卖点标签')),
        filter ? h('div', { class: 'row tight small' }, `只看「${filter[1]}」：${books.length} 本`, h('button', { class: 'mini', onclick: () => toggle(...filter) }, '看全部')) : null,
        h('div', { class: 'tr-scroll' },
          h('table', { class: 'table td-table' },
            h('tr', null, ['#', '书名', '作者', '分类', '字数', metric ? (fanqie ? '在读' : '热度') : null, fanqie ? '状态' : null, '简介'].filter(Boolean).map((t) => h('th', null, t))),
            books.map((b) => h('tr', null,
              h('td', null, b.rank),
              h('td', { class: 'nowrap' }, h('b', null, b.title)),
              h('td', { class: 'nowrap' }, b.author),
              h('td', { class: 'nowrap small' }, b.category),
              h('td', { class: 'nowrap small' }, b.words),
              metric ? h('td', { class: 'nowrap small' }, b.metric) : null,
              fanqie ? h('td', { class: 'nowrap small' }, b.status) : null,
              h('td', { class: 'small' }, h('div', { class: 'tr-desc', title: b.desc }, b.desc)))))));
    }

    load();
  }

  function drawPreference() {
    if (!book) {
      pane.replaceChildren(h('div', { class: 'empty' }, '还没有作品。先开一本书，再按榜单预测读者偏好。'));
      return;
    }
    const key = `trendsPref:${book.id}`;
    const out = h('div');
    const runBtn = h('button', { class: 'btn primary', onclick: () => busy(runBtn, run, '正在抓榜单、让 AI 分析…') }, '按当前榜单预测');
    const qidian = '起点同频道（男频或女频）的月票榜、阅读指数榜和新人榜（各前 60 本）';
    const fanqieBasis = '番茄同题材的阅读榜和新书榜（各前 50 本）';
    const basis = book.platform === 'fanqie' ? fanqieBasis : book.platform === 'qidian' ? qidian : `${qidian}，以及${fanqieBasis}`;
    pane.replaceChildren(
      bookBar(runBtn),
      h('div', { class: 'muted small' }, `取${basis}作依据，先统计题材分布、书名高频词和卖点标签，再让 AI 总结读者当下的偏好，评估这本书的契合度，并给出书名、简介、开篇等的改法。结果是基于榜单快照的估计，仅供参考。`),
      out);

    async function run() {
      if (!modelReady()) return toast('请先在设置里配置模型', 'warn');
      const r = await api.post('/trends/preference', { book_id: book.id });
      const v = { ...r, at: now() };
      keep(key, v);
      draw(v);
    }

    function draw(v) {
      if (!v) return;
      const r = v.result || {};
      const s = Number(r.score);
      const ok = r.score != null && Number.isFinite(s);
      const prefs = r.preferences || [];
      const tips = r.suggestions || [];
      fill(out,
        h('div', { class: 'score-line' },
          h('span', { class: 'score ' + (!ok ? '' : s >= 70 ? 'good' : s >= 50 ? 'mid' : 'bad') }, ok ? s : '—'),
          h('div', null, h('div', { class: 'muted small' }, '契合度（满分 100）'), h('div', null, text(r.summary)))),
        h('div', { class: 'muted small' }, `依据：${v.basis}；榜单${when(v.fetched_at)}，${fmtTime(v.at)} 分析。`),
        h('h4', null, '读者当下的偏好'),
        prefs.length
          ? h('ul', { class: 'tr-list' }, prefs.map((p) => h('li', null, typeof p === 'string' ? p : [h('b', null, p.point), p.evidence ? h('div', { class: 'muted small' }, p.evidence) : null])))
          : h('div', { class: 'muted small' }, '无'),
        h('div', { class: 'grid2' },
          h('div', null, h('h4', null, '契合的地方'), list(r.strengths)),
          h('div', null, h('h4', null, '可能留不住读者的地方'), list(r.risks))),
        tips.length
          ? [h('h4', null, '改法'), h('table', { class: 'table td-table' },
            h('tr', null, h('th', null, '改哪里'), h('th', null, '怎么改')),
            tips.map((x) => (typeof x === 'string'
              ? h('tr', null, h('td', null), h('td', null, x))
              : h('tr', null, h('td', { class: 'nowrap' }, x.target), h('td', null, x.advice)))))]
          : null);
    }

    draw(loadSaved(key));
  }

  show(tab);
}
