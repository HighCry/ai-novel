//! 叙事逻辑的规则检查：不调用模型，找出设定超前、新名词过密、大段纯设定、境界走向和物品去向的问题，每条都附原文；
//! 有揭示计划时再查泄露词提前出现、信息进度断档、揭开前没唤醒和揭示逾期。
//! 新名词密度、纯设定段落、信息进度和记忆温度改编自 webnovel-handbook（MIT）第 26、28 章的信息投放规则；
//! 境界、物品按章节入账后比对前后状态，思路参考 FactTrack（NAACL 2025）。结果并进连续性体检，由作者判断。

use crate::continuity::{finding, Finding};
use crate::memory::BookData;
use crate::models::*;
use crate::text::{clip, paragraphs, parse_cn_number};
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

pub fn check(d: &BookData) -> Vec<Finding> {
    let mut out = Vec::new();
    leaks(d, &mut out);
    reveals(d, &mut out);
    title_leaks(d, &mut out);
    opening(d, &mut out);
    payoff_milestone(d, &mut out);
    new_terms(d, &mut out);
    exposition(d, &mut out);
    realms(d, &mut out);
    items(d, &mut out);
    out
}

fn written(d: &BookData) -> Vec<&Chapter> {
    d.chapters.iter().filter(|c| !c.content.trim().is_empty()).collect()
}

/// 物品名常带品级前缀或「（残）」后缀，正文里多半写短名，比如「极品毒灵髓」写成「毒灵髓」。
fn short_name(e: &Entry) -> Option<String> {
    if e.kind != "item" {
        return None;
    }
    let full = e.name.trim();
    let mut n = full;
    if let Some(rest) = ["极品", "上品", "中品", "下品", "绝品", "仙品", "残破的", "残缺的"].iter().find_map(|p| n.strip_prefix(p)) {
        n = rest;
    }
    if let Some(rest) = ["（残）", "(残)"].iter().find_map(|s| n.strip_suffix(s)) {
        n = rest;
    }
    (n != full && n.chars().count() >= 3).then(|| n.to_string())
}

/// 匹配用的关键词：名称、两个字以上的别名，物品再加上短名。
fn keys(e: &Entry) -> Vec<String> {
    let mut ks: Vec<String> = e.keywords().into_iter().filter(|k| k.chars().count() >= 2 || *k == e.name.trim()).collect();
    if let Some(s) = short_name(e).filter(|s| !ks.contains(s)) {
        ks.push(s);
    }
    ks
}

fn first_hit(keys: &[String], text: &str) -> Option<(usize, String)> {
    keys.iter().filter_map(|k| text.find(k.as_str()).map(|i| (i, k.clone()))).min_by_key(|(i, _)| *i)
}

const SENTENCE_END: [char; 6] = ['。', '！', '？', '!', '?', '\n'];

/// 命中位置所在的整句，不截断。
pub(crate) fn sentence_around(text: &str, pos: usize) -> &str {
    let start = text[..pos].rfind(SENTENCE_END).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    let end = text[pos..].find(SENTENCE_END).map_or(text.len(), |i| pos + i + text[pos + i..].chars().next().map_or(1, char::len_utf8));
    text[start..end].trim()
}

/// 命中位置所在的句子，过长截断。
pub(crate) fn sentence_at(text: &str, pos: usize) -> String {
    clip(sentence_around(text, pos), 80)
}

fn sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(SENTENCE_END).map(str::trim).filter(|s| !s.is_empty())
}

// ---------- 设定超前 ----------

fn leaks(d: &BookData, out: &mut Vec<Finding>) {
    for c in written(d) {
        let at = Some(d.point(c));
        for e in d.entries.iter().filter(|e| !e.gate().open_at(at)) {
            if let Some((pos, kw)) = first_hit(&keys(e), &e.scan_text(&c.content)) {
                let mut f = finding("critical", "设定超前", format!("{}出现了「{kw}」，这条设定是「{}」，这一章还不该写出来", d.label(c), e.gate().label()));
                f.chapter_id = Some(c.id);
                f.entry_id = Some(e.id);
                f.quote = sentence_at(&c.content, pos);
                out.push(f);
            }
        }
    }
}

// ---------- 揭示计划：泄露词、信息进度、记忆温度、揭示逾期 ----------

/// 开着的秘密隔这么多章没有任何种子、线索就提醒（webnovel-handbook：每 3～8 章给一次信息进度）
const QUIET_REVEAL_CHAPTERS: i64 = 8;
/// 揭开前最近一次线索隔了这么多章，读者多半已经忘了（webnovel-handbook：关键线索隔十章以上再揭示，揭示前要唤醒）
const COLD_REVEAL_CHAPTERS: i64 = 10;
/// 离计划揭开还剩这么多章以内才提醒唤醒，太早提醒没用
const WARM_UP_AHEAD: i64 = 5;

/// 先把例外说法换成等长的空格再找泄露词，「天轨护道盟」就不算点破「天轨」，命中位置仍对得上原文。
fn leak_hit(text: &str, terms: &[String], exceptions: &[String]) -> Option<(usize, String)> {
    first_hit(terms, &mask_excluded(text, exceptions))
}

/// 揭示计划草稿的泄露词里，已写章节在揭开之前就写出来的：每个词和它第一次出现的章号。
/// 读者早就见过的名字当了泄露词，体检会误报，写正文时还会被列进禁区，所以采纳前要让作者看到。
pub fn early_terms(d: &BookData, terms: &[String], exceptions: &[String], reveal_at: Option<i64>) -> Vec<(String, i64)> {
    let stop = reveal_at.and_then(|n| d.chapters.iter().position(|c| d.number(c) == Some(n))).unwrap_or(d.chapters.len());
    let mut out: Vec<(String, i64)> = Vec::new();
    for c in &d.chapters[..stop] {
        let Some(n) = d.number(c).filter(|_| !c.content.trim().is_empty()) else { continue };
        for t in terms {
            if !out.iter().any(|(k, _)| k == t) && leak_hit(&c.content, std::slice::from_ref(t), exceptions).is_some() {
                out.push((t.clone(), n));
            }
        }
    }
    out
}

/// 秘密的标题会原样进写正文时的投放清单和禁区，标题里带着还没开放的设定名，模型会当成能写的词。
fn title_leaks(d: &BookData, out: &mut Vec<Finding>) {
    let hidden: Vec<&Entry> = d.entries.iter().filter(|e| !matches!(e.gate(), crate::visibility::Gate::Public)).collect();
    for r in d.active_reveals() {
        let title = r.title.trim();
        let Some((e, k)) = hidden.iter().find_map(|e| keys(e).into_iter().filter(|k| k.chars().count() >= 2).find(|k| title.contains(k.as_str())).map(|k| (*e, k))) else {
            continue;
        };
        let when = if e.visibility.trim().is_empty() { "对 AI 隐藏".to_string() } else { e.visibility.trim().to_string() };
        let mut f = finding(
            "warn",
            "秘密标题带出设定",
            format!("秘密「{title}」的标题里有「{k}」（设定「{}」{when}）。写正文时标题会进投放清单和禁区，模型会当成能写的词，建议改成不带这个名字的话题", e.name.trim()),
        );
        f.reveal_id = Some(r.id);
        out.push(f);
    }
}

