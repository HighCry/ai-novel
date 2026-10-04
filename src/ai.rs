//! AI 任务：根据任务类型组装上下文和提示词，流式（SSE）或一次性（JSON）返回结果。

use crate::api::{bad_request, load_book_data, not_found, upstream, ApiResult, AppError};
use crate::db::AiLog;
use crate::library;
use crate::llm::{estimate_tokens, extract_json, ChatRequest, LlmEvent, Message, Usage};
use crate::memory::{compose, match_entries, render, render_entry, BookData, ComposeOpts};
use crate::models::{Book, Chapter, Entry, Role, Settings};
use crate::prompts::{self, golden_hint, instruction_block, render_id, titled, titled_block, Brief};
use crate::state::AppState;
use crate::stylelib::{guide_block, refs_block, tags_for, Hit, LibQuery};
use crate::text::{clip, head_chars, tail_chars};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct AiRequest {
    pub task: String,
    pub book_id: Option<i64>,
    pub chapter_id: Option<i64>,
    pub volume_id: Option<i64>,
    pub entry_id: Option<i64>,
    /// 选中的文字（扩写/缩写/改写/润色），文风提取时是样章
    pub selection: String,
    /// 光标或选区之前的正文
    pub before: String,
    /// 光标或选区之后的正文
    pub after: String,
    pub instruction: String,
    pub words: Option<i64>,
    /// 多版本对比时的序号，序号越大温度略高
    pub variant: u32,
    pub count: Option<i64>,
    /// 写整章/节拍规划时按大结局处理
    pub finale: bool,
    pub messages: Vec<Message>,
    /// 开书向导的输入：title / genre / platform / idea / protagonist / logline / worldview / characters / outline
    pub fields: Value,
    /// 想参考的范文场景标签，如 对话、打斗
    pub tags: Vec<String>,
    /// 斜杠指令和起名要写的内容
    pub goal: String,
}

pub struct Prepared {
    pub role: Role,
    pub messages: Vec<Message>,
    pub json: bool,
}

/// 写正文时附加的文风指南和检索到的范文
#[derive(Default)]
pub struct StyleCtx {
    pub guide: String,
    pub refs: Vec<Hit>,
}

impl StyleCtx {
    fn block(&self) -> String {
        [guide_block(&self.guide), refs_block(&self.refs)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
    }
}

type Vars = HashMap<&'static str, String>;

/// 所有 JSON 任务都追加这条，作者改过模板也能保证格式（中文模型常在字符串里用英文引号）。
pub const JSON_RULE: &str = "\n\n输出格式要求：只输出一个合法的 JSON 对象，不要加任何说明文字；JSON 字符串内部需要引号时用「」或“”，不要用英文双引号。";

/// 会带上文风指南和范文参考的写正文任务
const STYLED_TASKS: &[&str] = &["continue", "write_chapter", "expand", "rewrite", "polish", "ghost", "insert"];

fn field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}

fn writer_system(book: &Book, ov: &HashMap<String, String>, style: &StyleCtx) -> String {
    let mut v = Vars::new();
    v.insert("genre_block", library::genre_block(&book.genre));
    v.insert("style_block", titled_block("本书文风要求", &book.style_guide, 2000));
    let sample = if book.style_sample.trim().is_empty() {
        String::new()
    } else {
        format!("【文风参考：模仿它的语感、句式和节奏，不要照抄内容】\n{}", clip(&book.style_sample, 1500))
    };
    v.insert("sample_block", sample);
    let system = render_id("system.writer", &v, ov);
    let extra = style.block();
    if extra.is_empty() { system } else { format!("{system}\n\n{extra}") }
}

fn ghost_size(words: i64) -> String {
    match words {
        w if w <= 0 => "一两句话，约 50 字".into(),
        w if w <= 80 => format!("一两句话，约 {w} 字"),
        w if w <= 250 => format!("一小段，约 {w} 字"),
        w => format!("约 {w} 字"),
    }
}

fn editor_system(genre: &str, ov: &HashMap<String, String>) -> String {
    let mut v = Vars::new();
    v.insert("genre_block", library::genre_block(genre));
    render_id("system.editor", &v, ov)
}

