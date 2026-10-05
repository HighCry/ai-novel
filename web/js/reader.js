import { api } from './api.js';
import { store, chapterLabel } from './store.js';
import { h, toast, icon, pushLayer } from './ui.js';

const END = '。！？!?…';
const CLOSE = '”’」』"）)';

/** 按句末标点切句，引号、括号跟着前一句；切出来的每一句都是原段落的原文片段 */
function sentences(p) {
  const out = [];
  let start = 0;
  for (let i = 0; i < p.length; i++) {
    if (!END.includes(p[i])) continue;
    let j = i + 1;
    while (j < p.length && (END.includes(p[j]) || CLOSE.includes(p[j]))) j++;
    out.push(p.slice(start, j));
    start = j;
    i = j - 1;
  }
  if (start < p.length) out.push(p.slice(start));
  return out.filter((s) => s.trim());
}

/** 阅读模式：整屏看正文，上一章 / 下一章翻页；可以朗读，边听边找错 */
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
  const body = h('div', { class: 'reader-body', onclick: (e) => jumpTo(e.target.closest?.('.s')) });
  const pos = h('span', { class: 'muted small' });
  const prev = h('button', { class: 'btn', onclick: () => go(index - 1) }, '‹ 上一章');
  const next = h('button', { class: 'btn', onclick: () => go(index + 1) }, '下一章 ›');
  const setSize = (d) => {
    size = Math.min(28, Math.max(14, size + d));
    body.style.setProperty('--reader-size', size + 'px');
    localStorage.setItem('readerSize', String(size));
  };

  // ---------- 朗读 ----------
  const synth = window.speechSynthesis;
  /** 朗读进度：k 是当前句在本章的序号；为空表示没在朗读 */
  let tts = null;
  /** 每次开始读一句或停止都加一，旧句子迟到的回调据此忽略 */
  let session = 0;
  let voices = [];
  const ttsBtn = h('button', { class: 'mini', title: '从屏幕上看到的位置开始朗读，读完自动翻到下一章；朗读时点哪句就从哪句读', onclick: () => toggle() }, '朗读');
  const rate = h('select', { class: 'mini-select', title: '语速', onchange: () => { localStorage.setItem('ttsRate', rate.value); restart(); } },
    [0.8, 1, 1.2, 1.5, 2].map((r) => h('option', { value: r, selected: r === (Number(localStorage.getItem('ttsRate')) || 1) }, r + '×')));
  const voiceSel = h('select', { class: 'mini-select', title: '朗读的声音', hidden: true, onchange: () => { localStorage.setItem('ttsVoice', voiceSel.value); restart(); } });
  const fixBtn = h('button', { class: 'mini', hidden: true, title: '回到编辑器，选中正在读的这一句', onclick: () => fixHere() }, '去改这句');
  const loadVoices = () => {
    voices = synth.getVoices().filter((v) => /^zh|^cmn/i.test(v.lang));
    const keep = localStorage.getItem('ttsVoice');
    voiceSel.replaceChildren(...voices.map((v) => h('option', { value: v.name, selected: v.name === keep }, v.name.replace(/^Microsoft\s+/, '').replace(/\s+-\s+.*$/, ''))));
    voiceSel.hidden = voices.length < 2;
  };
  if (synth) {
    loadVoices();
    synth.addEventListener?.('voiceschanged', loadVoices);
  }
  const spans = () => [...body.querySelectorAll('.s')];
  const mark = (k) => {
    body.querySelector('.s.speaking')?.classList.remove('speaking');
    const s = spans()[k];
    if (!s) return;
    s.classList.add('speaking');
    const r = s.getBoundingClientRect();
    const b = body.getBoundingClientRect();
    if (r.top < b.top + 40 || r.bottom > b.bottom - 40) s.scrollIntoView({ block: 'center', behavior: 'smooth' });
  };
  const refreshTts = () => {
    ttsBtn.textContent = tts?.playing ? '暂停' : tts ? '继续' : '朗读';
    fixBtn.hidden = !tts;
    el.classList.toggle('reading', !!tts);
  };

  function speak(k) {
    const all = spans();
    if (k >= all.length) {
      if (index < list.length - 1) go(index + 1, { keepReading: true });
      else { stopTts(); toast('读完了'); }
      return;
    }
    const id = ++session;
    const busy = synth.speaking || synth.pending;
    synth.cancel();
    tts.k = k;
    mark(k);
    const u = new SpeechSynthesisUtterance(all[k].textContent);
    u.lang = 'zh-CN';
    u.rate = Number(rate.value) || 1;
    const v = voices.find((x) => x.name === voiceSel.value) || voices[0];
    if (v) u.voice = v;
    u.onend = () => { if (id === session && tts?.playing) speak(k + 1); };
    u.onerror = (e) => {
      if (id !== session || e.error === 'interrupted' || e.error === 'canceled') return;
      toast('朗读出错：' + e.error, 'error');
      stopTts();
    };
    // Chrome 在 cancel 之后立刻 speak 偶尔没声音，隔一下再读
    if (busy) setTimeout(() => { if (id === session) synth.speak(u); }, 60);
    else synth.speak(u);
  }

  function firstVisible() {
    const top = body.getBoundingClientRect().top;
    return Math.max(0, spans().findIndex((s) => s.getBoundingClientRect().bottom > top + 4));
  }

  function toggle() {
    if (!synth) return toast('这个浏览器不支持朗读', 'warn');
    if (!tts) {
      if (!spans().length) return toast('这一章还没有正文', 'warn');
      if (!voices.length && synth.getVoices().length) toast('系统里没有中文语音，可能读不出来。Windows 可以在「设置 → 时间和语言 → 语音」里添加中文语音。', 'warn', 6000);
      tts = { k: firstVisible(), playing: true };
      speak(tts.k);
    } else if (tts.playing) {
      tts.playing = false;
      session++;
      synth.cancel();
    } else {
      tts.playing = true;
      speak(tts.k);
    }
    refreshTts();
  }

  function restart() {
    if (tts?.playing) speak(tts.k);
  }

  function stopTts() {
    session++;
    synth?.cancel();
    tts = null;
    body.querySelector('.s.speaking')?.classList.remove('speaking');
    refreshTts();
  }

  function jumpTo(s) {
    if (!s || !tts) return;
    tts.k = spans().indexOf(s);
    tts.playing = true;
    speak(tts.k);
    refreshTts();
  }

  /** 回编辑器选中正在读的句子：同一句话在本章出现多次时按第几次出现来找 */
  async function fixHere() {
    const all = spans();
    const s = all[tts?.k ?? -1];
    if (!s) return;
    const text = s.textContent;
    const nth = all.slice(0, tts.k).filter((x) => x.textContent === text).length;
    const id = list[index].id;
    close();
    if (store.chapter?.id !== id) await store.editor.open(id);
    const v = store.editor.text();
    let at = -1;
    for (let n = 0; n <= nth; n++) {
      at = v.indexOf(text, at + 1);
      if (at < 0) break;
    }
    if (at < 0) return toast('在正文里没找到这一句，可能刚改过', 'warn');
    store.editor.select(at, at + text.length);
  }

  const close = () => {
    stopTts();
    synth?.removeEventListener?.('voiceschanged', loadVoices);
    unlayer();
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
      h('button', { class: 'icon-btn', title: '返回（Esc）', onclick: close }, icon('back')),
      title,
      synth ? h('span', { class: 'reader-tts' }, ttsBtn, fixBtn, rate, voiceSel) : null,
      h('button', { class: 'mini', title: '字小一点', onclick: () => setSize(-1) }, 'A−'),
      h('button', { class: 'mini', title: '字大一点', onclick: () => setSize(1) }, 'A+')),
    body,
    h('div', { class: 'reader-foot' }, prev, pos, next));
  document.body.append(el);
  document.addEventListener('keydown', onKey);
  const unlayer = pushLayer(close);
  setSize(0);
  go(index);

  async function go(i, { keepReading = false } = {}) {
    if (i < 0 || i >= list.length) {
      toast(i < 0 ? '已经是第一章了' : '已经是最后一章了');
      return;
    }
    if (!keepReading && tts) stopTts();
    const ticket = ++loading;
    let ch;
    try {
      ch = await api.get(`/chapters/${list[i].id}`);
    } catch (e) {
      toast(e.message, 'error');
      if (keepReading) stopTts();
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
      paras.length ? paras.map((p) => h('p', null, sentences(p).map((s) => h('span', { class: 's' }, s)))) : h('p', { class: 'muted' }, '（这一章还没有正文）')));
    body.scrollTop = 0;
    if (keepReading && tts?.playing) {
      if (paras.length) speak(0);
      else if (i < list.length - 1) go(i + 1, { keepReading: true });
      else { stopTts(); toast('读完了'); }
    }
  }
}