fn reveals(d: &BookData, out: &mut Vec<Finding>) {
    let chapters = written(d);
    let last = chapters.iter().rev().find_map(|c| d.number(c)).unwrap_or(0);
    let pos = |id: i64| d.chapters.iter().position(|c| c.id == id);
    let pos_of = |n: i64| d.chapters.iter().position(|c| d.number(c) == Some(n));
    for r in d.active_reveals() {
        let events = d.reveal_events_before(r, None);
        let revealed = events.iter().find(|e| e.step == "reveal");
        let reveal_num = revealed.and_then(|e| d.event_number(e)).or(r.reveal_at);
        // 揭开那一章及以后可以写；计划的揭开章还没写到时，已写的章节都在揭开之前
        let reveal_pos = revealed.and_then(|e| pos(e.chapter_id)).or_else(|| r.reveal_at.and_then(pos_of));
        let seed_pos = events.iter().filter(|e| e.step != "reveal").find_map(|e| pos(e.chapter_id)).or_else(|| r.seed_at.or(r.clue_at).and_then(pos_of));
        let terms = r.term_list();
        if !terms.is_empty() && reveal_num.is_some() {
            let exceptions = r.exception_list();
            let when = reveal_num.map_or_else(String::new, |n| format!("计划第{n}章才揭开"));
            let mut hinted = false;
            for c in &chapters {
                let Some(cp) = pos(c.id) else { continue };
                if reveal_pos.is_some_and(|rp| cp >= rp) {
                    break;
                }
                let Some((at, kw)) = leak_hit(&c.content, &terms, &exceptions) else { continue };
                let quote = sentence_at(&c.content, at);
                if out.iter().any(|f| f.chapter_id == Some(c.id) && f.quote == quote) {
                    continue;
                }
                let mut f = if seed_pos.is_some_and(|sp| cp >= sp) {
                    // 埋过种子之后、揭开之前出现泄露词，可能是有意的线索，只提醒第一处
                    if hinted {
                        continue;
                    }
                    hinted = true;
                    finding("warn", "提前点破", format!("{}出现了「{kw}」，秘密「{}」{when}：这里是线索，还是已经说破了？", d.label(c), r.title.trim()))
                } else {
                    finding("critical", "设定超前", format!("{}出现了「{kw}」，秘密「{}」{when}，这一章之前还没埋过种子", d.label(c), r.title.trim()))
                };
                f.chapter_id = Some(c.id);
                f.reveal_id = Some(r.id);
                f.quote = quote;
                out.push(f);
            }
        }
        if revealed.is_some() {
            continue;
        }
        let latest = events.iter().rev().find_map(|e| d.event_number(e));
        if let Some(l) = latest.filter(|l| last - l >= QUIET_REVEAL_CHAPTERS) {
            let mut f = finding("info", "信息进度", format!("秘密「{}」最近一次线索在第{l}章，已经 {} 章没再提，读者可能忘了它", r.title.trim(), last - l));
            f.reveal_id = Some(r.id);
            out.push(f);
        }
        if let Some(rn) = r.reveal_at {
            let mut f = if rn < last {
                finding("warn", "揭示逾期", format!("秘密「{}」计划第{rn}章揭开，现在写到第{last}章还没揭开：按计划揭开，或者把计划往后挪", r.title.trim()))
            } else if rn > last && rn - last <= WARM_UP_AHEAD && latest.map_or(true, |l| rn - l >= COLD_REVEAL_CHAPTERS) {
                let why = match latest {
                    Some(l) => format!("最近一次线索在第{l}章，隔了 {} 章", rn - l),
                    None => "前面还没埋过种子、给过线索".to_string(),
                };
                finding("info", "记忆温度", format!("秘密「{}」计划第{rn}章揭开，{why}；揭开前先让读者想起它：再给一次线索，或让人物提起", r.title.trim()))
            } else {
                continue;
            };
            f.reveal_id = Some(r.id);
            out.push(f);
        }
    }
}

// ---------- 新名词密度、纯设定段落 ----------

/// 【】里常见的标签，不算设定名词
const LABELS: [&str; 15] = ["规则", "效果", "效用", "规则效用", "代价", "位阶", "等级", "品阶", "名称", "状态", "类型", "说明", "限制", "副作用", "备注"];

fn bracket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"《([^《》\n]{1,20})》|【([^【】\n]{1,30})】").unwrap())
}

/// 正文里括起来的设定名词：书名号里的，和【】里的（「位阶：界外遗物」这种拆开、去掉标签）。
fn bracket_terms(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in bracket_re().captures_iter(text) {
        if let Some(book) = c.get(1) {
            out.insert(format!("《{}》", book.as_str().trim()));
            continue;
        }
        for part in c[2].split(['：', ':']) {
            let p = part.trim();
            if (2..=10).contains(&p.chars().count()) && !LABELS.contains(&p) {
                out.insert(p.to_string());
            }
        }
    }
    out
}

/// 算作设定名词的条目：人物以外、名字是个名词而不是一句描述的。
fn noun_entries(d: &BookData) -> Vec<&Entry> {
    d.entries.iter().filter(|e| e.kind != "character" && !e.name.contains('的') && e.name.chars().count() <= 10).collect()
}

/// 每章首次出现的设定名词上限：第 1 章三个（webnovel-handbook），之后逐步放宽。规划前 10 章时也按这个给信息预算。
pub fn new_term_limit(n: Option<i64>) -> usize {
    match n.unwrap_or(1) {
        ..=1 => 3,
        2..=3 => 5,
        _ => 7,
    }
}

fn new_terms(d: &BookData, out: &mut Vec<Finding>) {
    let entries = noun_entries(d);
    let known: HashSet<String> = d.entries.iter().flat_map(keys).collect();
    let (mut seen_entries, mut seen_terms) = (HashSet::new(), HashSet::new());
    for c in written(d) {
        let mut fresh: Vec<String> = Vec::new();
        for e in &entries {
            if seen_entries.contains(&e.id) {
                continue;
            }
            let text = e.scan_text(&c.content);
            if keys(e).iter().any(|k| text.contains(k.as_str())) {
                seen_entries.insert(e.id);
                fresh.push(e.name.trim().to_string());
            }
        }
        for t in bracket_terms(&c.content) {
            if !known.contains(&t) && !known.contains(t.trim_matches(['《', '》'])) && seen_terms.insert(t.clone()) {
                fresh.push(t);
            }
        }
        let max = new_term_limit(d.number(c));
        if fresh.len() > max {
            let mut f = finding(
                "warn",
                "新名词过多",
                format!("{}首次出现 {} 个设定名词：{}。读者一章记不住这么多，建议不超过 {max} 个，其余等剧情用到时再出", d.label(c), fresh.len(), fresh.join("、")),
            );
            f.chapter_id = Some(c.id);
            out.push(f);
        }
    }
}

