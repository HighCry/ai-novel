use crate::models::Provider;
use crate::text::head_chars;
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(s: impl Into<String>) -> Self {
        Self { role: "system".into(), content: s.into() }
    }
    pub fn user(s: impl Into<String>) -> Self {
        Self { role: "user".into(), content: s.into() }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: s.into() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: f32,
    pub max_tokens: u32,
    pub json_mode: bool,
    /// 流式请求附带 stream_options.include_usage
    pub stream_usage: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

impl Usage {
    fn from_value(v: &Value) -> Option<Self> {
        let u = v.get("usage").filter(|u| u.is_object())?;
        let p = u["prompt_tokens"].as_i64().unwrap_or(0);
        let c = u["completion_tokens"].as_i64().unwrap_or(0);
        (p > 0 || c > 0).then_some(Self { prompt_tokens: p, completion_tokens: c })
    }
}

/// 一次非流式调用的结果
#[derive(Debug, Clone, Default)]
pub struct ChatOutput {
    pub text: String,
    pub usage: Option<Usage>,
    /// stop / length（被 max_tokens 截断）等
    pub finish_reason: String,
}

/// 没有返回用量时按字数粗估 token（中文大约 1 字 ≈ 0.8 token）。
pub fn estimate_tokens(chars: i64) -> i64 {
    (chars as f64 * 0.8).ceil() as i64
}

#[derive(Debug)]
pub enum LlmEvent {
    Delta(String),
    /// 推理模型的思考过程（只回传字数，不展示内容）
    Reasoning(usize),
    Done(String),
    Error(String),
}

const CANCELLED: &str = "客户端已断开，生成中止";

/// 用户填的接口地址可能带或不带 /chat/completions，这里统一成 base + path。
pub fn endpoint(base_url: &str, path: &str) -> String {
    let mut base = base_url.trim().trim_end_matches('/').to_string();
    for suffix in ["/chat/completions", "/models", "/embeddings"] {
        if let Some(s) = base.strip_suffix(suffix) {
            base = s.to_string();
        }
    }
    format!("{base}/{path}")
}

fn net_err(e: reqwest::Error) -> anyhow::Error {
    let url = e.url().map(|u| u.to_string()).unwrap_or_default();
    if e.is_connect() {
        anyhow!("连不上模型接口 {url}：请检查接口地址、网络或代理")
    } else if e.is_timeout() {
        anyhow!("模型接口响应超时：{url}")
    } else {
        anyhow!("请求模型接口失败：{e}")
    }
}

/// 去掉推理模型放在正文里的 <think>…</think>。
pub fn strip_think(text: &str) -> String {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        return match rest.find("</think>") {
            Some(end) => rest[end + "</think>".len()..].trim_start().to_string(),
            None => String::new(),
        };
    }
    text.to_string()
}

fn message_content(v: &Value) -> Result<String> {
    if let Some(err) = v.get("error") {
        bail!("模型接口报错：{err}");
    }
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or_default();
    Ok(strip_think(content))
}

/// 流式输出里逐块过滤 <think> 段落。
#[derive(Default)]
struct ThinkFilter {
    mode: u8, // 0 未判定，1 在思考段内，2 直接透传
    pending: String,
}

impl ThinkFilter {
    fn push(&mut self, s: &str) -> (String, usize) {
        if self.mode == 2 {
            return (s.to_string(), 0);
        }
        self.pending.push_str(s);
        if self.mode == 0 {
            let head = self.pending.trim_start();
            if head.len() < "<think>".len() && "<think>".starts_with(head) {
                return (String::new(), 0);
            }
            if head.starts_with("<think>") {
                self.mode = 1;
            } else {
                self.mode = 2;
                return (std::mem::take(&mut self.pending), 0);
            }
        }
        match self.pending.find("</think>") {
            Some(pos) => {
                let rest = self.pending[pos + "</think>".len()..].trim_start().to_string();
                self.mode = 2;
                self.pending.clear();
                (rest, 0)
            }
            None => (String::new(), s.chars().count()),
        }
    }

    fn finish(&mut self) -> String {
        if self.mode == 0 {
            std::mem::take(&mut self.pending)
        } else {
            String::new()
        }
    }
}

/// 把字节流切成 SSE 的 data 负载；只在换行处切分，避免截断多字节字符。
#[derive(Default)]
pub struct SseParser {
    raw: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.raw.extend_from_slice(bytes);
        let Some(last_nl) = self.raw.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let complete: Vec<u8> = self.raw.drain(..=last_nl).collect();
        String::from_utf8_lossy(&complete)
            .lines()
            .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:").map(|d| d.trim_start().to_string()))
            .collect()
    }

    pub fn finish(&mut self) -> Vec<String> {
        if self.raw.is_empty() {
            return Vec::new();
        }
        self.raw.push(b'\n');
        self.push(&[])
    }
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
}

