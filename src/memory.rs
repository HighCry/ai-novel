//! 长篇记忆：把设定库、分层摘要、伏笔、相关前文片段组装成有字数预算的上下文。
//! 写某一章时先按可见性剔除这一章还不该看见的资料（见 visibility.rs），再按预算装配；
//! 有揭示计划时再附上读者已知的秘密、本章投放清单和禁区（见 docs/叙事逻辑架构方案.md 5.2）。

use crate::models::*;
use crate::text::*;
use crate::visibility::{visible_text, Gate, Point};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub struct BookData {
    pub book: Book,
    pub volumes: Vec<Volume>,
    pub chapters: Vec<Chapter>,
    pub entries: Vec<Entry>,
    pub threads: Vec<Thread>,
    pub states: Vec<EntryState>,
    pub relations: Vec<Relation>,
    pub reveals: Vec<Reveal>,
    pub reveal_events: Vec<RevealEvent>,
    pub progressions: Vec<Progression>,
    numbers: HashMap<i64, Option<i64>>,
}

impl BookData {
    pub fn new(book: Book, volumes: Vec<Volume>, chapters: Vec<Chapter>, entries: Vec<Entry>, threads: Vec<Thread>, states: Vec<EntryState>) -> Self {
        let nums = number_chapters(chapters.iter().map(|c| c.title.as_str()));
        let numbers = chapters.iter().map(|c| c.id).zip(nums).collect();
        Self {
            book,
            volumes,
            chapters,
            entries,
            threads,
            states,
            relations: Vec::new(),
            reveals: Vec::new(),
            reveal_events: Vec::new(),
            progressions: Vec::new(),
            numbers,
        }
    }

