//! AI 使用声明：把 AI 调用按平台声明里常见的用途归类，列出正文里采用了 AI 文字的章节，
//! 生成可以直接贴到平台的声明；创作过程记录导出大纲、各章的修改历史和每天的码字，
//! 被质疑时用来说明创作由作者主导。只统计在本软件里的 AI 使用。

use crate::api::{require_book, ApiResult};
use crate::db::{now, ChapterHistory};
use crate::models::{Book, ChapterMeta, Volume};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 起点 2026 年 8 月起，正文 AI 生成占比超过 10% 的作品会被撤榜、取消推荐
pub const RED_LINE: f64 = 0.10;

/// 平台声明里常见的用途：名称、声明里怎么描述、包含哪些任务
const CATEGORIES: &[(&str, &str, &[&str])] = &[
    ("灵感与设定", "讨论创意、世界观、人物和起名", &["ideas", "world", "characters", "names", "chat"]),
    ("大纲与规划", "辅助整理总纲、卷纲、章纲和场景节拍", &["outline", "volume_outline", "chapter_outlines", "beats", "reveal_plan"]),
    ("简介文案", "辅助撰写作品简介", &["synopsis"]),
    ("讨论与资料查询", "讨论剧情、推演人物反应、查询资料", &["free", "simulate"]),
    ("校对与润色", "检查错别字、病句和标点，润色个别句子", &["proofread", "polish", "deslop", "shorten"]),
    ("正文生成", "续写、扩写或改写部分段落", &["continue", "write_chapter", "expand", "rewrite", "insert", "ghost"]),
    (
        "分析与检查",
        "生成章节摘要、核对设定前后是否一致、分析节奏，不产生正文",
        &["summarize", "extract", "check", "review", "tension", "volume_summary", "revision_plan", "first_read", "style_profile", "library_analyze", "style_distill"],
    ),
];
const OTHER: (&str, &str) = ("其他", "其他辅助");

pub fn category(task: &str) -> &'static str {
    CATEGORIES.iter().find(|(_, _, tasks)| tasks.contains(&task)).map_or(OTHER.0, |(name, ..)| name)
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub category: &'static str,
    pub note: &'static str,
    pub count: i64,
    /// 用到的任务，按次数从多到少
    pub tasks: Vec<String>,
}

