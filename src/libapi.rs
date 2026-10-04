//! 文风库接口：范文增删改查、导入、AI 分析、检索测试、文风指南的版本与提炼、修改样本、向量生成。

use crate::ai::{complete_json, complete_text, AiRequest, Prepared, JSON_RULE};
use crate::api::{apply_patch, bad_request, not_found, ApiResult, AppError};
use crate::db::Db;
use crate::importer;
use crate::lint::style_stats_text;
use crate::llm::Message;
use crate::models::{LibChunk, LibItem, Role, StyleGuide};
use crate::prompts::{instruction_block, render_id, titled, titled_block};
use crate::state::AppState;
use crate::stylelib::{feedback, make_chunks, LibQuery, TAGS};
use crate::text::{clip, head_chars, split_chapters};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

type Vars = HashMap<&'static str, String>;

fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

fn default_title(content: &str) -> String {
    let first = content.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let t = head_chars(first, 16);
    if t.is_empty() { "未命名范文".into() } else { t }
}

// ---------- 范文 ----------

pub async fn list_items(State(st): State<AppState>) -> ApiResult<Vec<LibItem>> {
    Ok(Json(st.db.list_lib_items(false)?))
}

const MIN_CHARS: usize = 8;

pub async fn create_item(State(st): State<AppState>, Json(mut item): Json<LibItem>) -> ApiResult<LibItem> {
    item.content = item.content.trim().to_string();
    if item.content.chars().count() < MIN_CHARS {
        return Err(bad_request(format!("范文太短了，至少 {MIN_CHARS} 个字")));
    }
    item.id = 0;
    item.tags = clean_tags(&item.tags);
    if item.title.trim().is_empty() {
        item.title = default_title(&item.content);
    }
    let chunks = make_chunks(&item.content, false);
    Ok(Json(st.db.save_lib_item(&item, Some(&chunks))?))
}

pub async fn get_item(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let item = st.db.get_lib_item(id)?.ok_or_else(|| not_found("范文"))?;
    let chunks: Vec<Value> = st
        .db
        .list_lib_chunks(id)?
        .into_iter()
        .map(|c| json!({ "id": c.id, "seq": c.seq, "text": c.text, "tags": c.tags, "embedded": c.embedding.is_some() }))
        .collect();
    let stats = if item.content.chars().count() >= 200 { style_stats_text(&item.content) } else { String::new() };
    Ok(Json(json!({ "item": item, "chunks": chunks, "stats": stats })))
}

pub async fn patch_item(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<LibItem> {
    let old = st.db.get_lib_item(id)?.ok_or_else(|| not_found("范文"))?;
    let mut item: LibItem = apply_patch(&old, &p, &["id", "word_count", "created_at", "updated_at", "book_id"])?;
    item.tags = clean_tags(&item.tags);
    item.content = item.content.trim().to_string();
    if item.content.chars().count() < MIN_CHARS {
        return Err(bad_request(format!("范文太短了，至少 {MIN_CHARS} 个字")));
    }
    let rechunk = item.content != old.content;
    let chunks = rechunk.then(|| make_chunks(&item.content, false));
    Ok(Json(st.db.save_lib_item(&item, chunks.as_deref())?))
}

pub async fn delete_item(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_lib_item(id)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct LibImport {
    pub filename: String,
    pub data: String,
    pub genre: String,
    pub source: String,
    /// 识别到章节标题时，每章存成一篇范文
    pub split: bool,
}

pub async fn import_items(State(st): State<AppState>, Json(r): Json<LibImport>) -> ApiResult<Value> {
    let bytes = importer::decode_base64(&r.data).map_err(|e| bad_request(e.to_string()))?;
    let lower = r.filename.to_lowercase();
    let text = if lower.ends_with(".docx") {
        importer::docx_to_text(&bytes).map_err(|e| bad_request(format!("{e:#}")))?
    } else if lower.ends_with(".epub") {
        importer::epub_to_text(&bytes).map_err(|e| bad_request(format!("{e:#}")))?
    } else {
        importer::decode_text(&bytes)
    };
    if text.trim().chars().count() < 20 {
        return Err(bad_request("文件里没有识别到正文"));
    }
    let stem = r.filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(&r.filename).trim().to_string();
    let source = if r.source.trim().is_empty() { stem.clone() } else { r.source.trim().to_string() };
    let (_, parsed) = split_chapters(&text);
    let by_chapter = r.split && parsed.iter().filter(|c| !c.title.is_empty()).count() >= 2;
    let mut ids = Vec::new();
    if by_chapter {
        for ch in parsed.iter().filter(|c| c.content.chars().count() >= 20).take(500) {
            let item = LibItem {
                title: if ch.title.is_empty() { default_title(&ch.content) } else { ch.title.clone() },
                source: source.clone(),
                genre: r.genre.clone(),
                content: ch.content.clone(),
                ..Default::default()
            };
            ids.push(st.db.save_lib_item(&item, Some(&make_chunks(&item.content, true)))?.id);
        }
    } else {
        let item = LibItem { title: stem, source, genre: r.genre.clone(), content: text.trim().to_string(), ..Default::default() };
        ids.push(st.db.save_lib_item(&item, Some(&make_chunks(&item.content, false)))?.id);
    }
    Ok(Json(json!({ "created": ids.len(), "ids": ids })))
}

/// 片段太多时均匀抽样，让分析覆盖整篇
fn sample_chunks(chunks: &[LibChunk], max: usize) -> Vec<&LibChunk> {
    if chunks.len() <= max {
        return chunks.iter().collect();
    }
    (0..max).map(|i| &chunks[i * chunks.len() / max]).collect()
}

fn render_analysis(v: &Value) -> String {
    let s = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let mut out = Vec::new();
    if !s("summary").is_empty() {
        out.push(format!("亮点：{}", s("summary")));
    }
    let techniques: Vec<String> = v["techniques"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| format!("· {}", x.trim())).collect()).unwrap_or_default();
    if !techniques.is_empty() {
        out.push(format!("写法：\n{}", techniques.join("\n")));
    }
    for (k, label) in [("rhythm", "节奏"), ("dialogue", "对话"), ("imitate", "仿写要点")] {
        let t = s(k);
        if !t.is_empty() && t != "无" {
            out.push(format!("{label}：{t}"));
        }
    }
    out.join("\n")
}

fn valid_tags(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).filter(|x| TAGS.contains(&x.as_str())).collect())
        .unwrap_or_default()
}

