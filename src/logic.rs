//! 叙事逻辑的规则检查：不调用模型，找出设定超前、新名词过密、大段纯设定、境界走向和物品去向的问题，每条都附原文。
//! 新名词密度和纯设定段落改编自 webnovel-handbook（MIT）第 28 章的信息投放规则；
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

/// 命中位置所在的句子。
fn sentence_at(text: &str, pos: usize) -> String {
    let start = text[..pos].rfind(SENTENCE_END).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8));
    let end = text[pos..].find(SENTENCE_END).map_or(text.len(), |i| pos + i + text[pos + i..].chars().next().map_or(1, char::len_utf8));
    clip(text[start..end].trim(), 80)
}

fn sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(SENTENCE_END).map(str::trim).filter(|s| !s.is_empty())
}

// ---------- 设定超前 ----------

fn leaks(d: &BookData, out: &mut Vec<Finding>) {
    for c in written(d) {
        let at = Some(d.point(c));
        for e in d.entries.iter().filter(|e| !e.gate().open_at(at)) {
            if let Some((pos, kw)) = first_hit(&keys(e), &c.content) {
                let mut f = finding("critical", "设定超前", format!("{}出现了「{kw}」，这条设定是「{}」，这一章还不该写出来", d.label(c), e.gate().label()));
                f.chapter_id = Some(c.id);
                f.entry_id = Some(e.id);
                f.quote = sentence_at(&c.content, pos);
                out.push(f);
            }
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

/// 每章首次出现的设定名词上限：第 1 章三个（webnovel-handbook），之后逐步放宽。
fn new_term_limit(n: Option<i64>) -> usize {
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
            if !seen_entries.contains(&e.id) && keys(e).iter().any(|k| c.content.contains(k.as_str())) {
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
    let mentions = |e: &Entry| -> usize { keys(e).iter().map(|k| d.chapters.iter().map(|c| c.content.matches(k.as_str()).count()).sum::<usize>()).sum() };
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

fn items(d: &BookData, out: &mut Vec<Finding>) {
    let chapters = written(d);
    for e in d.entries.iter().filter(|e| e.kind == "item") {
        let ks = keys(e);
        let mut gone: Option<(&Chapter, String)> = None;
        for c in &chapters {
            if let Some((g, why)) = &gone {
                if let Some(quote) = used_in(&c.content, &ks) {
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
