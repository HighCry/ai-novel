use crate::api::{bad_request, load_book_data, not_found, ApiResult, AppError};
use crate::models::{Book, Chapter, Volume};
use crate::state::AppState;
use crate::text::{count_words, split_chapters};
use crate::{export, importer};
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ExportQuery {
    pub format: Option<String>,
    /// 第几个章节开始（按目录顺序，从 1 开始）
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub chapter_id: Option<i64>,
    pub numerals: Option<String>,
    pub indent: Option<bool>,
    pub blank_line: Option<bool>,
    pub header: Option<bool>,
}

fn content_disposition(filename: &str, ext: &str) -> String {
    let encoded: String = filename
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"novel.{ext}\"; filename*=UTF-8''{encoded}")
}

pub async fn export(State(st): State<AppState>, Path(book_id): Path<i64>, Query(q): Query<ExportQuery>) -> Result<Response, AppError> {
    let data = load_book_data(&st.db, book_id)?;
    let format = q.format.clone().unwrap_or_else(|| "txt".into());
    let mut opts = export::opts_for(&format);
    if let Some(n) = &q.numerals {
        opts.chinese_numerals = n == "chinese";
    }
    if let Some(v) = q.indent {
        opts.indent = v;
    }
    if let Some(v) = q.blank_line {
        opts.blank_line = v;
    }
    let all: Vec<(Option<i64>, &Chapter)> = data.chapters.iter().map(|c| (data.number(c), c)).collect();
    let list: Vec<(Option<i64>, &Chapter)> = match q.chapter_id {
        Some(id) => all.into_iter().filter(|(_, c)| c.id == id).collect(),
        None => {
            let from = q.from.unwrap_or(1).max(1);
            let to = q.to.unwrap_or(usize::MAX);
            all.into_iter().enumerate().filter(|(i, _)| (from..=to).contains(&(i + 1))).map(|(_, x)| x).collect()
        }
    };
    if list.is_empty() {
        return Err(bad_request("没有可导出的章节"));
    }
    let title = data.book.title.trim().to_string();
    let (bytes, mime, ext): (Vec<u8>, &str, &str) = match format.as_str() {
        "epub" => (export::export_epub(&data.book, &list, crate::db::now())?, "application/epub+zip", "epub"),
        "md" => (export::export_markdown(&data.book, &list).into_bytes(), "text/markdown; charset=utf-8", "md"),
        _ => {
            let with_header = q.header.unwrap_or(q.chapter_id.is_none());
            (export::export_txt(&data.book, &list, &opts, with_header).into_bytes(), "text/plain; charset=utf-8", "txt")
        }
    };
    let headers = [
        (header::CONTENT_TYPE, mime.to_string()),
        (header::CONTENT_DISPOSITION, content_disposition(&format!("{title}.{ext}"), ext)),
    ];
    Ok((headers, bytes).into_response())
}