    pub fn entry_name(&self, id: i64) -> Option<&str> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.name.as_str())
    }

    /// 和给定条目有关的关系，一行一条；已结束的关系标注出来，避免写成还很亲密。
    /// 写某一章时只给这一章及之前建立的关系。
    pub fn relation_lines(&self, ids: &HashSet<i64>, current: Option<&Chapter>) -> Vec<String> {
        let cur = current.and_then(|c| self.position(c.id));
        self.relations
            .iter()
            .filter(|r| ids.contains(&r.a_id) || ids.contains(&r.b_id))
            .filter(|r| self.not_after(r.since_chapter_id, cur))
            .filter(|r| r.status != "ended" || (ids.contains(&r.a_id) && ids.contains(&r.b_id)))
            .filter_map(|r| {
                let mut line = format!("· {} 与 {}：{}", self.entry_name(r.a_id)?, self.entry_name(r.b_id)?, r.kind.trim());
                if !r.detail.trim().is_empty() {
                    line += &format!("（{}）", clip(&r.detail, 60));
                }
                if r.status == "ended" {
                    line += "（这段关系已经结束）";
                }
                Some(line)
            })
            .collect()
    }

    /// 快照在时间轴上的位置：初始状态最早；同一章里“开始生效”早于“章末状态”。
    fn state_key(&self, s: &EntryState) -> Option<(i64, i64)> {
        match s.chapter_id {
            None => Some((-1, 0)),
            Some(cid) => self.position(cid).map(|p| (p as i64, if s.phase == "start" { 0 } else { 1 })),
        }
    }

    /// 写某一章时这个设定应该是什么状态：只看这一章之前生效的快照，避免泄露后文。
    /// 没有当前章节时（比如规划后续章纲）用最新状态。
    pub fn state_at(&self, e: &Entry, current: Option<&Chapter>) -> String {
        let snaps: Vec<(&EntryState, (i64, i64))> = self
            .states
            .iter()
            .filter(|s| s.entry_id == e.id)
            .filter_map(|s| self.state_key(s).map(|k| (s, k)))
            .collect();
        if snaps.is_empty() {
            // 没有快照时只知道最新状态：回改前面的章节（后面已经写了）就不给，免得用上后文的状态
            let later_written = current
                .and_then(|c| self.position(c.id))
                .is_some_and(|p| self.chapters[p + 1..].iter().any(|c| !c.content.trim().is_empty()));
            return if later_written { String::new() } else { e.state_text() };
        }
        let cur = current.and_then(|c| self.position(c.id)).map(|p| p as i64);
        snaps
            .into_iter()
            .filter(|(_, (pos, phase))| match cur {
                Some(c) => *pos < c || (*pos == c && *phase == 0),
                None => true,
            })
            .max_by_key(|(s, k)| (*k, s.id))
            .map(|(s, _)| s.text())
            .unwrap_or_default()
    }

    pub fn number(&self, ch: &Chapter) -> Option<i64> {
        self.numbers.get(&ch.id).copied().flatten()
    }

    pub fn label(&self, ch: &Chapter) -> String {
        match self.number(ch) {
            Some(n) if ch.title.trim().is_empty() => format!("第{n}章"),
            Some(n) => format!("第{n}章 {}", ch.title.trim()),
            None => ch.title.trim().to_string(),
        }
    }

    pub fn chapter(&self, id: i64) -> Option<&Chapter> {
        self.chapters.iter().find(|c| c.id == id)
    }

    fn position(&self, id: i64) -> Option<usize> {
        self.chapters.iter().position(|c| c.id == id)
    }

    /// 当前章节之前的所有章节（按顺序）；没有当前章节时返回全部。
    pub fn before(&self, current: Option<&Chapter>) -> &[Chapter] {
        match current.and_then(|c| self.position(c.id)) {
            Some(i) => &self.chapters[..i],
            None => &self.chapters,
        }
    }

    /// 挂在某一章上的资料，写第 cur 章时能不能看见：本章及之前的可以；章节不明或没有当前章节时照旧给。
    fn not_after(&self, chapter_id: Option<i64>, cur: Option<usize>) -> bool {
        match (chapter_id.and_then(|id| self.position(id)), cur) {
            (Some(pos), Some(cur)) => pos <= cur,
            _ => true,
        }
    }

    /// 写这一章时所处的位置：第几卷（按卷的先后数）、第几章。
    pub fn point(&self, ch: &Chapter) -> Point {
        let volume = ch.volume_id.and_then(|vid| self.volumes.iter().position(|v| v.id == vid)).map_or(1, |i| i as i64 + 1);
        Point { volume, chapter: self.number(ch) }
    }

    /// 没有当前章节（规划类任务）时为 None，看作者层。
    pub fn at(&self, current: Option<&Chapter>) -> Option<Point> {
        current.map(|c| self.point(c))
    }

    pub fn uses_volumes(&self) -> bool {
        !self.volumes.is_empty()
    }

    /// 写到 current 时可以给模型看的世界观；没有当前章节时只去掉对 AI 隐藏的节。
    pub fn world_at(&self, current: Option<&Chapter>) -> String {
        visible_text(&self.book.worldview, self.at(current), self.uses_volumes())
    }

    pub fn outline_at(&self, current: Option<&Chapter>) -> String {
        visible_text(&self.book.outline, self.at(current), self.uses_volumes())
    }

    /// 卷纲本身属于某一卷，里面只按标记和「结局」「真相」这类标题筛。
    pub fn volume_outline_at(&self, v: &Volume, current: Option<&Chapter>) -> String {
        visible_text(&v.outline, self.at(current), false)
    }

    /// 写到 current 时可以给模型看的设定条目。
    pub fn entries_at(&self, current: Option<&Chapter>) -> Vec<&Entry> {
        let at = self.at(current);
        self.entries.iter().filter(|e| e.gate().open_at(at)).collect()
    }

    /// 伏笔详情在写 current 时能给多少：定稿会在末尾追加【推进】【回收】记录，但不记是哪一章写的；
    /// 这条伏笔在本章之后还推进或回收过时，分不清哪些记录在后面，只给埋下时的原文。
    pub fn thread_detail_at(&self, t: &Thread, current: Option<&Chapter>) -> String {
        let cur = current.and_then(|c| self.position(c.id));
        let after = |id: Option<i64>| matches!((id.and_then(|id| self.position(id)), cur), (Some(p), Some(c)) if p > c);
        if !after(t.last_chapter_id) && !after(t.resolved_chapter_id) {
            return t.detail.clone();
        }
        let cut = ["【推进】", "【回收】"].iter().filter_map(|m| t.detail.find(m)).min().unwrap_or(t.detail.len());
        t.detail[..cut].trim().to_string()
    }

    /// 写 current 这一章时还没回收的伏笔：本章及之前埋下、到本章时还没回收的。
    pub fn open_threads(&self, current: Option<&Chapter>) -> Vec<&Thread> {
        let cur = current.and_then(|c| self.position(c.id));
        let resolved_later = |t: &Thread| match (t.resolved_chapter_id.and_then(|id| self.position(id)), cur) {
            (Some(r), Some(c)) => r >= c,
            _ => false,
        };
        self.threads
            .iter()
            .filter(|t| self.not_after(t.planted_chapter_id, cur))
            .filter(|t| t.status == "open" || resolved_later(t))
            .collect()
    }

    /// 可见性从第几章起生效，给设定进展排先后用：按卷的取这一卷第一章的章号，这一卷还没有章节时排在已有章节之后。
    fn gate_start(&self, g: Gate) -> i64 {
        match g {
            Gate::Public => 0,
            Gate::Chapter(n) => n,
            Gate::Volume(v) => usize::try_from(v - 1)
                .ok()
                .and_then(|i| self.volumes.get(i))
                .and_then(|vol| self.chapters.iter().filter(|c| c.volume_id == Some(vol.id)).find_map(|c| self.number(c)))
                .unwrap_or(1_000_000 + v),
            Gate::Planning | Gate::Hidden => i64::MAX,
        }
    }

    /// 写到 current 时这个设定的描述：原描述加上已经生效的设定进展，补充接在后面，替换从那里起重写。
    /// 没有当前章节（规划类任务）时看作者层，进展全部生效；写法看不懂的进展按对 AI 隐藏处理。
    pub fn description_at(&self, e: &Entry, current: Option<&Chapter>) -> String {
        let at = self.at(current);
        let mut steps: Vec<(i64, i64, &Progression)> = self
            .progressions
            .iter()
            .filter(|p| p.entry_id == e.id && !p.text.trim().is_empty())
            .filter_map(|p| {
                let g = Gate::parse(&p.gate).unwrap_or(Gate::Hidden);
                g.open_at(at).then(|| (self.gate_start(g), p.id, p))
            })
            .collect();
        steps.sort_by_key(|(start, id, _)| (*start, *id));
        let mut desc = e.description.trim().to_string();
        for (_, _, p) in steps {
            desc = if p.mode == "replace" || desc.is_empty() { p.text.trim().to_string() } else { format!("{desc}\n{}", p.text.trim()) };
        }
        desc
    }

    pub fn active_reveals(&self) -> impl Iterator<Item = &Reveal> {
        self.reveals.iter().filter(|r| r.is_active())
    }

    /// 写 current 这一章时已经发生的揭示进度：本章之前的章节里记下的，按章节先后。没有当前章节时是全部。
    pub fn reveal_events_before(&self, r: &Reveal, current: Option<&Chapter>) -> Vec<&RevealEvent> {
        let cur = current.and_then(|c| self.position(c.id));
        let mut evs: Vec<(usize, &RevealEvent)> = self
            .reveal_events
            .iter()
            .filter(|e| e.reveal_id == r.id)
            .filter_map(|e| self.position(e.chapter_id).map(|p| (p, e)))
            .filter(|(p, _)| cur.map_or(true, |c| *p < c))
            .collect();
        evs.sort_by_key(|(p, e)| (*p, e.id));
        evs.into_iter().map(|(_, e)| e).collect()
    }

    /// 写 current 时读者是否已经知道这个秘密：本章之前有揭开的记录。
    pub fn revealed_before(&self, r: &Reveal, current: Option<&Chapter>) -> bool {
        self.reveal_events_before(r, current).iter().any(|e| e.step == "reveal")
    }

    /// 揭示进度记在第几章，章节删掉了时为 None。
    pub fn event_number(&self, e: &RevealEvent) -> Option<i64> {
        self.chapter(e.chapter_id).and_then(|c| self.number(c))
    }
}