/// 按用途汇总，顺序同 CATEGORIES，没用过的用途不列
pub fn group_usage(by_task: &[(String, i64)]) -> Vec<Usage> {
    let groups = CATEGORIES.iter().map(|&(name, note, _)| (name, note)).chain([OTHER]);
    groups
        .filter_map(|(name, note)| {
            let mut hits: Vec<&(String, i64)> = by_task.iter().filter(|(t, _)| category(t) == name).collect();
            if hits.is_empty() {
                return None;
            }
            hits.sort_by(|a, b| b.1.cmp(&a.1));
            Some(Usage { category: name, note, count: hits.iter().map(|h| h.1).sum(), tasks: hits.iter().map(|h| h.0.clone()).collect() })
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct ChapterAi {
    pub id: i64,
    pub number: Option<i64>,
    pub label: String,
    pub words: i64,
    /// 放进正文时累计的 AI 字数，不超过本章字数；作者后来删改的部分没有扣除，是上限
    pub ai_chars: i64,
    pub ratio: f64,
    pub published: bool,
}

pub fn ai_chapters(metas: &[ChapterMeta]) -> Vec<ChapterAi> {
    metas
        .iter()
        .filter(|m| m.ai_chars > 0 && m.word_count > 0)
        .map(|m| {
            let ai = m.ai_chars.min(m.word_count);
            ChapterAi {
                id: m.id,
                number: m.number,
                label: m.label(),
                words: m.word_count,
                ai_chars: ai,
                ratio: ai as f64 / m.word_count as f64,
                published: m.published_at.is_some(),
            }
        })
        .collect()
}

/// 「第3、7、12章」；没有章号的用标题，超过 10 章只列前 10 章
fn chapter_list(chapters: &[ChapterAi]) -> String {
    let numbered: Vec<String> = chapters.iter().filter_map(|c| c.number).take(10).map(|n| n.to_string()).collect();
    let named: Vec<&str> = chapters.iter().filter(|c| c.number.is_none()).map(|c| c.label.as_str()).collect();
    let mut parts = Vec::new();
    if !numbered.is_empty() {
        parts.push(format!("第{}章", numbered.join("、")));
    }
    parts.extend(named.iter().take(10usize.saturating_sub(numbered.len())).map(|t| format!("「{t}」")));
    let mut s = parts.join("、");
    if chapters.len() > 10 {
        s += &format!("等 {} 章", chapters.len());
    }
    s
}

pub fn declaration_text(title: &str, usage: &[Usage], words: i64, chapters: &[ChapterAi], today: &str) -> String {
    let ai_chars: i64 = chapters.iter().map(|c| c.ai_chars).sum();
    let mut s = format!("《{}》AI 辅助创作说明\n\n", title.trim());
    if usage.is_empty() && ai_chars == 0 {
        s += "创作本书时没有使用 AI 工具。\n";
    } else {
        if !usage.is_empty() {
            s += "创作本书时使用了 AI 工具辅助，情况如下：\n";
            for u in usage {
                s += &format!("· {}：{}（{} 次）\n", u.category, u.note, u.count);
            }
            s.push('\n');
        }
        if ai_chars == 0 {
            s += "正文中没有采用 AI 生成的文字。\n";
        } else {
            let pct = ai_chars as f64 * 100.0 / words.max(1) as f64;
            s += &format!(
                "正文中采用的 AI 生成文字约 {ai_chars} 字，约占全书正文的 {pct:.1}%，分布在{}（按放进正文时的字数统计，之后作者的修改没有扣除）。\n",
                chapter_list(chapters)
            );
        }
    }
    s += &format!("\n以上依据创作软件的使用记录统计，截至 {today}。");
    s
}

/// 创作过程记录要用的数据
pub struct LogInput<'a> {
    pub book: &'a Book,
    pub volumes: &'a [Volume],
    pub metas: &'a [ChapterMeta],
    /// 章节 id → 章纲
    pub outlines: &'a HashMap<i64, String>,
    pub history: &'a HashMap<i64, ChapterHistory>,
    /// (章节 id, 任务, 次数)
    pub usage: &'a [(Option<i64>, String, i64)],
    /// (日期, 当天码字)，按日期从早到晚
    pub daily: &'a [(String, i64)],
    pub created: &'a str,
    pub exported: &'a str,
}

/// 表格单元格里不能有竖线和换行
fn cell(s: &str) -> String {
    s.trim().replace('|', "｜").replace(['\r', '\n'], " ")
}

/// 按用途合并次数：「校对与润色 2、分析与检查 5」，顺序同 CATEGORIES
fn calls_brief(calls: &[(&str, i64)]) -> String {
    let groups = CATEGORIES.iter().map(|&(name, ..)| name).chain([OTHER.0]);
    let parts: Vec<String> = groups
        .filter_map(|name| {
            let n: i64 = calls.iter().filter(|(c, _)| *c == name).map(|(_, n)| n).sum();
            (n > 0).then(|| format!("{name} {n}"))
        })
        .collect();
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join("、")
    }
}

pub fn creation_log_text(x: &LogInput) -> String {
    let words: i64 = x.metas.iter().map(|m| m.word_count).sum();
    let done = x.metas.iter().filter(|m| m.status == "done").count();
    let published = x.metas.iter().filter(|m| m.published_at.is_some()).count();
    let mut s = format!("# 《{}》创作过程记录\n\n", x.book.title.trim());
    s += &format!("- 作品创建：{}\n- 导出时间：{}\n", x.created, x.exported);
    s += &format!("- 总字数：{words} 字，共 {} 章（已定稿 {done} 章，已发布 {published} 章）\n", x.metas.len());
    s += "- 由创作软件根据本机数据自动导出，包含大纲、各章的修改历史、每天的码字记录和 AI 使用记录。\n\n";

    s += "## 每日码字\n\n";
    if x.daily.is_empty() {
        s += "（没有记录）\n\n";
    } else {
        s += "| 日期 | 当天码字 |\n|---|---|\n";
        for (day, w) in x.daily {
            s += &format!("| {day} | {w} |\n");
        }
        s.push('\n');
    }

    s += "## 总纲\n\n";
    s += &if x.book.outline.trim().is_empty() { "（未填写）".to_string() } else { x.book.outline.trim().to_string() };
    s += "\n\n";
    let vols: Vec<&Volume> = x.volumes.iter().filter(|v| !v.outline.trim().is_empty()).collect();
    if !vols.is_empty() {
        s += "## 卷纲\n\n";
        for v in vols {
            s += &format!("### {}\n\n{}\n\n", v.title.trim(), v.outline.trim());
        }
    }

    let mut per: HashMap<i64, Vec<(&str, i64)>> = HashMap::new();
    let mut book_level: Vec<(&str, i64)> = Vec::new();
    for (ch, task, n) in x.usage {
        match ch {
            Some(id) => per.entry(*id).or_default().push((category(task), *n)),
            None => book_level.push((category(task), *n)),
        }
    }
    s += "## 各章创作过程\n\n";
    s += "| 章节 | 创建 | 最后修改 | 字数 | 历史版本 | AI 调用 | 采用的 AI 字数 |\n|---|---|---|---|---|---|---|\n";
    for m in x.metas {
        let h = x.history.get(&m.id);
        let versions = match h {
            Some(h) if h.versions > 0 && h.first_version != h.last_version => format!("{} 个（{} 至 {}）", h.versions, h.first_version, h.last_version),
            Some(h) if h.versions > 0 => format!("{} 个（{}）", h.versions, h.first_version),
            _ => "0".into(),
        };
        s += &format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            cell(&m.label()),
            h.map_or("", |h| h.created.as_str()),
            h.map_or("", |h| h.updated.as_str()),
            m.word_count,
            versions,
            calls_brief(per.get(&m.id).map_or(&[][..], |v| v.as_slice())),
            m.ai_chars.min(m.word_count),
        );
    }
    s.push('\n');
    if !book_level.is_empty() {
        s += &format!("不属于某一章的 AI 调用（开书、大纲、设定等）：{}\n\n", calls_brief(&book_level));
    }

    let outlined: Vec<(&ChapterMeta, &String)> =
        x.metas.iter().filter_map(|m| x.outlines.get(&m.id).filter(|o| !o.trim().is_empty()).map(|o| (m, o))).collect();
    if !outlined.is_empty() {
        s += "## 章纲\n\n";
        for (m, o) in outlined {
            s += &format!("### {}\n\n{}\n\n", m.label(), o.trim());
        }
    }
    s.trim_end().to_string() + "\n"
}

