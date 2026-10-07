/** 动效：退场、页面切换、换肤、标签下划线、书卡倾斜。系统开了「减少动态效果」或浏览器不支持时都直接切换 */

const reduce = matchMedia('(prefers-reduced-motion: reduce)');
export const calm = () => reduce.matches;

const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const finite = (a) => Number.isFinite(a.effect?.getComputedTiming().endTime);

/** 等元素和子元素上正在播的动画播完；页面在后台时动画可能停着不动，到点也算完 */
function settled(el, ms) {
  const anims = el.getAnimations({ subtree: true }).filter(finite);
  if (!anims.length) return Promise.resolve();
  return Promise.race([Promise.allSettled(anims.map((a) => a.finished)), new Promise((r) => setTimeout(r, ms))]);
}

/** 加上 .leaving 播完退场动画再移除；可以重复调用，退场期间点不到也聚焦不到 */
export function leave(el, ms = 500) {
  if (!el?.isConnected || el.classList.contains('leaving')) return;
  if (calm()) return el.remove();
  el.inert = true;
  el.classList.add('leaving');
  settled(el, ms).then(() => el.remove());
}

const plays = new WeakMap();

/** 让元素播一遍某个动画类，播完摘掉，之后重画子元素不会跟着重播 */
export function play(el, cls) {
  if (!el || calm()) return;
  const token = {};
  const own = plays.get(el) || new Map();
  plays.set(el, own.set(cls, token));
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
  settled(el, 2000).then(() => { if (own.get(cls) === token) el.classList.remove(cls); });
}

/** 去掉一个元素，同一列里其余元素从原来的位置滑到新位置，不会一下跳过去 */
export function reflowOut(el) {
  const box = el.parentElement;
  if (!box || calm()) return el.remove();
  const rest = [...box.children].filter((c) => c !== el);
  const before = rest.map((c) => c.getBoundingClientRect().top);
  el.remove();
  const easing = cssVar('--spring-soft') || 'ease-out';
  rest.forEach((c, i) => {
    const dy = before[i] - c.getBoundingClientRect().top;
    if (Math.abs(dy) > 0.5) c.animate([{ transform: `translateY(${dy}px)` }, { transform: 'none' }], { duration: 480, easing, composite: 'add' });
  });
}

/** 书架和写作页互相切换：书名从书卡飞到顶栏（返回时飞回去），页面整体前进或后退。hero 返回当前页面里的书名元素 */
export function navigate(update, { back = false, hero } = {}) {
  if (calm() || !document.startViewTransition) return update();
  const root = document.documentElement;
  const mark = () => {
    const el = hero?.();
    if (el) el.style.viewTransitionName = 'book-title';
    return el;
  };
  let named = mark();
  root.classList.add(back ? 'vt-back' : 'vt-forward');
  const vt = document.startViewTransition(async () => {
    named?.style.removeProperty('view-transition-name');
    await update();
    named = mark();
    // 新页面由过渡整体淡入，壳自己的淡入就不再叠一层；书名所在的书卡直接就位，落地时才不会闪
    document.querySelector('#app > .shell')?.getAnimations().forEach((a) => a.finish());
    named?.closest('.book-card')?.getAnimations().forEach((a) => a.finish());
  });
  vt.ready.catch(() => {});
  vt.finished.catch(() => {}).then(() => {
    root.classList.remove('vt-forward', 'vt-back');
    named?.style.removeProperty('view-transition-name');
  });
  return vt.updateCallbackDone;
}

/** 换肤：新配色从按钮处像墨一样晕开 */
export function themeSwap(apply, origin) {
  if (calm() || !document.startViewTransition) return apply();
  const r = origin?.getBoundingClientRect();
  const x = r ? r.left + r.width / 2 : innerWidth / 2;
  const y = r ? r.top + r.height / 2 : 0;
  const far = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
  const root = document.documentElement;
  root.classList.add('vt-theme');
  const vt = document.startViewTransition(apply);
  vt.ready.then(() => root.animate(
    { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${far}px at ${x}px ${y}px)`] },
    { duration: 650, easing: 'cubic-bezier(.7, 0, .25, 1)', pseudoElement: '::view-transition-new(root)' },
  )).catch(() => {});
  vt.finished.catch(() => {}).then(() => root.classList.remove('vt-theme'));
}

// 切换标签时下划线从旧标签滑过去。有的标签栏一点就整排重建按钮，所以在捕获阶段先量旧位置，下一帧再找新标签
document.addEventListener('click', (e) => {
  const tab = e.target.closest?.('.tab');
  const bar = tab?.parentElement;
  if (!bar?.classList.contains('tabs') || tab.classList.contains('active') || calm()) return;
  const from = bar.querySelector('.tab.active')?.getBoundingClientRect();
  if (!from) return;
  requestAnimationFrame(() => {
    const to = bar.querySelector('.tab.active');
    const r = to?.getBoundingClientRect();
    if (!r?.width) return;
    to.style.setProperty('--ink-dx', `${from.left - r.left}px`);
    to.style.setProperty('--ink-sx', String(from.width / r.width));
    play(to, 'ink-slide');
  });
}, true);

document.addEventListener('change', (e) => {
  const t = e.target;
  if (t.checked && t.matches?.('input[type="checkbox"], input[type="radio"]')) play(t, 'pop');
});

// 有鼠标的设备上，书卡跟着指针轻轻倾斜，高光跟着走
if (matchMedia('(hover: hover) and (pointer: fine)').matches) {
  let last = null;
  const tilt = () => {
    const e = last;
    last = null;
    const card = e.target.closest?.('.book-card');
    if (!card || calm()) return;
    const r = card.getBoundingClientRect();
    const px = (e.clientX - r.left) / r.width;
    const py = (e.clientY - r.top) / r.height;
    card.style.setProperty('--ry', `${((px - 0.5) * 8).toFixed(2)}deg`);
    card.style.setProperty('--rx', `${((0.5 - py) * 8).toFixed(2)}deg`);
    card.style.setProperty('--mx', `${(px * 100).toFixed(1)}%`);
    card.style.setProperty('--my', `${(py * 100).toFixed(1)}%`);
  };
  document.addEventListener('pointermove', (e) => {
    if (!last) requestAnimationFrame(tilt);
    last = e;
  }, { passive: true });
}