/// 计划里的三步，写成「第3章埋种子、第8章给线索、第40章揭开」。
pub fn plan_text(r: &Reveal) -> String {
    [(r.seed_at, "埋种子"), (r.clue_at, "给线索"), (r.reveal_at, "揭开")]
        .into_iter()
        .filter_map(|(n, label)| n.map(|n| format!("第{n}章{label}")))
        .collect::<Vec<_>>()
        .join("、")
}

/// 读者已经知道的秘密：本章之前揭开的，给出真相。
fn reader_known(data: &BookData, cur: &Chapter) -> Vec<String> {
    data.active_reveals()
        .filter_map(|r| {
            let at = data.reveal_events_before(r, Some(cur)).into_iter().find(|e| e.step == "reveal")?;
            let when = data.event_number(at).map(|n| format!("（第{n}章揭开）")).unwrap_or_default();
            let truth = if r.truth.trim().is_empty() { r.title.trim().to_string() } else { clip(&r.truth, 160) };
            Some(format!("· {}{when}：{truth}", r.title.trim()))
        })
        .collect()
}

/// 本章投放清单：按揭示计划，本章要揭开、要埋的种子、要给的线索，以及近期会用到、可以先挂钩子的秘密。
fn delivery(data: &BookData, cur: &Chapter) -> String {
    let Some(n) = data.number(cur) else { return String::new() };
    let (mut reveal, mut seed, mut clue, mut soon) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for r in data.active_reveals().filter(|r| !data.revealed_before(r, Some(cur))) {
        let title = r.title.trim();
        if r.reveal_at == Some(n) {
            let mut line = format!("· 「{title}」：{}", if r.truth.trim().is_empty() { "按计划在本章揭开" } else { r.truth.trim() });
            if !r.payoff.trim().is_empty() {
                line += &format!("（揭开后：{}）", clip(&r.payoff, 80));
            }
            reveal.push(line);
            continue;
        }
        if r.seed_at == Some(n) {
            seed.push(format!("· 「{title}」{}", if r.seed_note.trim().is_empty() { String::new() } else { format!("：{}", clip(&r.seed_note, 120)) }));
        }
        if r.clue_at == Some(n) {
            clue.push(format!("· 「{title}」{}", if r.clue_note.trim().is_empty() { String::new() } else { format!("：{}", clip(&r.clue_note, 120)) }));
        }
        let planned_now = r.seed_at == Some(n) || r.clue_at == Some(n);
        let started = !data.reveal_events_before(r, Some(cur)).is_empty();
        let next = [r.clue_at, r.reveal_at].into_iter().flatten().filter(|&x| x > n).min();
        if !planned_now && started && !r.is_surprise() && next.is_some_and(|x| x - n <= 10) {
            soon.push(format!("· 「{title}」"));
        }
    }
    [
        ("本章揭开（可以写明）", reveal),
        ("本章要埋的种子（顺嘴带一笔，不解释）", seed),
        ("本章要给的线索（让人物撞见一角，不点破）", clue),
        ("近期会用（可以挂钩子、让人物撞见异常，不解释原理）", soon),
    ]
    .into_iter()
    .filter(|(_, lines)| !lines.is_empty())
    .map(|(head, lines)| format!("{head}：\n{}", lines.join("\n")))
    .collect::<Vec<_>>()
    .join("\n")
}

/// 禁区：写到这一章还没揭开、本章也不揭开的秘密，只给话题和泄露词，不给真相。
fn forbidden(data: &BookData, cur: &Chapter) -> Vec<String> {
    let n = data.number(cur);
    data.active_reveals()
        .filter(|r| !data.revealed_before(r, Some(cur)) && (n.is_none() || r.reveal_at != n))
        .map(|r| {
            let terms = r.term_list();
            if terms.is_empty() { format!("· 「{}」：不点破", r.title.trim()) } else { format!("· 「{}」：不写「{}」", r.title.trim(), terms.join("」「")) }
        })
        .collect()
}