/// 开篇里程碑（webnovel-handbook）：第 10 章前至少回收一次设定。拿伏笔回收当依据：
/// 前 10 章都写完了，却没有一条伏笔是在这 10 章里回收的，就提醒一次。
fn payoff_milestone(d: &BookData, out: &mut Vec<Finding>) {
    let first_ten: Vec<i64> = d.chapters.iter().filter(|c| d.number(c).is_some_and(|n| n <= 10)).map(|c| c.id).collect();
    let all_written = first_ten.len() == 10 && first_ten.iter().all(|id| d.chapter(*id).is_some_and(|c| !c.content.trim().is_empty()));
    if !all_written || d.threads.iter().any(|t| t.resolved_chapter_id.is_some_and(|id| first_ten.contains(&id))) {
        return;
    }
    out.push(finding(
        "info",
        "开篇里程碑",
        "前 10 章还没有回收过任何伏笔：前面出现过的规则、物品、禁忌，最好在第 10 章前变成一次爽点或危机的解法，读者才会相信这套设定有用".into(),
    ));
}

// ---------- 开篇 ----------

/// 番茄作者圈总结的签约硬线：主角前 300 字登场、金手指前 1000 字亮出、前三章章末都要有钩子
const HERO_WITHIN: usize = 300;
const GOLDEN_WITHIN: usize = 1000;

fn opening(d: &BookData, out: &mut Vec<Finding>) {
    let chapters = written(d);
    let Some(first) = chapters.iter().copied().find(|c| d.number(c) == Some(1)) else { return };
    let text = first.content.as_str();
    let mut flag = |message: String, c: &Chapter, at: Option<usize>| {
        let mut f = finding("warn", "开篇", message);
        f.chapter_id = Some(c.id);
        if let Some(i) = at {
            f.quote = sentence_at(&c.content, i);
        }
        out.push(f);
    };
    if let Some(hero) = d.entries.iter().find(|e| e.kind == "character" && e.role == "主角") {
        match first_hit(&keys(hero), &hero.scan_text(text)) {
            None => flag(format!("第1章没写到主角「{}」", hero.name), first, None),
            Some((i, _)) => {
                let n = text[..i].chars().count();
                if n > HERO_WITHIN {
                    flag(format!("主角「{}」到第1章第 {n} 字才出场，签约编辑一般要求前 {HERO_WITHIN} 字内登场", hero.name), first, Some(i));
                }
            }
        }
    }
    let golden = split_terms(&d.book.golden_finger);
    if !golden.is_empty() {
        match first_hit(&golden, text) {
            None => flag(format!("第1章没亮出金手指「{}」，签约编辑一般要求前 {GOLDEN_WITHIN} 字内亮出来", golden[0]), first, None),
            Some((i, k)) => {
                let n = text[..i].chars().count();
                if n > GOLDEN_WITHIN {
                    flag(format!("金手指「{k}」到第1章第 {n} 字才亮出来，签约编辑一般要求前 {GOLDEN_WITHIN} 字内"), first, Some(i));
                }
            }
        }
    }
    for c in chapters.iter().copied().filter(|c| matches!(d.number(c), Some(1..=3))) {
        if c.metrics["hook"]["type"].as_str().map(str::trim).is_some_and(|t| t.is_empty() || t == "无") {
            flag(format!("{}没有章末钩子：开篇三章每章结尾都要让读者想点下一章", d.label(c)), c, None);
        }
    }
}

fn has_dialogue(p: &str) -> bool {
    p.contains(['“', '”', '「', '」', '"'])
}

/// 连续三段以上没有对话、设定名词又密的段落，多半是在讲设定而不是在讲故事。
fn exposition(d: &BookData, out: &mut Vec<Finding>) {
    let realms = realm_names(d);
    let mut terms: Vec<String> = noun_entries(d).into_iter().flat_map(keys).collect();
    terms.extend(realms.iter().map(|r| format!("{r}境")));
    let dense = |p: &str| {
        let n = p.chars().count();
        let hits = terms.iter().filter(|t| p.contains(t.as_str())).count() + bracket_terms(p).len();
        n >= 40 && !has_dialogue(p) && hits >= 3 && hits * 40 >= n
    };
    for c in written(d) {
        let paras = paragraphs(&c.content);
        let mut run: Vec<&str> = Vec::new();
        for p in paras.iter().copied().chain(std::iter::once("")) {
            if dense(p) {
                run.push(p);
                continue;
            }
            if run.len() >= 3 {
                let mut f = finding("info", "大段纯设定", format!("{}有连续 {} 段没有对话、设定名词扎堆，读者容易跳过；把设定拆进动作、冲突和后果里", d.label(c), run.len()));
                f.chapter_id = Some(c.id);
                f.quote = clip(run[0], 80);
                out.push(f);
                break;
            }
            run.clear();
        }
    }
}

// ---------- 境界走向 ----------

fn realm_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\p{Han}{2})境").unwrap())
}

fn level_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\p{Han}{2})境?第?([一二三四五六七八九十两0-9]{1,3})[重层道]").unwrap())
}

/// 境界名按世界观里「X境」出现的先后排高低，世界观没写的按正文首次出现排在后面。
fn realm_names(d: &BookData) -> Vec<String> {
    let mut text = format!("{}\n{}", d.book.worldview, d.book.outline);
    for c in &d.chapters {
        text.push('\n');
        text.push_str(&c.content);
    }
    for e in &d.entries {
        text.push('\n');
        text.push_str(&e.description);
        text.push_str(&e.state_text());
    }
    let mut names: Vec<String> = Vec::new();
    for m in realm_word_re().captures_iter(&text) {
        if !names.iter().any(|n| n == &m[1]) {
            names.push(m[1].to_string());
        }
    }
    names
}

type Level = (usize, i64);

/// 文字里的第一个「境界 + 层数」，比如「炼体境四重」「炼体第六重」。
fn first_level<'t>(text: &'t str, realms: &[String]) -> Option<(Level, &'t str)> {
    level_re().captures_iter(text).find_map(|m| {
        let rank = realms.iter().position(|r| r == &m[1])?;
        Some(((rank, parse_cn_number(&m[2])?), m.get(0)?.as_str()))
    })
}

/// 快照里写着这些字时，境界低了是有原因的，不算倒退
const EXPLAINED: [&str; 17] = ["伪装", "压制", "压下", "跌", "受伤", "重伤", "封印", "散功", "废", "生前", "死", "只剩", "假装", "隐藏", "收敛", "装作", "表面"];

const BREAK_VERBS: [&str; 12] = ["突破至", "突破到", "突破了", "踏进了", "踏进", "踏入", "晋入", "迈入", "跨入", "破入", "晋升到", "晋升"];

/// 段落里写突破到的境界：段首就是「炼体四重。」、紧跟在「突破至」后面，或者「……，炼体四重！」。
fn breakthroughs<'p>(p: &'p str, realms: &[String]) -> Vec<(Level, &'p str, usize)> {
    let lead = p.len() - p.trim_start().len();
    level_re()
        .captures_iter(p)
        .filter_map(|m| {
            let all = m.get(0)?;
            let rank = realms.iter().position(|r| r == &m[1])?;
            let level = parse_cn_number(&m[2])?;
            let before = &p[..all.start()];
            let after = p[all.end()..].chars().next();
            let at_start = all.start() == lead && matches!(after, None | Some('！' | '!' | '。' | '，' | ',' | '…'));
            let by_verb = BREAK_VERBS.iter().any(|v| before.ends_with(v)) && after != Some('的');
            let exclaim = before.ends_with('，') && matches!(after, Some('！' | '!'));
            (at_start || by_verb || exclaim).then_some(((rank, level), all.as_str(), all.start()))
        })
        .collect()
}