pub async fn analyze_item(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<LibItem> {
    let mut item = st.db.get_lib_item(id)?.ok_or_else(|| not_found("范文"))?;
    let chunks = st.db.list_lib_chunks(id)?;
    if chunks.is_empty() {
        return Err(bad_request("这篇范文没有内容"));
    }
    let picked = sample_chunks(&chunks, 16);
    let numbered = picked.iter().enumerate().map(|(i, c)| format!("〔片段{}〕\n{}", i + 1, c.text)).collect::<Vec<_>>().join("\n\n");
    let settings = st.db.get_settings()?;
    let ov = st.db.prompt_overrides()?;
    let mut v = Vars::new();
    v.insert("title", item.title.clone());
    v.insert("note_block", titled("作者的收藏理由", &item.note));
    v.insert("chunks", clip(&numbered, 12000));
    v.insert("tag_list", TAGS.join("、"));
    let prep = Prepared {
        role: Role::Analyst,
        messages: vec![Message::system(render_id("system.analyst", &Vars::new(), &ov)), Message::user(render_id("task.library_analyze", &v, &ov) + JSON_RULE)],
        json: true,
    };
    let req = AiRequest { task: "library_analyze".into(), ..Default::default() };
    let value = complete_json(&st, &settings, prep, &req).await?;
    item.analysis = render_analysis(&value);
    let mut tags = item.tags.clone();
    tags.extend(valid_tags(&value["tags"]));
    item.tags = clean_tags(&tags).into_iter().take(8).collect();
    let mut updates = Vec::new();
    if let Some(map) = value["chunk_tags"].as_object() {
        for (k, t) in map {
            let tags = valid_tags(t);
            if let (Some(c), false) = (k.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| picked.get(i)), tags.is_empty()) {
                updates.push((c.id, tags));
            }
        }
    }
    if !updates.is_empty() {
        st.db.set_chunk_tags(&updates)?;
    }
    Ok(Json(st.db.save_lib_item(&item, None)?))
}

// ---------- 检索 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SearchReq {
    pub text: String,
    pub tags: Vec<String>,
    pub genre: String,
    pub k: usize,
}

pub async fn search(State(st): State<AppState>, Json(r): Json<SearchReq>) -> ApiResult<Value> {
    let index = st.library_index()?;
    let settings = st.db.get_settings()?;
    let mut tags = clean_tags(&r.tags);
    for t in crate::stylelib::tags_for(&r.text) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    let mut vector = None;
    let mut vector_error = None;
    if let Some((model, provider)) = settings.resolve_embedding().filter(|(m, _)| index.has_vectors(m) && !r.text.trim().is_empty()) {
        match st.llm.embed(&provider, &model, &[r.text.clone()]).await {
            Ok(mut v) => vector = v.pop().map(|v| (v, model)),
            Err(e) => vector_error = Some(format!("{e:#}")),
        }
    }
    let hits = index.search(&LibQuery {
        text: &r.text,
        tags: &tags,
        genre: &r.genre,
        vector: vector.as_ref().map(|(v, m)| (v.as_slice(), m.as_str())),
        k: if r.k == 0 { 5 } else { r.k.min(20) },
    });
    Ok(Json(json!({ "hits": hits, "tags": tags, "vector": vector.is_some(), "vector_error": vector_error })))
}

