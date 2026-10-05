//! 拆书 / 对标爆款：导入对标作品，本地统计每章的长度和对话占比，AI 拆解前 N 章的开头、张力、爽点、钩子和写法，
//! 再和作者自己的书并排对比。只学结构：对标作品的原文不会进入任何写作提示词。

use crate::ai::{run_json, AiRequest};
use crate::api::{apply_patch, bad_request, load_book_data, not_found, ApiResult};
use crate::io::file_text;
use crate::models::{Chapter, RefBook, RefChapter};
use crate::state::AppState;
use crate::text::split_chapters;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// 默认对比前多少章，最多多少章
const COMPARE_DEFAULT: usize = 30;
const COMPARE_MAX: usize = 100;

fn local_stats(text: &str) -> Value {
    let s = crate::lint::stats(text);
    json!({
        "chars": s.chars,
        "paragraphs": s.paragraphs,
        "dialogue": (s.dialogue_ratio * 100.0).round() / 100.0,
        "avg_sentence": s.avg_sentence.round(),
        "avg_paragraph": s.chars.checked_div(s.paragraphs).unwrap_or(0),
    })
}

/// 「第3章 二度入梦」；标题里已经有章号的原样用
pub fn chapter_label(seq: i64, title: &str) -> String {
    let t = title.trim();
    if t.starts_with('第') && t.contains('章') {
        t.to_string()
    } else if t.is_empty() {
        format!("第{seq}章")
    } else {
        format!("第{seq}章 {t}")
    }
}