/// 主角：标成主角的人物，其次是常驻人物，再次是正文里出现最多的人物。
fn protagonist(d: &BookData) -> Option<&Entry> {
    let people = || d.entries.iter().filter(|e| e.kind == "character");
    let mentions = |e: &Entry| -> usize { d.chapters.iter().map(|c| { let t = e.scan_text(&c.content); keys(e).iter().map(|k| t.matches(k.as_str()).count()).sum::<usize>() }).sum() };
    people().find(|e| e.role == "主角").or_else(|| people().find(|e| e.always_include)).or_else(|| people().max_by_key(|e| mentions(e)))
}

/// 一个人物各章章末快照里的境界，按章节先后；初始状态排最前，章节记为 None。
fn snapshot_levels<'a>(d: &'a BookData, e: &Entry, realms: &[String]) -> Vec<(Option<&'a Chapter>, Level, String)> {
    let mut snaps: Vec<(Option<usize>, &EntryState)> = d
        .states
        .iter()
        .filter(|s| s.entry_id == e.id && (s.chapter_id.is_none() || s.phase == "end"))
        .filter_map(|s| match s.chapter_id {
            None => Some((None, s)),
            Some(cid) => d.chapters.iter().position(|c| c.id == cid).map(|p| (Some(p), s)),
        })
        .collect();
    snaps.sort_by_key(|(p, s)| (p.map_or(-1, |p| p as i64), s.id));
    snaps
        .into_iter()
        .filter_map(|(p, s)| {
            let power = s.fields.get("power").cloned().unwrap_or_else(|| s.text());
            if EXPLAINED.iter().any(|w| power.contains(w)) {
                return None;
            }
            let (lv, txt) = first_level(&power, realms)?;
            Some((p.map(|p| &d.chapters[p]), lv, txt.to_string()))
        })
        .collect()
}

fn realms(d: &BookData, out: &mut Vec<Finding>) {
    let realms = realm_names(d);
    let place = |c: Option<&Chapter>| c.map_or("初始设定".to_string(), |c| d.label(c));

    // 快照里的境界倒退：每个人物各报第一处
    for e in d.entries.iter().filter(|e| e.kind == "character") {
        let mut best: Option<(Level, String, Option<&Chapter>)> = None;
        for (c, lv, txt) in snapshot_levels(d, e, &realms) {
            if let Some((top, top_txt, top_c)) = &best {
                if lv < *top {
                    let mut f = finding("warn", "境界倒退", format!("「{}」在{}是「{top_txt}」，到{}变成了「{txt}」，前后没写受伤、压制或跌落", e.name, place(*top_c), place(c)));
                    f.entry_id = Some(e.id);
                    f.chapter_id = c.map(|c| c.id);
                    out.push(f);
                    break;
                }
            }
            if best.as_ref().map_or(true, |(top, ..)| lv > *top) {
                best = Some((lv, txt, c));
            }
        }
    }

    // 主角把已经到过的一层又突破一次
    let Some(hero) = protagonist(d) else { return };
    let others: Vec<String> = d.entries.iter().filter(|e| e.kind == "character" && e.id != hero.id).flat_map(keys).collect();
    let snaps = snapshot_levels(d, hero, &realms);
    let mut reached: Option<(Level, String, String)> = None;
    let raise = |reached: &mut Option<(Level, String, String)>, lv: Level, txt: &str, at: String| {
        if reached.as_ref().map_or(true, |(top, ..)| lv > *top) {
            *reached = Some((lv, txt.to_string(), at));
        }
    };
    for (_, lv, txt) in snaps.iter().filter(|(c, ..)| c.is_none()) {
        raise(&mut reached, *lv, txt, "初始设定".into());
    }
    for c in written(d) {
        let mut repeats: Vec<(&str, String)> = Vec::new();
        let mut gained: Vec<(Level, &str)> = Vec::new();
        for p in paragraphs(&c.content).into_iter().filter(|p| !others.iter().any(|k| p.contains(k.as_str()))) {
            for (lv, txt, pos) in breakthroughs(p, &realms) {
                match &reached {
                    Some((top, ..)) if lv <= *top => {
                        if !repeats.iter().any(|(t, _)| *t == txt) {
                            repeats.push((txt, sentence_at(p, pos)));
                        }
                    }
                    _ => gained.push((lv, txt)),
                }
            }
        }
        if let (Some((_, top_txt, top_at)), Some((_, quote))) = (&reached, repeats.first()) {
            let levels: Vec<&str> = repeats.iter().map(|(t, _)| *t).collect();
            let mut f = finding(
                "warn",
                "境界重复突破",
                format!("{}写{}突破到「{}」，但{}就已经是「{top_txt}」了，同一层不能突破两次", d.label(c), hero.name, levels.join("」「"), top_at),
            );
            f.chapter_id = Some(c.id);
            f.entry_id = Some(hero.id);
            f.quote = quote.clone();
            out.push(f);
        }
        for (lv, txt) in gained {
            raise(&mut reached, lv, txt, d.label(c));
        }
        for (_, lv, txt) in snaps.iter().filter(|(sc, ..)| sc.is_some_and(|sc| sc.id == c.id)) {
            raise(&mut reached, *lv, txt, d.label(c));
        }
    }
}

// ---------- 物品去向 ----------

/// 物品用掉、毁掉、交出去的说法，越靠前越确定
const GONE: [&str; 29] = [
    "耗竭", "耗尽", "无存", "殆尽", "用尽", "用光", "彻底损毁", "损毁", "已毁", "毁去", "化为飞灰", "化作飞灰", "化为齑粉", "化作齑粉", "崩碎", "碎裂", "粉碎", "炸裂", "交出",
    "上交", "被夺", "被抢", "丢失", "遗失", "服下", "咽下", "吞服", "吞下", "吞噬",
];
/// 只用掉一部分
const PART: [&str; 14] = ["一半", "半块", "一角", "剩下", "剩余", "余下", "其余", "残晶", "还剩", "留下", "揣回", "藏好", "收好", "包好"];
/// 又拿出来用
const USE: [&str; 27] = [
    "取出", "拿出", "掏出", "摸出", "握着", "握住", "攥着", "攥住", "捏着", "托着", "捧着", "递给", "递过", "扣在", "放在", "拍在", "拍进", "塞进", "揣着", "祭出", "催动", "激发", "卖给",
    "当铺", "交给", "服用", "炼化",
];

/// 快照里写物品没了的那一小句；同一句里有「一半、半块」这类字样的不算。
fn gone_in_state(state: &str) -> Option<String> {
    let whole: Vec<&str> = sentences(state).filter(|s| !PART.iter().any(|w| s.contains(w))).collect();
    GONE.iter().find_map(|w| whole.iter().flat_map(|s| s.split(['，', '；', ',', ';'])).find(|cl| cl.contains(w)).map(|cl| cl.trim().to_string()))
}

