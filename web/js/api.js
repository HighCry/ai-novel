import { cleanAi, confirmBox, toast } from './ui.js';

async function request(method, path, body) {
  const init = { method, headers: {} };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  const res = await fetch('/api' + path, init);
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { data = text; }
  if (!res.ok) throw new Error((data && data.error) || `请求失败（${res.status}）`);
  return data;
}

export const api = {
  get: (p) => request('GET', p),
  post: (p, b = {}) => request('POST', p, b),
  put: (p, b) => request('PUT', p, b),
  patch: (p, b) => request('PATCH', p, b),
  del: (p) => request('DELETE', p),
  async blob(p) {
    const res = await fetch('/api' + p);
    if (!res.ok) {
      const t = await res.text();
      let msg = t;
      try { msg = JSON.parse(t).error; } catch { /* 原样显示 */ }
      throw new Error(msg || `请求失败（${res.status}）`);
    }
    return res.blob();
  },
};

/** 把流式生成的内容实时写进文本框；失败时恢复原内容 */
export async function streamInto(target, body, btn) {
  if (target.value.trim() && !(await confirmBox('AI 生成的内容会覆盖文本框里现有的内容，继续吗？'))) return null;
  const old = target.value;
  const label = btn?.textContent;
  if (btn) { btn.disabled = true; btn.textContent = '生成中…'; }
  target.value = '';
  target.classList.add('streaming');
  try {
    const text = await stream(body, {
      onDelta: (_, full) => { target.value = full; target.scrollTop = target.scrollHeight; },
      onThinking: (n) => { if (!target.value && btn) btn.textContent = `思考中…`; },
    });
    target.value = cleanAi(text);
    target.dispatchEvent(new Event('input', { bubbles: true }));
    return target.value;
  } catch (e) {
    target.value = old;
    toast(e.message, 'error', 6000);
    return null;
  } finally {
    target.classList.remove('streaming');
    if (btn) { btn.disabled = false; btn.textContent = label; }
  }
}

/** 调用流式 AI 接口；返回完整文本。onDelta(片段, 累计全文)，onThinking(思考字数)，onMeta({ guide, refs }) 告知用了哪些范文 */
export async function stream(body, { onDelta, onThinking, onMeta, signal } = {}) {
  const res = await fetch('/api/ai/stream', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal,
  });
  if (!res.ok) {
    const t = await res.text();
    let msg = t;
    try { msg = JSON.parse(t).error; } catch { /* 原样显示 */ }
    throw new Error(msg || `请求失败（${res.status}）`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  let full = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true }).replace(/\r\n/g, '\n');
    let idx;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      const block = buf.slice(0, idx);
      buf = buf.slice(idx + 2);
      let event = 'message';
      let data = '';
      for (const line of block.split('\n')) {
        if (line.startsWith('event:')) event = line.slice(6).trim();
        else if (line.startsWith('data:')) data += line.slice(5).trim();
      }
      if (!data) continue;
      const payload = JSON.parse(data);
      if (event === 'delta') {
        full += payload.text;
        onDelta?.(payload.text, full);
      } else if (event === 'thinking') {
        onThinking?.(payload.chars);
      } else if (event === 'meta') {
        onMeta?.(payload);
      } else if (event === 'done') {
        return payload.text || full;
      } else if (event === 'error') {
        throw new Error(payload.message);
      }
    }
  }
  return full;
}
