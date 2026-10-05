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

fn require_book(db: &Db, id: i64) -> Result<Book, AppError> {
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
    require_book(&st.db, id)?;
    let metas = st.db.list_chapter_metas(id)?;
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
    })))
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
        rel.status = if r.status == "ended" { "ended".into() } else { "active".into() };
        st.db.save_relation(&rel)?;
        relations += 1;
    }
    Ok(Json(json!({
        "created": created,
        "updated": updated,
        "threads_added": threads_added,
        "threads_progressed": threads_progressed,
        "threads_resolved": threads_resolved,
        "relations": relations,
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