/// 规划用的揭示计划：作者层，带真相、表面误读、三步计划和进度。
fn plan_lines(data: &BookData) -> Vec<String> {
    data.active_reveals()
        .map(|r| {
            let mut line = format!("· 「{}」", r.title.trim());
            if !r.gap.trim().is_empty() {
                line += &format!("（{}）", r.gap.trim());
            }
            if !r.truth.trim().is_empty() {
                line += &format!("真相：{}", clip(&r.truth, 120));
            }
            if !r.misread.trim().is_empty() {
                line += &format!("；读者以为：{}", clip(&r.misread, 60));
            }
            let plan = plan_text(r);
            if !plan.is_empty() {
                line += &format!("；计划：{plan}");
            }
            let done: Vec<String> = data
                .reveal_events_before(r, None)
                .into_iter()
                .filter_map(|e| data.event_number(e).map(|n| format!("第{n}章{}", step_label(&e.step))))
                .collect();
            line += &format!("；进度：{}", if done.is_empty() { "还没动".to_string() } else { done.join("、") });
            if !r.terms.trim().is_empty() {
                line += &format!("；泄露词：{}", r.term_list().join("、"));
            }
            line
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Section {
    pub title: String,
    pub body: String,
}

pub struct ComposeOpts<'a> {
    pub current: Option<&'a Chapter>,
    /// 光标前的正文或选中的文字，用来匹配出场设定
    pub focus_text: &'a str,
    pub instruction: &'a str,
    /// 上下文总字数预算
    pub budget: usize,
    pub include_world: bool,
    pub include_outline: bool,
}

pub struct Chunk {
    pub label: String,
    pub text: String,
}

pub fn make_chunks(data: &BookData, chapters: &[Chapter], target: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    for ch in chapters {
        let label = data.label(ch);
        let mut buf = String::new();
        for p in paragraphs(&ch.content) {
            if !buf.is_empty() && buf.chars().count() + p.chars().count() > target {
                out.push(Chunk { label: label.clone(), text: std::mem::take(&mut buf) });
            }
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(p);
        }
        if !buf.trim().is_empty() {
            out.push(Chunk { label, text: buf });
        }
    }
    out
}

/// 基于中文二元组的 BM25，返回（片段下标, 得分），按得分降序。
pub fn bm25(chunks: &[Chunk], query: &str, top_k: usize) -> Vec<(usize, f32)> {
    let terms: HashSet<String> = tokenize(query).into_iter().filter(|t| t.chars().count() >= 2).collect();
    if terms.is_empty() || chunks.is_empty() {
        return Vec::new();
    }
    let mut lens = Vec::with_capacity(chunks.len());
    let mut tfs: Vec<HashMap<&str, f32>> = Vec::with_capacity(chunks.len());
    let mut df: HashMap<&str, f32> = HashMap::new();
    for c in chunks {
        let toks = tokenize(&c.text);
        lens.push(toks.len() as f32);
        let mut tf: HashMap<&str, f32> = HashMap::new();
        for t in &toks {
            if let Some(term) = terms.get(t) {
                *tf.entry(term.as_str()).or_insert(0.0) += 1.0;
            }
        }
        for term in tf.keys() {
            *df.entry(term).or_insert(0.0) += 1.0;
        }
        tfs.push(tf);
    }
    let n = chunks.len() as f32;
    let avgdl = (lens.iter().sum::<f32>() / n).max(1.0);
    let (k1, b) = (1.2_f32, 0.75_f32);
    let mut scored: Vec<(usize, f32)> = tfs
        .iter()
        .enumerate()
        .map(|(i, tf)| {
            let s = tf
                .iter()
                .map(|(term, &f)| {
                    let d = df[term];
                    let idf = ((n - d + 0.5) / (d + 0.5) + 1.0).ln();
                    idf * f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * lens[i] / avgdl))
                })
                .sum::<f32>();
            (i, s)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    scored
}

/// desc 是写到这里时的描述（见 BookData::description_at），state 是写到这里时的状态。
pub fn render_entry(e: &Entry, desc: &str, state: &str) -> String {
    let mut s = format!("· {}「{}」", e.kind_label(), e.name.trim());
    if !e.role.trim().is_empty() {
        s += &format!("（{}）", e.role.trim());
    }
    if !e.aliases.trim().is_empty() {
        s += &format!("，又称：{}", e.aliases.trim());
    }
    if !desc.trim().is_empty() {
        s += &format!("\n  设定：{}", clip(desc, 300));
    }
    if !e.immutable.trim().is_empty() {
        s += &format!("\n  不可改变：{}", clip(&e.immutable, 150));
    }
    if !state.trim().is_empty() {
        s += &format!("\n  当前状态：{}", clip(state, 260));
    }
    s
}

/// 找出在文本里出场的设定条目（常驻条目总是带上），按出现次数排序。
pub fn match_entries<'a>(entries: impl IntoIterator<Item = &'a Entry>, scan: &str) -> Vec<&'a Entry> {
    let mut hits: Vec<(&Entry, usize)> = entries
        .into_iter()
        .filter_map(|e| {
            let text = e.scan_text(scan);
            let n: usize = e
                .keywords()
                .iter()
                .filter(|k| k.chars().count() >= 2 || **k == e.name.trim())
                .map(|k| text.matches(k.as_str()).count())
                .sum();
            (e.always_include || n > 0).then_some((e, n))
        })
        .collect();
    hits.sort_by(|a, b| b.0.always_include.cmp(&a.0.always_include).then(b.1.cmp(&a.1)));
    hits.into_iter().map(|(e, _)| e).collect()
}

fn take_lines(lines: Vec<String>, budget: usize) -> Vec<String> {
    let mut used = 0;
    let mut out = Vec::new();
    for line in lines {
        let len = line.chars().count();
        if used + len > budget && !out.is_empty() {
            break;
        }
        used += len;
        out.push(line);
    }
    out
}

fn take_within(lines: Vec<String>, budget: usize) -> String {
    take_lines(lines, budget).join("\n")
}

