use crate::db::{now, Db};
use crate::llm::{ChatRequest, Message};
use crate::memory::{self, BookData, ComposeOpts};
use crate::models::*;
use crate::state::AppState;
use crate::text::{count_words, is_valid_day, utc_day};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub msg: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.msg }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        let e: anyhow::Error = e.into();
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, msg: format!("{e:#}") }
    }
}

pub fn not_found(what: &str) -> AppError {
    AppError { status: StatusCode::NOT_FOUND, msg: format!("{what}不存在") }
}

pub fn bad_request(msg: impl Into<String>) -> AppError {
    AppError { status: StatusCode::BAD_REQUEST, msg: msg.into() }
}

pub fn upstream(e: anyhow::Error) -> AppError {
    AppError { status: StatusCode::BAD_GATEWAY, msg: format!("{e:#}") }
}

pub type ApiResult<T> = Result<Json<T>, AppError>;

fn ok() -> ApiResult<Value> {
    Ok(Json(json!({ "ok": true })))
}

/// PATCH：只覆盖请求体里出现的已知字段，受保护字段忽略。
pub fn apply_patch<T: Serialize + DeserializeOwned>(orig: &T, patch: &Value, protected: &[&str]) -> Result<T, AppError> {
    let mut v = serde_json::to_value(orig)?;
    let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) else {
        return Err(bad_request("请求体必须是 JSON 对象"));
    };
    for (k, val) in p {
        if !protected.contains(&k.as_str()) && obj.contains_key(k) {
            obj.insert(k.clone(), val.clone());
        }
    }
    serde_json::from_value(v).map_err(|e| bad_request(format!("字段格式不对：{e}")))
}

pub fn load_book_data(db: &Db, book_id: i64) -> Result<BookData, AppError> {
    let book = db.get_book(book_id)?.ok_or_else(|| not_found("作品"))?;
    let mut data = BookData::new(
        book,
        db.list_volumes(book_id)?,
        db.list_chapters(book_id)?,
        db.list_entries(book_id)?,
        db.list_threads(book_id)?,
        db.list_book_states(book_id)?,
    );
    data.relations = db.list_relations(book_id)?;
    data.reveals = db.list_reveals(book_id)?;
    data.reveal_events = db.list_book_reveal_events(book_id)?;
    data.progressions = db.list_book_progressions(book_id)?;
    data.knowledge = db.list_book_knowledge(book_id)?;
    data.events = db.list_events(book_id)?;
    Ok(data)
}

// ---------- 人物关系 ----------

pub async fn list_relations(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Vec<Relation>> {
    Ok(Json(st.db.list_relations(book_id)?))
}

pub async fn create_relation(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut r): Json<Relation>) -> ApiResult<Relation> {
    require_book(&st.db, book_id)?;
    if r.a_id == r.b_id || r.kind.trim().is_empty() {
        return Err(bad_request("需要选两个不同的设定，并写上关系"));
    }
    r.id = 0;
    r.book_id = book_id;
    Ok(Json(st.db.save_relation(&r)?))
}

pub async fn patch_relation(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Relation> {
    let r = st.db.get_relation(id)?.ok_or_else(|| not_found("关系"))?;
    let updated: Relation = apply_patch(&r, &p, &["id", "book_id", "updated_at"])?;
    Ok(Json(st.db.save_relation(&updated)?))
}

pub async fn delete_relation(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_relation(id)?;
    ok()
}

pub(crate) fn require_book(db: &Db, id: i64) -> Result<Book, AppError> {
    db.get_book(id)?.ok_or_else(|| not_found("作品"))
}

pub async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "name": "ai-novel", "version": env!("CARGO_PKG_VERSION") }))
}

// ---------- 设置 ----------

pub async fn get_settings(State(st): State<AppState>) -> ApiResult<Settings> {
    Ok(Json(st.db.get_settings()?.masked()))
}

pub async fn put_settings(State(st): State<AppState>, Json(mut s): Json<Settings>) -> ApiResult<Settings> {
    let old = st.db.get_settings()?;
    s.restore_masked_keys(&old);
    for (i, p) in s.providers.iter_mut().enumerate() {
        if p.id.trim().is_empty() {
            p.id = format!("p{}-{}", now(), i);
        }
        p.base_url = p.base_url.trim().to_string();
    }
    s.context_budget = s.context_budget.clamp(3000, 200_000);
    st.db.save_settings(&s)?;
    Ok(Json(s.masked()))
}

#[derive(Deserialize)]
pub struct ProviderTest {
    pub provider: Provider,
    #[serde(default)]
    pub model: String,
}

pub async fn test_provider(State(st): State<AppState>, Json(t): Json<ProviderTest>) -> ApiResult<Value> {
    let mut p = t.provider;
    if p.api_key.contains("****") {
        p.api_key = st.db.get_settings()?.providers.into_iter().find(|o| o.id == p.id).map(|o| o.api_key).unwrap_or_default();
    }
    let mut result = json!({});
    match st.llm.list_models(&p).await {
        Ok(models) => result["models"] = json!(models),
        Err(e) => result["models_error"] = json!(format!("{e:#}")),
    }
    if !t.model.trim().is_empty() {
        let req = ChatRequest {
            model: t.model.trim().to_string(),
            messages: vec![Message::user("这是连接测试，请只回复两个字：收到")],
            temperature: 0.1,
            ..Default::default()
        };
        let started = Instant::now();
        match st.llm.chat(&p, &req).await.map(|o| o.text) {
            Ok(reply) => {
                result["chat_ok"] = json!(true);
                result["reply"] = json!(reply);
                result["latency_ms"] = json!(started.elapsed().as_millis() as u64);
            }
            Err(e) => {
                result["chat_ok"] = json!(false);
                result["chat_error"] = json!(format!("{e:#}"));
            }
        }
    }
    Ok(Json(result))
}

// ---------- 作品 ----------

pub async fn list_books(State(st): State<AppState>) -> ApiResult<Vec<BookCard>> {
    Ok(Json(st.db.list_books()?))
}

pub async fn create_book(State(st): State<AppState>, Json(b): Json<Book>) -> ApiResult<Book> {
    if b.title.trim().is_empty() {
        return Err(bad_request("书名不能为空"));
    }
    Ok(Json(st.db.create_book(&b)?))
}