pub async fn declaration(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let book = require_book(&st.db, id)?;
    let metas = st.db.list_chapter_metas(id)?;
    let by_task: Vec<(String, i64)> = st.db.ai_usage(id)?.into_iter().map(|(task, n, ..)| (task, n)).collect();
    let usage = group_usage(&by_task);
    let chapters = ai_chapters(&metas);
    let words: i64 = metas.iter().map(|m| m.word_count).sum();
    let ai_chars: i64 = chapters.iter().map(|c| c.ai_chars).sum();
    let today = st.db.local_time(now(), "%Y-%m-%d")?;
    let text = declaration_text(&book.title, &usage, words, &chapters, &today);
    Ok(Json(json!({ "usage": usage, "chapters": chapters, "words": words, "ai_chars": ai_chars, "red_line": RED_LINE, "text": text })))
}

pub async fn creation_log(State(st): State<AppState>, Path(id): Path<i64>) -> ApiResult<Value> {
    let book = require_book(&st.db, id)?;
    let metas = st.db.list_chapter_metas(id)?;
    let outlines: HashMap<i64, String> = st.db.list_chapters(id)?.into_iter().map(|c| (c.id, c.outline)).collect();
    let mut daily = st.db.daily_words(id, i64::MAX)?;
    daily.reverse();
    let markdown = creation_log_text(&LogInput {
        book: &book,
        volumes: &st.db.list_volumes(id)?,
        metas: &metas,
        outlines: &outlines,
        history: &st.db.chapter_history(id)?,
        usage: &st.db.ai_usage_by_chapter(id)?,
        daily: &daily,
        created: &st.db.local_time(book.created_at, "%Y-%m-%d")?,
        exported: &st.db.local_time(now(), "%Y-%m-%d %H:%M")?,
    });
    Ok(Json(json!({ "filename": format!("{}-创作过程记录.md", book.title.trim()), "markdown": markdown })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: i64, number: i64, words: i64, ai: i64) -> ChapterMeta {
        ChapterMeta {
            id,
            volume_id: None,
            sort: id,
            number: Some(number),
            title: format!("标题{number}"),
            status: "done".into(),
            word_count: words,
            ai_chars: ai,
            has_outline: false,
            has_summary: false,
            has_beats: false,
            updated_at: 0,
            published_at: None,
        }
    }

    #[test]
    fn groups_tasks_by_platform_category() {
        let by_task = vec![("summarize".to_string(), 30), ("proofread".to_string(), 2), ("polish".to_string(), 5), ("embed".to_string(), 1), ("continue".to_string(), 4)];
        let usage = group_usage(&by_task);
        let names: Vec<&str> = usage.iter().map(|u| u.category).collect();
        assert_eq!(names, vec!["校对与润色", "正文生成", "分析与检查", "其他"]);
        assert_eq!((usage[0].count, usage[0].tasks.clone()), (7, vec!["polish".to_string(), "proofread".to_string()]));
    }

    #[test]
    fn declaration_states_adopted_ai_text_honestly() {
        let metas = vec![meta(1, 1, 3000, 0), meta(2, 2, 2000, 500), meta(3, 3, 1000, 5000)];
        let chapters = ai_chapters(&metas);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].ai_chars, 1000, "累计采纳超过本章字数时按本章字数算");
        assert!(chapters[1].ratio > RED_LINE && chapters[0].ratio > RED_LINE);
        let usage = group_usage(&[("continue".to_string(), 3), ("check".to_string(), 9)]);
        let text = declaration_text("大梦长生", &usage, 6000, &chapters, "2026-10-05");
        assert!(text.starts_with("《大梦长生》AI 辅助创作说明"), "{text}");
        assert!(text.contains("· 正文生成：续写、扩写或改写部分段落（3 次）"), "{text}");
        assert!(text.contains("约 1500 字，约占全书正文的 25.0%，分布在第2、3章"), "{text}");
        assert!(text.ends_with("截至 2026-10-05。"));
        let clean = declaration_text("大梦长生", &group_usage(&[("proofread".to_string(), 1)]), 6000, &[], "2026-10-05");
        assert!(clean.contains("正文中没有采用 AI 生成的文字。"), "{clean}");
        assert!(declaration_text("书", &[], 100, &[], "2026-10-05").contains("没有使用 AI 工具"));
    }

    #[test]
    fn creation_log_lists_history_outlines_and_calls() {
        let book = Book { title: "大梦长生".into(), outline: "陈渊在梦里修行。".into(), ..Default::default() };
        let volumes = vec![Volume { id: 1, title: "第一卷".into(), outline: "入梦".into(), ..Default::default() }];
        let metas = vec![meta(1, 1, 3000, 0), meta(2, 2, 2000, 300)];
        let outlines = HashMap::from([(1, "醒来|发现梦潮".to_string()), (2, String::new())]);
        let history = HashMap::from([(
            1,
            ChapterHistory { created: "2026-10-01 09:00".into(), updated: "2026-10-03 22:10".into(), versions: 4, first_version: "2026-10-01".into(), last_version: "2026-10-03".into() },
        )]);
        let usage = vec![(Some(1), "proofread".to_string(), 2), (Some(1), "summarize".to_string(), 1), (None, "outline".to_string(), 3)];
        let daily = vec![("2026-10-01".to_string(), 1800), ("2026-10-02".to_string(), 1200)];
        let md = creation_log_text(&LogInput {
            book: &book,
            volumes: &volumes,
            metas: &metas,
            outlines: &outlines,
            history: &history,
            usage: &usage,
            daily: &daily,
            created: "2026-09-30",
            exported: "2026-10-05 23:00",
        });
        assert!(md.contains("- 总字数：5000 字，共 2 章（已定稿 2 章，已发布 0 章）"), "{md}");
        assert!(md.contains("| 2026-10-01 | 1800 |"));
        assert!(md.contains("## 总纲\n\n陈渊在梦里修行。") && md.contains("### 第一卷\n\n入梦"));
        assert!(md.contains("| 第1章 标题1 | 2026-10-01 09:00 | 2026-10-03 22:10 | 3000 | 4 个（2026-10-01 至 2026-10-03） | 校对与润色 2、分析与检查 1 | 0 |"), "{md}");
        assert!(md.contains("| 第2章 标题2 |  |  | 2000 | 0 | — | 300 |"), "{md}");
        assert!(md.contains("不属于某一章的 AI 调用（开书、大纲、设定等）：大纲与规划 3"));
        assert!(md.contains("### 第1章 标题1\n\n醒来|发现梦潮") && !md.contains("### 第2章"), "章纲原样保留，空章纲不列");
    }
}
