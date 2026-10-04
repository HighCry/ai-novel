//! 长篇记忆：把设定库、分层摘要、伏笔、相关前文片段组装成有字数预算的上下文。

use crate::models::*;
use crate::text::*;
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
    numbers: HashMap<i64, Option<i64>>,
}

impl BookData {
    pub fn new(book: Book, volumes: Vec<Volume>, chapters: Vec<Chapter>, entries: Vec<Entry>, threads: Vec<Thread>, states: Vec<EntryState>) -> Self {
        let nums = number_chapters(chapters.iter().map(|c| c.title.as_str()));
        let numbers = chapters.iter().map(|c| c.id).zip(nums).collect();
        Self { book, volumes, chapters, entries, threads, states, relations: Vec::new(), numbers }
    }

    pub fn entry_name(&self, id: i64) -> Option<&str> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.name.as_str())
    }

    /// 和给定条目有关的关系，一行一条；已结束的关系标注出来，避免写成还很亲密。
    pub fn relation_lines(&self, ids: &HashSet<i64>) -> Vec<String> {
        self.relations
            .iter()
            .filter(|r| ids.contains(&r.a_id) || ids.contains(&r.b_id))
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
            return e.state_text();
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

pub fn render_entry(e: &Entry, state: &str) -> String {
    let mut s = format!("· {}「{}」", e.kind_label(), e.name.trim());
    if !e.role.trim().is_empty() {
        s += &format!("（{}）", e.role.trim());
    }
    if !e.aliases.trim().is_empty() {
        s += &format!("，又称：{}", e.aliases.trim());
    }
    if !e.description.trim().is_empty() {
        s += &format!("\n  设定：{}", clip(&e.description, 300));
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
pub fn match_entries<'a>(entries: &'a [Entry], scan: &str) -> Vec<&'a Entry> {
    let mut hits: Vec<(&Entry, usize)> = entries
        .iter()
        .filter_map(|e| {
            let n: usize = e
                .keywords()
                .iter()
                .filter(|k| k.chars().count() >= 2 || **k == e.name.trim())
                .map(|k| scan.matches(k.as_str()).count())
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

    if o.include_world && !book.worldview.trim().is_empty() {
        out.push(Section { title: "世界观".into(), body: clip(&book.worldview, b * 12 / 100) });
    }

    let cur_vol = o.current.and_then(|c| c.volume_id).and_then(|vid| data.volumes.iter().find(|v| v.id == vid));
    if o.include_outline {
        if !book.outline.trim().is_empty() {
            out.push(Section { title: "总纲".into(), body: clip(&book.outline, b * 10 / 100) });
        }
        if let Some(v) = cur_vol.filter(|v| !v.outline.trim().is_empty()) {
            out.push(Section { title: format!("本卷卷纲（{}）", v.title), body: clip(&v.outline, b * 8 / 100) });
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
    let matched = match_entries(&data.entries, &scan);
    if !matched.is_empty() {
        let lines = matched.iter().map(|e| render_entry(e, &data.state_at(e, o.current))).collect();
        out.push(Section { title: "相关设定（必须遵守）".into(), body: take_within(lines, b * 22 / 100) });
        let ids: HashSet<i64> = matched.iter().map(|e| e.id).collect();
        let rels = data.relation_lines(&ids);
        if !rels.is_empty() {
            out.push(Section { title: "人物关系".into(), body: take_within(rels, b * 6 / 100) });
        }
    }

    // 未回收伏笔
    let cur_num = o.current.and_then(|c| data.number(c));
    let open: Vec<String> = data
        .threads
        .iter()
        .filter(|t| t.status == "open")
        .map(|t| {
            let planted = t.planted_chapter_id.and_then(|id| data.chapter(id));
            let mut line = format!("· {}", t.title.trim());
            if !t.detail.trim().is_empty() {
                line += &format!("：{}", clip(&t.detail, 120));
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
    fn relation_lines_mark_ended() {
        let mut data = sample();
        data.relations = vec![
            Relation { id: 1, a_id: 1, b_id: 2, kind: "师姐弟".into(), status: "active".into(), ..Default::default() },
            Relation { id: 2, a_id: 1, b_id: 3, kind: "盟友".into(), status: "ended".into(), ..Default::default() },
        ];
        let only_lin: HashSet<i64> = [1].into_iter().collect();
        assert_eq!(data.relation_lines(&only_lin), vec!["· 林凡 与 苏雨：师姐弟".to_string()]);
        let both: HashSet<i64> = [1, 3].into_iter().collect();
        assert!(data.relation_lines(&both).iter().any(|l| l.contains("这段关系已经结束")));
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
        let recap = sections.iter().find(|s| s.title == "前情提要").unwrap();
        let first = recap.body.find("第1章").unwrap();
        let third = recap.body.find("第3章").unwrap();
        assert!(first < third, "前情按时间顺序");
    }
}
