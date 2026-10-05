//! 全书查找替换：正文、章名、章纲、摘要、作品设定、卷纲、设定库（含各章的状态快照）、伏笔、秘密台账一起找，
//! 逐处勾选后替换，替换正文前给章节存快照。设定改名后用它把旧名同步到全书。

use crate::api::{bad_request, not_found, require_book, ApiResult, AppError};
use crate::db::Db;
use crate::models::{split_terms, Book, Chapter, Entry, EntryState, Reveal, Thread, Volume, CHAR_FIELDS};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

/// 一次最多列出多少处，太多时让作者把关键词写长一点
const MAX_HITS: usize = 3000;
/// 命中处前后各带多少字
const CONTEXT: usize = 24;

/// 可替换的字段：字段名、显示名、属于哪个范围（content 正文和章名 / notes 大纲和摘要 / settings 设定库）、取字段
type Field<T> = (&'static str, &'static str, &'static str, fn(&mut T) -> &mut String);

const CHAPTER_FIELDS: &[Field<Chapter>] = &[
    ("title", "章名", "content", |c| &mut c.title),
    ("content", "正文", "content", |c| &mut c.content),
    ("outline", "章纲", "notes", |c| &mut c.outline),
    ("beats", "节拍", "notes", |c| &mut c.beats),
    ("summary", "摘要", "notes", |c| &mut c.summary),
];
const BOOK_FIELDS: &[Field<Book>] = &[
    ("logline", "一句话简介", "notes", |b| &mut b.logline),
    ("synopsis", "作品简介", "notes", |b| &mut b.synopsis),
    ("worldview", "世界观", "notes", |b| &mut b.worldview),
    ("outline", "总纲", "notes", |b| &mut b.outline),
    ("golden_finger", "金手指", "notes", |b| &mut b.golden_finger),
];
const VOLUME_FIELDS: &[Field<Volume>] = &[
    ("title", "卷名", "notes", |v| &mut v.title),
    ("outline", "卷纲", "notes", |v| &mut v.outline),
    ("summary", "卷摘要", "notes", |v| &mut v.summary),
];
const ENTRY_FIELDS: &[Field<Entry>] = &[
    ("name", "名称", "settings", |e| &mut e.name),
    ("aliases", "别名", "settings", |e| &mut e.aliases),
    ("description", "描述", "settings", |e| &mut e.description),
    ("state", "当前状态", "settings", |e| &mut e.state),
];
const STATE_FIELDS: &[Field<EntryState>] = &[("state", "状态", "settings", |s| &mut s.state)];
const THREAD_FIELDS: &[Field<Thread>] = &[("title", "伏笔", "settings", |t| &mut t.title), ("detail", "伏笔说明", "settings", |t| &mut t.detail)];
const REVEAL_FIELDS: &[Field<Reveal>] = &[
    ("title", "秘密", "settings", |r| &mut r.title),
    ("truth", "真相", "settings", |r| &mut r.truth),
    ("misread", "表面解释", "settings", |r| &mut r.misread),
    ("terms", "泄露词", "settings", |r| &mut r.terms),
    ("exceptions", "不算点破的说法", "settings", |r| &mut r.exceptions),
    ("seed_note", "种子写法", "settings", |r| &mut r.seed_note),
    ("clue_note", "线索写法", "settings", |r| &mut r.clue_note),
    ("payoff", "揭开后的变化", "settings", |r| &mut r.payoff),
];

/// 一段可以查找的文字
struct Doc {
    target: &'static str,
    id: i64,
    label: String,
    field: String,
    field_label: String,
    text: String,
}

fn push<T>(out: &mut Vec<Doc>, has: &dyn Fn(&str) -> bool, target: &'static str, id: i64, label: &str, obj: &mut T, defs: &[Field<T>]) {
    for (field, field_label, scope, get) in defs {
        let text = get(obj);
        if has(scope) && !text.is_empty() {
            out.push(Doc { target, id, label: label.to_string(), field: field.to_string(), field_label: field_label.to_string(), text: text.clone() });
        }
    }
}

/// 人物的结构化状态：每一项是一个字段，比如「fields.location」显示成「位置」
fn push_map(out: &mut Vec<Doc>, target: &'static str, id: i64, label: &str, prefix: &str, map: &BTreeMap<String, String>) {
    for (k, v) in map.iter().filter(|(_, v)| !v.is_empty()) {
        let name = CHAR_FIELDS.iter().find(|(key, _)| key == k).map_or(k.as_str(), |(_, l)| l);
        out.push(Doc { target, id, label: label.to_string(), field: format!("fields.{k}"), field_label: format!("{prefix}{name}"), text: v.clone() });
    }
}

fn collect(db: &Db, book: &Book, scopes: &[String]) -> anyhow::Result<Vec<Doc>> {
    let has = |s: &str| scopes.is_empty() || scopes.iter().any(|x| x == s);
    let labels: HashMap<i64, String> = db.list_chapter_metas(book.id)?.into_iter().map(|m| (m.id, m.label())).collect();
    let mut out = Vec::new();
    if has("content") || has("notes") {
        for mut ch in db.list_chapters(book.id)? {
            let label = labels.get(&ch.id).cloned().unwrap_or_else(|| ch.title.clone());
            push(&mut out, &has, "chapter", ch.id, &label, &mut ch, CHAPTER_FIELDS);
        }
    }
    if has("notes") {
        let mut b = book.clone();
        push(&mut out, &has, "book", b.id, "作品设定", &mut b, BOOK_FIELDS);
        for mut v in db.list_volumes(book.id)? {
            let label = v.title.clone();
            push(&mut out, &has, "volume", v.id, &label, &mut v, VOLUME_FIELDS);
        }
    }
    if has("settings") {
        let entries = db.list_entries(book.id)?;
        let names: HashMap<i64, String> = entries.iter().map(|e| (e.id, e.name.clone())).collect();
        for mut e in entries {
            let label = e.name.clone();
            push(&mut out, &has, "entry", e.id, &label, &mut e, ENTRY_FIELDS);
            push_map(&mut out, "entry", e.id, &label, "状态·", &e.fields);
        }
        for mut s in db.list_book_states(book.id)? {
            let when = s.chapter_id.and_then(|c| labels.get(&c)).map_or("初始状态".to_string(), |l| format!("{l}的状态"));
            let label = format!("{} · {when}", names.get(&s.entry_id).map_or("", |n| n.as_str()));
            push(&mut out, &has, "state", s.id, &label, &mut s, STATE_FIELDS);
            push_map(&mut out, "state", s.id, &label, "", &s.fields);
        }
        for mut t in db.list_threads(book.id)? {
            let label = t.title.clone();
            push(&mut out, &has, "thread", t.id, &label, &mut t, THREAD_FIELDS);
        }
        for mut r in db.list_reveals(book.id)? {
            let label = r.title.clone();
            push(&mut out, &has, "reveal", r.id, &label, &mut r, REVEAL_FIELDS);
        }
    }
    Ok(out)
}

fn matcher(query: &str, case_sensitive: bool) -> Result<Regex, AppError> {
    RegexBuilder::new(&regex::escape(query)).case_insensitive(!case_sensitive).build().map_err(|e| bad_request(e.to_string()))
}

/// 默认不替换的地方：别的设定名里含着这个词（改「白山」时的「白山宗」），以及改名设定自己的排除短语
pub fn guard_phrases(entries: &[Entry], query: &str, entry_id: Option<i64>, re: &Regex) -> Vec<String> {
    let longer = |n: &String| n.len() > query.len() && re.is_match(n);
    let mut out: Vec<String> = entries.iter().flat_map(|e| std::iter::once(e.name.trim().to_string()).chain(split_terms(&e.aliases))).filter(longer).collect();
    if let Some(e) = entry_id.and_then(|id| entries.iter().find(|e| e.id == id)) {
        out.extend(e.excludes());
    }
    let mut seen = Vec::new();
    out.retain(|p| !p.is_empty() && !seen.contains(p) && {
        seen.push(p.clone());
        true
    });
    out
}

#[derive(Debug, Serialize)]
pub struct Hit {
    /// 字节位置，替换时原样传回
    pub start: usize,
    pub end: usize,
    /// UTF-16 位置和长度，网页里定位用
    pub at: usize,
    pub len: usize,
    pub before: String,
    pub text: String,
    pub after: String,
    /// 落在这个短语里，默认不勾选
    pub excluded: Option<String>,
}

/// 命中处前面同一段里最多 CONTEXT 个字
fn before_of(text: &str, start: usize) -> String {
    let head = &text[..start];
    let para = head.rfind('\n').map_or(head, |i| &head[i + 1..]);
    let skip = para.chars().count().saturating_sub(CONTEXT);
    para.chars().skip(skip).collect()
}

fn after_of(text: &str, end: usize) -> String {
    text[end..].chars().take_while(|&c| c != '\n').take(CONTEXT).collect()
}

pub fn find_hits(text: &str, re: &Regex, guards: &[String]) -> Vec<Hit> {
    let guarded: Vec<(usize, usize, &str)> =
        guards.iter().flat_map(|g| text.match_indices(g.as_str()).map(move |(i, s)| (i, i + s.len(), g.as_str()))).collect();
    let (mut units, mut last) = (0, 0);
    re.find_iter(text)
        .map(|m| {
            units += text[last..m.start()].encode_utf16().count();
            last = m.start();
            Hit {
                start: m.start(),
                end: m.end(),
                at: units,
                len: m.as_str().encode_utf16().count(),
                before: before_of(text, m.start()),
                text: m.as_str().to_string(),
                after: after_of(text, m.end()),
                excluded: guarded.iter().find(|(s, e, _)| *s <= m.start() && m.end() <= *e).map(|(_, _, g)| g.to_string()),
            }
        })
        .collect()
}

/// 把勾选的几处换掉：每处都核对原文还是要找的词（查找之后改过的地方跳过），返回新文本和换掉的处数
pub fn apply(text: &str, spans: &[(usize, usize)], re: &Regex, replacement: &str) -> (String, usize) {
    let mut spans = spans.to_vec();
    spans.sort_unstable();
    let (mut out, mut last, mut n) = (String::with_capacity(text.len()), 0, 0);
    for (s, e) in spans {
        let Some(piece) = text.get(s..e).filter(|_| s >= last) else { continue };
        if !re.find(piece).is_some_and(|m| m.start() == 0 && m.end() == piece.len()) {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(replacement);
        last = e;
        n += 1;
    }
    out.push_str(&text[last..]);
    (out, n)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SearchRequest {
    pub query: String,
    pub case_sensitive: bool,
    /// content / notes / settings，空表示全部
    pub scopes: Vec<String>,
    /// 设定改名时传入，用它的排除短语
    pub entry_id: Option<i64>,
}

#[derive(Serialize)]
struct Group {
    target: &'static str,
    id: i64,
    label: String,
    field: String,
    field_label: String,
    hits: Vec<Hit>,
}

pub async fn search(State(st): State<AppState>, Path(book_id): Path<i64>, Json(r): Json<SearchRequest>) -> ApiResult<Value> {
    let book = require_book(&st.db, book_id)?;
    if r.query.trim().is_empty() {
        return Err(bad_request("请输入要查找的文字"));
    }
    let re = matcher(&r.query, r.case_sensitive)?;
    let guards = guard_phrases(&st.db.list_entries(book_id)?, &r.query, r.entry_id, &re);
    let (mut groups, mut total, mut truncated) = (Vec::new(), 0, false);
    for d in collect(&st.db, &book, &r.scopes)? {
        let hits = find_hits(&d.text, &re, &guards);
        if hits.is_empty() {
            continue;
        }
        total += hits.len();
        groups.push(Group { target: d.target, id: d.id, label: d.label, field: d.field, field_label: d.field_label, hits });
        if total >= MAX_HITS {
            truncated = true;
            break;
        }
    }
    Ok(Json(json!({ "total": total, "truncated": truncated, "groups": groups })))
}

#[derive(Deserialize)]
pub struct HitRef {
    pub target: String,
    pub id: i64,
    pub field: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ReplaceRequest {
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub hits: Vec<HitRef>,
}

type Spans = BTreeMap<String, Vec<(usize, usize)>>;

fn edit<T>(obj: &mut T, defs: &[Field<T>], spans: &Spans, re: &Regex, rep: &str) -> usize {
    let mut n = 0;
    for (field, _, _, get) in defs {
        if let Some(list) = spans.get(*field) {
            let slot = get(obj);
            let (text, k) = apply(slot, list, re, rep);
            if k > 0 {
                *slot = text;
                n += k;
            }
        }
    }
    n
}

fn edit_map(map: &mut BTreeMap<String, String>, spans: &Spans, re: &Regex, rep: &str) -> usize {
    let mut n = 0;
    for (field, list) in spans {
        if let Some(v) = field.strip_prefix("fields.").and_then(|k| map.get_mut(k)) {
            let (text, k) = apply(v, list, re, rep);
            if k > 0 {
                *v = text;
                n += k;
            }
        }
    }
    n
}

/// 改一个对象里勾选的字段并写回，返回换掉的处数
fn write_back(db: &Db, book_id: i64, target: &str, id: i64, spans: &Spans, re: &Regex, rep: &str, note: &str) -> Result<usize, AppError> {
    let n = match target {
        "chapter" => {
            let mut ch = db.get_chapter(id)?.filter(|c| c.book_id == book_id).ok_or_else(|| not_found("章节"))?;
            let before = ch.content.clone();
            let n = edit(&mut ch, CHAPTER_FIELDS, spans, re, rep);
            if n > 0 {
                if ch.content != before {
                    db.create_version(id, &before, note)?;
                }
                db.update_chapter(&ch)?;
            }
            n
        }
        "book" => {
            let mut b = require_book(db, book_id)?;
            let n = if id == book_id { edit(&mut b, BOOK_FIELDS, spans, re, rep) } else { 0 };
            if n > 0 {
                db.update_book(&b)?;
            }
            n
        }
        "volume" => {
            let mut v = db.get_volume(id)?.filter(|v| v.book_id == book_id).ok_or_else(|| not_found("卷"))?;
            let n = edit(&mut v, VOLUME_FIELDS, spans, re, rep);
            if n > 0 {
                db.update_volume(&v)?;
            }
            n
        }
        "entry" => {
            let mut e = db.get_entry(id)?.filter(|e| e.book_id == book_id).ok_or_else(|| not_found("设定"))?;
            let n = edit(&mut e, ENTRY_FIELDS, spans, re, rep) + edit_map(&mut e.fields, spans, re, rep);
            if n > 0 {
                db.update_entry(&e)?;
            }
            n
        }
        "state" => {
            let mut s = db.list_book_states(book_id)?.into_iter().find(|s| s.id == id).ok_or_else(|| not_found("设定状态"))?;
            let n = edit(&mut s, STATE_FIELDS, spans, re, rep) + edit_map(&mut s.fields, spans, re, rep);
            if n > 0 {
                db.update_entry_state(&s)?;
            }
            n
        }
        "thread" => {
            let mut t = db.get_thread(id)?.filter(|t| t.book_id == book_id).ok_or_else(|| not_found("伏笔"))?;
            let n = edit(&mut t, THREAD_FIELDS, spans, re, rep);
            if n > 0 {
                db.update_thread(&t)?;
            }
            n
        }
        "reveal" => {
            let mut r = db.get_reveal(id)?.filter(|r| r.book_id == book_id).ok_or_else(|| not_found("秘密"))?;
            let n = edit(&mut r, REVEAL_FIELDS, spans, re, rep);
            if n > 0 {
                db.save_reveal(&r)?;
            }
            n
        }
        _ => return Err(bad_request("不认识的替换对象")),
    };
    Ok(n)
}

pub async fn replace(State(st): State<AppState>, Path(book_id): Path<i64>, Json(r): Json<ReplaceRequest>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    if r.query.is_empty() {
        return Err(bad_request("请输入要查找的文字"));
    }
    let re = matcher(&r.query, r.case_sensitive)?;
    let mut groups: BTreeMap<(String, i64), Spans> = BTreeMap::new();
    for h in &r.hits {
        groups.entry((h.target.clone(), h.id)).or_default().entry(h.field.clone()).or_default().push((h.start, h.end));
    }
    let note = format!("查找替换前：{} → {}", r.query, r.replacement);
    let (mut replaced, mut chapters, mut book) = (0, 0, false);
    for ((target, id), spans) in &groups {
        let n = write_back(&st.db, book_id, target, *id, spans, &re, &r.replacement, &note)?;
        replaced += n;
        chapters += usize::from(n > 0 && target == "chapter");
        book |= n > 0 && target == "book";
    }
    st.db.touch_book(book_id)?;
    Ok(Json(json!({ "replaced": replaced, "skipped": r.hits.len() - replaced, "chapters": chapters, "book": book })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(q: &str) -> Regex {
        matcher(q, false).unwrap_or_else(|_| panic!("正则"))
    }

    #[test]
    fn finds_hits_with_context_and_guards() {
        let text = "白山站在白山宗门口。\n他看见一只白山羊，ABC 和 abc。";
        let entries = vec![
            Entry { id: 1, name: "白山".into(), exclude: "白山羊".into(), ..Default::default() },
            Entry { id: 2, name: "白山宗".into(), ..Default::default() },
        ];
        let guards = guard_phrases(&entries, "白山", Some(1), &re("白山"));
        assert_eq!(guards, vec!["白山宗".to_string(), "白山羊".to_string()]);
        let hits = find_hits(text, &re("白山"), &guards);
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].excluded.as_deref(), hits[1].excluded.as_deref(), hits[2].excluded.as_deref()), (None, Some("白山宗"), Some("白山羊")));
        assert_eq!((hits[1].at, hits[1].before.as_str(), hits[1].after.as_str()), (4, "白山站在", "宗门口。"));
        assert_eq!(hits[2].before, "他看见一只", "上下文不跨段");
        assert_eq!(find_hits(text, &re("abc"), &[]).len(), 2, "默认不区分大小写");
        assert_eq!(find_hits(text, &matcher("abc", true).unwrap_or_else(|_| panic!()), &[]).len(), 1);
    }

    #[test]
    fn applies_only_verified_spans() {
        let text = "白山说：白山宗的白山。";
        let hits = find_hits(text, &re("白山"), &[]);
        let spans: Vec<(usize, usize)> = [&hits[2], &hits[0]].iter().map(|h| (h.start, h.end)).collect();
        assert_eq!(apply(text, &spans, &re("白山"), "黑岩"), ("黑岩说：白山宗的黑岩。".to_string(), 2));
        let stale = vec![(hits[0].start + 3, hits[0].end + 3), (hits[1].start, hits[1].end), (hits[1].start, hits[1].end)];
        assert_eq!(apply(text, &stale, &re("白山"), "黑岩"), ("白山说：黑岩宗的白山。".to_string(), 1), "位置对不上的跳过，重复的只换一次");
        assert_eq!(apply("白山白山", &[(0, 6), (3, 9)], &re("白山"), "x").1, 1, "重叠的只换第一处");
    }
}