pub async fn submission_check(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Value> {
    let data = load_book_data(&st.db, book_id)?;
    let s = st.db.get_settings()?;
    let list: Vec<(Option<i64>, &Chapter)> = data.chapters.iter().map(|c| (data.number(c), c)).collect();
    let items = export::submission_check(&list, s.min_chapter_words, s.max_chapter_words, (&s.sensitive_words, &s.sensitive_ignore));
    Ok(Json(json!({ "chapters": list.len(), "items": items })))
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub filename: String,
    /// base64，可以带 data URL 前缀
    pub data: String,
    #[serde(default)]
    pub book_id: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
}

pub async fn import_novel(State(st): State<AppState>, Json(r): Json<ImportRequest>) -> ApiResult<Value> {
    let bytes = importer::decode_base64(&r.data).map_err(|e| bad_request(e.to_string()))?;
    let lower = r.filename.to_lowercase();
    let text = if lower.ends_with(".docx") {
        importer::docx_to_text(&bytes).map_err(|e| bad_request(format!("{e:#}")))?
    } else if lower.ends_with(".epub") {
        importer::epub_to_text(&bytes).map_err(|e| bad_request(format!("{e:#}")))?
    } else {
        importer::decode_text(&bytes)
    };
    let (preface, parsed) = split_chapters(&text);
    if parsed.is_empty() {
        return Err(bad_request("没有识别到正文内容"));
    }
    let preface_as_synopsis = r.book_id.is_none() && preface.chars().count() <= 1500;
    let book = match r.book_id {
        Some(id) => st.db.get_book(id)?.ok_or_else(|| not_found("作品"))?,
        None => {
            let stem = r.filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(&r.filename);
            let title = r
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| stem.trim().trim_matches(|c| c == '《' || c == '》').to_string());
            st.db.create_book(&Book {
                title,
                genre: r.genre.clone().unwrap_or_default(),
                synopsis: if preface_as_synopsis { preface.clone() } else { String::new() },
                ..Default::default()
            })?
        }
    };

    let mut vol_ids: HashMap<String, i64> = st.db.list_volumes(book.id)?.into_iter().map(|v| (v.title, v.id)).collect();
    let mut sort = st.db.list_chapter_metas(book.id)?.iter().map(|m| m.sort).max().unwrap_or(0);
    let mut chapters = Vec::with_capacity(parsed.len() + 1);
    if !preface.is_empty() && !preface_as_synopsis {
        sort += 1;
        chapters.push(Chapter { book_id: book.id, sort, title: "作品相关".into(), content: preface, status: "done".into(), ..Default::default() });
    }
    for p in parsed {
        let volume_id = match &p.volume {
            Some(name) => Some(match vol_ids.get(name) {
                Some(id) => *id,
                None => {
                    let v = st.db.create_volume(&Volume { book_id: book.id, title: name.clone(), ..Default::default() })?;
                    vol_ids.insert(name.clone(), v.id);
                    v.id
                }
            }),
            None => None,
        };
        sort += 1;
        chapters.push(Chapter { book_id: book.id, volume_id, sort, title: p.title, content: p.content, status: "done".into(), ..Default::default() });
    }
    let words: i64 = chapters.iter().map(|c| count_words(&c.content)).sum();
    let n = st.db.bulk_create_chapters(&chapters)?;
    st.db.touch_book(book.id)?;
    Ok(Json(json!({ "book_id": book.id, "chapters": n, "volumes": vol_ids.len(), "words": words })))
}

#[derive(Deserialize)]
pub struct TavernImport {
    #[serde(default)]
    pub filename: String,
    pub data: String,
}

/// 导入酒馆角色卡（PNG/JSON）或世界书 JSON 到设定库；同名条目覆盖描述。
pub async fn import_tavern(State(st): State<AppState>, Path(book_id): Path<i64>, Json(r): Json<TavernImport>) -> ApiResult<Value> {
    st.db.get_book(book_id)?.ok_or_else(|| not_found("作品"))?;
    let bytes = importer::decode_base64(&r.data).map_err(|e| bad_request(e.to_string()))?;
    let entries = importer::parse_tavern_file(&bytes).map_err(|e| bad_request(format!("{}：{e:#}", r.filename)))?;
    let existing = st.db.list_entries(book_id)?;
    let (mut created, mut updated) = (0, 0);
    let mut names = Vec::new();
    for mut e in entries {
        e.book_id = book_id;
        names.push(e.name.clone());
        match existing.iter().find(|x| x.name == e.name) {
            Some(found) => {
                let mut ne = found.clone();
                ne.description = e.description;
                if !e.aliases.is_empty() {
                    ne.aliases = e.aliases;
                }
                ne.always_include |= e.always_include;
                st.db.update_entry(&ne)?;
                updated += 1;
            }
            None => {
                st.db.create_entry(&e)?;
                created += 1;
            }
        }
    }
    Ok(Json(json!({ "created": created, "updated": updated, "names": names })))
}
