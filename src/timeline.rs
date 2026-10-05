//! 时间线：每章结束时的故事时间、关键事件，以及正文里定下的时限（三日内交出灵铁、七天后宗门大比）。
//! 定稿时 AI 顺带提取；写作时告诉模型现在是故事第几天、哪些时限快到了；体检查时限过期和时间倒流。
//! 和设定状态一样随章节演变：写第 N 章时只看第 N 章之前定下的时限，后面才了结的仍算未了结。

use crate::api::{apply_patch, bad_request, not_found, require_book, ApiResult};
use crate::continuity::{finding, Finding};
use crate::memory::BookData;
use crate::models::{Chapter, Event};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

/// 还剩几天以内算快到期
const DUE_SOON_DAYS: i64 = 1;

fn has_time(c: &Chapter) -> bool {
    c.story_day.is_some() || !c.story_time.trim().is_empty()
}

/// 「第3天傍晚（故事第 3 天）」
pub fn describe(story_time: &str, day: Option<i64>) -> String {
    match (story_time.trim(), day) {
        ("", Some(day)) => format!("故事第 {day} 天"),
        (t, Some(day)) => format!("{t}（故事第 {day} 天）"),
        (t, None) => t.to_string(),
    }
}

/// current 之前最近一章记了时间的章节；current 为空时是全书最后一章记了时间的
fn last_timed<'a>(d: &'a BookData, current: Option<&Chapter>) -> Option<&'a Chapter> {
    d.before(current).iter().rev().find(|c| has_time(c))
}

/// 写 current 时还没了结的时限：current 之前定下，并且没在 current 之前了结
pub fn open_deadlines<'a>(d: &'a BookData, current: Option<&Chapter>) -> Vec<&'a Event> {
    let cur = current.and_then(|c| d.position(c.id));
    let pos = |id: Option<i64>| id.and_then(|id| d.position(id));
    d.events
        .iter()
        .filter(|e| e.kind == "deadline")
        .filter(|e| match cur {
            Some(p) => pos(e.chapter_id).is_none_or(|q| q < p) && (e.status != "done" || pos(e.done_chapter_id).is_some_and(|q| q >= p)),
            None => e.status != "done",
        })
        .collect()
}

fn deadline_line(e: &Event, today: Option<i64>) -> String {
    let mut line = format!("· {}", e.title.trim());
    if !e.who.trim().is_empty() {
        line += &format!("（{}）", e.who.trim());
    }
    match (e.day, today) {
        (Some(due), Some(now)) if due < now => line += &format!("——第 {due} 天到期，已经过了 {} 天，要交代结果", now - due),
        (Some(due), Some(now)) => line += &format!("——第 {due} 天到期，还剩 {} 天", due - now),
        (Some(due), None) => line += &format!("——故事第 {due} 天到期"),
        _ if !e.story_time.trim().is_empty() => line += &format!("——{}", e.story_time.trim()),
        _ => {}
    }
    line
}

/// 写作上下文里的「时间线」：上一章结束时的时间、还没了结的时限
pub fn context_lines(d: &BookData, current: Option<&Chapter>) -> Vec<String> {
    let last = last_timed(d, current);
    let today = last.and_then(|c| c.story_day);
    let mut out: Vec<String> = last.map(|c| format!("{}结束时：{}", d.label(c), describe(&c.story_time, c.story_day))).into_iter().collect();
    let deadlines = open_deadlines(d, current);
    if !deadlines.is_empty() {
        out.push("还没了结的时限：".into());
        out.extend(deadlines.into_iter().map(|e| deadline_line(e, today)));
    }
    out
}

/// 定稿提取用：上一章的时间和本章之前定下的时限（带编号，AI 用编号报告了结了哪条）
pub fn extract_brief(d: &BookData, c: &Chapter) -> String {
    let last = last_timed(d, Some(c));
    let mut s = match last {
        Some(p) => format!("上一章（{}）结束时：{}", d.label(p), describe(&p.story_time, p.story_day)),
        None if d.position(c.id) == Some(0) => "这是第一章：从故事第 1 天算起。".to_string(),
        None => "前面的章节还没有记录时间：按情节估计这一章是故事第几天。".to_string(),
    };
    let deadlines = open_deadlines(d, Some(c));
    if !deadlines.is_empty() {
        s += "\n还没了结的时限（编号. 内容｜到期）：";
        for e in deadlines {
            let due = describe(&e.story_time, e.day);
            s += &format!("\n{}. {}｜{}", e.id, e.title.trim(), if due.is_empty() { "未定" } else { &due });
        }
    }
    s
}

/// 体检：时限过期、快到期，时间倒流
pub fn check(d: &BookData) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut latest: Option<(&Chapter, i64)> = None;
    for c in &d.chapters {
        let Some(day) = c.story_day else { continue };
        if let Some((p, pd)) = latest.filter(|(_, pd)| day < *pd) {
            let mut f = finding(
                "info",
                "时间倒流",
                format!("{}结束时是故事第 {day} 天，比前面的{}（第 {pd} 天）还早；回忆、插叙可以忽略，否则检查一下时间", d.label(c), d.label(p)),
            );
            f.chapter_id = Some(c.id);
            out.push(f);
            continue;
        }
        latest = Some((c, day));
    }
    let Some((now_ch, now)) = latest else { return out };
    for e in open_deadlines(d, None) {
        let Some(due) = e.day else { continue };
        let mut f = if now > due {
            finding(
                "warn",
                "时限已过",
                format!("「{}」应在故事第 {due} 天了结，现在已经写到第 {now} 天（{}），正文里还没交代结果；写过了就在时间线里标成已了结", e.title.trim(), d.label(now_ch)),
            )
        } else if due - now <= DUE_SOON_DAYS {
            finding("info", "时限将到", format!("「{}」第 {due} 天到期，现在是第 {now} 天，还剩 {} 天", e.title.trim(), due - now))
        } else {
            continue;
        };
        f.chapter_id = e.chapter_id;
        out.push(f);
    }
    out
}