pub fn compose(data: &BookData, o: &ComposeOpts) -> Vec<Section> {
    let b = o.budget.max(3000);
    let book = &data.book;
    let mut out = Vec::new();

    let mut info = format!("书名：{}", book.title);
    if !book.genre.trim().is_empty() {
        info += &format!("\n题材：{}", book.genre.trim());
    }
    match book.platform.as_str() {
        "fanqie" => info += "\n平台：番茄小说（免费阅读，节奏要快，爽点密集）",
        "qidian" => info += "\n平台：起点中文网（付费阅读，重视设定和剧情逻辑）",
        _ => {}
    }
    if !book.logline.trim().is_empty() {
        info += &format!("\n一句话梗概：{}", book.logline.trim());
    }
    if !book.synopsis.trim().is_empty() {
        info += &format!("\n简介：{}", clip(&book.synopsis, 300));
    }
    out.push(Section { title: "作品信息".into(), body: info });

    if o.include_world {
        let world = data.world_at(o.current);
        if !world.trim().is_empty() {
            out.push(Section { title: "世界观".into(), body: clip(&world, b * 12 / 100) });
        }
        if let Some(cur) = o.current {
            let known = reader_known(data, cur);
            if !known.is_empty() {
                out.push(Section { title: "世界（读者已知）".into(), body: take_within(known, b * 6 / 100) });
            }
        }
    }

    let cur_vol = o.current.and_then(|c| c.volume_id).and_then(|vid| data.volumes.iter().find(|v| v.id == vid));
    if o.include_outline {
        let outline = data.outline_at(o.current);
        if !outline.trim().is_empty() {
            out.push(Section { title: "总纲".into(), body: clip(&outline, b * 10 / 100) });
        }
        if let Some(v) = cur_vol {
            let vo = data.volume_outline_at(v, o.current);
            if !vo.trim().is_empty() {
                out.push(Section { title: format!("本卷卷纲（{}）", v.title), body: clip(&vo, b * 8 / 100) });
            }
        }
    }

    let prev = data.before(o.current);

    // 前情提要：已完结的卷用卷摘要，其余用最近的章节摘要
    let summary_budget = b * 18 / 100;
    let prev_ids: HashSet<i64> = prev.iter().map(|c| c.id).collect();
    let covered: HashSet<i64> = data
        .volumes
        .iter()
        .filter(|v| Some(v.id) != cur_vol.map(|c| c.id) && !v.summary.trim().is_empty())
        .filter(|v| {
            let mut chs = data.chapters.iter().filter(|c| c.volume_id == Some(v.id)).peekable();
            chs.peek().is_some() && chs.all(|c| prev_ids.contains(&c.id))
        })
        .map(|v| v.id)
        .collect();
    let recent: Vec<String> = prev
        .iter()
        .rev()
        .filter(|c| !c.volume_id.map(|v| covered.contains(&v)).unwrap_or(false))
        .filter(|c| !c.summary.trim().is_empty())
        .map(|c| format!("{}：{}", data.label(c), c.summary.trim()))
        .collect();
    let mut kept = take_lines(recent, summary_budget * 7 / 10);
    kept.reverse();
    let recent_text = kept.join("\n");
    let vol_lines: Vec<String> = data
        .volumes
        .iter()
        .filter(|v| covered.contains(&v.id))
        .map(|v| format!("【{}】{}", v.title, v.summary.trim()))
        .collect();
    let vol_text = take_within(vol_lines, summary_budget.saturating_sub(recent_text.chars().count()));
    let recap = [vol_text, recent_text].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
    if !recap.is_empty() {
        out.push(Section { title: "前情提要".into(), body: recap });
    }

    // 出场设定
    let mut scan = String::new();
    if let Some(c) = o.current {
        scan.push_str(&c.title);
        scan.push_str(&c.outline);
    }
    scan.push_str(&tail_chars(o.focus_text, 6000));
    scan.push_str(o.instruction);
    if let Some(p) = prev.last() {
        scan.push_str(&tail_chars(&p.content, 1500));
    }
    let matched = match_entries(data.entries_at(o.current), &scan);
    if !matched.is_empty() {
        let lines = matched.iter().map(|e| render_entry(e, &data.description_at(e, o.current), &data.state_at(e, o.current))).collect();
        out.push(Section { title: "相关设定（必须遵守）".into(), body: take_within(lines, b * 22 / 100) });
        let ids: HashSet<i64> = matched.iter().map(|e| e.id).collect();
        let rels = data.relation_lines(&ids, o.current);
        if !rels.is_empty() {
            out.push(Section { title: "人物关系".into(), body: take_within(rels, b * 6 / 100) });
        }
    }

    // 未回收伏笔
    let cur_num = o.current.and_then(|c| data.number(c));
    let open: Vec<String> = data
        .open_threads(o.current)
        .into_iter()
        .map(|t| {
            let planted = t.planted_chapter_id.and_then(|id| data.chapter(id));
            let mut line = format!("· {}", t.title.trim());
            let detail = data.thread_detail_at(t, o.current);
            if !detail.is_empty() {
                line += &format!("：{}", clip(&detail, 120));
            }
            if let Some(p) = planted {
                line += &format!("（埋于{}", data.label(p));
                if let (Some(now), Some(then)) = (cur_num, data.number(p)) {
                    if now - then >= 30 {
                        line += &format!("，已过 {} 章，可以考虑回收", now - then);
                    }
                }
                line += "）";
            }
            line
        })
        .collect();
    if !open.is_empty() {
        out.push(Section { title: "未回收的伏笔".into(), body: take_within(open, b * 8 / 100) });
    }

    // 相关前文片段：排除上一章（下面单独给结尾）
    if prev.len() > 1 {
        let pool = &prev[..prev.len() - 1];
        let mut query = String::new();
        if let Some(c) = o.current {
            query.push_str(&c.outline);
        }
        query.push_str(o.instruction);
        for e in matched.iter().filter(|e| !e.always_include).take(5) {
            query.push_str(&e.name);
        }
        query.push_str(&tail_chars(o.focus_text, 300));
        let chunks = make_chunks(data, pool, 500);
        let hits = bm25(&chunks, &query, 4);
        if !hits.is_empty() {
            let lines = hits.iter().map(|(i, _)| format!("（{}）{}", chunks[*i].label, chunks[*i].text)).collect();
            out.push(Section { title: "相关前文片段".into(), body: take_within(lines, b * 12 / 100) });
        }
    }

    if let Some(p) = prev.last() {
        if o.focus_text.chars().count() < 2000 && !p.content.trim().is_empty() {
            out.push(Section { title: format!("上一章结尾（{}）", data.label(p)), body: tail_chars(p.content.trim(), 1200) });
        }
    }

    // 揭示计划：写某一章时给投放清单和禁区，规划时给整份计划；没有计划的书什么都不加
    match o.current {
        Some(cur) => {
            if o.include_world {
                let list = delivery(data, cur);
                if !list.is_empty() {
                    out.push(Section { title: "本章投放清单".into(), body: clip(&list, b * 6 / 100) });
                }
            }
            let no_go = forbidden(data, cur);
            if !no_go.is_empty() {
                let body = format!("下面这些还没到揭开的时候：正文里不要出现这些词，也不要用旁白暗示、替人物说破。\n{}", take_within(no_go, b * 5 / 100));
                out.push(Section { title: "禁区".into(), body });
            }
        }
        None if o.include_outline => {
            let plan = plan_lines(data);
            if !plan.is_empty() {
                out.push(Section { title: "揭示计划（作者层：按计划安排揭示，还没到时间的不要提前写破）".into(), body: take_within(plan, b * 10 / 100) });
            }
        }
        None => {}
    }
    out
}