/// 没有快照时看正文：同一小句里写了物品和「耗尽、吞服」这类字样，并且全章没有哪句说只用了一部分。
fn gone_in_prose(text: &str, ks: &[String]) -> Option<String> {
    let has_key = |s: &str| ks.iter().any(|k| s.contains(k.as_str()));
    if sentences(text).any(|s| has_key(s) && PART.iter().any(|w| s.contains(w))) {
        return None;
    }
    GONE.iter().find_map(|w| sentences(text).flat_map(|s| s.split(['，', '；', ',', ';'])).find(|cl| has_key(cl) && cl.contains(w)).map(|cl| cl.trim().to_string()))
}

fn used_in(text: &str, ks: &[String]) -> Option<String> {
    sentences(text).find(|s| ks.iter().any(|k| s.contains(k.as_str())) && USE.iter().any(|w| s.contains(w))).map(|s| clip(s, 80))
}

// ---------- 带有效区间的事实 ----------

/// 一条带有效区间的事实（参考 FactTrack）：某人从第几章起是什么境界、某件物品从第几章起已经没了。
/// 都从定稿时按章入账的快照推出来，到下一次变化为止一直有效。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Fact {
    pub entry_id: i64,
    pub name: String,
    /// realm 境界 / gone 物品没了
    pub kind: &'static str,
    pub value: String,
    /// 从第几章起；None 是开篇前
    pub from: Option<i64>,
    /// 到第几章被新的取代；None 是一直到现在
    pub to: Option<i64>,
    #[serde(skip)]
    pos: Option<usize>,
}

pub fn facts(d: &BookData) -> Vec<Fact> {
    let realms = realm_names(d);
    let mut out = Vec::new();
    for e in d.entries.iter().filter(|e| e.kind == "character") {
        // 按解析出的境界和层数比，「炼体境四重」「炼体四重」算同一个
        let mut runs: Vec<(Option<usize>, String, Level)> = Vec::new();
        for (c, lv, txt) in snapshot_levels(d, e, &realms) {
            if runs.last().map_or(true, |(_, _, l)| *l != lv) {
                runs.push((c.and_then(|c| d.position(c.id)), txt, lv));
            }
        }
        let number = |p: Option<usize>| p.and_then(|p| d.number(&d.chapters[p]));
        for (i, (pos, value, _)) in runs.iter().enumerate() {
            let to = runs.get(i + 1).and_then(|(p, _, _)| number(*p));
            out.push(Fact { entry_id: e.id, name: e.name.trim().to_string(), kind: "realm", value: value.clone(), from: number(*pos), to, pos: *pos });
        }
    }
    for e in d.entries.iter().filter(|e| e.kind == "item") {
        let ks = keys(e);
        for c in written(d) {
            let snap = d.states.iter().filter(|s| s.entry_id == e.id && s.chapter_id == Some(c.id) && s.phase == "end").max_by_key(|s| s.id);
            let why = match snap {
                Some(s) => gone_in_state(&s.text()),
                None => gone_in_prose(&c.content, &ks),
            };
            if let Some(why) = why {
                out.push(Fact { entry_id: e.id, name: e.name.trim().to_string(), kind: "gone", value: why, from: d.number(c), to: None, pos: d.position(c.id) });
                break;
            }
        }
    }
    out
}

/// 写 current 这一章时，出场的人物和物品已经生效的事实：本章之前入账的（本章里才变的，写本章时还没变）。
pub fn facts_at(d: &BookData, current: &Chapter, present: &[&Entry]) -> Vec<String> {
    let Some(cur) = d.position(current.id) else { return Vec::new() };
    let ids: HashSet<i64> = present.iter().map(|e| e.id).collect();
    let all = facts(d);
    let valid = |f: &&Fact| ids.contains(&f.entry_id) && f.pos.map_or(true, |p| p < cur);
    let mut out = Vec::new();
    for e in present.iter().filter(|e| e.kind == "character") {
        if let Some(f) = all.iter().filter(valid).filter(|f| f.entry_id == e.id && f.kind == "realm").last() {
            let since = f.from.map_or_else(|| "开篇前".to_string(), |n| format!("第{n}章起"));
            out.push(format!("· {}：{}（{since}）", f.name, f.value));
        }
    }
    for f in all.iter().filter(valid).filter(|f| f.kind == "gone") {
        let since = f.from.map_or_else(String::new, |n| format!("第{n}章起"));
        out.push(format!("· 「{}」{since}已经没了（{}），不能再拿出来用", f.name, clip(&f.value, 40)));
    }
    out
}

