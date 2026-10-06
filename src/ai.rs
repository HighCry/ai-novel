//! AI 任务：根据任务类型组装上下文和提示词，流式（SSE）或一次性（JSON）返回结果。

use crate::api::{bad_request, content_hash, load_book_data, mark_early_terms, not_found, upstream, ApiResult, AppError};
use crate::db::AiLog;
use crate::library;
use crate::llm::{estimate_tokens, extract_json, ChatRequest, LlmClient, LlmEvent, Message, Usage, CANCELLED};
use crate::memory::{compose, match_entries, plan_text, render, render_entry, secret_ledger, BookData, ComposeOpts};
use crate::models::{Book, Chapter, Entry, Provider, Role, Settings};
use crate::prompts::{self, golden_hint, instruction_block, render_id, titled, titled_block, Brief};
use crate::skills::{self, Skill};
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
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
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
    /// 技能修订（deslop）用哪个技能，空着用内置的「网文去AI味」
    pub skill: String,
}

pub struct Prepared {
    pub role: Role,
    pub messages: Vec<Message>,
    pub json: bool,
    /// 不用这个角色设置里的温度时填上
    pub temperature: Option<f32>,
}

/// 技能修订要用的：技能、要修订的正文、本地检测结果、必须原样保留的设定词
pub struct Revise {
    pub skill: Skill,
    pub text: String,
    pub report: crate::lint::LintReport,
    pub keep: Vec<String>,
}

/// 写正文时附加的写作技能、文风指南和检索到的范文
#[derive(Default)]
pub struct StyleCtx {
    /// 启用技能的「写作规则」
    pub skills: String,
    pub guide: String,
    pub refs: Vec<Hit>,
    pub revise: Option<Revise>,
    /// 校对时正文里的设定词：前端核对修改稿里有没有被改掉
    pub proofread_keep: Option<Vec<String>>,
    /// 分块修订、校对时块与块之间的段落分隔符
    pub sep: &'static str,
}