impl Default for LlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("初始化 HTTP 客户端");
        Self { http }
    }

    fn body(req: &ChatRequest, stream: bool) -> Value {
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature,
            "stream": stream,
        });
        if req.max_tokens > 0 {
            body["max_tokens"] = json!(req.max_tokens);
        }
        if req.json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if stream && req.stream_usage {
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }

    fn post(&self, p: &Provider, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.post(endpoint(&p.base_url, path));
        if !p.api_key.trim().is_empty() {
            r = r.bearer_auth(p.api_key.trim());
        }
        r
    }

    pub async fn list_models(&self, p: &Provider) -> Result<Vec<String>> {
        let mut r = self.http.get(endpoint(&p.base_url, "models")).timeout(Duration::from_secs(30));
        if !p.api_key.trim().is_empty() {
            r = r.bearer_auth(p.api_key.trim());
        }
        let resp = r.send().await.map_err(net_err)?;
        let status = resp.status();
        let text = resp.text().await.map_err(net_err)?;
        if !status.is_success() {
            bail!("获取模型列表失败（{status}）：{}", head_chars(&text, 300));
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("模型列表不是 JSON：{}", head_chars(&text, 200)))?;
        let mut ids: Vec<String> = v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default();
        ids.sort();
        Ok(ids)
    }

    /// OpenAI 兼容的 /embeddings，按输入顺序返回向量
    pub async fn embed(&self, p: &Provider, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let resp = self
            .post(p, "embeddings")
            .timeout(Duration::from_secs(60))
            .json(&json!({ "model": model, "input": inputs }))
            .send()
            .await
            .map_err(net_err)?;
        let status = resp.status();
        let text = resp.text().await.map_err(net_err)?;
        if !status.is_success() {
            bail!("向量接口返回 {status}：{}", head_chars(&text, 300));
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("向量接口返回的不是 JSON：{}", head_chars(&text, 200)))?;
        let mut data: Vec<(usize, Vec<f32>)> = v["data"]
            .as_array()
            .ok_or_else(|| anyhow!("向量接口返回的格式不对：{}", head_chars(&text, 200)))?
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
                let emb = d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default();
                (idx, emb)
            })
            .collect();
        data.sort_by_key(|d| d.0);
        if data.len() != inputs.len() || data.iter().any(|(_, e)| e.is_empty()) {
            bail!("向量接口返回了 {} 条结果，应该是 {} 条", data.len(), inputs.len());
        }
        Ok(data.into_iter().map(|(_, e)| e).collect())
    }

    pub async fn chat(&self, p: &Provider, req: &ChatRequest) -> Result<ChatOutput> {
        let resp = self
            .post(p, "chat/completions")
            .timeout(Duration::from_secs(600))
            .json(&Self::body(req, false))
            .send()
            .await
            .map_err(net_err)?;
        let status = resp.status();
        let text = resp.text().await.map_err(net_err)?;
        if !status.is_success() {
            bail!("模型接口返回 {status}：{}", head_chars(&text, 500));
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("模型接口返回的不是 JSON：{}", head_chars(&text, 300)))?;
        Ok(ChatOutput {
            text: message_content(&v)?,
            usage: Usage::from_value(&v),
            finish_reason: v["choices"][0]["finish_reason"].as_str().unwrap_or_default().to_string(),
        })
    }

    /// 流式生成：增量通过 tx 发出，返回完整文本和用量。接收端断开时立即停止，节省调用费用。
    pub async fn chat_stream(&self, p: &Provider, req: &ChatRequest, tx: &mpsc::Sender<LlmEvent>) -> Result<(String, Option<Usage>)> {
        let resp = self
            .post(p, "chat/completions")
            .header("Accept", "text/event-stream")
            .json(&Self::body(req, true))
            .send()
            .await
            .map_err(net_err)?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("模型接口返回 {status}：{}", head_chars(&text, 500));
        }
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("event-stream"))
            .unwrap_or(false);
        if !is_sse {
            // 有些网关忽略 stream 参数，直接返回完整 JSON
            let text = resp.text().await.map_err(net_err)?;
            let v: Value = serde_json::from_str(&text).map_err(|_| anyhow!("模型接口返回的不是 JSON：{}", head_chars(&text, 300)))?;
            let content = message_content(&v)?;
            if tx.send(LlmEvent::Delta(content.clone())).await.is_err() {
                bail!(CANCELLED);
            }
            return Ok((content, Usage::from_value(&v)));
        }

        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut filter = ThinkFilter::default();
        let mut full = String::new();
        let mut usage = None;
        let mut finished = false;
        while !finished {
            let payloads = match stream.next().await {
                Some(chunk) => parser.push(&chunk.map_err(net_err)?),
                None => {
                    finished = true;
                    parser.finish()
                }
            };
            for data in payloads {
                if data == "[DONE]" {
                    finished = true;
                    break;
                }
                let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                if let Some(err) = v.get("error") {
                    bail!("模型接口报错：{err}");
                }
                if let Some(u) = Usage::from_value(&v) {
                    usage = Some(u);
                }
                let delta = &v["choices"][0]["delta"];
                if let Some(r) = delta["reasoning_content"].as_str().filter(|r| !r.is_empty()) {
                    if tx.send(LlmEvent::Reasoning(r.chars().count())).await.is_err() {
                        bail!(CANCELLED);
                    }
                }
                if let Some(c) = delta["content"].as_str().filter(|c| !c.is_empty()) {
                    let (emit, thinking) = filter.push(c);
                    if thinking > 0 && tx.send(LlmEvent::Reasoning(thinking)).await.is_err() {
                        bail!(CANCELLED);
                    }
                    if !emit.is_empty() {
                        full.push_str(&emit);
                        if tx.send(LlmEvent::Delta(emit)).await.is_err() {
                            bail!(CANCELLED);
                        }
                    }
                }
            }
        }
        let rest = filter.finish();
        if !rest.is_empty() {
            full.push_str(&rest);
            let _ = tx.send(LlmEvent::Delta(rest)).await;
        }
        Ok((full, usage))
    }
}