/// 模型偶尔把张力写成字符串、漏掉数组：统一成 0～10 的整数和数组，方便画图
pub fn normalize(mut v: Value) -> Value {
    if !v.is_object() {
        v = json!({});
    }
    let tension = match &v["tension"] {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    v["tension"] = json!(tension.map(|t| t.round().clamp(0.0, 10.0) as i64));
    for k in ["cool_points", "new_elements", "techniques"] {
        if !v[k].is_array() {
            v[k] = json!([]);
        }
    }
    if !v["hook"].is_object() {
        v["hook"] = json!({ "type": "无", "desc": "" });
    }
    v
}

/// 一章拿来对比的数据；张力、爽点、钩子来自拆书分析（对标书）或定稿时的张力分析（自己的书）
pub struct Point {
    pub words: i64,
    pub dialogue: f64,
    pub tension: Option<f64>,
    pub cools: Vec<String>,
    pub hook: Option<String>,
    /// 有没有做过分析
    pub analyzed: bool,
}

fn from_metrics(words: i64, dialogue: f64, m: &Value) -> Point {
    let analyzed = m.is_object() && m.get("tension").is_some_and(|t| !t.is_null());
    Point {
        words,
        dialogue,
        tension: m["tension"].as_f64(),
        cools: m["cool_points"].as_array().map(|a| a.iter().map(|p| p["type"].as_str().unwrap_or("").trim().to_string()).collect()).unwrap_or_default(),
        hook: m["hook"]["type"].as_str().map(|s| s.trim().to_string()),
        analyzed,
    }
}

fn ref_point(c: &RefChapter) -> Point {
    from_metrics(c.word_count, c.stats["dialogue"].as_f64().unwrap_or(0.0), &c.analysis)
}

fn my_point(c: &Chapter) -> Point {
    from_metrics(c.word_count, crate::lint::stats(&c.content).dialogue_ratio as f64, &c.metrics)
}

fn has_hook(h: &Option<String>) -> bool {
    h.as_deref().is_some_and(|t| !t.is_empty() && t != "无")
}

#[derive(Debug, Default, Serialize)]
pub struct Side {
    /// 参与对比的章数、其中做过分析的章数
    pub chapters: usize,
    pub analyzed: usize,
    pub avg_words: f64,
    pub dialogue: f64,
    pub avg_tension: Option<f64>,
    pub cool_per_chapter: Option<f64>,
    pub hook_rate: Option<f64>,
    pub hooks: BTreeMap<String, usize>,
    pub cool_types: BTreeMap<String, usize>,
    pub curve: Vec<Value>,
}

pub fn summarize(points: &[Point]) -> Side {
    let n = points.len();
    if n == 0 {
        return Side::default();
    }
    let done: Vec<&Point> = points.iter().filter(|p| p.analyzed).collect();
    let avg = |xs: Vec<f64>| (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64);
    let mut side = Side {
        chapters: n,
        analyzed: done.len(),
        avg_words: points.iter().map(|p| p.words as f64).sum::<f64>() / n as f64,
        dialogue: points.iter().map(|p| p.dialogue).sum::<f64>() / n as f64,
        avg_tension: avg(done.iter().filter_map(|p| p.tension).collect()),
        cool_per_chapter: avg(done.iter().map(|p| p.cools.len() as f64).collect()),
        hook_rate: avg(done.iter().map(|p| if has_hook(&p.hook) { 1.0 } else { 0.0 }).collect()),
        ..Default::default()
    };
    for p in &done {
        if let Some(h) = p.hook.as_ref().filter(|_| has_hook(&p.hook)) {
            *side.hooks.entry(h.clone()).or_default() += 1;
        }
        for t in p.cools.iter().filter(|t| !t.is_empty()) {
            *side.cool_types.entry(t.clone()).or_default() += 1;
        }
    }
    side.curve = points
        .iter()
        .enumerate()
        .map(|(i, p)| json!({ "n": i + 1, "words": p.words, "tension": p.tension, "cools": p.cools.len(), "hook": p.hook }))
        .collect();
    side
}

/// 和对标书比出来的差距，一条一句话
pub fn compare_notes(r: &Side, m: &Side) -> Vec<String> {
    let mut out = Vec::new();
    if m.chapters == 0 {
        return vec!["你的书还没有正文，先写几章再来对比。".into()];
    }
    if r.analyzed == 0 {
        out.push("对标书还没有做 AI 拆解：拆解后才能对比张力、爽点和章末钩子。".into());
    }
    if m.analyzed == 0 {
        out.push("你的书还没有定稿分析：章节定稿时会顺带分析张力、爽点和章末钩子，定稿几章后就能对比这些。".into());
    }
    if let (Some(a), Some(b)) = (r.cool_per_chapter, m.cool_per_chapter) {
        if b < a * 0.7 {
            out.push(format!("爽点偏少：对标书平均每章 {a:.1} 个，你的书 {b:.1} 个。"));
        } else if a > 0.0 && b > a * 1.5 {
            out.push(format!("爽点比对标书密（每章 {b:.1} 个对 {a:.1} 个），注意别写得廉价、没有铺垫。"));
        }
    }
    if let (Some(a), Some(b)) = (r.hook_rate, m.hook_rate) {
        if b + 0.15 < a {
            out.push(format!("章末钩子偏弱：对标书 {}% 的章节结尾有钩子，你的书 {}%。", (a * 100.0).round(), (b * 100.0).round()));
        }
    }
    if let (Some(a), Some(b)) = (r.avg_tension, m.avg_tension) {
        if b + 1.0 <= a {
            out.push(format!("张力偏低：对标书平均 {a:.1} 分，你的书 {b:.1} 分。"));
        }
    }
    if r.avg_words > 0.0 && (m.avg_words - r.avg_words).abs() / r.avg_words > 0.25 {
        out.push(format!("章节长度不同：对标书平均每章 {} 字，你的书 {} 字。", r.avg_words.round(), m.avg_words.round()));
    }
    if (m.dialogue - r.dialogue).abs() >= 0.1 {
        out.push(format!("对话占比：对标书 {}%，你的书 {}%。", (r.dialogue * 100.0).round(), (m.dialogue * 100.0).round()));
    }
    if out.is_empty() {
        out.push("主要指标和对标书接近。".into());
    }
    out
}

// ---------- 接口 ----------

#[derive(Deserialize)]
pub struct ImportRef {
    pub filename: String,
    pub data: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub genre: String,
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Vec<RefBook>> {
    Ok(Json(st.db.list_refbooks()?))
}

pub async fn import(State(st): State<AppState>, Json(r): Json<ImportRef>) -> ApiResult<RefBook> {
    let text = file_text(&r.filename, &r.data)?;
    let (_, parsed) = split_chapters(&text);
    if parsed.is_empty() {
        return Err(bad_request("没有识别到章节，确认文件里有「第X章」这样的章节标题"));
    }
    let chapters: Vec<RefChapter> =
        parsed.into_iter().map(|p| RefChapter { stats: local_stats(&p.content), title: p.title, content: p.content, ..Default::default() }).collect();
    let stem = r.filename.rsplit_once('.').map_or(r.filename.as_str(), |(s, _)| s);
    let title = if r.title.trim().is_empty() { stem.trim().trim_matches(['《', '》']).to_string() } else { r.title.trim().to_string() };
    let id = st.db.create_refbook(&RefBook { title, genre: r.genre, ..Default::default() }, &chapters)?;
    Ok(Json(st.db.get_refbook(id)?.ok_or_else(|| not_found("对标作品"))?))
}

pub async fn get(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let book = st.db.get_refbook(id)?.ok_or_else(|| not_found("对标作品"))?;
    let chapters: Vec<Value> = st
        .db
        .list_refchapters(id, false)?
        .into_iter()
        .map(|c| json!({ "id": c.id, "seq": c.seq, "label": chapter_label(c.seq, &c.title), "words": c.word_count, "stats": c.stats, "analysis": c.analysis, "analyzed": c.analyzed_at.is_some() }))
        .collect();
    Ok(Json(json!({ "book": book, "chapters": chapters })))
}

pub async fn patch(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<RefBook> {
    let b = st.db.get_refbook(id)?.ok_or_else(|| not_found("对标作品"))?;
    let updated: RefBook = apply_patch(&b, &p, &["id", "created_at", "updated_at", "chapters", "words", "analyzed"])?;
    st.db.update_refbook(&updated)?;
    Ok(Json(st.db.get_refbook(id)?.ok_or_else(|| not_found("对标作品"))?))
}

pub async fn delete(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_refbook(id)?;
    Ok(Json(json!({ "ok": true })))
}

/// AI 拆解一章，结果存起来
pub async fn analyze(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let ch = st.db.get_refchapter(id)?.ok_or_else(|| not_found("章节"))?;
    let req = AiRequest { task: "teardown".into(), selection: ch.content.clone(), instruction: chapter_label(ch.seq, &ch.title), ..Default::default() };
    let analysis = normalize(run_json(&st, &req).await?);
    st.db.set_refchapter_analysis(id, &analysis)?;
    Ok(Json(analysis))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct CompareQuery {
    pub book_id: Option<i64>,
    pub limit: Option<usize>,
}

/// 对标书和作者的书各取前 N 章对比
pub async fn compare(State(st): State<AppState>, Path(id): Path<i64>, Query(q): Query<CompareQuery>) -> ApiResult<Value> {
    st.db.get_refbook(id)?.ok_or_else(|| not_found("对标作品"))?;
    let n = q.limit.unwrap_or(COMPARE_DEFAULT).clamp(1, COMPARE_MAX);
    let refs: Vec<Point> = st.db.list_refchapters(id, false)?.iter().take(n).map(ref_point).collect();
    let r = summarize(&refs);
    let (mine, title) = match q.book_id {
        Some(bid) => {
            let d = load_book_data(&st.db, bid)?;
            let points: Vec<Point> = d.chapters.iter().filter(|c| !c.content.trim().is_empty()).take(n).map(my_point).collect();
            (Some(summarize(&points)), d.book.title.clone())
        }
        None => (None, String::new()),
    };
    let notes = mine.as_ref().map(|m| compare_notes(&r, m)).unwrap_or_default();
    Ok(Json(json!({ "limit": n, "ref": r, "mine": mine, "book_title": title, "notes": notes })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(words: i64, tension: Option<f64>, cools: &[&str], hook: Option<&str>) -> Point {
        Point { words, dialogue: 0.3, tension, cools: cools.iter().map(|s| s.to_string()).collect(), hook: hook.map(String::from), analyzed: tension.is_some() }
    }

    #[test]
    fn normalizes_model_output() {
        let v = normalize(json!({ "tension": "7.6", "hook": "悬念", "cool_points": null }));
        assert_eq!((v["tension"].as_i64(), v["hook"]["type"].as_str(), v["cool_points"].as_array().map(Vec::len)), (Some(8), Some("无"), Some(0)));
        assert_eq!(normalize(json!({ "tension": 15 }))["tension"], 10);
        assert!(normalize(json!("不是对象"))["techniques"].is_array());
        assert_eq!(chapter_label(3, "二度入梦"), "第3章 二度入梦");
        assert_eq!(chapter_label(3, "第三章 二度入梦"), "第三章 二度入梦");
        assert_eq!(chapter_label(3, ""), "第3章");
    }

    #[test]
    fn compares_structure_with_reference() {
        let r = summarize(&[
            point(2000, Some(8.0), &["打脸", "升级"], Some("悬念")),
            point(2200, Some(7.0), &["打脸"], Some("危机")),
            point(2100, Some(8.0), &["收获", "打脸"], Some("无")),
        ]);
        assert_eq!((r.chapters, r.analyzed), (3, 3));
        assert_eq!(r.cool_types.get("打脸"), Some(&3));
        assert!((r.hook_rate.unwrap_or(0.0) - 2.0 / 3.0).abs() < 1e-9);
        let m = summarize(&[point(3200, Some(5.0), &[], Some("无")), point(3000, Some(6.0), &["升级"], None), point(3100, None, &[], None)]);
        assert_eq!((m.chapters, m.analyzed), (3, 2), "没定稿分析的章节只算字数");
        let notes = compare_notes(&r, &m);
        assert!(notes.iter().any(|n| n.starts_with("爽点偏少：对标书平均每章 1.7 个，你的书 0.5 个")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("章末钩子偏弱：对标书 67%")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("张力偏低：对标书平均 7.7 分，你的书 5.5 分")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("章节长度不同：对标书平均每章 2100 字，你的书 3100 字")), "{notes:?}");
        assert_eq!(compare_notes(&r, &summarize(&[])), vec!["你的书还没有正文，先写几章再来对比。".to_string()]);
        let fresh = summarize(&[point(2000, None, &[], None)]);
        assert!(compare_notes(&fresh, &m)[0].starts_with("对标书还没有做 AI 拆解"));
    }
}