// ---------- 修改样本 ----------

struct Learned {
    changed: Vec<(String, String)>,
    deleted: Vec<String>,
    ids: Vec<i64>,
    pending: usize,
}

/// 对比最近采纳的 AI 文字和章节现稿，找出作者改过和删掉的句子
fn collect_feedback(db: &Db, limit: usize) -> anyhow::Result<Learned> {
    let accepts = db.recent_accepts(limit)?;
    let mut contents: HashMap<i64, String> = HashMap::new();
    let mut out = Learned { changed: vec![], deleted: vec![], ids: vec![], pending: accepts.iter().filter(|a| !a.learned).count() };
    for a in &accepts {
        if !contents.contains_key(&a.chapter_id) {
            let content = db.get_chapter(a.chapter_id)?.map(|c| c.content).unwrap_or_default();
            contents.insert(a.chapter_id, content);
        }
        let content = &contents[&a.chapter_id];
        if content.trim().is_empty() {
            continue;
        }
        out.ids.push(a.id);
        let f = feedback(&a.text, &a.before, &a.after, content);
        for pair in f.changed {
            if out.changed.len() < 12 && !out.changed.contains(&pair) {
                out.changed.push(pair);
            }
        }
        for d in f.deleted {
            if out.deleted.len() < 8 && !out.deleted.contains(&d) {
                out.deleted.push(d);
            }
        }
    }
    Ok(out)
}

pub async fn feedback_list(State(st): State<AppState>) -> ApiResult<Value> {
    let f = collect_feedback(&st.db, 60)?;
    let changed: Vec<Value> = f.changed.iter().map(|(a, b)| json!({ "ai": a, "final": b })).collect();
    Ok(Json(json!({ "changed": changed, "deleted": f.deleted, "pending": f.pending })))
}

// ---------- 文风指南 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct GenreQuery {
    pub genre: String,
}