fn brief_from(req: &AiRequest, data: Option<&BookData>) -> Brief {
    let f = &req.fields;
    let book = data.map(|d| &d.book);
    let pick = |key: &str, stored: Option<&String>| {
        let v = field(f, key);
        if v.is_empty() { stored.map(|s| s.trim().to_string()).unwrap_or_default() } else { v }
    };
    let characters = {
        let v = field(f, "characters");
        if v.is_empty() {
            data.map(|d| {
                d.entries
                    .iter()
                    .filter(|e| e.kind == "character")
                    .map(|e| render_entry(e, &d.state_at(e, None)))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
        } else {
            v
        }
    };
    Brief {
        title: pick("title", book.map(|b| &b.title)),
        genre: pick("genre", book.map(|b| &b.genre)),
        platform: pick("platform", book.map(|b| &b.platform)),
        idea: field(f, "idea"),
        protagonist: field(f, "protagonist"),
        logline: pick("logline", book.map(|b| &b.logline)),
        worldview: pick("worldview", book.map(|b| &b.worldview)),
        characters,
        outline: pick("outline", book.map(|b| &b.outline)),
    }
}

fn need(data: Option<&BookData>) -> Result<&BookData, AppError> {
    data.ok_or_else(|| bad_request("缺少 book_id"))
}

fn finale_block(d: &BookData) -> String {
    let open: Vec<String> = d
        .threads
        .iter()
        .filter(|t| t.status == "open")
        .map(|t| if t.detail.trim().is_empty() { format!("· {}", t.title.trim()) } else { format!("· {}：{}", t.title.trim(), clip(&t.detail, 80)) })
        .collect();
    let mut s = "【大结局】这是全书最后一章：收束主线，给出明确的结局，回收所有未回收的伏笔，最后以“（全书完）”结尾。".to_string();
    if !open.is_empty() {
        s += &format!("\n必须回收的伏笔：\n{}", open.join("\n"));
    }
    s
}

fn outline_block(outline: &str, empty_hint: &str) -> String {
    titled("本章章纲", if outline.trim().is_empty() { empty_hint } else { outline })
}

fn chapter_task_vars(d: &BookData, cur: Option<&Chapter>, ctx: String, req: &AiRequest) -> Vars {
    let mut v = Vars::new();
    v.insert("context", ctx);
    v.insert("chapter", cur.map(|c| d.label(c)).unwrap_or_default());
    v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
    v
}

pub fn build(req: &AiRequest, data: Option<&BookData>, s: &Settings, ov: &HashMap<String, String>, style: &StyleCtx) -> Result<Prepared, AppError> {
    let task = req.task.as_str();
    let words = req.words.unwrap_or(0);
    let prepared = |role, system: String, id: &str, vars: &Vars, json: bool| {
        let mut user = render_id(id, vars, ov);
        if json {
            user.push_str(JSON_RULE);
        }
        Prepared { role, messages: vec![Message::system(system), Message::user(user)], json }
    };
    match task {
        "continue" | "write_chapter" | "beats" | "expand" | "shorten" | "rewrite" | "polish" | "free" | "ghost" | "insert" => {
            let d = need(data)?;
            let cur = req.chapter_id.and_then(|id| d.chapter(id));
            let outline = cur.map(|c| c.outline.as_str()).unwrap_or("");
            let focus: &str = if !req.before.is_empty() { &req.before } else { cur.map(|c| c.content.as_str()).unwrap_or("") };
            let small = matches!(task, "expand" | "shorten" | "rewrite" | "polish");
            let quick = matches!(task, "ghost" | "insert");
            let opts = ComposeOpts {
                current: cur,
                focus_text: if small { &req.selection } else { focus },
                instruction: &req.instruction,
                budget: if small { s.context_budget / 3 } else if quick { s.context_budget / 2 } else { s.context_budget },
                include_world: !small,
                include_outline: !small && !quick,
            };
            let mut v = chapter_task_vars(d, cur, render(&compose(d, &opts)), req);
            let target = if words > 0 { words } else { d.book.target_words.max(1000) };
            match task {
                "continue" => {
                    v.insert("outline_block", outline_block(outline, "（未填写，按前文自然推进）"));
                    v.insert("beats_block", titled_block("本章节拍", cur.map(|c| c.beats.as_str()).unwrap_or(""), 3000));
                    let before = tail_chars(focus, 3000);
                    v.insert("before", if before.trim().is_empty() { "（本章还没有内容，从开头写起）".into() } else { before });
                    v.insert("after_block", titled_block("光标之后已有的内容（续写要能自然衔接到这里）", &head_chars(&req.after, 600), 600));
                    v.insert("words", (if words > 0 { words } else { 800 }).to_string());
                    Ok(prepared(Role::Writer, writer_system(&d.book, ov, style), "task.continue", &v, false))
                }
                "ghost" | "insert" => {
                    v.insert("outline_block", outline_block(outline, "（未填写，按前文自然推进）"));
                    let before = tail_chars(&req.before, 2000);
                    v.insert("before", if before.trim().is_empty() { "（本章还没有内容，从开头写起）".into() } else { before });
                    v.insert("after_block", titled_block("光标之后已有的内容（要能自然衔接到这里）", &head_chars(&req.after, 400), 400));
                    if task == "ghost" {
                        v.insert("beats_block", titled_block("本章节拍", cur.map(|c| c.beats.as_str()).unwrap_or(""), 1500));
                        v.insert("size", ghost_size(words));
                        return Ok(prepared(Role::Writer, writer_system(&d.book, ov, style), "task.ghost", &v, false));
                    }
                    let goal = if req.goal.trim().is_empty() { req.instruction.trim() } else { req.goal.trim() };
                    if goal.is_empty() {
                        return Err(bad_request("请说明要写什么"));
                    }
                    v.insert("goal", goal.to_string());
                    if req.goal.trim().is_empty() {
                        v.insert("instruction_block", String::new());
                    }
                    v.insert("words", (if words > 0 { words } else { 200 }).to_string());
                    Ok(prepared(Role::Writer, writer_system(&d.book, ov, style), "task.insert", &v, false))
                }
                "write_chapter" | "beats" => {
                    let c = cur.ok_or_else(|| bad_request("请先选择章节"))?;
                    v.insert("outline_block", outline_block(&c.outline, "（未填写，请根据前情和大纲合理推进一章）"));
                    v.insert("golden_block", titled("开篇要求", golden_hint(d.number(c))));
                    v.insert("finale_block", if req.finale { finale_block(d) } else { String::new() });
                    v.insert("words", target.to_string());
                    if task == "beats" {
                        return Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.beats", &v, false));
                    }
                    let has_beats = !c.beats.trim().is_empty();
                    v.insert("beats_block", titled_block("本章节拍（按顺序写，每个节拍都要写到）", &c.beats, 3000));
                    v.insert(
                        "beats_rule",
                        if has_beats { "按节拍顺序推进，每个节拍都要写到，不要提前写后面章节的事；" } else { "按章纲推进，不要提前写后面章节的事；" }.into(),
                    );
                    v.insert(
                        "outline_rule",
                        if c.outline.trim().is_empty() {
                            "根据【前情】和【总纲】合理推进本章，只写这一章该发生的事，不要提前写后面章节的情节；"
                        } else {
                            "严格按【本章章纲】写这一章的情节、冲突、出场人物和结局，不要偏离、不要另写一个故事；【总纲】和【前情】只作背景参考，不要把后面章节才会发生的事提前写进本章；"
                        }
                        .into(),
                    );
                    Ok(prepared(Role::Writer, writer_system(&d.book, ov, style), "task.write_chapter", &v, false))
                }
                "free" => {
                    if req.instruction.trim().is_empty() {
                        return Err(bad_request("请输入你想问的问题"));
                    }
                    v.insert("chapter_block", titled("当前章节", &cur.map(|c| d.label(c)).unwrap_or_default()));
                    v.insert("outline_block", titled("本章章纲", outline));
                    v.insert("recent_block", titled_block("当前正文末尾", &tail_chars(focus, 1500), 1500));
                    v.insert("question", req.instruction.trim().to_string());
                    Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.free", &v, false))
                }
                _ => {
                    if req.selection.trim().is_empty() {
                        return Err(bad_request("请先在正文里选中一段文字"));
                    }
                    let n = req.selection.chars().count() as i64;
                    let default_words = match task {
                        "expand" => (n * 2).max(300),
                        "shorten" => (n / 2).max(50),
                        _ => n,
                    };
                    v.insert("before_block", titled_block("前文", &tail_chars(&req.before, 500), 500));
                    v.insert("after_block", titled_block("后文", &head_chars(&req.after, 300), 300));
                    v.insert("selection", req.selection.clone());
                    v.insert("words", (if words > 0 { words } else { default_words }).to_string());
                    let goal = if req.instruction.trim().is_empty() { "换一种写法，让表达更生动具体，情节和信息不变" } else { req.instruction.trim() };
                    v.insert("goal", goal.to_string());
                    Ok(prepared(Role::Writer, writer_system(&d.book, ov, style), &format!("task.{task}"), &v, false))
                }
            }
        }
        "chat" => {
            let d = need(data)?;
            let e = req.entry_id.and_then(|id| d.entries.iter().find(|e| e.id == id)).ok_or_else(|| not_found("人物"))?;
            let convo: String = req.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
            let related = match_entries(&d.entries, &format!("{}\n{}", e.description, convo))
                .into_iter()
                .filter(|x| x.id != e.id && !x.always_include)
                .take(6)
                .map(|x| render_entry(x, &d.state_at(x, None)))
                .collect::<Vec<_>>()
                .join("\n");
            let history: Vec<Message> = req.messages.iter().filter(|m| m.role == "user" || m.role == "assistant").cloned().collect();
            if history.is_empty() {
                return Err(bad_request("请先输入要说的话"));
            }
            let mut v = Vars::new();
            v.insert("book_title", d.book.title.clone());
            v.insert("name", e.name.clone());
            let profile = [
                titled_block("人物设定", &e.description, 1500),
                titled("不可改变的特征", &e.immutable),
                titled("当前状态", &d.state_at(e, None)),
            ]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
            v.insert("profile", profile);
            v.insert("worldview_block", titled_block("世界观", &d.book.worldview, 1200));
            v.insert("related_block", titled_block("相关人物和设定", &related, 1500));
            let skip = history.len().saturating_sub(24);
            let mut messages = vec![Message::system(render_id("system.chat", &v, ov))];
            messages.extend(history.into_iter().skip(skip));
            Ok(Prepared { role: Role::Writer, messages, json: false })
        }
        "world" | "outline" | "synopsis" | "volume_outline" | "ideas" | "characters" => {
            let b = brief_from(req, data);
            let mut v = Vars::new();
            v.insert("brief", b.render());
            v.insert("genre_block", library::genre_block(&b.genre));
            v.insert("platform", prompts::platform_name(&b.platform).to_string());
            v.insert("genre", if b.genre.is_empty() { "网络".into() } else { b.genre.clone() });
            if task == "volume_outline" {
                let d = need(data)?;
                let vol = req.volume_id.and_then(|id| d.volumes.iter().find(|x| x.id == id)).ok_or_else(|| not_found("卷"))?;
                v.insert("volume_title", vol.title.clone());
                v.insert("existing_block", titled_block("已有的卷纲草稿（在此基础上完善）", &vol.outline, 2000));
            }
            let json = matches!(task, "ideas" | "characters");
            Ok(prepared(Role::Writer, editor_system(&b.genre, ov), &format!("task.{task}"), &v, json))
        }
        "chapter_outlines" => {
            let d = need(data)?;
            let count = req.count.unwrap_or(10).clamp(1, 50);
            let start = d.chapters.iter().filter(|c| d.number(c).is_some()).count() as i64 + 1;
            let opts = ComposeOpts { current: None, focus_text: "", instruction: &req.instruction, budget: s.context_budget, include_world: true, include_outline: true };
            let mut ctx = render(&compose(d, &opts));
            if let Some(vol) = req.volume_id.and_then(|id| d.volumes.iter().find(|x| x.id == id)) {
                if !vol.outline.trim().is_empty() {
                    ctx += &format!("\n\n【本卷卷纲（{}）】\n{}", vol.title, clip(&vol.outline, 3000));
                }
            }
            let tail_start = d.chapters.len().saturating_sub(5);
            let recent = d.chapters[tail_start..]
                .iter()
                .map(|c| {
                    let gist = if c.summary.trim().is_empty() { &c.outline } else { &c.summary };
                    format!("{}：{}", d.label(c), clip(gist, 200))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut v = Vars::new();
            v.insert("context", ctx);
            v.insert("recent_block", titled_block("最近几章", &recent, 3000));
            v.insert("start", start.to_string());
            v.insert("end", (start + count - 1).to_string());
            v.insert("count", count.to_string());
            v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
            Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.chapter_outlines", &v, true))
        }
        "names" => {
            let d = need(data)?;
            let goal = if req.goal.trim().is_empty() { req.instruction.trim() } else { req.goal.trim() };
            if goal.is_empty() {
                return Err(bad_request("请说明要给什么起名"));
            }
            let opts = ComposeOpts { current: None, focus_text: goal, instruction: "", budget: s.context_budget / 2, include_world: true, include_outline: false };
            let mut v = Vars::new();
            v.insert("context", render(&compose(d, &opts)));
            v.insert("goal", goal.to_string());
            v.insert("instruction_block", if req.goal.trim().is_empty() { String::new() } else { instruction_block(&req.instruction, "补充要求") });
            Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.names", &v, true))
        }
        "simulate" => {
            let d = need(data)?;
            let opts = ComposeOpts { current: None, focus_text: "", instruction: &req.instruction, budget: s.context_budget, include_world: true, include_outline: true };
            let ctx = render(&compose(d, &opts));
            let mut cast: Vec<&Entry> = d.entries.iter().filter(|e| e.is_major()).take(8).collect();
            if cast.is_empty() {
                cast = d.entries.iter().filter(|e| e.kind == "character").take(6).collect();
            }
            if cast.is_empty() {
                return Err(bad_request("设定库里还没有人物"));
            }
            let ids: HashSet<i64> = cast.iter().map(|e| e.id).collect();
            let people = cast.iter().map(|e| render_entry(e, &d.state_at(e, None))).collect::<Vec<_>>().join("\n");
            let rels = d.relation_lines(&ids).join("\n");
            let mut v = Vars::new();
            v.insert("context", ctx);
            v.insert("characters_block", [titled_block("主要人物", &people, 4000), titled_block("人物关系", &rels, 1500)].join("\n"));
            v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
            Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.simulate", &v, false))
        }
        "summarize" | "extract" | "check" | "review" | "tension" | "first_read" | "revision_plan" => {
            let d = need(data)?;
            let c = req.chapter_id.and_then(|id| d.chapter(id)).ok_or_else(|| not_found("章节"))?;
            if c.content.trim().is_empty() {
                return Err(bad_request("这一章还没有正文"));
            }
            let system = render_id("system.analyst", &Vars::new(), ov);
            let mut v = Vars::new();
            v.insert("chapter", d.label(c));
            v.insert("content", clip(&c.content, 20000));
            v.insert("outline_block", titled("本章章纲", &c.outline));
            let json = !matches!(task, "summarize" | "first_read" | "revision_plan");
            match task {
                "first_read" => {
                    v.insert("platform", prompts::platform_name(&d.book.platform).to_string());
                    v.insert("genre", if d.book.genre.is_empty() { "网络".into() } else { d.book.genre.clone() });
                }
                "revision_plan" => {
                    let opts = ComposeOpts { current: Some(c), focus_text: &c.content, instruction: "", budget: s.context_budget / 2, include_world: true, include_outline: true };
                    v.insert("context", render(&compose(d, &opts)));
                    v.insert("feedback_block", titled_block("作者提供的反馈", &req.instruction, 3000));
                }
                "extract" => {
                    let entries = d
                        .entries
                        .iter()
                        .map(|e| {
                            let st = d.state_at(e, Some(c));
                            format!("{}｜{}｜{}", e.name, e.kind_label(), if st.trim().is_empty() { "—".to_string() } else { clip(&st, 120) })
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let threads = d.threads.iter().filter(|t| t.status == "open").map(|t| format!("{}. {}", t.id, t.title)).collect::<Vec<_>>().join("\n");
                    v.insert("entries", if entries.is_empty() { "（暂无）".into() } else { entries });
                    v.insert("threads", if threads.is_empty() { "（暂无）".into() } else { threads });
                }
                "check" => {
                    let opts = ComposeOpts { current: Some(c), focus_text: &c.content, instruction: "", budget: s.context_budget, include_world: true, include_outline: false };
                    v.insert("context", render(&compose(d, &opts)));
                }
                "review" => {
                    let mut info = format!("【作品】{}", d.book.title);
                    if !d.book.genre.is_empty() {
                        info += &format!("（{}）", d.book.genre);
                    }
                    if !d.book.logline.is_empty() {
                        info += &format!("\n【一句话梗概】{}", d.book.logline);
                    }
                    v.insert("info", info);
                    v.insert("platform", prompts::platform_name(&d.book.platform).to_string());
                }
                _ => {}
            }
            Ok(prepared(Role::Analyst, system, &format!("task.{task}"), &v, json))
        }
        "volume_summary" => {
            let d = need(data)?;
            let vol = req.volume_id.and_then(|id| d.volumes.iter().find(|x| x.id == id)).ok_or_else(|| not_found("卷"))?;
            let summaries = d
                .chapters
                .iter()
                .filter(|c| c.volume_id == Some(vol.id))
                .map(|c| {
                    let gist = if c.summary.trim().is_empty() { clip(&c.content, 300) } else { c.summary.trim().to_string() };
                    format!("{}：{}", d.label(c), gist)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if summaries.is_empty() {
                return Err(bad_request("这一卷还没有章节"));
            }
            let mut v = Vars::new();
            v.insert("volume_title", vol.title.clone());
            v.insert("summaries", clip(&summaries, 15000));
            Ok(prepared(Role::Analyst, render_id("system.analyst", &Vars::new(), ov), "task.volume_summary", &v, false))
        }
        "style_profile" => {
            let sample = if req.selection.trim().is_empty() { data.map(|d| d.book.style_sample.clone()).unwrap_or_default() } else { req.selection.clone() };
            if sample.trim().chars().count() < 200 {
                return Err(bad_request("样章太短，至少需要 200 字"));
            }
            let mut v = Vars::new();
            v.insert("stats", crate::lint::style_stats_text(&sample));
            v.insert("sample", clip(&sample, 4000));
            Ok(prepared(Role::Analyst, render_id("system.analyst", &Vars::new(), ov), "task.style_profile", &v, false))
        }
        _ => Err(bad_request(format!("未知的任务类型：{task}"))),
    }
}

/// 去掉模型在纯文本结果前加的标题、分隔线和“剧情摘要：”之类的前缀，以及 Markdown 粗体标记。
pub fn clean_plain(text: &str) -> String {
    let mut lines: Vec<&str> = text.trim().lines().collect();
    while let Some(first) = lines.first() {
        let s = first.trim();
        let short = s.chars().count() <= 40;
        let rule = s.len() >= 3 && s.chars().all(|c| matches!(c, '-' | '*' | '_' | '='));
        let heading = s.starts_with("# ") || (short && s.starts_with("**") && (s.ends_with("**") || s.ends_with("**：") || s.ends_with("：**")));
        if s.is_empty() || rule || heading {
            lines.remove(0);
        } else {
            break;
        }
    }
    let mut out = lines.join("\n").replace("**", "");
    for prefix in ["剧情摘要：", "本章摘要：", "卷摘要：", "摘要："] {
        if let Some(rest) = out.strip_prefix(prefix) {
            out = rest.trim_start().to_string();
            break;
        }
    }
    out.trim().to_string()
}

fn input_chars(messages: &[Message]) -> i64 {
    messages.iter().map(|m| m.content.chars().count() as i64).sum()
}

fn load(st: &AppState, req: &AiRequest) -> Result<Option<BookData>, AppError> {
    match req.book_id {
        Some(id) => Ok(Some(load_book_data(&st.db, id)?)),
        None => Ok(None),
    }
}

/// 检索范文用的查询文字和场景标签
fn style_query(req: &AiRequest, d: &BookData) -> (String, Vec<String>) {
    let cur = req.chapter_id.and_then(|id| d.chapter(id));
    let mut tags = req.tags.clone();
    let mut q = String::new();
    match req.task.as_str() {
        "expand" | "rewrite" | "polish" => q.push_str(&head_chars(&req.selection, 400)),
        "write_chapter" => {
            if let Some(c) = cur {
                q = format!("{}\n{}", clip(&c.outline, 300), clip(&c.beats, 400));
                if d.number(c) == Some(1) {
                    tags.push("开篇".into());
                }
            }
        }
        _ => {
            let before = if req.before.is_empty() { cur.map(|c| c.content.as_str()).unwrap_or("") } else { req.before.as_str() };
            q = format!("{}\n{}\n{}", tail_chars(before, 300), cur.map(|c| clip(&c.outline, 200)).unwrap_or_default(), req.goal);
        }
    }
    q.push('\n');
    q.push_str(&req.instruction);
    for t in tags_for(&format!("{} {}", req.instruction, req.goal)) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    (q, tags)
}

/// 写正文任务：取文风指南（本题材优先），从文风库检索范文；配置了向量模型时混合语义检索（灰字续写为了速度不用）。
async fn gather_style(st: &AppState, req: &AiRequest, data: Option<&BookData>, s: &Settings) -> StyleCtx {
    let mut ctx = StyleCtx::default();
    let Some(d) = data.filter(|_| STYLED_TASKS.contains(&req.task.as_str())) else { return ctx };
    if s.use_style_guide {
        match st.db.guide_for(&d.book.genre) {
            Ok(Some(g)) => ctx.guide = g.content,
            Ok(None) => {}
            Err(e) => tracing::warn!("读取文风指南失败：{e:#}"),
        }
    }
    let k = if req.task == "ghost" { s.library_refs.min(2) } else { s.library_refs.min(6) };
    if k == 0 {
        return ctx;
    }
    let index = match st.library_index() {
        Ok(ix) if !ix.is_empty() => ix,
        Ok(_) => return ctx,
        Err(e) => {
            tracing::warn!("建立文风库索引失败：{e:#}");
            return ctx;
        }
    };
    let (query, tags) = style_query(req, d);
    let mut vector = None;
    if req.task != "ghost" {
        if let Some((model, provider)) = s.resolve_embedding().filter(|(m, _)| index.has_vectors(m)) {
            match st.llm.embed(&provider, &model, &[query.clone()]).await {
                Ok(mut v) => vector = v.pop().map(|v| (v, model)),
                Err(e) => tracing::warn!("向量检索失败，只用关键词检索：{e:#}"),
            }
        }
    }
    ctx.refs = index.search(&LibQuery {
        text: &query,
        tags: &tags,
        genre: &d.book.genre,
        vector: vector.as_ref().map(|(v, m)| (v.as_slice(), m.as_str())),
        k,
    });
    ctx
}

async fn prepare_request(st: &AppState, req: &AiRequest) -> Result<(Settings, Prepared, StyleCtx), AppError> {
    let settings = st.db.get_settings()?;
    let overrides = st.db.prompt_overrides()?;
    let data = load(st, req)?;
    let style = gather_style(st, req, data.as_ref(), &settings).await;
    let prep = build(req, data.as_ref(), &settings, &overrides, &style)?;
    Ok((settings, prep, style))
}

fn style_meta(style: &StyleCtx) -> Value {
    json!({
        "guide": !style.guide.trim().is_empty(),
        "refs": style.refs.iter().map(|h| json!({ "item_id": h.item_id, "title": h.title, "tags": h.tags })).collect::<Vec<_>>(),
    })
}

fn log_usage(st: &crate::db::Db, req: &AiRequest, model: &str, chars_in: i64, output: &str, usage: Option<Usage>) {
    let chars_out = output.chars().count() as i64;
    let (prompt_tokens, completion_tokens, estimated) = match usage {
        Some(u) => (u.prompt_tokens, u.completion_tokens, false),
        None => (estimate_tokens(chars_in), estimate_tokens(chars_out), true),
    };
    let log = AiLog {
        book_id: req.book_id,
        chapter_id: req.chapter_id,
        task: &req.task,
        model,
        input_chars: chars_in,
        output_chars: chars_out,
        prompt_tokens,
        completion_tokens,
        estimated,
    };
    if let Err(e) = st.log_ai(&log) {
        tracing::warn!("记录 AI 调用失败：{e:#}");
    }
}

/// 只组装提示词、不调用模型，用于「查看提示词」。
pub async fn preview(State(st): State<AppState>, Json(req): Json<AiRequest>) -> ApiResult<Value> {
    let (settings, prep, style) = prepare_request(&st, &req).await?;
    let chars = input_chars(&prep.messages);
    let model = settings.resolve(prep.role).map(|(cfg, p)| format!("{} · {}", p.name, cfg.model)).unwrap_or_else(|e| e.to_string());
    Ok(Json(json!({
        "messages": prep.messages,
        "chars": chars,
        "estimated_tokens": estimate_tokens(chars),
        "model": model,
        "role": if prep.role == Role::Writer { "writer" } else { "analyst" },
        "style": style_meta(&style),
    })))
}

pub async fn stream(
    State(st): State<AppState>,
    Json(req): Json<AiRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let (settings, prep, style) = prepare_request(&st, &req).await?;
    let meta = Event::default().event("meta").data(style_meta(&style).to_string());
    let (cfg, provider) = settings.resolve(prep.role).map_err(|e| bad_request(e.to_string()))?;
    let chat = ChatRequest {
        model: cfg.model,
        messages: prep.messages,
        temperature: (cfg.temperature + req.variant as f32 * 0.08).min(1.5),
        max_tokens: cfg.max_tokens,
        json_mode: false,
        stream_usage: settings.stream_usage,
    };
    let chars_in = input_chars(&chat.messages);
    let (tx, rx) = mpsc::channel::<LlmEvent>(256);
    let (llm, db) = (st.llm.clone(), st.db.clone());
    tokio::spawn(async move {
        match llm.chat_stream(&provider, &chat, &tx).await {
            Ok((text, usage)) => {
                log_usage(&db, &req, &chat.model, chars_in, &text, usage);
                let _ = tx.send(LlmEvent::Done(text)).await;
            }
            Err(e) => {
                let _ = tx.send(LlmEvent::Error(format!("{e:#}"))).await;
            }
        }
    });
    let events = ReceiverStream::new(rx).map(|ev| {
        let (name, data) = match ev {
            LlmEvent::Delta(t) => ("delta", json!({ "text": t })),
            LlmEvent::Reasoning(n) => ("thinking", json!({ "chars": n })),
            LlmEvent::Done(t) => ("done", json!({ "text": t })),
            LlmEvent::Error(m) => ("error", json!({ "message": m })),
        };
        Ok::<Event, Infallible>(Event::default().event(name).data(data.to_string()))
    });
    let head = tokio_stream::once(Ok::<Event, Infallible>(meta));
    Ok(Sse::new(head.chain(events)).keep_alive(KeepAlive::default()))
}

fn chat_request(settings: &Settings, prep: Prepared) -> Result<(ChatRequest, crate::models::Provider), AppError> {
    let (cfg, provider) = settings.resolve(prep.role).map_err(|e| bad_request(e.to_string()))?;
    let chat = ChatRequest {
        model: cfg.model,
        messages: prep.messages,
        temperature: cfg.temperature,
        max_tokens: cfg.max_tokens,
        json_mode: prep.json && settings.json_mode,
        stream_usage: false,
    };
    Ok((chat, provider))
}

/// 一次性调用模型拿 JSON；输出不合法或被截断时，带着原输出让模型重发一次。
pub async fn complete_json(st: &AppState, settings: &Settings, prep: Prepared, req: &AiRequest) -> Result<Value, AppError> {
    let (chat, provider) = chat_request(settings, prep)?;
    let first = st.llm.chat(&provider, &chat).await.map_err(upstream)?;
    log_usage(&st.db, req, &chat.model, input_chars(&chat.messages), &first.text, first.usage);
    match extract_json(&first.text) {
        Ok(v) => Ok(v),
        Err(err) => {
            let ask = if first.finish_reason == "length" {
                "上面的输出被长度限制截断了。请精简内容，重新输出一个完整、合法的 JSON，不要任何说明。"
            } else {
                "上面的输出不是合法的 JSON。请重新输出完整、合法的 JSON：字符串里需要引号时用「」，不要任何说明。"
            };
            let mut retry = chat.clone();
            retry.messages.push(Message::assistant(first.text.clone()));
            retry.messages.push(Message::user(ask));
            let second = st.llm.chat(&provider, &retry).await.map_err(upstream)?;
            log_usage(&st.db, req, &retry.model, input_chars(&retry.messages), &second.text, second.usage);
            extract_json(&second.text).map_err(|e| upstream(anyhow::anyhow!("{err:#}；重试后仍然失败：{e:#}")))
        }
    }
}

/// 一次性调用模型拿纯文本，去掉标题、分隔线等多余格式。
pub async fn complete_text(st: &AppState, settings: &Settings, prep: Prepared, req: &AiRequest) -> Result<String, AppError> {
    let (chat, provider) = chat_request(settings, prep)?;
    let out = st.llm.chat(&provider, &chat).await.map_err(upstream)?;
    log_usage(&st.db, req, &chat.model, input_chars(&chat.messages), &out.text, out.usage);
    Ok(clean_plain(&out.text))
}

pub async fn json_task(State(st): State<AppState>, Json(req): Json<AiRequest>) -> ApiResult<Value> {
    let (settings, prep, _) = prepare_request(&st, &req).await?;
    if prep.json {
        let value = complete_json(&st, &settings, prep, &req).await?;
        if req.task == "tension" {
            if let Some(cid) = req.chapter_id {
                st.db.set_chapter_metrics(cid, &value)?;
            }
        }
        return Ok(Json(value));
    }
    let text = complete_text(&st, &settings, prep, &req).await?;
    match req.task.as_str() {
        "summarize" => {
            if let Some(mut c) = req.chapter_id.map(|id| st.db.get_chapter(id)).transpose()?.flatten() {
                c.summary = text.clone();
                st.db.update_chapter(&c)?;
            }
        }
        "volume_summary" => {
            if let Some(mut v) = req.volume_id.map(|id| st.db.get_volume(id)).transpose()?.flatten() {
                v.summary = text.clone();
                st.db.update_volume(&v)?;
            }
        }
        _ => {}
    }
    Ok(Json(json!({ "text": text })))
}

#[cfg(test)]
mod tests {
    use super::clean_plain;

    #[test]
    fn strips_titles_and_markdown() {
        assert_eq!(clean_plain("**剧情摘要：**\n\n林凡进了城，**遇到**苏雨。"), "林凡进了城，遇到苏雨。");
        assert_eq!(clean_plain("# 卷摘要\n---\n剧情摘要：第一卷讲了……"), "第一卷讲了……");
        assert_eq!(clean_plain("正常文本"), "正常文本");
    }
}