pub fn render(sections: &[Section]) -> String {
    sections
        .iter()
        .filter(|s| !s.body.trim().is_empty())
        .map(|s| format!("【{}】\n{}", s.title, s.body.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(id: i64, title: &str, content: &str, summary: &str) -> Chapter {
        Chapter { id, book_id: 1, sort: id, title: title.into(), content: content.into(), summary: summary.into(), ..Default::default() }
    }

    fn sample() -> BookData {
        let book = Book { id: 1, title: "剑来测试".into(), worldview: "灵气复苏的世界".into(), ..Default::default() };
        let chapters = vec![
            chapter(1, "山村", "林凡在山村长大。\n他捡到一块黑色玉佩，玉佩上刻着古老的符文。", "林凡捡到黑色玉佩。"),
            chapter(2, "进城", "林凡进了青云城。\n城里很热闹。", "林凡进城。"),
            chapter(3, "拍卖会", "拍卖会上有人认出了玉佩。", "拍卖会。"),
            chapter(4, "新章", "", ""),
        ];
        let entries = vec![
            Entry { id: 1, name: "林凡".into(), kind: "character".into(), state: "炼气三层".into(), ..Default::default() },
            Entry { id: 2, name: "苏雨".into(), kind: "character".into(), ..Default::default() },
            Entry { id: 3, name: "灵气".into(), kind: "concept".into(), always_include: true, ..Default::default() },
        ];
        let threads = vec![Thread { id: 1, title: "黑色玉佩的来历".into(), status: "open".into(), planted_chapter_id: Some(1), ..Default::default() }];
        BookData::new(book, vec![], chapters, entries, threads, vec![])
    }

    #[test]
    fn excluded_phrases_do_not_count_as_mentions() {
        let ping = Entry { id: 1, name: "平安".into(), kind: "character".into(), exclude: "平平安安、平安无事".into(), ..Default::default() };
        let other = Entry { id: 2, name: "陈渊".into(), kind: "character".into(), ..Default::default() };
        let names = |text: &str| match_entries([&ping, &other], text).into_iter().map(|e| e.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names("陈渊盼着平平安安回家，一路平安无事。"), vec!["陈渊"]);
        assert_eq!(names("平安背着陈渊往外跑，求个平平安安。"), vec!["平安", "陈渊"]);
        assert_eq!(ping.scan_text("平平安安").len(), "平平安安".len(), "盖掉后字节长度不变");
    }

    #[test]
    fn relation_lines_mark_ended() {
        let mut data = sample();
        data.relations = vec![
            Relation { id: 1, a_id: 1, b_id: 2, kind: "师姐弟".into(), status: "active".into(), ..Default::default() },
            Relation { id: 2, a_id: 1, b_id: 3, kind: "盟友".into(), status: "ended".into(), ..Default::default() },
        ];
        let only_lin: HashSet<i64> = [1].into_iter().collect();
        assert_eq!(data.relation_lines(&only_lin, None), vec!["· 林凡 与 苏雨：师姐弟".to_string()]);
        let both: HashSet<i64> = [1, 3].into_iter().collect();
        assert!(data.relation_lines(&both, None).iter().any(|l| l.contains("这段关系已经结束")));
    }

    #[test]
    fn compose_keeps_unopened_material_out() {
        let mut data = sample();
        data.volumes = vec![Volume { id: 1, title: "第一卷".into(), outline: "### 前十章\n进城拍卖\n### 本卷收尾〔仅规划〕\n林凡登顶青云城".into(), sort: 1, ..Default::default() }];
        for c in &mut data.chapters {
            c.volume_id = Some(1);
        }
        data.book.worldview = "灵气复苏的世界\n### 青云城\n城里很热闹\n### 天道真相\n天道是伪神\n### 上界〔第2卷起〕\n九霄天轨\n### 作者备注\n林凡是转世仙帝".into();
        data.book.outline = "### 核心主线\n林凡最终成神\n### 第一卷\n进城\n### 第二卷\n飞升灵界".into();
        data.entries[1].description = "青云城的医女".into();
        data.entries[1].secret = "魔教圣女".into();
        data.entries[1].always_include = true;
        data.entries.push(Entry { id: 4, name: "魔尊".into(), kind: "character".into(), visibility: "第2卷起".into(), always_include: true, ..Default::default() });
        data.entries.push(Entry { id: 5, name: "玉佩器灵".into(), kind: "character".into(), visibility: "对AI隐藏".into(), always_include: true, ..Default::default() });
        data.relations = vec![Relation { id: 1, a_id: 1, b_id: 2, kind: "道侣".into(), status: "active".into(), since_chapter_id: Some(3), ..Default::default() }];
        data.threads.push(Thread { id: 2, title: "拍卖会上的神秘人".into(), status: "open".into(), planted_chapter_id: Some(3), ..Default::default() });
        data.threads.push(Thread {
            id: 3,
            title: "城门守卫的暗号".into(),
            detail: "守卫换岗时对暗号\n【推进】苏雨认出暗号出自魔教".into(),
            status: "resolved".into(),
            planted_chapter_id: Some(1),
            resolved_chapter_id: Some(3),
            last_chapter_id: Some(3),
            ..Default::default()
        });

        let ch2 = Chapter { outline: "林凡在青云城遇见苏雨".into(), ..data.chapter(2).cloned().unwrap() };
        let opts = |cur| ComposeOpts { current: cur, focus_text: "", instruction: "", budget: 12000, include_world: true, include_outline: true };
        let text = render(&compose(&data, &opts(Some(&ch2))));
        for want in ["城里很热闹", "【总纲】\n### 第一卷\n进城", "进城拍卖", "青云城的医女", "城门守卫的暗号：守卫换岗时对暗号", "黑色玉佩的来历"] {
            assert!(text.contains(want), "缺少「{want}」：\n{text}");
        }
        for leak in ["伪神", "九霄天轨", "转世仙帝", "最终成神", "飞升灵界", "登顶青云城", "魔教圣女", "魔尊", "玉佩器灵", "道侣", "神秘人", "〔", "出自魔教"] {
            assert!(!text.contains(leak), "第2章不该看到「{leak}」：\n{text}");
        }

        let planning = render(&compose(&data, &opts(None)));
        for want in ["伪神", "九霄天轨", "最终成神", "飞升灵界", "魔尊"] {
            assert!(planning.contains(want), "规划要看到作者层「{want}」：\n{planning}");
        }
        for never in ["转世仙帝", "魔教圣女", "玉佩器灵"] {
            assert!(!planning.contains(never), "对 AI 隐藏的和作者底牌任何任务都不给：「{never}」");
        }
    }

    #[test]
    fn progressions_unlock_by_chapter_and_volume() {
        let mut data = sample();
        data.volumes = vec![Volume { id: 1, title: "第一卷".into(), sort: 1, ..Default::default() }, Volume { id: 2, title: "第二卷".into(), sort: 2, ..Default::default() }];
        for c in &mut data.chapters {
            c.volume_id = Some(if c.id <= 3 { 1 } else { 2 });
        }
        data.entries[1].description = "青云城的医女".into();
        let p = |id, gate: &str, mode: &str, text: &str| Progression { id, entry_id: 2, gate: gate.into(), mode: mode.into(), text: text.into(), ..Default::default() };
        data.progressions = vec![
            p(1, "第2卷起", "replace", "魔教圣女，潜伏在青云城"),
            p(2, "第3章起", "add", "她认得林凡的玉佩"),
            p(3, "以后再说", "add", "写法看不懂的不给"),
        ];
        let su = data.entries[1].clone();
        let at = |id| data.description_at(&su, data.chapter(id));
        assert_eq!(at(1), "青云城的医女");
        assert_eq!(at(3), "青云城的医女\n她认得林凡的玉佩");
        assert_eq!(at(4), "魔教圣女，潜伏在青云城", "第二卷第一章是第4章，替换排在第3章的补充之后");
        assert_eq!(data.description_at(&su, None), "魔教圣女，潜伏在青云城", "规划看作者层，进展全部生效");
        let text = render(&compose(&data, &ComposeOpts { current: data.chapter(1), focus_text: "苏雨", instruction: "", budget: 12000, include_world: true, include_outline: true }));
        assert!(text.contains("青云城的医女") && !text.contains("玉佩器灵") && !text.contains("魔教圣女"), "{text}");
    }

    fn reveal(id: i64, title: &str, truth: &str, terms: &str) -> Reveal {
        Reveal { id, book_id: 1, title: title.into(), truth: truth.into(), terms: terms.into(), status: "active".into(), ..Default::default() }
    }

    #[test]
    fn compose_follows_reveal_plan_without_leaking_truth() {
        let mut data = sample();
        data.reveals = vec![
            Reveal { gap: "好奇".into(), seed_at: Some(1), clue_at: Some(3), clue_note: "城墙上的符文和玉佩上的一样".into(), reveal_at: Some(40), ..reveal(1, "玉佩的来历", "玉佩是上古仙帝的残魂所化", "仙帝残魂、上古仙帝") },
            Reveal { gap: "惊奇".into(), seed_at: Some(2), seed_note: "苏雨的药箱里有魔教的香".into(), reveal_at: Some(3), payoff: "林凡不再信任苏雨".into(), ..reveal(2, "苏雨的身份", "苏雨是魔教圣女", "魔教圣女") },
            Reveal { reveal_at: Some(2), ..reveal(3, "拍卖会的东家", "拍卖会是林家的产业", "林家产业") },
            Reveal { status: "dropped".into(), ..reveal(4, "作废的秘密", "不该出现的真相", "作废词") },
        ];
        data.reveal_events = vec![
            RevealEvent { id: 1, reveal_id: 1, chapter_id: 1, step: "seed".into(), ..Default::default() },
            RevealEvent { id: 2, reveal_id: 3, chapter_id: 2, step: "reveal".into(), ..Default::default() },
        ];
        let opts = |cur| ComposeOpts { current: cur, focus_text: "", instruction: "", budget: 12000, include_world: true, include_outline: true };
        let section = |sections: &[Section], title: &str| sections.iter().find(|s| s.title == title).map(|s| s.body.clone()).unwrap_or_default();

        let ch2 = compose(&data, &opts(data.chapter(2)));
        let list2 = section(&ch2, "本章投放清单");
        assert!(list2.contains("本章揭开（可以写明）：\n· 「拍卖会的东家」：拍卖会是林家的产业"), "{list2}");
        assert!(list2.contains("本章要埋的种子（顺嘴带一笔，不解释）：\n· 「苏雨的身份」：苏雨的药箱里有魔教的香"), "{list2}");
        assert!(list2.contains("近期会用") && list2.contains("「玉佩的来历」"), "已埋种子、第3章要给线索的秘密可以先挂钩子：{list2}");
        let no_go2 = section(&ch2, "禁区");
        assert!(no_go2.contains("「玉佩的来历」：不写「仙帝残魂」「上古仙帝」") && no_go2.contains("「苏雨的身份」：不写「魔教圣女」"), "{no_go2}");
        assert!(!no_go2.contains("拍卖会的东家"), "本章揭开的不放进禁区");
        let text2 = render(&ch2);
        for truth in ["上古仙帝的残魂所化", "苏雨是魔教圣女", "作废"] {
            assert!(!text2.contains(truth), "第2章不该看到「{truth}」：\n{text2}");
        }
        assert!(section(&ch2, "世界（读者已知）").is_empty(), "第2章之前还没有揭开过什么");

        let ch3 = compose(&data, &opts(data.chapter(3)));
        assert_eq!(section(&ch3, "世界（读者已知）"), "· 拍卖会的东家（第2章揭开）：拍卖会是林家的产业");
        let list3 = section(&ch3, "本章投放清单");
        assert!(list3.contains("「苏雨的身份」：苏雨是魔教圣女（揭开后：林凡不再信任苏雨）"), "{list3}");
        assert!(list3.contains("本章要给的线索（让人物撞见一角，不点破）：\n· 「玉佩的来历」：城墙上的符文和玉佩上的一样"), "{list3}");
        assert!(!render(&ch3).contains("上古仙帝的残魂所化"));
        assert!(!section(&ch3, "禁区").contains("苏雨的身份"));

        let small = compose(&data, &ComposeOpts { include_world: false, include_outline: false, ..opts(data.chapter(2)) });
        assert!(section(&small, "本章投放清单").is_empty() && !section(&small, "禁区").is_empty(), "改写选段不给投放清单，但禁区照样给");

        let planning = render(&compose(&data, &opts(None)));
        assert!(planning.contains("「玉佩的来历」（好奇）真相：玉佩是上古仙帝的残魂所化"), "{planning}");
        assert!(planning.contains("计划：第1章埋种子、第3章给线索、第40章揭开；进度：第1章埋种子；泄露词：仙帝残魂、上古仙帝"), "{planning}");
        assert!(planning.contains("「拍卖会的东家」") && planning.contains("进度：第2章揭开") && !planning.contains("作废"), "{planning}");
        assert!(!planning.contains("【禁区】") && !planning.contains("【本章投放清单】"));
    }

    #[test]
    fn state_without_snapshots_not_leaked_when_revising() {
        let data = sample();
        let lin = data.entries[0].clone();
        assert_eq!(data.state_at(&lin, data.chapter(1)), "", "回改第1章时后面已经写了，不能拿最新状态当那时的状态");
        assert_eq!(data.state_at(&lin, data.chapter(4)), "炼气三层", "写新章节时最新状态就是当前状态");
        assert_eq!(data.state_at(&lin, None), "炼气三层");
    }

    #[test]
    fn state_follows_timeline() {
        let mut data = sample();
        let snap = |id, chapter: Option<i64>, phase: &str, power: &str| EntryState {
            id,
            entry_id: 1,
            chapter_id: chapter,
            phase: phase.into(),
            fields: [("power".to_string(), power.to_string())].into_iter().collect(),
            ..Default::default()
        };
        data.states = vec![snap(1, None, "start", "炼气一层"), snap(2, Some(2), "end", "炼气三层"), snap(3, Some(3), "start", "炼气五层")];
        let lin = data.entries[0].clone();
        let chs: Vec<Chapter> = (1..=4).map(|id| data.chapter(id).cloned().unwrap()).collect();
        assert_eq!(data.state_at(&lin, Some(&chs[0])), "实力：炼气一层");
        assert_eq!(data.state_at(&lin, Some(&chs[1])), "实力：炼气一层", "第2章的章末状态不能提前用在第2章");
        assert_eq!(data.state_at(&lin, Some(&chs[2])), "实力：炼气五层", "本章开始生效的手动修改要用上");
        assert_eq!(data.state_at(&lin, Some(&chs[3])), "实力：炼气五层");
        assert_eq!(data.state_at(&lin, None), "实力：炼气五层");
        data.states = vec![snap(4, Some(3), "end", "筑基")];
        assert_eq!(data.state_at(&lin, Some(&chs[1])), "", "只有后文的快照时不泄露");
    }

    #[test]
    fn bm25_ranks_relevant_chunk_first() {
        let data = sample();
        let chunks = make_chunks(&data, &data.chapters[..3], 500);
        let hits = bm25(&chunks, "玉佩上的符文", 2);
        assert_eq!(chunks[hits[0].0].label, "第1章 山村");
    }

    #[test]
    fn compose_includes_memory_sections() {
        let data = sample();
        let cur = data.chapter(4).cloned().unwrap();
        let cur = Chapter { outline: "林凡在拍卖会上用玉佩换取丹药".into(), ..cur };
        let sections = compose(&data, &ComposeOpts { current: Some(&cur), focus_text: "", instruction: "", budget: 12000, include_world: true, include_outline: true });
        let text = render(&sections);
        assert!(text.contains("【前情提要】"));
        assert!(text.contains("第1章 山村：林凡捡到黑色玉佩。"));
        assert!(text.contains("人物「林凡」"));
        assert!(text.contains("炼气三层"));
        assert!(text.contains("设定「灵气」"), "常驻条目总是带上");
        assert!(!text.contains("苏雨"), "未出场人物不注入");
        assert!(text.contains("黑色玉佩的来历"));
        assert!(text.contains("【相关前文片段】"));
        assert!(text.contains("上一章结尾（第3章 拍卖会）"));
        assert!(!text.contains("【禁区】") && !text.contains("【本章投放清单】") && !text.contains("读者已知"), "没有揭示计划的书不加这些段落");
        let recap = sections.iter().find(|s| s.title == "前情提要").unwrap();
        let first = recap.body.find("第1章").unwrap();
        let third = recap.body.find("第3章").unwrap();
        assert!(first < third, "前情按时间顺序");
    }
}