impl StyleCtx {
    fn block(&self) -> String {
        [self.skills.clone(), guide_block(&self.guide), refs_block(&self.refs)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

type Vars = HashMap<&'static str, String>;

/// 所有 JSON 任务都追加这条，作者改过模板也能保证格式（中文模型常在字符串里用英文引号）。
pub const JSON_RULE: &str = "\n\n输出格式要求：只输出一个合法的 JSON 对象，不要加任何说明文字；JSON 字符串内部需要引号时用「」或“”，不要用英文双引号。";

/// 读者能看到的作品信息：书名、题材、梗概、简介、金手指、写了多少、开篇三章；给挑梗和读者偏好预测用
fn book_brief(d: &BookData) -> String {
    let b = &d.book;
    let mut s = format!("书名：《{}》\n平台：{}", b.title.trim(), prompts::platform_name(&b.platform));
    for (label, v, n) in [("题材", &b.genre, 40), ("一句话梗概", &b.logline, 200), ("简介", &b.synopsis, 400), ("金手指", &b.golden_finger, 100)] {
        if !v.trim().is_empty() {
            s += &format!("\n{label}：{}", clip(v.trim(), n));
        }
    }
    let written: Vec<&Chapter> = d.chapters.iter().filter(|c| !c.content.trim().is_empty()).collect();
    s += &format!("\n已写：{} 章，{} 字", written.len(), written.iter().map(|c| c.word_count).sum::<i64>());
    let opening: Vec<String> = written
        .iter()
        .take(3)
        .map(|c| {
            let gist = if c.summary.trim().is_empty() { &c.outline } else { &c.summary };
            let mut line = format!("{}：{}", d.label(c), clip(gist.trim(), 150));
            if let Some(h) = c.metrics["hook"]["type"].as_str().filter(|h| !h.is_empty()) {
                line += &format!("（章末钩子：{h}）");
            }
            line
        })
        .collect();
    if !opening.is_empty() {
        s += &format!("\n开篇三章：\n{}", opening.join("\n"));
    }
    s
}

/// 拆书时每章最多发给模型的字数
const TEARDOWN_CHARS: usize = 9000;

const PROOFREADER: &str = "你是严谨的中文校对编辑，熟悉网络小说。你只改正文字硬伤：错别字、明显的病句和标点错误；不评价、不润色、不改写作者的表达。";

/// 会带上文风指南和范文参考的写正文任务
const STYLED_TASKS: &[&str] = &["continue", "write_chapter", "expand", "rewrite", "polish", "ghost", "insert", "deslop"];

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
                d.entries_at(None)
                    .into_iter()
                    .filter(|e| e.kind == "character")
                    .map(|e| render_entry(e, &d.description_at(e, None), &d.state_at(e, None)))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
        } else {
            v
        }
    };
    let world = data.map(|d| d.world_at(None));
    let outline = data.map(|d| d.outline_at(None));
    Brief {
        title: pick("title", book.map(|b| &b.title)),
        genre: pick("genre", book.map(|b| &b.genre)),
        platform: pick("platform", book.map(|b| &b.platform)),
        idea: field(f, "idea"),
        protagonist: field(f, "protagonist"),
        logline: pick("logline", book.map(|b| &b.logline)),
        worldview: pick("worldview", world.as_ref()),
        characters,
        outline: pick("outline", outline.as_ref()),
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
        Prepared { role, messages: vec![Message::system(system), Message::user(user)], json, temperature: None }
    };
    match task {
        "memes" | "preference" => {
            let d = need(data)?;
            if req.selection.trim().is_empty() {
                return Err(bad_request("没有拿到热搜或榜单"));
            }
            let mut v = Vars::new();
            v.insert("book_info", book_brief(d));
            v.insert("platform", prompts::platform_name(&d.book.platform).to_string());
            if task == "memes" {
                v.insert("hot", req.selection.clone());
            } else {
                v.insert("ranks", req.selection.clone());
                v.insert("basis", req.instruction.trim().to_string());
            }
            Ok(prepared(Role::Analyst, editor_system(&d.book.genre, ov), &format!("task.{task}"), &v, true))
        }
        "teardown" => {
            if req.selection.trim().is_empty() {
                return Err(bad_request("这一章没有正文"));
            }
            let mut v = Vars::new();
            v.insert("chapter", req.instruction.trim().to_string());
            v.insert("content", clip(&req.selection, TEARDOWN_CHARS));
            Ok(prepared(Role::Analyst, editor_system("", ov), "task.teardown", &v, true))
        }
        "proofread" => {
            let d = need(data)?;
            if req.selection.trim().is_empty() {
                return Err(bad_request("没有要校对的正文"));
            }
            let mut v = Vars::new();
            v.insert("before_block", titled_block("前文（只供理解，不要校对）", &tail_chars(&req.before, 300), 300));
            v.insert("after_block", titled_block("后文（只供理解，不要校对）", &head_chars(&req.after, 200), 200));
            v.insert("keep_block", titled("设定词（不是错别字，原样保留）", &skills::keep_terms(&d.entries, &req.selection).join("、")));
            v.insert("selection", req.selection.clone());
            v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
            Ok(prepared(Role::Writer, PROOFREADER.to_string(), "task.proofread", &v, false))
        }
        "deslop" => {
            let d = need(data)?;
            let cur = req.chapter_id.and_then(|id| d.chapter(id));
            let r = style.revise.as_ref().ok_or_else(|| bad_request("没有选择修订用的技能"))?;
            if r.text.trim().is_empty() {
                return Err(bad_request("没有可修订的正文"));
            }
            let opts = ComposeOpts {
                current: cur,
                focus_text: &r.text,
                instruction: &req.instruction,
                budget: s.context_budget / 3,
                include_world: false,
                include_outline: false,
            };
            let mut v = chapter_task_vars(d, cur, render(&compose(d, &opts)), req);
            v.insert("before_block", titled_block("前文", &tail_chars(&req.before, 500), 500));
            v.insert("after_block", titled_block("后文", &head_chars(&req.after, 300), 300));
            v.insert("issues", crate::lint::ai_brief(&r.report));
            v.insert("keep_block", titled("必须原样保留的设定词（一个字都不要改，所在的信息也不要删）", &r.keep.join("、")));
            // 模型对百分比不敏感，换算成字数下限更管用
            let chars = r.report.stats.chars;
            v.insert("words", chars.to_string());
            v.insert("min_words", ((chars as f32) * (1.0 - crate::lint::cut_ratio(&r.report.ai_level))).round().to_string());
            v.insert("selection", r.text.clone());
            let system = format!("{}\n\n{}", writer_system(&d.book, ov, style), skills::revise_block(&r.skill));
            Ok(prepared(Role::Writer, system, "task.deslop", &v, false))
        }
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
            let desc = d.description_at(e, None);
            let related = match_entries(d.entries_at(None), &format!("{desc}\n{convo}"))
                .into_iter()
                .filter(|x| x.id != e.id && !x.always_include)
                .take(6)
                .map(|x| render_entry(x, &d.description_at(x, None), &d.state_at(x, None)))
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
                titled_block("人物设定", &desc, 1500),
                titled("不可改变的特征", &e.immutable),
                titled("当前状态", &d.state_at(e, None)),
            ]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
            v.insert("profile", profile);
            v.insert("worldview_block", titled_block("世界观", &d.world_at(None), 1200));
            v.insert("related_block", titled_block("相关人物和设定", &related, 1500));
            let skip = history.len().saturating_sub(24);
            let mut messages = vec![Message::system(render_id("system.chat", &v, ov))];
            messages.extend(history.into_iter().skip(skip));
            Ok(Prepared { role: Role::Writer, messages, json: false, temperature: None })
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
                v.insert("existing_block", titled_block("已有的卷纲草稿（在此基础上完善）", &d.volume_outline_at(vol, None), 2000));
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
                let vo = d.volume_outline_at(vol, None);
                if !vo.trim().is_empty() {
                    ctx += &format!("\n\n【本卷卷纲（{}）】\n{}", vol.title, clip(&vo, 3000));
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
        "reveal_plan" => {
            let d = need(data)?;
            // 作者底牌默认不发给模型，作者在生成揭示计划时勾选了才带上
            let secrets = req.fields.get("include_secrets").and_then(Value::as_bool).unwrap_or(false);
            let written = d.chapters.iter().filter(|c| !c.content.trim().is_empty()).filter_map(|c| d.number(c)).max().unwrap_or(0);
            let volumes = d
                .volumes
                .iter()
                .filter_map(|vol| {
                    let vo = d.volume_outline_at(vol, None);
                    (!vo.trim().is_empty()).then(|| format!("【卷纲 · {}】\n{}", vol.title, clip(&vo, 6000)))
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let entries = d
                .entries_at(None)
                .into_iter()
                .map(|e| {
                    let mut line = format!("{}｜{}", e.name.trim(), e.kind_label());
                    let desc = d.description_at(e, None);
                    if !desc.trim().is_empty() {
                        line += &format!("｜{}", clip(&desc, 80));
                    }
                    if secrets && !e.secret.trim().is_empty() {
                        line += &format!("｜作者底牌：{}", clip(&e.secret, 200));
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n");
            let chapters = d
                .chapters
                .iter()
                .filter(|c| !c.content.trim().is_empty())
                .map(|c| format!("{}：{}", d.label(c), clip(if c.summary.trim().is_empty() { &c.outline } else { &c.summary }, 300)))
                .collect::<Vec<_>>()
                .join("\n");
            let existing = d
                .active_reveals()
                .map(|r| {
                    let plan = plan_text(r);
                    if plan.is_empty() { format!("· {}", r.title.trim()) } else { format!("· {}（{plan}）", r.title.trim()) }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut v = Vars::new();
            v.insert("world_block", titled_block("世界观（作者层）", &d.world_at(None), 10000));
            v.insert("outline_block", titled_block("总纲（作者层）", &d.outline_at(None), 10000));
            v.insert("volumes_block", volumes);
            v.insert("entries_block", titled_block("设定库（名称｜类别｜描述）", &entries, 8000));
            v.insert("written_block", titled_block("已经写完的章节（章节：摘要）", &chapters, 15000));
            v.insert("existing_block", titled_block("已有的秘密（在此基础上补充和修订，同一个秘密沿用原标题）", &existing, 3000));
            v.insert("written", written.to_string());
            v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
            Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.reveal_plan", &v, true))
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
            let visible = d.entries_at(None);
            let mut cast: Vec<&Entry> = visible.iter().copied().filter(|e| e.is_major()).take(8).collect();
            if cast.is_empty() {
                cast = visible.iter().copied().filter(|e| e.kind == "character").take(6).collect();
            }
            if cast.is_empty() {
                return Err(bad_request("设定库里还没有人物"));
            }
            let ids: HashSet<i64> = cast.iter().map(|e| e.id).collect();
            let people = cast.iter().map(|e| render_entry(e, &d.description_at(e, None), &d.state_at(e, None))).collect::<Vec<_>>().join("\n");
            let rels = d.relation_lines(&ids, None).join("\n");
            let mut v = Vars::new();
            v.insert("context", ctx);
            v.insert("characters_block", [titled_block("主要人物", &people, 4000), titled_block("人物关系", &rels, 1500)].join("\n"));
            v.insert("instruction_block", instruction_block(&req.instruction, "作者的要求"));
            Ok(prepared(Role::Writer, editor_system(&d.book.genre, ov), "task.simulate", &v, false))
        }
        "logic_check" => {
            let d = need(data)?;
            let c = req.chapter_id.and_then(|id| d.chapter(id)).ok_or_else(|| not_found("章节"))?;
            if c.content.trim().is_empty() {
                return Err(bad_request("这一章还没有正文"));
            }
            let opts = ComposeOpts { current: Some(c), focus_text: &c.content, instruction: "", budget: s.context_budget, include_world: true, include_outline: false };
            let pov = d.pov(c).into_iter().map(|e| e.name.trim().to_string()).collect::<Vec<_>>().join("、");
            let mut v = Vars::new();
            v.insert("context", render(&compose(d, &opts)));
            v.insert("secrets_block", titled_block("秘密台账（作者层，只给审校用：真相、读者知道到哪一步、写本章之前谁知情）", &secret_ledger(d, c).join("\n"), 6000));
            v.insert("pov", if pov.is_empty() { "没有标出，按正文判断".into() } else { pov });
            v.insert("chapter", d.label(c));
            v.insert("content", clip(&c.content, 20000));
            // 审校用写作模型（设置里通常是最强的那个），温度按分析模型压低
            let mut p = prepared(Role::Writer, render_id("system.analyst", &Vars::new(), ov), "task.logic_check", &v, true);
            p.temperature = Some(s.analyst.temperature.min(0.3));
            Ok(p)
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
                        .entries_at(Some(c))
                        .into_iter()
                        .map(|e| {
                            let st = d.state_at(e, Some(c));
                            format!("{}｜{}｜{}", e.name, e.kind_label(), if st.trim().is_empty() { "—".to_string() } else { clip(&st, 120) })
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let threads = d.open_threads(Some(c)).into_iter().map(|t| format!("{}. {}", t.id, t.title)).collect::<Vec<_>>().join("\n");
                    // 读者已经知道的秘密也列上：人物知道得比读者晚，角色知识要按人记
                    let reveals = d
                        .active_reveals()
                        .map(|r| {
                            let mut line = format!("{}. {}", r.id, r.title.trim());
                            if !r.truth.trim().is_empty() {
                                line += &format!("｜{}", clip(&r.truth, 80));
                            }
                            let terms = r.term_list();
                            if !terms.is_empty() {
                                line += &format!("｜{}", terms.join("、"));
                            }
                            let plan = plan_text(r);
                            if !plan.is_empty() {
                                line += &format!("｜{plan}");
                            }
                            line += if d.revealed_before(r, Some(c)) { "｜读者已经知道" } else { "｜读者还不知道" };
                            let who: Vec<String> =
                                d.holders_at(r, Some(c)).into_iter().map(|(e, k)| format!("{}{}", e.name.trim(), if k.misread { "（误会）" } else { "" })).collect();
                            if !who.is_empty() {
                                line += &format!("｜已经知情：{}", who.join("、"));
                            }
                            line
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    v.insert("entries", if entries.is_empty() { "（暂无）".into() } else { entries });
                    v.insert("threads", if threads.is_empty() { "（暂无）".into() } else { threads });
                    v.insert("reveals", if reveals.is_empty() { "（暂无）".into() } else { reveals });
                    v.insert("timeline", crate::timeline::extract_brief(d, c));
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

/// 审校结果里每条问题标上 found：引用的原文在正文里找不找得到（模型偶尔会改字或编造）。
fn mark_quotes(content: &str, value: &mut Value) {
    let Some(items) = value.get_mut("issues").and_then(Value::as_array_mut) else { return };
    for it in items {
        let quote = it["quote"].as_str().unwrap_or("").trim().trim_end_matches(|c| c == '…' || c == '.').trim().to_string();
        let found = !quote.is_empty() && content.contains(&quote);
        if let Some(o) = it.as_object_mut() {
            o.insert("found".into(), json!(found));
        }
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
        "expand" | "rewrite" | "polish" | "deslop" => q.push_str(&head_chars(&req.selection, 400)),
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

/// 技能修订每块的大致字数。整章一次修订时模型会大段压缩、顺手删掉台词和细节，切成小块分别修订忠实得多
const REVISE_CHUNK: usize = 800;
/// 分块修订时同时请求的块数上限
const REVISE_PARALLEL: usize = 4;

/// 正文的段落分隔：有空行就按空行分段
fn para_sep(text: &str) -> &'static str {
    if text.contains("\n\n") {
        "\n\n"
    } else {
        "\n"
    }
}

/// 按段落把要修订的正文切成每块约 REVISE_CHUNK 字；不到一块半不切
fn revise_chunks(text: &str) -> Vec<String> {
    let sep = para_sep(text);
    let count = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    if count(text) < REVISE_CHUNK * 3 / 2 {
        return vec![text.to_string()];
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut n = 0;
    for p in text.split(sep).filter(|p| !p.trim().is_empty()) {
        cur.push(p);
        n += count(p);
        if n >= REVISE_CHUNK {
            chunks.push(cur.join(sep));
            cur.clear();
            n = 0;
        }
    }
    if !cur.is_empty() {
        let rest = cur.join(sep);
        match chunks.last_mut() {
            // 剩下的太短就并进上一块
            Some(last) if n < REVISE_CHUNK / 3 => {
                last.push_str(sep);
                last.push_str(&rest);
            }
            _ => chunks.push(rest),
        }
    }
    chunks
}

/// 组装提示词。一般只有一份；技能修订和校对的正文较长时按段落分块，每块一份
async fn prepare_request(st: &AppState, req: &AiRequest) -> Result<(Settings, Vec<Prepared>, StyleCtx), AppError> {
    let settings = st.db.get_settings()?;
    let overrides = st.db.prompt_overrides()?;
    let data = load(st, req)?;
    let mut style = gather_style(st, req, data.as_ref(), &settings).await;
    if data.is_some() {
        style.skills = skills::rules_block(&skills::active(&st.db, &settings));
    }
    if req.task != "deslop" && req.task != "proofread" {
        let prep = build(req, data.as_ref(), &settings, &overrides, &style)?;
        return Ok((settings, vec![prep], style));
    }
    let d = need(data.as_ref())?;
    let skill = if req.task == "deslop" {
        let id = if req.skill.trim().is_empty() { skills::DESLOP_ID } else { req.skill.trim() };
        Some(skills::find(&st.db, id)?.ok_or_else(|| not_found("技能"))?)
    } else {
        None
    };
    let text = if req.selection.trim().is_empty() {
        req.chapter_id.and_then(|id| d.chapter(id)).map(|c| c.content.clone()).unwrap_or_default()
    } else {
        req.selection.clone()
    };
    let revise = |text: &str| {
        skill.as_ref().map(|skill| Revise {
            skill: skill.clone(),
            text: text.to_string(),
            report: crate::lint::lint(text, &settings.extra_cliches),
            keep: skills::keep_terms(&d.entries, text),
        })
    };
    let chunks = revise_chunks(&text);
    let sep = para_sep(&text);
    let mut parts = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        // 每块的前后文用相邻的块
        let sub = AiRequest {
            selection: chunk.clone(),
            before: if i == 0 { req.before.clone() } else { chunks[..i].join(sep) },
            after: if i + 1 == chunks.len() { req.after.clone() } else { chunks[i + 1..].join(sep) },
            ..req.clone()
        };
        style.revise = revise(chunk);
        parts.push(build(&sub, data.as_ref(), &settings, &overrides, &style)?);
    }
    // meta 里给前端核对用的是整段的检测结果和保护词
    style.revise = revise(&text);
    if skill.is_none() {
        style.proofread_keep = Some(skills::keep_terms(&d.entries, &text));
    }
    style.sep = sep;
    Ok((settings, parts, style))
}

fn style_meta(style: &StyleCtx) -> Value {
    let mut meta = json!({
        "guide": !style.guide.trim().is_empty(),
        "refs": style.refs.iter().map(|h| json!({ "item_id": h.item_id, "title": h.title, "tags": h.tags })).collect::<Vec<_>>(),
    });
    // 前端拿这些核对修改稿：删减是否超过上限、有没有设定词被改掉
    if let Some(r) = &style.revise {
        meta["revise"] = json!({
            "skill": r.skill.name,
            "keep": r.keep,
            "max_cut": crate::lint::max_cut(&r.report.ai_level),
            "ai_level": r.report.ai_level,
            "ai_density": r.report.ai_density,
        });
    }
    // 校对只改个别字，删掉超过 3% 多半是把句子删了
    if let Some(keep) = &style.proofread_keep {
        meta["revise"] = json!({ "skill": "校对", "keep": keep, "max_cut": 3 });
    }
    meta
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
    let (settings, parts, style) = prepare_request(&st, &req).await?;
    let chars: i64 = parts.iter().map(|p| input_chars(&p.messages)).sum();
    // 分块修订时只展示第一块的提示词，字数按所有块合计
    let prep = &parts[0];
    let model = settings.resolve(prep.role).map(|(cfg, p)| format!("{} · {}", p.name, cfg.model)).unwrap_or_else(|e| e.to_string());
    Ok(Json(json!({
        "messages": prep.messages,
        "chars": chars,
        "estimated_tokens": estimate_tokens(chars),
        "model": model,
        "role": if prep.role == Role::Writer { "writer" } else { "analyst" },
        "style": style_meta(&style),
        "parts": parts.len(),
    })))
}

/// 拼接分块修订的输出：去掉每块开头的空行，块末尾的空白先压着，块与块之间只留一个段落分隔符
struct Joiner {
    sep: &'static str,
    out: String,
    /// 当前块已经输出过内容
    started: bool,
    /// 暂不输出的空白：块开头的空行，或者可能是块结尾的换行
    held: String,
}

impl Joiner {
    fn new(sep: &'static str) -> Self {
        Self { sep, out: String::new(), started: false, held: String::new() }
    }

    fn next_chunk(&mut self) {
        self.started = false;
        self.held.clear();
    }

    /// 收进一段增量，返回现在可以发给前端的部分
    fn push(&mut self, t: &str) -> String {
        self.held.push_str(t);
        let mut emit = String::new();
        if !self.started {
            let Some(i) = self.held.find(|c: char| !c.is_whitespace()) else { return emit };
            // 丢掉开头的空行，保留首段的缩进
            let start = self.held[..i].rfind('\n').map_or(0, |j| j + 1);
            self.held.drain(..start);
            if !self.out.is_empty() {
                emit.push_str(self.sep);
            }
            self.started = true;
        }
        let body = self.held.trim_end().len();
        emit.push_str(&self.held[..body]);
        self.held.drain(..body);
        self.out.push_str(&emit);
        emit
    }
}

/// 分块修订：几块同时请求，按顺序转发。第一块边生成边发，后面的先攒着，轮到时一次发出再接着实时转发
async fn relay_parts(
    llm: &LlmClient,
    provider: &Provider,
    chats: Vec<ChatRequest>,
    sep: &'static str,
    tx: &mpsc::Sender<LlmEvent>,
) -> anyhow::Result<(String, Option<Usage>)> {
    let gate = Arc::new(Semaphore::new(REVISE_PARALLEL));
    let lanes: Vec<_> = chats
        .into_iter()
        .map(|chat| {
            // 容量要攒得下一整块的输出，没轮到的块才不会卡住
            let (ptx, prx) = mpsc::channel::<LlmEvent>(4096);
            let (llm, provider, gate) = (llm.clone(), provider.clone(), gate.clone());
            let task = tokio::spawn(async move {
                let _permit = gate.acquire_owned().await?;
                // 前面的块出错或用户停止后接收端已关闭，排队的块就不再请求
                if ptx.is_closed() {
                    anyhow::bail!(CANCELLED);
                }
                llm.chat_stream(&provider, &chat, &ptx).await
            });
            (prx, task)
        })
        .collect();
    let mut join = Joiner::new(sep);
    let mut usage = Some(Usage::default());
    for (i, (mut prx, task)) in lanes.into_iter().enumerate() {
        join.next_chunk();
        while let Some(ev) = prx.recv().await {
            let ev = match ev {
                LlmEvent::Delta(t) => {
                    let emit = join.push(&t);
                    if emit.is_empty() {
                        continue;
                    }
                    LlmEvent::Delta(emit)
                }
                other => other,
            };
            tx.send(ev).await.map_err(|_| anyhow::anyhow!(CANCELLED))?;
        }
        let (_, u) = task.await??;
        if !join.started {
            anyhow::bail!("第 {} 块的修订结果是空的", i + 1);
        }
        // 有一块没返回用量就整体按字数估算
        usage = usage.zip(u).map(|(a, b)| Usage {
            prompt_tokens: a.prompt_tokens + b.prompt_tokens,
            completion_tokens: a.completion_tokens + b.completion_tokens,
        });
    }
    Ok((join.out, usage))
}

pub async fn stream(
    State(st): State<AppState>,
    Json(req): Json<AiRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let (settings, parts, style) = prepare_request(&st, &req).await?;
    let meta = Event::default().event("meta").data(style_meta(&style).to_string());
    let sep = if style.sep.is_empty() { "\n" } else { style.sep };
    let (cfg, provider) = settings.resolve(parts[0].role).map_err(|e| bad_request(e.to_string()))?;
    // 修订要克制：温度高了模型会顺手改掉没问题的句子
    let base = match req.task.as_str() {
        "deslop" => cfg.temperature.min(0.7),
        "proofread" => cfg.temperature.min(0.3),
        _ => cfg.temperature,
    };
    let chats: Vec<ChatRequest> = parts
        .into_iter()
        .map(|p| ChatRequest {
            model: cfg.model.clone(),
            messages: p.messages,
            temperature: (base + req.variant as f32 * 0.08).min(1.5),
            max_tokens: cfg.max_tokens,
            json_mode: false,
            stream_usage: settings.stream_usage,
        })
        .collect();
    let chars_in: i64 = chats.iter().map(|c| input_chars(&c.messages)).sum();
    let model = cfg.model;
    let (tx, rx) = mpsc::channel::<LlmEvent>(256);
    let (llm, db) = (st.llm.clone(), st.db.clone());
    tokio::spawn(async move {
        let result = if chats.len() == 1 {
            llm.chat_stream(&provider, &chats[0], &tx).await
        } else {
            relay_parts(&llm, &provider, chats, sep, &tx).await
        };
        match result {
            Ok((text, usage)) => {
                log_usage(&db, &req, &model, chars_in, &text, usage);
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
        temperature: prep.temperature.unwrap_or(cfg.temperature),
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

/// 不经过网页的 JSON 任务（拆书分析）：组装提示词、调用模型、解析 JSON，并记下用量
pub async fn run_json(st: &AppState, req: &AiRequest) -> Result<Value, AppError> {
    let (settings, parts, _) = prepare_request(st, req).await?;
    let Ok([prep]) = <[Prepared; 1]>::try_from(parts) else { return Err(bad_request("这个任务只能用流式接口")) };
    complete_json(st, &settings, prep, req).await
}

pub async fn json_task(State(st): State<AppState>, Json(req): Json<AiRequest>) -> ApiResult<Value> {
    let (settings, parts, _) = prepare_request(&st, &req).await?;
    let Ok([prep]) = <[Prepared; 1]>::try_from(parts) else { return Err(bad_request("分块修订只能用流式接口")) };
    if prep.json {
        let mut value = complete_json(&st, &settings, prep, &req).await?;
        if req.task == "tension" {
            if let Some(cid) = req.chapter_id {
                st.db.set_chapter_metrics(cid, &value)?;
            }
        }
        if req.task == "reveal_plan" {
            if let Some(bid) = req.book_id {
                mark_early_terms(&load_book_data(&st.db, bid)?, &mut value);
            }
        }
        if req.task == "logic_check" {
            if let Some(c) = req.chapter_id.map(|id| st.db.get_chapter(id)).transpose()?.flatten() {
                mark_quotes(&c.content, &mut value);
                let stored = json!({ "at": crate::db::now(), "hash": content_hash(&c.content), "issues": value["issues"].as_array().cloned().unwrap_or_default() });
                st.db.set_chapter_logic(c.id, &stored)?;
                value["at"] = stored["at"].clone();
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
    use super::{clean_plain, mark_quotes, revise_chunks, Joiner};

    #[test]
    fn marks_quotes_missing_from_text() {
        let mut v = serde_json::json!({ "issues": [{ "quote": "陈渊冷笑一声……" }, { "quote": "陈渊冷笑了一声" }, { "quote": "" }, { "problem": "没有原文" }] });
        mark_quotes("陈渊冷笑一声，转身就走。", &mut v);
        let found: Vec<bool> = v["issues"].as_array().unwrap().iter().map(|i| i["found"].as_bool().unwrap()).collect();
        assert_eq!(found, vec![true, false, false, false], "去掉末尾省略号再找；改过字的、空的都算没找到");
    }

    #[test]
    fn strips_titles_and_markdown() {
        assert_eq!(clean_plain("**剧情摘要：**\n\n林凡进了城，**遇到**苏雨。"), "林凡进了城，遇到苏雨。");
        assert_eq!(clean_plain("# 卷摘要\n---\n剧情摘要：第一卷讲了……"), "第一卷讲了……");
        assert_eq!(clean_plain("正常文本"), "正常文本");
    }

    #[test]
    fn proofread_prompt_keeps_setting_terms() {
        use super::{build, AiRequest, StyleCtx};
        use crate::memory::BookData;
        use crate::models::{Book, Entry, Settings};
        let entries = vec![Entry { id: 1, kind: "character".into(), name: "陈渊".into(), ..Default::default() }];
        let d = BookData::new(Book { style_guide: "多用短句".into(), ..Default::default() }, vec![], vec![], entries, vec![], vec![]);
        let req = AiRequest { task: "proofread".into(), selection: "陈渊再屋里坐着.".into(), before: "天黑了。".into(), ..Default::default() };
        let p = build(&req, Some(&d), &Settings::default(), &Default::default(), &StyleCtx::default()).unwrap_or_else(|_| panic!("组装失败"));
        let user = &p.messages[1].content;
        assert!(user.contains("【设定词（不是错别字，原样保留）】陈渊"), "{user}");
        assert!(user.contains("【前文（只供理解，不要校对）】\n天黑了。") && user.contains("陈渊再屋里坐着."), "{user}");
        assert!(!p.messages[0].content.contains("多用短句"), "校对不带文风要求");
        let empty = AiRequest { task: "proofread".into(), ..Default::default() };
        assert!(build(&empty, Some(&d), &Settings::default(), &Default::default(), &StyleCtx::default()).is_err());
    }

    #[test]
    fn splits_revision_by_paragraph() {
        let para = format!("　　{}", "字".repeat(150));
        let book = |n: usize, sep: &str| vec![para.as_str(); n].join(sep);
        assert_eq!(revise_chunks(&book(7, "\n\n")).len(), 1, "不到一块半不切");
        // 每 6 段（900 字）一块，剩下 2 段 300 字单独成块
        let text = book(20, "\n\n");
        let chunks = revise_chunks(&text);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks.join("\n\n"), text, "拼回去和原文一样");
        // 剩下 1 段太短，并进上一块
        let chunks = revise_chunks(&book(19, "\n"));
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[2].matches('\n').count(), 6);
    }

    #[test]
    fn joins_revised_chunks() {
        let mut j = Joiner::new("\n\n");
        let mut sent = String::new();
        for t in ["\n\n　　第一段", "。\n\n　　第二", "段。\n", "\n"] {
            sent += &j.push(t);
        }
        j.next_chunk();
        for t in ["\n", "　　", "第三段。", "\n\n"] {
            sent += &j.push(t);
        }
        assert_eq!(j.out, "　　第一段。\n\n　　第二段。\n\n　　第三段。");
        assert_eq!(sent, j.out, "边生成边发的内容和最终结果一致");
    }
}