// ---------- 接口 ----------

pub async fn get_timeline(State(st): State<AppState>, Path(book_id): Path<i64>) -> ApiResult<Value> {
    require_book(&st.db, book_id)?;
    let metas = st.db.list_chapter_metas(book_id)?;
    let today = metas.iter().rev().find_map(|m| m.story_day);
    let chapters: Vec<Value> = metas
        .iter()
        .map(|m| json!({ "id": m.id, "number": m.number, "label": m.label(), "story_time": m.story_time, "story_day": m.story_day, "words": m.word_count }))
        .collect();
    Ok(Json(json!({ "chapters": chapters, "events": st.db.list_events(book_id)?, "today": today })))
}

pub async fn create_event(State(st): State<AppState>, Path(book_id): Path<i64>, Json(mut e): Json<Event>) -> ApiResult<Event> {
    require_book(&st.db, book_id)?;
    if e.title.trim().is_empty() {
        return Err(bad_request("内容不能为空"));
    }
    e.id = 0;
    e.book_id = book_id;
    Ok(Json(st.db.save_event(&e)?))
}

pub async fn patch_event(State(st): State<AppState>, Path(id): Path<i64>, Json(p): Json<Value>) -> ApiResult<Event> {
    let e = st.db.get_event(id)?.ok_or_else(|| not_found("事件"))?;
    let updated: Event = apply_patch(&e, &p, &["id", "book_id", "updated_at"])?;
    if updated.title.trim().is_empty() {
        return Err(bad_request("内容不能为空"));
    }
    Ok(Json(st.db.save_event(&updated)?))
}

pub async fn delete_event(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    st.db.delete_event(id)?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Book;

    fn ch(id: i64, day: Option<i64>, time: &str) -> Chapter {
        Chapter { id, book_id: 1, sort: id, content: "正文".into(), story_day: day, story_time: time.into(), ..Default::default() }
    }

    fn deadline(id: i64, at: i64, due: i64, done_at: Option<i64>) -> Event {
        Event {
            id,
            book_id: 1,
            chapter_id: Some(at),
            kind: "deadline".into(),
            title: format!("时限{id}"),
            who: "陈渊".into(),
            day: Some(due),
            status: if done_at.is_some() { "done".into() } else { "open".into() },
            done_chapter_id: done_at,
            ..Default::default()
        }
    }

    fn book(chapters: Vec<Chapter>, events: Vec<Event>) -> BookData {
        let mut d = BookData::new(Book::default(), vec![], chapters, vec![], vec![], vec![]);
        d.events = events;
        d
    }

    #[test]
    fn deadlines_follow_the_chapter_being_written() {
        let d = book(
            vec![ch(1, Some(1), "第1天夜里"), ch(2, Some(3), ""), ch(3, None, ""), ch(4, Some(8), "第8天")],
            vec![deadline(10, 1, 4, Some(3)), deadline(11, 2, 6, None), deadline(12, 4, 9, None)],
        );
        let ids = |v: Vec<&Event>| v.iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(open_deadlines(&d, d.chapter(2))), vec![10], "第2章之前只定下了 10");
        assert_eq!(ids(open_deadlines(&d, d.chapter(3))), vec![10, 11], "10 在第3章才了结，写第3章时还没了结");
        assert_eq!(ids(open_deadlines(&d, d.chapter(4))), vec![11]);
        assert_eq!(ids(open_deadlines(&d, None)), vec![11, 12]);

        let lines = context_lines(&d, d.chapter(3));
        assert_eq!(lines[0], "第2章结束时：故事第 3 天", "第3章没记时间，用前面最近的一章");
        assert!(lines.contains(&"· 时限10（陈渊）——第 4 天到期，还剩 1 天".to_string()), "{lines:?}");
        assert!(lines.contains(&"· 时限11（陈渊）——第 6 天到期，还剩 3 天".to_string()));
        let brief = extract_brief(&d, d.chapter(4).unwrap_or_else(|| panic!()));
        assert!(brief.starts_with("上一章（第2章）结束时：故事第 3 天") && brief.contains("\n11. 时限11｜故事第 6 天"), "{brief}");
        assert_eq!(extract_brief(&d, d.chapter(1).unwrap_or_else(|| panic!())), "这是第一章：从故事第 1 天算起。");
    }

    #[test]
    fn flags_missed_deadlines_and_time_going_back() {
        let d = book(vec![ch(1, Some(2), ""), ch(2, Some(9), ""), ch(3, Some(5), "回忆"), ch(4, Some(10), "")], vec![deadline(10, 1, 7, None), deadline(11, 2, 11, None), deadline(12, 1, 3, Some(2))]);
        let found = check(&d);
        let kinds: Vec<&str> = found.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(kinds, vec!["时间倒流", "时限已过", "时限将到"], "{found:?}");
        assert_eq!(found[0].chapter_id, Some(3));
        assert!(found[1].message.contains("「时限10」应在故事第 7 天了结，现在已经写到第 10 天（第4章）"), "{}", found[1].message);
        assert!(found[2].message.contains("还剩 1 天"));
    }
}