/// 修复中文模型常见的 JSON 错误：字符串里没转义的英文双引号（换成中文引号）和原始换行。
/// 判断依据：字符串里的 " 后面紧跟 , } ] : 或结尾时才是结束引号，否则是正文里的引号。
fn repair_json(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut in_str = false;
    let mut opening = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_str {
            if c == '"' {
                in_str = true;
                opening = true;
            }
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = chars.get(i + 1) {
                    out.push(*n);
                }
                i += 2;
                continue;
            }
            '"' => {
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if matches!(next, None | Some(',') | Some('}') | Some(']') | Some(':')) {
                    in_str = false;
                    out.push('"');
                } else {
                    out.push(if opening { '“' } else { '”' });
                    opening = !opening;
                }
            }
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

fn parse_loose(candidate: &str) -> Option<Value> {
    static TRAILING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let trailing = TRAILING.get_or_init(|| regex::Regex::new(r",\s*([}\]])").unwrap());
    if let Ok(v) = serde_json::from_str::<Value>(candidate) {
        return Some(v);
    }
    let fixed = trailing.replace_all(candidate, "$1").into_owned();
    if let Ok(v) = serde_json::from_str::<Value>(&fixed) {
        return Some(v);
    }
    serde_json::from_str::<Value>(&repair_json(&fixed)).ok()
}

/// 从模型输出里取出 JSON：兼容代码块包裹、前后多余文字、尾逗号、正文里的英文引号。
pub fn extract_json(text: &str) -> Result<Value> {
    let stripped = strip_think(text);
    let t = stripped.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Ok(v);
    }
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").or_else(|| after.strip_prefix("JSON")).unwrap_or(after);
        if let Some(end) = after.find("```") {
            if let Some(v) = parse_loose(after[..end].trim()) {
                return Ok(v);
            }
        }
    }
    if let Some(i) = t.find(|c: char| c == '{' || c == '[') {
        let close = if t[i..].starts_with('{') { '}' } else { ']' };
        if let Some(j) = t.rfind(close) {
            if j > i {
                if let Some(v) = parse_loose(&t[i..=j]) {
                    return Ok(v);
                }
            }
        }
    }
    bail!("模型没有返回合法的 JSON。开头：{} …… 结尾：{}", head_chars(t, 150), crate::text::tail_chars(t, 150))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(endpoint("https://api.x.com/v1/", "chat/completions"), "https://api.x.com/v1/chat/completions");
        assert_eq!(endpoint("https://api.x.com/v1/chat/completions", "models"), "https://api.x.com/v1/models");
    }

    #[test]
    fn sse_parser_handles_split_utf8() {
        let mut p = SseParser::default();
        let line = "data: {\"t\":\"林凡\"}\n\ndata: [DONE]\n".as_bytes();
        let (a, b) = line.split_at(12); // 在“林”字中间切开
        assert!(p.push(a).is_empty());
        assert_eq!(p.push(b), vec!["{\"t\":\"林凡\"}".to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn think_filter() {
        let mut f = ThinkFilter::default();
        assert_eq!(f.push("<thi").0, "");
        assert_eq!(f.push("nk>想一想").0, "");
        assert_eq!(f.push("</think>\n正文").0, "正文");
        assert_eq!(f.push("继续").0, "继续");
        let mut g = ThinkFilter::default();
        assert_eq!(g.push("你好").0, "你好");
        assert_eq!(strip_think("<think>abc</think>\n答案"), "答案");
    }

    #[test]
    fn json_extraction() {
        assert_eq!(extract_json("好的：\n```json\n{\"a\":1}\n```").unwrap()["a"], 1);
        assert_eq!(extract_json("结果如下 {\"a\":[1,2,],} 完").unwrap()["a"][1], 2);
        assert!(extract_json("没有 JSON").is_err());
    }

    #[test]
    fn repairs_unescaped_quotes_and_newlines() {
        let broken = "```json\n{\"characters\":[{\"name\":\"林凡\",\"description\":\"被判为\"杂灵根废柴\"，实为天才\n嘴毒心软\"}]}\n```";
        let v = extract_json(broken).unwrap();
        assert_eq!(v["characters"][0]["description"], "被判为“杂灵根废柴”，实为天才\n嘴毒心软");
        let v = extract_json("{\"a\": \"他说\"走\"\", \"b\": [1, 2,]}").unwrap();
        assert_eq!(v["a"], "他说“走”");
        assert_eq!(v["b"][1], 2);
    }
}