fn items(d: &BookData, out: &mut Vec<Finding>) {
    let chapters = written(d);
    for e in d.entries.iter().filter(|e| e.kind == "item") {
        let ks = keys(e);
        let mut gone: Option<(&Chapter, String)> = None;
        for c in &chapters {
            if let Some((g, why)) = &gone {
                if let Some(quote) = used_in(&e.scan_text(&c.content), &ks) {
                    let mut f = finding(
                        "warn",
                        "物品去向",
                        format!("「{}」在{}已经「{why}」，{}又被拿出来用了，请确认是回忆、另一件同名的东西，还是前文写错了", e.name, d.label(g), d.label(c)),
                    );
                    f.chapter_id = Some(c.id);
                    f.entry_id = Some(e.id);
                    f.quote = quote;
                    out.push(f);
                    break;
                }
                continue;
            }
            let snap = d.states.iter().filter(|s| s.entry_id == e.id && s.chapter_id == Some(c.id) && s.phase == "end").max_by_key(|s| s.id);
            let why = match snap {
                Some(s) => gone_in_state(&s.text()),
                None => gone_in_prose(&c.content, &ks),
            };
            if let Some(why) = why {
                gone = Some((c, why));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: i64, content: &str) -> Chapter {
        Chapter { id, book_id: 1, sort: id, content: content.into(), volume_id: Some(1), ..Default::default() }
    }

    fn book(chapters: Vec<Chapter>, entries: Vec<Entry>, states: Vec<EntryState>) -> BookData {
        let book = Book { worldview: "1. 炼体境（九重）\n2. 开脉境\n3. 灵台境".into(), ..Default::default() };
        let vols = vec![Volume { id: 1, title: "第一卷".into(), sort: 1, ..Default::default() }];
        BookData::new(book, vols, chapters, entries, vec![], states)
    }

    fn person(id: i64, name: &str) -> Entry {
        Entry { id, kind: "character".into(), name: name.into(), ..Default::default() }
    }

    fn kinds(found: &[Finding]) -> Vec<&str> {
        found.iter().map(|f| f.kind.as_str()).collect()
    }

    #[test]
    fn facts_have_validity_intervals() {
        let hero = person(1, "陈渊");
        let pill = Entry { id: 2, kind: "item".into(), name: "毒灵髓".into(), ..Default::default() };
        let snap = |id, entry_id, chapter: Option<i64>, power: &str, state: &str| EntryState {
            id,
            entry_id,
            chapter_id: chapter,
            phase: "end".into(),
            state: state.into(),
            fields: if power.is_empty() { Default::default() } else { [("power".to_string(), power.to_string())].into_iter().collect() },
            ..Default::default()
        };
        let states = vec![snap(1, 1, None, "炼体三重", ""), snap(2, 1, Some(2), "炼体五重", ""), snap(3, 1, Some(3), "炼体境五重", ""), snap(4, 1, Some(4), "炼体七重", ""), snap(5, 2, Some(3), "", "毒灵髓耗竭无存")];
        let d = book((1..=5).map(|i| ch(i, "陈渊挖矿。")).collect(), vec![hero.clone(), pill.clone()], states);
        let realm: Vec<(String, Option<i64>, Option<i64>)> = facts(&d).iter().filter(|f| f.kind == "realm").map(|f| (f.value.clone(), f.from, f.to)).collect();
        assert_eq!(realm, vec![("炼体三重".into(), None, Some(2)), ("炼体五重".into(), Some(2), Some(4)), ("炼体七重".into(), Some(4), None)], "没变化的快照不切区间，写法不同也算同一层");
        let at4 = facts_at(&d, d.chapter(4).unwrap(), &[&hero, &pill]);
        assert_eq!(at4, vec!["· 陈渊：炼体五重（第2章起）".to_string(), "· 「毒灵髓」第3章起已经没了（毒灵髓耗竭无存），不能再拿出来用".to_string()]);
        let at3 = facts_at(&d, d.chapter(3).unwrap(), &[&hero, &pill]);
        assert!(at3.iter().all(|l| !l.contains("毒灵髓")), "第3章里才用完的，写第3章时还在：{at3:?}");
        assert!(facts_at(&d, d.chapter(4).unwrap(), &[&hero]).iter().all(|l| !l.contains("毒灵髓")), "没出场的物品不提");
    }

    #[test]
    fn reveal_title_must_not_carry_hidden_names() {
        let sky = Entry { id: 7, kind: "concept".into(), name: "三十三重天元天轨".into(), aliases: "天元天轨".into(), visibility: "第4卷起".into(), ..Default::default() };
        let mut d = book(vec![ch(1, "陈渊抬头看天。")], vec![sky], vec![]);
        let secret = |title: &str| Reveal { id: 1, book_id: 1, title: title.into(), status: "active".into(), ..Default::default() };
        d.reveals = vec![secret("天元天轨与天地浊灵气的来历")];
        let found: Vec<Finding> = check(&d).into_iter().filter(|f| f.kind == "秘密标题带出设定").collect();
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("标题里有「天元天轨」（设定「三十三重天元天轨」第4卷起）") && found[0].reveal_id == Some(1), "{found:?}");
        d.reveals = vec![secret("天轨与浊灵气的来历")];
        assert!(check(&d).iter().all(|f| f.kind != "秘密标题带出设定"), "公开的说法不算");
    }

    #[test]
    fn payoff_milestone_needs_a_resolved_thread_in_first_ten() {
        let milestone = |d: &BookData| check(d).into_iter().filter(|f| f.kind == "开篇里程碑").count();
        let mut d = book((1..=9).map(|i| ch(i, "陈渊挖矿。")).collect(), vec![], vec![]);
        assert_eq!(milestone(&d), 0, "前 10 章没写完不提醒");
        d = book((1..=10).map(|i| ch(i, "陈渊挖矿。")).collect(), vec![], vec![]);
        assert_eq!(milestone(&d), 1);
        d.threads = vec![Thread { id: 1, title: "草偶的用法".into(), status: "resolved".into(), planted_chapter_id: Some(2), resolved_chapter_id: Some(8), ..Default::default() }];
        assert_eq!(milestone(&d), 0, "第 8 章回收过伏笔就不提醒");
    }

    #[test]
    fn opening_rules() {
        use serde_json::json;
        let hero = Entry { id: 1, kind: "character".into(), name: "陈渊".into(), role: "主角".into(), ..Default::default() };
        let mut c1 = ch(1, &format!("{}陈渊睁开眼。{}太虚梦潮在他眼前展开。", "矿洞里很黑。".repeat(60), "风很冷。".repeat(200)));
        c1.metrics = json!({ "hook": { "type": "截断" } });
        let mut c2 = ch(2, "陈渊继续往前走。");
        c2.metrics = json!({ "hook": { "type": "无" } });
        let c3 = ch(3, "陈渊到了矿口。");
        let b = Book { golden_finger: "太虚梦潮、梦潮".into(), ..Default::default() };
        let vols = vec![Volume { id: 1, title: "第一卷".into(), sort: 1, ..Default::default() }];
        let d = BookData::new(b, vols, vec![c1, c2, c3], vec![hero], vec![], vec![]);
        let found: Vec<Finding> = check(&d).into_iter().filter(|f| f.kind == "开篇").collect();
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[0].message.contains("主角「陈渊」到第1章第 360 字才出场") && found[0].quote == "陈渊睁开眼。");
        assert!(found[1].message.contains("金手指「太虚梦潮」到第1章第 1166 字"));
        assert!(found[2].chapter_id == Some(2) && found[2].message.contains("没有章末钩子"), "第3章没做定稿分析，不报");
    }

    #[test]
    fn flags_settings_written_too_early() {
        let entries = vec![
            Entry { id: 1, kind: "concept".into(), name: "伪神".into(), aliases: "三十三重天轨".into(), visibility: "第3卷起".into(), ..Default::default() },
            Entry { id: 2, kind: "character".into(), name: "姜沉雪".into(), visibility: "".into(), ..Default::default() },
            Entry { id: 3, kind: "concept".into(), name: "执秤真皇".into(), visibility: "仅规划".into(), ..Default::default() },
        ];
        let d = book(vec![ch(1, "矿道塌了。\n天上那道锁链，是三十三重天轨的一截。"), ch(2, "姜沉雪在药铺里配药。")], entries, vec![]);
        let found = check(&d);
        let leak: Vec<&Finding> = found.iter().filter(|f| f.kind == "设定超前").collect();
        assert_eq!(leak.len(), 1, "{found:?}");
        assert_eq!(leak[0].level, "critical");
        assert!(leak[0].message.contains("「三十三重天轨」") && leak[0].message.contains("第3卷起"));
        assert_eq!(leak[0].quote, "天上那道锁链，是三十三重天轨的一截。");
    }

    #[test]
    fn counts_new_setting_terms() {
        let entries = vec![
            person(1, "陈渊"),
            person(2, "王奎"),
            Entry { id: 3, kind: "location".into(), name: "黑灵矿区".into(), ..Default::default() },
            Entry { id: 4, kind: "faction".into(), name: "天轨护道盟".into(), ..Default::default() },
            Entry { id: 5, kind: "item".into(), name: "因果置换草偶（残）".into(), ..Default::default() },
            Entry { id: 6, kind: "item".into(), name: "装着三十枚中品灵石的锦囊".into(), ..Default::default() },
        ];
        let first = "陈渊在黑灵矿区醒来，王奎提刀走近。天轨护道盟的税吏还在路上。\n【诸天禁物：因果置换草偶（残）】\n【位阶：界外遗物】\n装着三十枚中品灵石的锦囊掉在地上。";
        let mut entries = entries;
        entries.push(Entry { id: 7, kind: "item".into(), name: "《天轨律法》".into(), ..Default::default() });
        let second = "陈渊回到黑灵矿区，翻开《天元度支簿》，又想起《天轨律法》第二卷。";
        let d = book(vec![ch(1, first), ch(2, second)], entries, vec![]);
        let found = check(&d);
        let dense: Vec<&Finding> = found.iter().filter(|f| f.kind == "新名词过多").collect();
        assert_eq!(dense.len(), 1, "{found:?}");
        assert_eq!(dense[0].chapter_id, Some(1));
        assert!(dense[0].message.contains("5 个") && dense[0].message.contains("诸天禁物") && dense[0].message.contains("界外遗物"), "{}", dense[0].message);
        assert!(!dense[0].message.contains("陈渊") && !dense[0].message.contains("锦囊"), "人名和描述性的条目不算");
    }

    #[test]
    fn flags_long_exposition() {
        let entries = vec![
            Entry { id: 1, kind: "faction".into(), name: "天轨护道盟".into(), ..Default::default() },
            Entry { id: 2, kind: "faction".into(), name: "逆命司".into(), ..Default::default() },
            Entry { id: 3, kind: "location".into(), name: "九霄天轨".into(), ..Default::default() },
            Entry { id: 4, kind: "location".into(), name: "归墟神座".into(), ..Default::default() },
        ];
        let lore = "天轨护道盟坐镇九霄天轨，替归墟神座收取天寿税，逆命司则在暗处与之周旋，开脉境以上的功法尽归其手。";
        let text = format!("陈渊睁开眼。\n{lore}\n{lore}\n{lore}\n“走。”他说。");
        let d = book(vec![ch(1, &text)], entries, vec![]);
        let found = check(&d);
        assert!(kinds(&found).contains(&"大段纯设定"), "{found:?}");
        let two = format!("{lore}\n“走。”\n{lore}");
        assert!(!kinds(&check(&book(vec![ch(1, &two)], d.entries.clone(), vec![]))).contains(&"大段纯设定"), "中间有对话就不算");
    }

    /// 原稿回归样本：摘自《大梦长生》重写前的第 2、5、7、10 章和当时的设定快照。
    fn original_drafts() -> BookData {
        let chapters = vec![
            ch(1, "自身此刻不过是炼体三重的神魂强度，微弱如风中残烛。\n炼体六重，监工王奎。"),
            ch(2, "原本停滞在炼体境三重的滞塞关卡，在这股不计代价的狂暴冲击之下，轰然被冲垮踏平！\n炼体境四重！"),
            ch(3, "体内两枚血气丹的狂暴药力仍在经脉里横冲直撞，刚突破至炼体四重的气血如重锤般撞击着耳膜。"),
            ch(4, "陈渊将晶石与锦囊迅速塞入自己怀中，用布条勒紧。赵陵看着那块毒灵髓落入陈渊手中。"),
            ch(5, "陈渊单手握住毒灵髓，掌心发力，五指如钢索收紧。\n换作常人，绝不敢在毫无丹炉调和的情况下吞噬极品毒灵髓，那无异于服毒自尽。\n炼体五重。"),
            ch(6, "陈渊没有接话，上前一步，将半块裹着油纸的毒灵髓残晶与从王奎身上缴获的储物锦囊扣在木质柜台上。"),
            ch(7, "第一道壁障粉碎，气血自丹田轰然涌出，炼体四重！\n药力继续下压，枯萎的杂役气血被破障丹的纯净源质疯狂替换，炼体五重！\n炼体六重！\n炼体六重，气血如汞。"),
            ch(8, "赵黑虎浑身横肉猛地炸开，炼体六重的狂暴气血毫无保留地灌入四肢。"),
            ch(9, "井下阴影中，陈渊静静看着这一切。"),
            ch(10, "第二块血髓碎裂成粉末。\n炼体第六重，皮如硬革，刀枪难入！\n炼体第七重，骨若沉铁，气血如汞！"),
        ];
        let entries = vec![
            Entry { id: 1, always_include: true, ..person(1, "陈渊") },
            person(2, "王奎"),
            person(3, "赵陵"),
            person(4, "赵黑虎"),
            Entry { id: 5, kind: "item".into(), name: "极品毒灵髓".into(), ..Default::default() },
        ];
        let power = |id, chapter: Option<i64>, p: &str| EntryState {
            id,
            entry_id: 1,
            chapter_id: chapter,
            phase: if chapter.is_some() { "end" } else { "start" }.into(),
            fields: [("power".to_string(), p.to_string())].into_iter().collect(),
            ..Default::default()
        };
        let states = vec![
            power(1, None, "炼体境三重"),
            power(2, Some(2), "炼体境四重（刚破境）"),
            power(3, Some(5), "炼体五重（连破第四、第五道气血玄关）"),
            power(4, Some(7), "炼体六重（气血如汞，服极品无漏破障丹连破三重）"),
            EntryState { id: 5, entry_id: 5, chapter_id: Some(5), phase: "end".into(), state: "已被陈渊捏碎外壳、按入胸口断骨伤处直接吞噬，药力用于连破两道气血玄关，晶髓耗竭无存；赵陵尸骸旁另留有被煞气撑爆的深绿色结晶碎屑。".into(), ..Default::default() },
        ];
        book(chapters, entries, states)
    }

    #[test]
    fn original_drafts_report_realm_and_item_contradictions() {
        let found = check(&original_drafts());
        let repeat: Vec<&Finding> = found.iter().filter(|f| f.kind == "境界重复突破").collect();
        assert_eq!(repeat.iter().map(|f| f.chapter_id).collect::<Vec<_>>(), vec![Some(7), Some(10)], "{found:#?}");
        assert!(repeat[0].message.contains("「炼体四重」「炼体五重」") && repeat[0].message.contains("第5章就已经是「炼体五重」"), "{}", repeat[0].message);
        assert!(repeat[1].message.contains("「炼体第六重」") && repeat[1].message.contains("第7章就已经是「炼体六重」"), "{}", repeat[1].message);
        let item: Vec<&Finding> = found.iter().filter(|f| f.kind == "物品去向").collect();
        assert_eq!(item.len(), 1, "{found:#?}");
        assert_eq!(item[0].chapter_id, Some(6));
        assert!(item[0].message.contains("晶髓耗竭无存"), "{}", item[0].message);
        assert!(item[0].quote.contains("扣在木质柜台上"));
    }

    #[test]
    fn prose_alone_catches_used_up_items() {
        let mut d = original_drafts();
        d.states.retain(|s| s.entry_id != 5);
        let item: Vec<Finding> = check(&d).into_iter().filter(|f| f.kind == "物品去向").collect();
        assert_eq!(item.len(), 1, "没有快照时看正文：第5章吞噬了整块，第6章又拿出来");
        assert!(item[0].message.contains("吞噬极品毒灵髓"), "{}", item[0].message);
    }

    #[test]
    fn rewritten_chapters_stay_clean() {
        let mut d = original_drafts();
        let fixed = [
            (5, "他五指一合，把毒灵髓掰成两半，一半揣回怀里，另一半攥在掌心捏碎。\n没有丹炉调和就生吞极品毒灵髓，换了旁人，跟服毒自尽没有分别。\n炼体五重。"),
            (7, "井底激荡起一圈肉眼看得见的气流，墙上的青苔被震成了粉末。\n炼体六重。"),
            (10, "他把炼体六重的气血一层层压回骨髓深处，只在皮肉表面留下薄薄一层炼体三重的气息。\n炼体七重。骨沉如铁，气血如汞。"),
        ];
        for (id, text) in fixed {
            d.chapters.iter_mut().find(|c| c.id == id).unwrap().content = text.into();
        }
        d.states.retain(|s| s.entry_id != 5);
        d.states.push(EntryState { id: 9, entry_id: 5, chapter_id: Some(5), phase: "end".into(), state: "陈渊将其掰成两半，一半捏碎后按入胸口伤处炼化；另半块残晶用油纸包好留下。".into(), ..Default::default() });
        let found = check(&d);
        for k in ["境界重复突破", "境界倒退", "物品去向"] {
            assert!(!kinds(&found).contains(&k), "重写后的稿子不该报「{k}」：{found:#?}");
        }
    }

    fn plan(id: i64, title: &str) -> Reveal {
        Reveal { id, book_id: 1, title: title.into(), status: "active".into(), ..Default::default() }
    }

    fn event(id: i64, reveal_id: i64, chapter_id: i64, step: &str) -> RevealEvent {
        RevealEvent { id, reveal_id, chapter_id, step: step.into(), ..Default::default() }
    }

    #[test]
    fn reveal_terms_flag_leaks_and_early_hints() {
        let chapters = vec![
            ch(1, "天轨护道盟的税吏来了。"),
            ch(2, "矿道塌了。\n他抬头看见天轨又亮了一下。"),
            ch(3, "老矿奴说，天上那道光每年会暗一次。"),
            ch(4, "陈渊想起天轨上那些铸造的纹路。"),
            ch(5, "天轨在动。"),
            ch(6, "天轨原来是一条锁链。"),
        ];
        let mut d = book(chapters, vec![], vec![]);
        d.reveals = vec![Reveal { terms: "天轨".into(), exceptions: "天轨护道盟".into(), seed_at: Some(3), reveal_at: Some(6), ..plan(1, "天上那道光的来历") }];
        let found = check(&d);
        let leak: Vec<&Finding> = found.iter().filter(|f| f.reveal_id == Some(1)).collect();
        assert_eq!(leak.len(), 2, "{found:#?}");
        assert_eq!((leak[0].kind.as_str(), leak[0].level.as_str(), leak[0].chapter_id), ("设定超前", "critical", Some(2)));
        assert_eq!(leak[0].quote, "他抬头看见天轨又亮了一下。");
        assert!(leak[0].message.contains("计划第6章才揭开"), "{}", leak[0].message);
        assert_eq!((leak[1].kind.as_str(), leak[1].chapter_id), ("提前点破", Some(4)), "埋种子之后只提醒第一处，第5章不再报，揭开那章不报");

        d.reveal_events = vec![event(1, 1, 2, "seed")];
        let found = check(&d);
        let kinds: Vec<(&str, Option<i64>)> = found.iter().filter(|f| f.reveal_id == Some(1)).map(|f| (f.kind.as_str(), f.chapter_id)).collect();
        assert_eq!(kinds, vec![("提前点破", Some(2))], "定稿记下第2章埋了种子，那一处就算线索");

        d.reveals[0].reveal_at = None;
        d.reveal_events.clear();
        assert!(check(&d).iter().all(|f| f.reveal_id.is_none()), "没有揭开时间也没揭开过，判断不了");
    }

    #[test]
    fn early_terms_in_written_chapters() {
        let chapters = vec![ch(1, "陈渊的太虚梦潮又转了一圈。天轨护道盟来收天寿税。"), ch(2, "他看见伪神的影子。"), ch(3, "伪神原来是铸天者。")];
        let d = book(chapters, vec![], vec![]);
        let all = split_terms("太虚梦潮、伪神、天轨、铸天者");
        let pair = |t: &str, n: i64| (t.to_string(), n);
        assert_eq!(early_terms(&d, &all, &split_terms("天轨护道盟"), Some(3)), vec![pair("太虚梦潮", 1), pair("伪神", 2)], "揭开那章起不算，例外说法不算");
        assert_eq!(early_terms(&d, &all, &[], Some(3)), vec![pair("太虚梦潮", 1), pair("天轨", 1), pair("伪神", 2)]);
        assert_eq!(early_terms(&d, &split_terms("铸天者"), &[], Some(40)), vec![pair("铸天者", 3)], "揭开的章还没写到，已写的都在揭开之前");
        assert_eq!(early_terms(&d, &split_terms("铸天者"), &[], None), vec![pair("铸天者", 3)], "没定揭开时间也照样提醒：禁区里同样会列这些词");
    }

    #[test]
    fn reveal_progress_checks() {
        let chapters = (1..=10).map(|i| ch(i, "矿道里很黑。")).collect();
        let mut d = book(chapters, vec![], vec![]);
        d.reveals = vec![
            plan(1, "沉寂的秘密"),
            Reveal { reveal_at: Some(5), ..plan(2, "逾期的秘密") },
            Reveal { reveal_at: Some(13), ..plan(3, "冷掉的秘密") },
            Reveal { reveal_at: Some(50), ..plan(4, "还早的秘密") },
            Reveal { reveal_at: Some(3), ..plan(5, "揭开了的秘密") },
            Reveal { reveal_at: Some(12), ..plan(6, "没埋过种子的秘密") },
        ];
        d.reveal_events = vec![event(1, 1, 1, "seed"), event(2, 3, 2, "clue"), event(3, 5, 3, "reveal"), event(4, 2, 9, "clue")];
        let found = check(&d);
        let got: Vec<(i64, &str)> = found.iter().filter_map(|f| f.reveal_id.map(|id| (id, f.kind.as_str()))).collect();
        assert_eq!(got, vec![(1, "信息进度"), (2, "揭示逾期"), (3, "信息进度"), (3, "记忆温度"), (6, "记忆温度")], "{found:#?}");
        let cold = found.iter().find(|f| f.reveal_id == Some(3) && f.kind == "记忆温度").unwrap();
        assert!(cold.message.contains("计划第13章揭开，最近一次线索在第2章，隔了 11 章"), "{}", cold.message);
        let fresh = found.iter().find(|f| f.reveal_id == Some(6)).unwrap();
        assert!(fresh.message.contains("前面还没埋过种子"), "{}", fresh.message);
    }

    #[test]
    fn snapshot_regression_needs_a_reason() {
        let entries = vec![Entry { always_include: true, ..person(1, "陈渊") }, person(2, "赵陵")];
        let snap = |id, entry_id, chapter, p: &str| EntryState { id, entry_id, chapter_id: Some(chapter), phase: "end".into(), fields: [("power".to_string(), p.to_string())].into_iter().collect(), ..Default::default() };
        let states = vec![
            snap(1, 1, 2, "炼体四重"),
            snap(2, 1, 5, "炼体三重"),
            snap(3, 2, 2, "炼体六重"),
            snap(4, 2, 5, "生前炼体六重，被砝码压下一重，只剩约炼体三重的气力"),
        ];
        let chapters = (1..=5).map(|i| ch(i, "矿道里很黑。")).collect();
        let found = check(&book(chapters, entries, states));
        let back: Vec<&Finding> = found.iter().filter(|f| f.kind == "境界倒退").collect();
        assert_eq!(back.len(), 1, "{found:#?}");
        assert!(back[0].message.contains("「陈渊」在第2章是「炼体四重」，到第5章变成了「炼体三重」"), "{}", back[0].message);
    }
}