pub async fn get_book(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Book> {
    Ok(Json(require_book(&st.db, id)?))
}

pub async fn patch_book(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Book> {
    let book = require_book(&st.db, id)?;
    let updated: Book = apply_patch(&book, &p, &["id", "created_at", "updated_at"])?;
    if updated.title.trim().is_empty() {
        return Err(bad_request("书名不能为空"));
    }
    st.db.update_book(&updated)?;
    Ok(Json(require_book(&st.db, id)?))
}

pub async fn delete_book(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_book(id)?;
    ok()
}

pub async fn book_stats(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let book = require_book(&st.db, id)?;
    let metas = st.db.list_chapter_metas(id)?;
    let (draft_chapters, draft_words) =
        metas.iter().filter(|m| m.word_count > 0 && m.published_at.is_none()).fold((0, 0), |(n, w), m| (n + 1, w + m.word_count));
    let words: i64 = metas.iter().map(|m| m.word_count).sum();
    let ai_chars: i64 = metas.iter().map(|m| m.ai_chars).sum();
    let daily: Vec<Value> = st.db.daily_words(id, 30)?.into_iter().map(|(day, w)| json!({ "day": day, "words": w })).collect();
    let usage: Vec<Value> = st
        .db
        .ai_usage(id)?
        .into_iter()
        .map(|(task, n, input, output, pt, ct)| {
            json!({ "task": task, "count": n, "input_chars": input, "output_chars": output, "prompt_tokens": pt, "completion_tokens": ct })
        })
        .collect();
    let settings = st.db.get_settings()?;
    let price = |model: &str| -> (f64, f64) {
        [&settings.writer, &settings.analyst]
            .into_iter()
            .find(|r| !r.model.is_empty() && r.model == model)
            .map(|r| (r.input_price, r.output_price))
            .unwrap_or((0.0, 0.0))
    };
    let (mut cost, mut prompt_tokens, mut completion_tokens, mut estimated) = (0.0, 0, 0, false);
    for (model, p, c, est) in st.db.ai_tokens_by_model(id)? {
        let (ip, op) = price(&model);
        cost += p as f64 / 1_000_000.0 * ip + c as f64 / 1_000_000.0 * op;
        prompt_tokens += p;
        completion_tokens += c;
        estimated |= est;
    }
    let numbers: HashMap<i64, Option<i64>> = metas.iter().map(|m| (m.id, m.number)).collect();
    let curve: Vec<Value> = st
        .db
        .list_chapters(id)?
        .into_iter()
        .filter(|c| c.metrics.is_object())
        .map(|c| {
            let m = &c.metrics;
            json!({
                "id": c.id,
                "number": numbers.get(&c.id).copied().flatten(),
                "title": c.title,
                "tension": m["tension"],
                "emotion": m["emotion"],
                "hook": m["hook"]["type"],
                "cool_points": m["cool_points"],
                "debts": m["debts"],
            })
        })
        .collect();
    let per_chapter: Vec<Value> = metas
        .iter()
        .filter(|m| m.ai_chars > 0)
        .map(|m| json!({ "id": m.id, "number": m.number, "title": m.title, "words": m.word_count, "ai_chars": m.ai_chars }))
        .collect();
    Ok(Json(json!({
        "chapters": metas.len(),
        "done": metas.iter().filter(|m| m.status == "done").count(),
        "words": words,
        "ai_chars": ai_chars,
        "ai_ratio": if words > 0 { (ai_chars as f64 / words as f64).min(1.0) } else { 0.0 },
        "daily": daily,
        "ai_usage": usage,
        "ai_chapters": per_chapter,
        "tokens": { "prompt": prompt_tokens, "completion": completion_tokens, "estimated": estimated },
        "cost": (cost * 100.0).round() / 100.0,
        "curve": curve,
        "published": metas.iter().filter(|m| m.published_at.is_some()).count(),
        "drafts": {
            "chapters": draft_chapters,
            "words": draft_words,
            "update_target": book.update_target,
            "days": (book.update_target > 0).then(|| (draft_words as f64 / book.update_target as f64 * 10.0).floor() / 10.0),
        },
    })))
}

/// 网页文本框的 UTF-16 下标换成字节下标；超出正文或落在代理对中间时返回 None
fn utf16_to_byte(text: &str, at: usize) -> Option<usize> {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units == at {
            return Some(i);
        }
        units += c.len_utf16();
        if units > at {
            return None;
        }
    }
    (units == at).then_some(text.len())
}

#[derive(Deserialize)]
pub struct SplitRequest {
    /// 拆分位置：UTF-16 下标，和网页文本框的光标位置一致
    pub at: usize,
}

/// 在光标处拆成两章：前半留在原章，后半放进紧跟其后的新章；拆之前给原章存快照
pub async fn split_chapter(State(st): State<AppState>, Path(id): Path<i64>, Json(r): Json<SplitRequest>) -> ApiResult<Chapter> {
    let mut ch = st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?;
    let byte = utf16_to_byte(&ch.content, r.at).ok_or_else(|| bad_request("拆分位置超出了正文"))?;
    let (head, tail) = ch.content.split_at(byte);
    let (head, tail) = (head.trim_end().to_string(), tail.trim_start().to_string());
    if head.is_empty() || tail.is_empty() {
        return Err(bad_request("光标要放在正文中间，前后都得有内容"));
    }
    st.db.create_version(id, &ch.content, "拆章前")?;
    ch.content = head;
    st.db.update_chapter(&ch)?;
    let new = st.db.create_chapter(&Chapter {
        book_id: ch.book_id,
        volume_id: ch.volume_id,
        content: tail,
        status: "draft".into(),
        story_time: ch.story_time.clone(),
        story_day: ch.story_day,
        ..Default::default()
    })?;
    let mut ids: Vec<i64> = st.db.list_chapter_metas(ch.book_id)?.into_iter().map(|m| m.id).filter(|&x| x != new.id).collect();
    let pos = ids.iter().position(|&x| x == id).map_or(ids.len(), |p| p + 1);
    ids.insert(pos, new.id);
    st.db.reorder_chapters(ch.book_id, &ids)?;
    st.db.touch_book(ch.book_id)?;
    Ok(Json(st.db.get_chapter(new.id)?.ok_or_else(|| not_found("章节"))?))
}

/// 把下一章接到这一章后面并删掉下一章；它的版本、设定状态、揭示进度等都转到这一章，
/// 合并前这一章的原文、下一章的原文各存一个快照
pub async fn merge_next_chapter(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Chapter> {
    let mut ch = st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?;
    let metas = st.db.list_chapter_metas(ch.book_id)?;
    let pos = metas.iter().position(|m| m.id == id).ok_or_else(|| not_found("章节"))?;
    let next_meta = metas.get(pos + 1).ok_or_else(|| bad_request("已经是最后一章了"))?;
    let next = st.db.get_chapter(next_meta.id)?.ok_or_else(|| not_found("章节"))?;
    st.db.create_version(id, &ch.content, "合并前")?;
    st.db.create_version(id, &next.content, &format!("合并进来的「{}」原文", next_meta.label()))?;
    let join = |a: &str, b: &str, sep: &str| match (a.trim().is_empty(), b.trim().is_empty()) {
        (true, _) => b.trim().to_string(),
        (_, true) => a.trim().to_string(),
        _ => format!("{}{sep}{}", a.trim_end(), b.trim_start()),
    };
    let sep = if ch.content.contains("\n\n") { "\n\n" } else { "\n" };
    ch.content = join(&ch.content, &next.content, sep);
    ch.outline = join(&ch.outline, &next.outline, "\n");
    ch.summary = join(&ch.summary, &next.summary, "\n");
    ch.beats = join(&ch.beats, &next.beats, "\n");
    // 章末换成了下一章的结尾，张力和钩子要重新分析
    ch.metrics = Value::Null;
    if next.status != "done" {
        ch.status = next.status.clone();
    }
    if next.published_at.is_none() {
        ch.published_at = None;
    }
    if next.story_day.is_some() || !next.story_time.trim().is_empty() {
        ch.story_time = next.story_time.clone();
        ch.story_day = next.story_day;
    }
    st.db.update_chapter(&ch)?;
    st.db.add_ai_chars(id, next.ai_chars)?;
    st.db.repoint_chapter(next.id, id)?;
    st.db.delete_chapter(next.id)?;
    st.db.touch_book(ch.book_id)?;
    Ok(Json(st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?))
}

#[derive(Deserialize)]
pub struct PublishRequest {
    pub chapter_id: i64,
    pub published: bool,
}

/// 连载进度：标记到某一章为止都已发布，或者从某一章起取消发布
pub async fn publish_chapters(State(st): State<AppState>, Path(book_id): Path<i64>, Json(r): Json<PublishRequest>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    st.db.get_chapter(r.chapter_id)?.filter(|c| c.book_id == book_id).ok_or_else(|| not_found("章节"))?;
    let changed = st.db.publish_through(book_id, r.chapter_id, r.published)?;
    Ok(Json(json!({ "changed": changed })))
}

// ---------- 卷 ----------

pub async fn list_volumes(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Vec<Volume>> {
    Ok(Json(st.db.list_volumes(book_id)?))
}

pub async fn create_volume(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut v): Json<Volume>) -> ApiResult<Volume> {
    require_book(&st.db, book_id)?;
    v.book_id = book_id;
    if v.title.trim().is_empty() {
        let n = st.db.list_volumes(book_id)?.len() + 1;
        v.title = format!("第{}卷", crate::text::cn_number(n as i64));
    }
    Ok(Json(st.db.create_volume(&v)?))
}

pub async fn patch_volume(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Volume> {
    let v = st.db.get_volume(id)?.ok_or_else(|| not_found("卷"))?;
    let updated: Volume = apply_patch(&v, &p, &["id", "book_id"])?;
    st.db.update_volume(&updated)?;
    Ok(Json(st.db.get_volume(id)?.ok_or_else(|| not_found("卷"))?))
}

pub async fn delete_volume(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_volume(id)?;
    ok()
}

// ---------- 章节 ----------

pub async fn list_chapters(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Vec<ChapterMeta>> {
    Ok(Json(st.db.list_chapter_metas(book_id)?))
}

pub async fn create_chapter(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut c): Json<Chapter>) -> ApiResult<Chapter> {
    require_book(&st.db, book_id)?;
    c.book_id = book_id;
    let created = st.db.create_chapter(&c)?;
    st.db.touch_book(book_id)?;
    Ok(Json(created))
}

pub async fn get_chapter(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Chapter> {
    Ok(Json(st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?))
}

/// 额外支持两个字段：snapshot（内容变化前先存快照，值为备注）和 client_day（本地日期，用于日更统计）。
pub async fn patch_chapter(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Chapter> {
    let old = st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?;
    let updated: Chapter = apply_patch(&old, &p, &["id", "book_id", "created_at", "updated_at", "word_count", "ai_chars"])?;
    if let Some(note) = p.get("snapshot").and_then(|v| v.as_str()) {
        if old.content != updated.content && !old.content.trim().is_empty() {
            st.db.create_version(id, &old.content, note)?;
        }
    }
    st.db.update_chapter(&updated)?;
    let delta = count_words(&updated.content) - old.word_count;
    if delta > 0 {
        let day = p
            .get("client_day")
            .and_then(|v| v.as_str())
            .filter(|d| is_valid_day(d))
            .map(String::from)
            .unwrap_or_else(|| utc_day(now()));
        st.db.add_daily_words(old.book_id, &day, delta)?;
    }
    st.db.touch_book(old.book_id)?;
    Ok(Json(st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?))
}

pub async fn delete_chapter(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_chapter(id)?;
    ok()
}

pub async fn reorder_chapters(State(st): State<AppState>, Path(book_id): Path<i64>, Json(ids): Json<Vec<i64>>) -> ApiResult<Value> {
    st.db.reorder_chapters(book_id, &ids)?;
    ok()
}

/// 作者把 AI 生成的文字放进正文时记一笔，用于统计 AI 参与度。
/// 记录采纳的 AI 字数；带上原文时一并保存，之后和作者改过的现稿对比，用来提炼文风指南。
pub async fn ai_accept(State(st): State<AppState>, Path(id): Path<i64>, Json(v): Json<Value>) -> ApiResult<Value> {
    let n = v["chars"].as_i64().unwrap_or(0).max(0);
    st.db.add_ai_chars(id, n)?;
    if let Some(text) = v["text"].as_str().map(str::trim).filter(|t| t.chars().count() >= 8) {
        if let Some(ch) = st.db.get_chapter(id)? {
            let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
            let a = crate::db::Accepted { chapter_id: id, task: s("task"), text: text.to_string(), before: s("before"), after: s("after"), ..Default::default() };
            st.db.add_accept(ch.book_id, &a)?;
        }
    }
    ok()
}

// ---------- 版本快照 ----------

pub async fn list_versions(State(st): State<AppState>, Path(chapter_id): Path<i64>) -> ApiResult<Vec<Version>> {
    Ok(Json(st.db.list_versions(chapter_id)?))
}

pub async fn create_version(State(st): State<AppState>, Path(chapter_id): Path<i64>, Json(v): Json<Value>) -> ApiResult<Value> {
    let ch = st.db.get_chapter(chapter_id)?.ok_or_else(|| not_found("章节"))?;
    let note = v["note"].as_str().unwrap_or("手动快照");
    let id = st.db.create_version(chapter_id, &ch.content, note)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn get_version(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Version> {
    Ok(Json(st.db.get_version(id)?.ok_or_else(|| not_found("版本"))?))
}

pub async fn restore_version(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Chapter> {
    let v = st.db.get_version(id)?.ok_or_else(|| not_found("版本"))?;
    let mut ch = st.db.get_chapter(v.chapter_id)?.ok_or_else(|| not_found("章节"))?;
    st.db.create_version(ch.id, &ch.content, "恢复旧版本前自动备份")?;
    ch.content = v.content;
    st.db.update_chapter(&ch)?;
    Ok(Json(st.db.get_chapter(ch.id)?.ok_or_else(|| not_found("章节"))?))
}

// ---------- 设定库 ----------

pub async fn list_entries(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Vec<Entry>> {
    Ok(Json(st.db.list_entries(book_id)?))
}

/// 人物有结构化字段时，自由文本状态同步成字段的文字版。
fn sync_state_text(e: &mut Entry) {
    let rendered = render_fields(&e.fields);
    if !rendered.is_empty() {
        e.state = rendered;
    }
}

/// 可见性按统一写法存，写法看不懂时直接报错，免得存进去被当成对 AI 隐藏。
fn normalize_visibility(e: &mut Entry) -> Result<(), AppError> {
    e.visibility = crate::visibility::normalize(&e.visibility)
        .ok_or_else(|| bad_request(format!("可见性「{}」看不懂，可以写：公开、第2卷起、第91章起、仅规划、对AI隐藏", e.visibility.trim())))?;
    Ok(())
}

pub async fn create_entry(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut e): Json<Entry>) -> ApiResult<Entry> {
    require_book(&st.db, book_id)?;
    if e.name.trim().is_empty() {
        return Err(bad_request("名称不能为空"));
    }
    e.book_id = book_id;
    normalize_visibility(&mut e)?;
    sync_state_text(&mut e);
    let created = st.db.create_entry(&e)?;
    if !created.state.trim().is_empty() {
        st.db.put_entry_state(created.id, None, "start", &created.state, &created.fields)?;
    }
    Ok(Json(created))
}

/// 额外字段 as_of_chapter_id：状态从这一章开始生效；不传表示修改初始状态。
pub async fn patch_entry(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Entry> {
    let e = st.db.get_entry(id)?.ok_or_else(|| not_found("设定"))?;
    let mut updated: Entry = apply_patch(&e, &p, &["id", "book_id", "updated_at"])?;
    normalize_visibility(&mut updated)?;
    sync_state_text(&mut updated);
    st.db.update_entry(&updated)?;
    if updated.state != e.state || updated.fields != e.fields {
        let as_of = p.get("as_of_chapter_id").and_then(|v| v.as_i64());
        st.db.put_entry_state(id, as_of, "start", &updated.state, &updated.fields)?;
    }
    Ok(Json(st.db.get_entry(id)?.ok_or_else(|| not_found("设定"))?))
}

// ---------- 设定进展 ----------

/// 进展的生效位置按可见性的写法存；空着或写法看不懂时报错，免得存进去永远不生效。
fn normalize_progression(p: &mut Progression) -> Result<(), AppError> {
    p.gate = crate::visibility::normalize(&p.gate)
        .filter(|g| !g.is_empty())
        .ok_or_else(|| bad_request(format!("从哪里起生效「{}」看不懂，可以写：第91章起、第2卷起", p.gate.trim())))?;
    p.mode = if p.mode == "replace" { "replace".into() } else { "add".into() };
    if p.text.trim().is_empty() {
        return Err(bad_request("进展内容不能为空"));
    }
    Ok(())
}

pub async fn list_progressions(State(st): State<AppState>, Path(entry_id): Path<i64>) -> ApiResult<Vec<Progression>> {
    Ok(Json(st.db.list_entry_progressions(entry_id)?))
}

pub async fn create_progression(State(st): State<AppState>, Path(entry_id): Path<i64>, Json(mut p): Json<Progression>) -> ApiResult<Progression> {
    st.db.get_entry(entry_id)?.ok_or_else(|| not_found("设定"))?;
    p.id = 0;
    p.entry_id = entry_id;
    normalize_progression(&mut p)?;
    Ok(Json(st.db.save_progression(&p)?))
}

pub async fn patch_progression(State(st): State<AppState>, Path(id): Path<i64>, Json(v): Json<Value>) -> ApiResult<Progression> {
    let p = st.db.get_progression(id)?.ok_or_else(|| not_found("设定进展"))?;
    let mut updated: Progression = apply_patch(&p, &v, &["id", "entry_id", "created_at"])?;
    normalize_progression(&mut updated)?;
    Ok(Json(st.db.save_progression(&updated)?))
}

pub async fn delete_progression(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_progression(id)?;
    ok()
}

// ---------- 秘密台账（揭示计划） ----------

fn normalize_gap(gap: &str) -> String {
    ["惊奇", "悬念", "好奇"].into_iter().find(|g| gap.contains(g)).map_or_else(|| gap.trim().to_string(), str::to_string)
}

/// 秘密列表，附每条的揭示进度（带章号）、角色知识和写到第几章，给信息节奏面板画时间条和知情表；
/// cast 是知情表的列：主角、重要配角、反派、常驻人物，加上有角色知识记录的人物，主角排前面（hero）。
pub async fn list_reveals(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    let data = load_book_data(&st.db, book_id)?;
    let written = data.chapters.iter().filter(|c| !c.content.trim().is_empty()).filter_map(|c| data.number(c)).max().unwrap_or(0);
    let chapters = data.chapters.iter().filter_map(|c| data.number(c)).max().unwrap_or(0);
    let order = |k: &Knowledge| (k.chapter_id.and_then(|id| data.position(id)).map_or(-1, |p| p as i64), k.id);
    let reveals: Vec<Value> = data
        .reveals
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            v["events"] = json!(data
                .reveal_events_before(r, None)
                .into_iter()
                .map(|e| json!({ "id": e.id, "chapter_id": e.chapter_id, "number": data.event_number(e), "step": e.step, "quote": e.quote, "note": e.note }))
                .collect::<Vec<_>>());
            let mut knows: Vec<&Knowledge> = data.knowledge.iter().filter(|k| k.reveal_id == r.id).collect();
            knows.sort_by_key(|k| order(k));
            v["knows"] = json!(knows
                .into_iter()
                .map(|k| {
                    let number = k.chapter_id.and_then(|id| data.chapter(id)).and_then(|c| data.number(c));
                    json!({ "id": k.id, "entry_id": k.entry_id, "name": data.entry_name(k.entry_id), "chapter_id": k.chapter_id, "number": number,
                            "source": k.source, "misread": k.misread, "note": k.note, "quote": k.quote, "when": data.knowledge_when(k) })
                })
                .collect::<Vec<_>>());
            v
        })
        .collect();
    let heroes: Vec<i64> = data.protagonists(None).into_iter().map(|e| e.id).collect();
    let mut cast: Vec<&Entry> =
        data.entries.iter().filter(|e| e.kind == "character" && (e.is_major() || data.knowledge.iter().any(|k| k.entry_id == e.id))).collect();
    cast.sort_by_key(|e| !heroes.contains(&e.id));
    let cast: Vec<Value> = cast.into_iter().map(|e| json!({ "id": e.id, "name": e.name, "role": e.role, "hero": heroes.contains(&e.id) })).collect();
    Ok(Json(json!({ "reveals": reveals, "written": written, "chapters": chapters, "cast": cast })))
}

// ---------- 角色知识 ----------

/// 秘密、人物、章节都要是这本书的；来源统一写法。
fn check_knowledge(db: &Db, book_id: i64, k: &mut Knowledge) -> Result<(), AppError> {
    db.get_reveal(k.reveal_id)?.filter(|r| r.book_id == book_id).ok_or_else(|| not_found("秘密"))?;
    db.get_entry(k.entry_id)?.filter(|e| e.book_id == book_id).ok_or_else(|| not_found("人物"))?;
    if let Some(cid) = k.chapter_id {
        db.get_chapter(cid)?.filter(|c| c.book_id == book_id).ok_or_else(|| not_found("章节"))?;
    }
    k.source = normalize_source(&k.source);
    Ok(())
}

pub async fn create_knowledge(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut k): Json<Knowledge>) -> ApiResult<Knowledge> {
    require_book(&st.db, book_id)?;
    k.id = 0;
    check_knowledge(&st.db, book_id, &mut k)?;
    Ok(Json(st.db.save_knowledge(&k)?))
}

pub async fn patch_knowledge(State(st): State<AppState>, Path(id): Path<i64>, Json(v): Json<Value>) -> ApiResult<Knowledge> {
    let k = st.db.get_knowledge(id)?.ok_or_else(|| not_found("角色知识"))?;
    let book_id = st.db.get_reveal(k.reveal_id)?.map(|r| r.book_id).ok_or_else(|| not_found("秘密"))?;
    let mut updated: Knowledge = apply_patch(&k, &v, &["id", "created_at"])?;
    check_knowledge(&st.db, book_id, &mut updated)?;
    Ok(Json(st.db.save_knowledge(&updated)?))
}

pub async fn delete_knowledge(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_knowledge(id)?;
    ok()
}

/// 正文的指纹，用来判断逻辑审校之后正文改没改过（FNV-1a）
pub(crate) fn content_hash(text: &str) -> String {
    let h = text.trim().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// 这一章上次模型逻辑审校的结果；正文之后改过时 stale 为 true。没审过时返回 null。
pub async fn chapter_logic(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let ch = st.db.get_chapter(id)?.ok_or_else(|| not_found("章节"))?;
    let mut v = st.db.chapter_logic(id)?;
    if v.is_object() {
        v["stale"] = json!(v["hash"].as_str() != Some(content_hash(&ch.content).as_str()));
    }
    Ok(Json(v))
}

/// 模型给的是非：true、"true"、"是"、"误会" 都算是。
fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().is_some_and(|n| n != 0),
        Value::String(s) => matches!(s.trim(), "true" | "是" | "误会" | "yes" | "1"),
        _ => false,
    }
}

fn check_reveal(r: &mut Reveal) -> Result<(), AppError> {
    if r.title.trim().is_empty() {
        return Err(bad_request("秘密要有个标题"));
    }
    r.gap = normalize_gap(&r.gap);
    r.terms = split_terms(&r.terms).join("、");
    r.exceptions = split_terms(&r.exceptions).join("、");
    Ok(())
}

pub async fn create_reveal(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut r): Json<Reveal>) -> ApiResult<Reveal> {
    require_book(&st.db, book_id)?;
    r.id = 0;
    r.book_id = book_id;
    check_reveal(&mut r)?;
    Ok(Json(st.db.save_reveal(&r)?))
}

pub async fn patch_reveal(State(st): State<AppState>, Path(id): Path<i64>, Json(v): Json<Value>) -> ApiResult<Reveal> {
    let r = st.db.get_reveal(id)?.ok_or_else(|| not_found("秘密"))?;
    let mut updated: Reveal = apply_patch(&r, &v, &["id", "book_id", "updated_at"])?;
    check_reveal(&mut updated)?;
    Ok(Json(st.db.save_reveal(&updated)?))
}

pub async fn delete_reveal(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_reveal(id)?;
    ok()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct EventIn {
    pub chapter_id: i64,
    pub step: String,
    pub quote: String,
    pub note: String,
}

pub async fn add_reveal_event(State(st): State<AppState>, Path(id): Path<i64>, Json(e): Json<EventIn>) -> ApiResult<Value> {
    let r = st.db.get_reveal(id)?.ok_or_else(|| not_found("秘密"))?;
    let step = normalize_step(&e.step).ok_or_else(|| bad_request("步骤只能是 seed（埋种子）、clue（给线索）、reveal（揭开）"))?;
    st.db.get_chapter(e.chapter_id)?.filter(|c| c.book_id == r.book_id).ok_or_else(|| not_found("章节"))?;
    let id = st.db.add_reveal_event(&RevealEvent { reveal_id: id, chapter_id: e.chapter_id, step: step.into(), quote: e.quote, note: e.note, ..Default::default() })?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_reveal_event(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_reveal_event(id)?;
    ok()
}

/// 模型给的词表可能是数组，也可能是一串用顿号隔开的字。
fn value_terms(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items.iter().filter_map(|x| x.as_str()).flat_map(split_terms).collect(),
        Value::String(s) => split_terms(s),
        _ => Vec::new(),
    }
}

/// 模型给的章号可能是 12、"12"、"第12章"。
fn value_chapter(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().filter(|n| *n > 0),
        Value::String(s) => {
            let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
            digits.parse().ok().or_else(|| crate::text::parse_cn_number(s.trim().trim_start_matches('第').trim_end_matches('章'))).filter(|n| *n > 0)
        }
        _ => None,
    }
}

/// 给 reveal_plan 的草稿标上 seen：哪些泄露词在揭开之前的已写章节里已经出现过，审核时提醒作者删掉。
/// 揭开的章按采纳时的算法定：已写章节里做过的 reveal（章节存在才算），否则用计划的 reveal_at。
pub fn mark_early_terms(d: &BookData, plan: &mut Value) {
    let exists = |n: &i64| d.chapters.iter().any(|c| d.number(c) == Some(*n));
    let Some(secrets) = plan.get_mut("secrets").and_then(Value::as_array_mut) else { return };
    for s in secrets {
        let done_reveal = s["done"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| normalize_step(x["step"].as_str().unwrap_or("")) == Some("reveal"))
            .filter_map(|x| value_chapter(&x["chapter"]))
            .find(exists);
        let reveal = done_reveal.or_else(|| value_chapter(&s["reveal_at"]));
        let seen = crate::logic::early_terms(d, &value_terms(&s["terms"]), &value_terms(&s["exceptions"]), reveal);
        if let Some(obj) = s.as_object_mut() {
            obj.insert("seen".into(), seen.into_iter().map(|(term, chapter)| json!({ "term": term, "chapter": chapter })).collect());
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct PlanApply {
    /// 先删掉这本书原有的秘密和揭示进度，整份换成新的
    pub replace: bool,
    pub secrets: Vec<Value>,
}

/// 采纳 reveal_plan 生成、作者审过的揭示计划：同标题的秘密更新，其余新建；已写章节里做过的步骤记成揭示进度，
/// knows 里开篇前就知道（chapter 为空或 0）和在已有章节里知道的人记成角色知识，对不上的人名和章号跳过。
pub async fn apply_reveal_plan(State(st): State<AppState>, Path(book_id): Path<i64>, Json(p): Json<PlanApply>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    let data = load_book_data(&st.db, book_id)?;
    if p.replace {
        st.db.delete_book_reveals(book_id)?;
    }
    let existing = if p.replace { Vec::new() } else { st.db.list_reveals(book_id)? };
    let entry_id = |name: &str| data.entries.iter().find(|e| e.keywords().iter().any(|k| k == name)).map(|e| e.id);
    let chapter_id = |n: i64| data.chapters.iter().find(|c| data.number(c) == Some(n)).map(|c| c.id);
    let text = |s: &Value, k: &str| s[k].as_str().unwrap_or("").trim().to_string();
    let (mut created, mut updated, mut events, mut knows) = (0, 0, 0, 0);
    for (i, s) in p.secrets.iter().enumerate() {
        let title = text(s, "title");
        if title.is_empty() {
            continue;
        }
        let mut r = existing.iter().find(|r| r.title.trim() == title).cloned().unwrap_or(Reveal { book_id, ..Default::default() });
        let is_new = r.id == 0;
        r.title = title;
        r.truth = text(s, "truth");
        r.misread = text(s, "misread");
        r.gap = normalize_gap(&text(s, "gap"));
        r.terms = value_terms(&s["terms"]).join("、");
        r.exceptions = value_terms(&s["exceptions"]).join("、");
        r.entry_ids = value_terms(&s["entries"]).iter().filter_map(|n| entry_id(n)).collect();
        r.entry_ids.dedup();
        r.seed_at = value_chapter(&s["seed_at"]);
        r.clue_at = value_chapter(&s["clue_at"]);
        r.reveal_at = value_chapter(&s["reveal_at"]);
        r.seed_note = text(s, "seed_note");
        r.clue_note = text(s, "clue_note");
        r.payoff = text(s, "payoff");
        r.status = "active".into();
        r.sort = i as i64;
        let saved = st.db.save_reveal(&r)?;
        if is_new {
            created += 1;
        } else {
            updated += 1;
        }
        for d in s["done"].as_array().into_iter().flatten() {
            let (Some(step), Some(cid)) = (normalize_step(d["step"].as_str().unwrap_or("")), value_chapter(&d["chapter"]).and_then(chapter_id)) else { continue };
            st.db.add_reveal_event(&RevealEvent { reveal_id: saved.id, chapter_id: cid, step: step.into(), quote: text(d, "quote"), note: text(d, "note"), ..Default::default() })?;
            events += 1;
        }
        for k in s["knows"].as_array().into_iter().flatten() {
            let Some(eid) = entry_id(&text(k, "who")) else { continue };
            let chapter = match value_chapter(&k["chapter"]) {
                None => None,
                Some(n) => match chapter_id(n) {
                    Some(id) => Some(id),
                    None => continue,
                },
            };
            let source = text(k, "source");
            st.db.save_knowledge(&Knowledge { reveal_id: saved.id, entry_id: eid, chapter_id: chapter, source, misread: truthy(&k["misread"]), note: text(k, "note"), ..Default::default() })?;
            knows += 1;
        }
    }
    Ok(Json(json!({ "created": created, "updated": updated, "events": events, "knows": knows })))
}

pub async fn entry_states(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Vec<EntryState>> {
    Ok(Json(st.db.list_entry_states(id)?))
}

pub async fn delete_entry_state(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_entry_state(id)?;
    ok()
}

pub async fn delete_entry(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_entry(id)?;
    ok()
}

// ---------- 伏笔 ----------

pub async fn list_threads(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Vec<Thread>> {
    Ok(Json(st.db.list_threads(book_id)?))
}

pub async fn create_thread(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut t): Json<Thread>) -> ApiResult<Thread> {
    require_book(&st.db, book_id)?;
    if t.title.trim().is_empty() {
        return Err(bad_request("伏笔标题不能为空"));
    }
    t.book_id = book_id;
    Ok(Json(st.db.create_thread(&t)?))
}

pub async fn patch_thread(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Thread> {
    let t = st.db.get_thread(id)?.ok_or_else(|| not_found("伏笔"))?;
    let updated: Thread = apply_patch(&t, &p, &["id", "book_id", "updated_at"])?;
    st.db.update_thread(&updated)?;
    Ok(Json(st.db.get_thread(id)?.ok_or_else(|| not_found("伏笔"))?))
}

pub async fn delete_thread(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_thread(id)?;
    ok()
}

// ---------- 定稿：把提取出的设定变化写回设定库 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Updates {
    pub chapter_id: Option<i64>,
    pub entries: Vec<EntryUpdate>,
    pub threads_new: Vec<ThreadNew>,
    pub threads_progressed: Vec<ThreadResolved>,
    pub threads_resolved: Vec<ThreadResolved>,
    pub relations: Vec<RelationUpdate>,
    /// 本章对哪些秘密埋了种子、给了线索或揭开了
    pub reveals: Vec<RevealStep>,
    /// 计划外的新秘密
    pub reveals_new: Vec<RevealNew>,
    /// 本章里谁知道了（或误会了）哪个秘密
    pub knowledge: Vec<KnowledgeIn>,
    /// 本章结束时的故事时间
    pub time: TimeUpdate,
    pub events: Vec<EventNew>,
    pub deadlines_new: Vec<DeadlineNew>,
    pub deadlines_done: Vec<ThreadResolved>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct TimeUpdate {
    pub story_time: String,
    pub day: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct EventNew {
    pub title: String,
    pub who: String,
    pub time: String,
    pub day: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct DeadlineNew {
    pub title: String,
    pub who: String,
    pub due: String,
    pub due_day: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RevealStep {
    pub id: i64,
    pub step: String,
    pub quote: String,
    pub note: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct KnowledgeIn {
    /// 人物名
    pub who: String,
    /// 秘密编号
    pub id: i64,
    pub source: String,
    pub misread: Value,
    pub note: String,
    pub quote: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RevealNew {
    pub title: String,
    pub truth: String,
    pub gap: String,
    pub quote: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RelationUpdate {
    pub a: String,
    pub b: String,
    pub kind: String,
    pub detail: String,
    pub status: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct EntryUpdate {
    pub name: String,
    pub kind: String,
    pub aliases: String,
    pub description: String,
    pub state: String,
    pub fields: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ThreadNew {
    pub title: String,
    pub detail: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ThreadResolved {
    pub id: i64,
    pub note: String,
}

pub async fn apply_updates(State(st): State<AppState>, Path(book_id): Path<i64>, Json(u): Json<Updates>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    let existing = st.db.list_entries(book_id)?;
    let (mut created, mut updated) = (0, 0);
    for e in u.entries {
        let name = e.name.trim();
        if name.is_empty() {
            continue;
        }
        let fields: std::collections::BTreeMap<String, String> =
            e.fields.iter().filter(|(_, v)| !v.trim().is_empty()).map(|(k, v)| (k.clone(), v.trim().to_string())).collect();
        match existing.iter().find(|x| x.keywords().iter().any(|k| k == name)) {
            Some(found) => {
                let mut ne = found.clone();
                if !fields.is_empty() {
                    ne.fields.extend(fields);
                    sync_state_text(&mut ne);
                } else if !e.state.trim().is_empty() {
                    ne.state = e.state.trim().to_string();
                }
                let mut aliases = ne.keywords();
                let incoming = Entry { aliases: e.aliases.clone(), ..Default::default() };
                for a in incoming.keywords() {
                    if !aliases.contains(&a) {
                        aliases.push(a);
                    }
                }
                ne.aliases = aliases.into_iter().filter(|a| *a != ne.name).collect::<Vec<_>>().join("、");
                if ne.description.trim().is_empty() {
                    ne.description = e.description.trim().to_string();
                }
                st.db.update_entry(&ne)?;
                if ne.state != found.state || ne.fields != found.fields {
                    st.db.put_entry_state(ne.id, u.chapter_id, "end", &ne.state, &ne.fields)?;
                }
                updated += 1;
            }
            None => {
                let mut ne = Entry {
                    book_id,
                    kind: e.kind,
                    name: name.to_string(),
                    aliases: e.aliases,
                    description: e.description,
                    state: e.state,
                    fields,
                    ..Default::default()
                };
                sync_state_text(&mut ne);
                let made = st.db.create_entry(&ne)?;
                if !made.state.trim().is_empty() {
                    st.db.put_entry_state(made.id, u.chapter_id, "end", &made.state, &made.fields)?;
                }
                created += 1;
            }
        }
    }
    let mut threads_progressed = 0;
    for r in u.threads_progressed {
        if let Some(mut t) = st.db.get_thread(r.id)? {
            if t.book_id != book_id || t.status != "open" {
                continue;
            }
            t.last_chapter_id = u.chapter_id.or(t.last_chapter_id);
            if !r.note.trim().is_empty() {
                t.detail = format!("{}\n【推进】{}", t.detail.trim(), r.note.trim()).trim().to_string();
            }
            st.db.update_thread(&t)?;
            threads_progressed += 1;
        }
    }
    let mut threads_added = 0;
    for t in u.threads_new.into_iter().filter(|t| !t.title.trim().is_empty()) {
        st.db.create_thread(&Thread {
            book_id,
            title: t.title,
            detail: t.detail,
            status: "open".into(),
            planted_chapter_id: u.chapter_id,
            ..Default::default()
        })?;
        threads_added += 1;
    }
    let mut threads_resolved = 0;
    for r in u.threads_resolved {
        if let Some(mut t) = st.db.get_thread(r.id)? {
            if t.book_id != book_id || t.status != "open" {
                continue;
            }
            t.status = "resolved".into();
            t.resolved_chapter_id = u.chapter_id;
            if !r.note.trim().is_empty() {
                t.detail = format!("{}\n【回收】{}", t.detail.trim(), r.note.trim()).trim().to_string();
            }
            st.db.update_thread(&t)?;
            threads_resolved += 1;
        }
    }
    // 关系：按名称找到双方，已有同一对关系就更新，否则新建
    let entries_now = st.db.list_entries(book_id)?;
    let find = |name: &str| entries_now.iter().find(|e| e.keywords().iter().any(|k| k == name.trim())).map(|e| e.id);
    let existing_rel = st.db.list_relations(book_id)?;
    let mut relations = 0;
    for r in u.relations.iter().filter(|r| !r.kind.trim().is_empty()) {
        let (Some(a), Some(b)) = (find(&r.a), find(&r.b)) else { continue };
        if a == b {
            continue;
        }
        let mut rel = existing_rel
            .iter()
            .find(|x| (x.a_id == a && x.b_id == b) || (x.a_id == b && x.b_id == a))
            .cloned()
            .unwrap_or(Relation { book_id, a_id: a, b_id: b, since_chapter_id: u.chapter_id, ..Default::default() });
        rel.kind = r.kind.trim().to_string();
        if !r.detail.trim().is_empty() {
            rel.detail = r.detail.trim().to_string();
        }
        let ended_now = r.status == "ended" && rel.status != "ended";
        rel.status = if r.status == "ended" { "ended".into() } else { "active".into() };
        if ended_now {
            rel.until_chapter_id = u.chapter_id;
        }
        st.db.save_relation(&rel)?;
        relations += 1;
    }
    // 角色知识：只认这本书的秘密和设定库里的人，记成本章知道的
    let mut knowledge = 0;
    if let Some(cid) = u.chapter_id {
        let known = st.db.list_reveals(book_id)?;
        for k in u.knowledge.iter().filter(|k| known.iter().any(|r| r.id == k.id)) {
            let Some(eid) = find(&k.who) else { continue };
            st.db.save_knowledge(&Knowledge {
                reveal_id: k.id,
                entry_id: eid,
                chapter_id: Some(cid),
                source: k.source.clone(),
                misread: truthy(&k.misread),
                note: k.note.clone(),
                quote: k.quote.clone(),
                ..Default::default()
            })?;
            knowledge += 1;
        }
    }
    // 揭示进度：只认这本书的秘密；计划外的新秘密记成从本章埋下
    let (mut reveal_steps, mut reveals_added) = (0, 0);
    if let Some(cid) = u.chapter_id {
        let known = st.db.list_reveals(book_id)?;
        for s in &u.reveals {
            let Some(step) = normalize_step(&s.step) else { continue };
            if !known.iter().any(|r| r.id == s.id) {
                continue;
            }
            st.db.add_reveal_event(&RevealEvent { reveal_id: s.id, chapter_id: cid, step: step.into(), quote: s.quote.clone(), note: s.note.clone(), ..Default::default() })?;
            reveal_steps += 1;
        }
        let number = st.db.list_chapter_metas(book_id)?.into_iter().find(|m| m.id == cid).and_then(|m| m.number);
        for n in u.reveals_new.iter().filter(|n| !n.title.trim().is_empty()) {
            if known.iter().any(|r| r.title.trim() == n.title.trim()) {
                continue;
            }
            let r = st.db.save_reveal(&Reveal { book_id, title: n.title.clone(), truth: n.truth.trim().to_string(), gap: normalize_gap(&n.gap), seed_at: number, ..Default::default() })?;
            st.db.add_reveal_event(&RevealEvent { reveal_id: r.id, chapter_id: cid, step: "seed".into(), quote: n.quote.clone(), ..Default::default() })?;
            reveals_added += 1;
        }
    }
    // 时间线：本章结束时的时间、关键事件（重新定稿时换掉上次的）、新定下和了结的时限
    let (mut events, mut deadlines_added, mut deadlines_done) = (0, 0, 0);
    if let Some(cid) = u.chapter_id {
        if !u.time.story_time.trim().is_empty() || u.time.day.is_some() {
            if let Some(mut ch) = st.db.get_chapter(cid)?.filter(|c| c.book_id == book_id) {
                ch.story_time = u.time.story_time.trim().to_string();
                ch.story_day = u.time.day.filter(|d| *d > 0);
                st.db.update_chapter(&ch)?;
            }
        }
        let fresh: Vec<&EventNew> = u.events.iter().filter(|e| !e.title.trim().is_empty()).collect();
        if !fresh.is_empty() {
            st.db.delete_chapter_events(cid)?;
        }
        for e in fresh {
            st.db.save_event(&Event { book_id, chapter_id: Some(cid), kind: "event".into(), title: e.title.clone(), who: e.who.clone(), story_time: e.time.clone(), day: e.day, ..Default::default() })?;
            events += 1;
        }
        let existing = st.db.list_events(book_id)?;
        for d in u.deadlines_new.iter().filter(|d| !d.title.trim().is_empty()) {
            if existing.iter().any(|x| x.kind == "deadline" && x.status != "done" && x.title.trim() == d.title.trim()) {
                continue;
            }
            st.db.save_event(&Event { book_id, chapter_id: Some(cid), kind: "deadline".into(), title: d.title.clone(), who: d.who.clone(), story_time: d.due.clone(), day: d.due_day, ..Default::default() })?;
            deadlines_added += 1;
        }
        for r in &u.deadlines_done {
            let Some(mut e) = existing.iter().find(|x| x.id == r.id && x.kind == "deadline" && x.status != "done").cloned() else { continue };
            e.status = "done".into();
            e.done_chapter_id = Some(cid);
            if !r.note.trim().is_empty() {
                e.detail = format!("{}\n【了结】{}", e.detail.trim(), r.note.trim()).trim().to_string();
            }
            st.db.save_event(&e)?;
            deadlines_done += 1;
        }
    }
    Ok(Json(json!({
        "events": events,
        "deadlines_added": deadlines_added,
        "deadlines_done": deadlines_done,
        "created": created,
        "updated": updated,
        "threads_added": threads_added,
        "threads_progressed": threads_progressed,
        "threads_resolved": threads_resolved,
        "relations": relations,
        "reveal_steps": reveal_steps,
        "reveals_added": reveals_added,
        "knowledge": knowledge,
    })))
}

// ---------- 连续性体检、提示词模板、内置资料 ----------

pub async fn continuity(State(st): State<AppState>, Path(book_id): Path<i64>, Query(q): Query<ContextQuery>) -> ApiResult<Value> {
    let data = load_book_data(&st.db, book_id)?;
    let current = q.chapter_id.and_then(|id| data.chapter(id));
    let findings = crate::continuity::check(&data, current);
    Ok(Json(json!({ "findings": findings })))
}

pub async fn list_prompts(State(st): State<AppState>) -> ApiResult<Value> {
    let overrides = st.db.prompt_overrides()?;
    let list: Vec<Value> = crate::prompts::TEMPLATES
        .iter()
        .map(|t| {
            let custom = overrides.get(t.id).filter(|x| !x.trim().is_empty());
            json!({
                "id": t.id,
                "name": t.name,
                "group": t.group,
                "description": t.description,
                "vars": t.vars.iter().map(|(k, d)| json!({ "name": k, "desc": d })).collect::<Vec<_>>(),
                "default": t.text,
                "text": custom.map(|s| s.as_str()).unwrap_or(t.text),
                "custom": custom.is_some(),
            })
        })
        .collect();
    let craft: Vec<Value> = crate::library::CRAFT.iter().map(|c| json!({ "id": c.id, "title": c.title })).collect();
    Ok(Json(json!({ "templates": list, "craft": craft })))
}

pub async fn put_prompt(State(st): State<AppState>, Path(id): Path<String>, Json(v): Json<Value>) -> ApiResult<Value> {
    let t = crate::prompts::template(&id).ok_or_else(|| not_found("提示词模板"))?;
    let text = v["text"].as_str().unwrap_or("");
    let text = if text.trim() == t.text.trim() { "" } else { text };
    st.db.set_prompt_override(&id, text)?;
    Ok(Json(json!({ "ok": true, "custom": !text.trim().is_empty() })))
}

pub async fn genres() -> Json<Value> {
    Json(json!(crate::library::genres()))
}

pub async fn craft_list() -> Json<Value> {
    Json(json!(crate::library::CRAFT))
}

pub async fn craft_doc(Path(id): Path<String>) -> ApiResult<Value> {
    let text = crate::library::craft_text(&id).ok_or_else(|| not_found("手册"))?;
    let meta = crate::library::CRAFT.iter().find(|c| c.id == id).copied();
    Ok(Json(json!({ "meta": meta, "text": text })))
}

// ---------- 上下文预览、质量检查 ----------

#[derive(Deserialize)]
pub struct ContextQuery {
    pub chapter_id: Option<i64>,
}

pub async fn context_preview(State(st): State<AppState>, Path(book_id): Path<i64>, Query(q): Query<ContextQuery>) -> ApiResult<Value> {
    let data = load_book_data(&st.db, book_id)?;
    let settings = st.db.get_settings()?;
    let current = q.chapter_id.and_then(|id| data.chapter(id));
    let opts = ComposeOpts {
        current,
        focus_text: current.map(|c| c.content.as_str()).unwrap_or(""),
        instruction: "",
        budget: settings.context_budget,
        include_world: true,
        include_outline: true,
    };
    let sections = memory::compose(&data, &opts);
    let text = memory::render(&sections);
    let at = data.at(current);
    let withheld = json!({
        "worldview": crate::visibility::withheld(&data.book.worldview, at, data.uses_volumes()),
        "outline": crate::visibility::withheld(&data.book.outline, at, data.uses_volumes()),
        "entries": data.entries.iter().filter(|e| !e.gate().open_at(at)).map(|e| json!({ "id": e.id, "name": e.name, "visibility": e.gate() })).collect::<Vec<_>>(),
    });
    Ok(Json(json!({ "sections": sections, "chars": text.chars().count(), "budget": settings.context_budget, "withheld": withheld })))
}

// ---------- 分节可见性 ----------

pub async fn get_visibility(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Value> {
    use crate::visibility::describe;
    let book = require_book(&st.db, book_id)?;
    let volumes = st.db.list_volumes(book_id)?;
    let uses = !volumes.is_empty();
    Ok(Json(json!({
        "uses_volumes": uses,
        "worldview": describe(&book.worldview, uses),
        "outline": describe(&book.outline, uses),
        "volumes": volumes.iter().map(|v| json!({ "id": v.id, "title": v.title, "parts": describe(&v.outline, false) })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct SetVisibility {
    /// worldview / outline / volume
    pub field: String,
    #[serde(default)]
    pub volume_id: Option<i64>,
    pub index: usize,
    /// 空字符串表示去掉标记、按标题推断
    #[serde(default)]
    pub visibility: String,
}

/// 改世界观、总纲或卷纲里某一节标题末尾的可见性标记。
pub async fn set_visibility(State(st): State<AppState>, Path(book_id): Path<i64>, Json(r): Json<SetVisibility>) -> ApiResult<Value> {
    use crate::visibility::{set_marker, Gate};
    let gate = match r.visibility.trim() {
        "" => None,
        s => Some(Gate::parse(s).ok_or_else(|| bad_request(format!("可见性「{s}」看不懂，可以写：公开、第2卷起、第91章起、仅规划、对AI隐藏")))?),
    };
    let missing = || bad_request("没有这一节，可能刚改过原文，请刷新后再试");
    match r.field.as_str() {
        "worldview" | "outline" => {
            let mut book = require_book(&st.db, book_id)?;
            let text = if r.field == "worldview" { &mut book.worldview } else { &mut book.outline };
            *text = set_marker(text, r.index, gate).ok_or_else(missing)?;
            st.db.update_book(&book)?;
        }
        "volume" => {
            let mut v = match r.volume_id {
                Some(id) => st.db.get_volume(id)?.filter(|v| v.book_id == book_id).ok_or_else(|| not_found("卷"))?,
                None => return Err(bad_request("缺少 volume_id")),
            };
            v.outline = set_marker(&v.outline, r.index, gate).ok_or_else(missing)?;
            st.db.update_volume(&v)?;
        }
        _ => return Err(bad_request("field 只能是 worldview、outline 或 volume")),
    }
    get_visibility(State(st), Path(book_id)).await
}

pub async fn normalize_text(Json(v): Json<Value>) -> Json<Value> {
    let text = v["text"].as_str().unwrap_or_default();
    let fixed = crate::text::normalize_punct(text);
    let changed = text.chars().zip(fixed.chars()).filter(|(a, b)| a != b).count() + text.chars().count().abs_diff(fixed.chars().count());
    Json(json!({ "text": fixed, "changed": changed }))
}

#[derive(Deserialize)]
pub struct LintRequest {
    pub text: String,
    #[serde(default)]
    pub book_id: Option<i64>,
    #[serde(default)]
    pub chapter_id: Option<i64>,
}

/// 传了 book_id 和 chapter_id 时，额外和前 10 章做跨章重复检查；始终和文风库范文比对，防止照抄。
pub async fn lint(State(st): State<AppState>, Json(r): Json<LintRequest>) -> ApiResult<crate::lint::LintReport> {
    let settings = st.db.get_settings()?;
    let mut report = crate::lint::lint(&r.text, &settings.extra_cliches);
    let flagged = crate::lint::sensitive_issues(&r.text, &settings.sensitive_words, &settings.sensitive_ignore);
    if !flagged.is_empty() {
        let penalty: i32 = flagged.iter().map(|i| match i.severity.as_str() { "high" => 6, "medium" => 3, _ => 1 }).sum();
        report.score = (report.score - penalty).max(0);
        report.issues.splice(0..0, flagged);
    }
    let copied = crate::lint::source_overlap(&r.text, &st.db.lib_sources(r.book_id)?);
    if !copied.is_empty() {
        report.score = (report.score - copied.len() as i32 * 5).max(0);
        report.issues.splice(0..0, copied);
    }
    if let (Some(book_id), Some(chapter_id)) = (r.book_id, r.chapter_id) {
        let chapters = st.db.list_chapters(book_id)?;
        if let Some(pos) = chapters.iter().position(|c| c.id == chapter_id) {
            let previous: Vec<(String, String)> = chapters[pos.saturating_sub(10)..pos]
                .iter()
                .filter(|c| !c.content.trim().is_empty())
                .map(|c| (c.title.clone(), c.content.clone()))
                .collect();
            let extra = crate::lint::cross_chapter(&r.text, &previous);
            report.score = (report.score - extra.len() as i32 * 2).max(0);
            report.issues.extend(extra);
        }
    }
    Ok(Json(report))
}