pub async fn guide_state(State(st): State<AppState>, Query(q): Query<GenreQuery>) -> ApiResult<Value> {
    let settings = st.db.get_settings()?;
    let embed_model = settings.resolve_embedding().map(|(m, _)| m).unwrap_or_default();
    let (items, analyzed, chunks, words, embedded) = st.db.lib_counts(&embed_model)?;
    let all = st.db.list_lib_items(true)?;
    let guides = st.db.list_guides()?;
    let mut genres: BTreeSet<String> = all.iter().map(|i| i.genre.trim().to_string()).filter(|g| !g.is_empty()).collect();
    genres.extend(guides.iter().map(|g| g.genre.clone()).filter(|g| !g.is_empty()));
    let sample: String = all.iter().filter(|i| i.enabled && (q.genre.is_empty() || i.genre == q.genre)).map(|i| head_chars(&i.content, 3000)).collect::<Vec<_>>().join("\n");
    let rhythm = if sample.chars().count() >= 200 { style_stats_text(&clip(&sample, 40000)) } else { String::new() };
    let current = guides.iter().find(|g| g.genre == q.genre.trim()).cloned();
    let pending = st.db.recent_accepts(200)?.iter().filter(|a| !a.learned).count();
    Ok(Json(json!({
        "current": current,
        "versions": guides,
        "genres": genres,
        "rhythm": rhythm,
        "counts": { "items": items, "analyzed": analyzed, "chunks": chunks, "words": words, "embedded": embedded, "embed_model": embed_model },
        "feedback_pending": pending,
    })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct GuideSave {
    pub genre: String,
    pub content: String,
    pub instruction: String,
}

pub async fn save_guide(State(st): State<AppState>, Json(r): Json<GuideSave>) -> ApiResult<StyleGuide> {
    if r.content.trim().is_empty() {
        return Err(bad_request("指南内容是空的"));
    }
    Ok(Json(st.db.add_guide(&r.genre, &r.content, "手动修改")?))
}

pub async fn restore_guide(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<StyleGuide> {
    let g = st.db.get_guide(id)?.ok_or_else(|| not_found("指南版本"))?;
    let note = format!("恢复到 {} 的版本", crate::text::iso_datetime(g.created_at).replace('T', " ").chars().take(16).collect::<String>());
    Ok(Json(st.db.add_guide(&g.genre, &g.content, &note)?))
}

pub async fn distill_guide(State(st): State<AppState>, Json(r): Json<GuideSave>) -> ApiResult<StyleGuide> {
    let items: Vec<LibItem> = st.db.list_lib_items(true)?.into_iter().filter(|i| i.enabled && (r.genre.is_empty() || i.genre == r.genre)).collect();
    let learned = collect_feedback(&st.db, 40)?;
    if items.is_empty() && learned.changed.is_empty() && learned.deleted.is_empty() {
        return Err(bad_request("文风库还是空的：先收藏几篇范文，或者在写作时采纳并修改一些 AI 文字"));
    }
    let listing = items
        .iter()
        .take(40)
        .map(|i| {
            let mut head = format!("《{}》", i.title);
            if !i.tags.is_empty() {
                head += &format!("〔{}〕", i.tags.join("、"));
            }
            if !i.note.trim().is_empty() {
                head += &format!(" 作者的收藏理由：{}", clip(&i.note, 100));
            }
            let body = if i.analysis.trim().is_empty() { format!("（还没分析，节选）{}", clip(&i.content, 300)) } else { i.analysis.clone() };
            format!("{head}\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let sample: String = items.iter().map(|i| head_chars(&i.content, 3000)).collect::<Vec<_>>().join("\n");
    let stats = if sample.chars().count() >= 200 { style_stats_text(&clip(&sample, 40000)) } else { "（范文太少，暂无统计）".into() };
    let mut fb = String::new();
    if !learned.changed.is_empty() {
        fb += "【作者对 AI 文字的修改（AI 原文 → 作者改后）】\n";
        for (i, (a, b)) in learned.changed.iter().enumerate() {
            fb += &format!("{}. AI：{}\n   作者：{}\n", i + 1, clip(a, 200), clip(b, 300));
        }
    }
    if !learned.deleted.is_empty() {
        fb += "【作者整句删掉的 AI 文字】\n";
        for d in &learned.deleted {
            fb += &format!("· {}\n", clip(d, 150));
        }
    }
    let current = st.db.latest_guide(&r.genre)?;
    let settings = st.db.get_settings()?;
    let ov = st.db.prompt_overrides()?;
    let mut v = Vars::new();
    v.insert("current_block", titled_block("现有的文风指南", current.as_ref().map(|g| g.content.as_str()).unwrap_or(""), 3000));
    v.insert("count", items.len().to_string());
    v.insert("items", if listing.is_empty() { "（暂无，主要参考作者的修改）".into() } else { clip(&listing, 12000) });
    v.insert("stats", stats);
    let words: i64 = items.iter().map(|i| i.word_count).sum();
    let few = items.len() < 5 || words < 5000;
    v.insert("sample_size", format!("共 {} 篇、{} 字{}", items.len(), words, if few { "，样本很少，只能参考" } else { "" }));
    v.insert("feedback_block", fb.trim_end().to_string());
    v.insert("instruction_block", instruction_block(&r.instruction, "作者这次特别想强调的"));
    v.insert("action", if current.is_some() { "在现有指南的基础上更新".into() } else { "写出".into() });
    let prep = Prepared {
        role: Role::Analyst,
        messages: vec![Message::system(render_id("system.analyst", &Vars::new(), &ov)), Message::user(render_id("task.style_distill", &v, &ov))],
        json: false,
    };
    let req = AiRequest { task: "style_distill".into(), ..Default::default() };
    let text = complete_text(&st, &settings, prep, &req).await?;
    if text.trim().is_empty() {
        return Err(bad_request("模型没有返回内容，请重试"));
    }
    let note = format!("AI 提炼：{} 篇范文，{} 处修改", items.len(), learned.changed.len() + learned.deleted.len());
    let guide = st.db.add_guide(&r.genre, &text, &note)?;
    st.db.mark_accepts_learned(&learned.ids)?;
    Ok(Json(guide))
}

// ---------- 向量 ----------

/// 给还没有向量的片段生成向量，每次最多处理 64 段，前端循环调用直到 remaining 为 0
pub async fn embed_pending(State(st): State<AppState>) -> Result<Json<Value>, AppError> {
    let settings = st.db.get_settings()?;
    let (model, provider) = settings.resolve_embedding().ok_or_else(|| bad_request("还没有配置向量模型：在「设置 → 文风库」里填写"))?;
    let (todo, remaining) = st.db.chunks_without_embedding(&model, 64)?;
    let mut done = 0;
    for batch in todo.chunks(16) {
        let inputs: Vec<String> = batch.iter().map(|(_, t)| clip(t, 2000)).collect();
        let vectors = st.llm.embed(&provider, &model, &inputs).await.map_err(crate::api::upstream)?;
        let items: Vec<(i64, Vec<f32>)> = batch.iter().map(|(id, _)| *id).zip(vectors).collect();
        st.db.set_chunk_embeddings(&model, &items)?;
        done += items.len();
    }
    Ok(Json(json!({ "done": done, "remaining": remaining - done as i64, "model": model })))
}
