//! 免费的连续性体检：不调用模型，按规则检查伏笔、人物出场、状态和节奏；设定超前、境界、物品等叙事逻辑检查见 logic.rs。
//! 检查项参考 Novel-OS 的 continuity_engine（MIT），阈值按网文节奏调整。

use crate::memory::BookData;
use crate::models::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    /// critical / warn / info
    pub level: String,
    pub kind: String,
    pub message: String,
    pub chapter_id: Option<i64>,
    pub entry_id: Option<i64>,
    pub thread_id: Option<i64>,
    /// 揭示计划里的秘密
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reveal_id: Option<i64>,
    /// 原文依据：规则检查找到的那一句
    #[serde(skip_serializing_if = "String::is_empty")]
    pub quote: String,
}

/// 网文伏笔周期比 Novel-OS 默认的 3 章长得多
const DORMANT_THREAD_CHAPTERS: i64 = 20;
const OLD_THREAD_CHAPTERS: i64 = 30;
const ABSENT_MAJOR_CHAPTERS: usize = 5;
const LOW_TENSION: f64 = 4.0;
const LOW_TENSION_STREAK: usize = 3;
const NO_COOL_POINT_STREAK: usize = 5;
const DEATH_WORDS: [&str; 7] = ["死亡", "已死", "身亡", "阵亡", "牺牲", "死了", "去世"];
/// 死者名字所在的那句里有这些说法，写的是尸体、死讯或回忆，不算本人出场
const REMAINS_WORDS: &[&str] = &[
    "尸首", "尸体", "遗体", "尸身", "尸骨", "骸骨", "遗物", "遗言", "遗愿", "临死", "死前", "生前", "死后", "死了", "已死", "身亡", "遇害",
    "回忆", "想起", "记起", "梦见", "灵位", "牌位", "坟",
];
/// 紧跟在死者名字后面，写的是他留下的东西
const REMAINS_AFTER_NAME: &[&str] = &["的血", "的心头血", "那包", "那把", "那件", "那块", "那柄", "留下的"];

pub(crate) fn finding(level: &str, kind: &str, message: String) -> Finding {
    Finding { level: level.into(), kind: kind.into(), message, chapter_id: None, entry_id: None, thread_id: None, reveal_id: None, quote: String::new() }
}

fn mentions(e: &Entry, text: &str) -> bool {
    let text = e.scan_text(text);
    e.keywords().iter().filter(|k| k.chars().count() >= 2 || **k == e.name.trim()).any(|k| text.contains(k.as_str()))
}

/// 死者在这段文字里真正出场的第一句（截断后的原文）：名字后面紧跟的不是他留下的东西，整句也没有尸体、死讯、回忆这类说法。
fn appearance(e: &Entry, text: &str) -> Option<String> {
    let scan = e.scan_text(text);
    let mut hits: Vec<(usize, usize)> = e
        .keywords()
        .iter()
        .filter(|k| k.chars().count() >= 2 || **k == e.name.trim())
        .flat_map(|k| scan.match_indices(k.as_str()).map(|(i, m)| (i, i + m.len())).collect::<Vec<_>>())
        .collect();
    hits.sort_unstable();
    hits.into_iter()
        .find(|&(start, end)| {
            !REMAINS_AFTER_NAME.iter().any(|w| text[end..].starts_with(w))
                && !REMAINS_WORDS.iter().any(|w| crate::logic::sentence_around(text, start).contains(w))
        })
        .map(|(start, _)| crate::logic::sentence_at(text, start))
}

fn is_dead(state: &str) -> bool {
    DEATH_WORDS.iter().any(|w| state.contains(w))
}

/// 人物自己是死是活只看身体、实力、位置，并去掉引号里转述的话；「知道的秘密」「近期经历」和别人的议论里常写别人的死，不能算到他头上。
fn vital(fields: &std::collections::BTreeMap<String, String>, state: &str) -> String {
    let text = if fields.is_empty() { state.to_string() } else { ["body", "power", "location"].iter().filter_map(|k| fields.get(*k)).cloned().collect::<Vec<_>>().join("；") };
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '「' | '“' | '『' => depth += 1,
            '」' | '”' | '』' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn check(d: &BookData, current: Option<&Chapter>) -> Vec<Finding> {
    let mut out = Vec::new();
    let written: Vec<&Chapter> = d.chapters.iter().filter(|c| !c.content.trim().is_empty()).collect();
    let ref_num = current
        .and_then(|c| d.number(c))
        .or_else(|| written.iter().rev().find_map(|c| d.number(c)))
        .unwrap_or(0);

    // 伏笔
    for t in d.threads.iter().filter(|t| t.status == "open") {
        let planted = t.planted_chapter_id.and_then(|id| d.chapter(id)).and_then(|c| d.number(c));
        let last = t.last_chapter_id.and_then(|id| d.chapter(id)).and_then(|c| d.number(c)).or(planted);
        let mut f = if let Some(target) = t.target_chapter.filter(|tc| ref_num > *tc) {
            finding("critical", "伏笔逾期", format!("「{}」计划在第{}章回收，现在已经写到第{}章", t.title, target, ref_num))
        } else if let Some(gap) = last.map(|l| ref_num - l).filter(|g| *g >= DORMANT_THREAD_CHAPTERS) {
            finding("warn", "伏笔沉寂", format!("「{}」已经 {} 章没有推进，读者可能已经忘了", t.title, gap))
        } else if let Some(age) = planted.map(|p| ref_num - p).filter(|a| *a >= OLD_THREAD_CHAPTERS && t.target_chapter.is_none()) {
            finding("info", "伏笔未定回收", format!("「{}」埋下 {} 章了，可以给它定一个回收章节", t.title, age))
        } else {
            continue;
        };
        f.thread_id = Some(t.id);
        out.push(f);
    }

    // 人物
    for e in d.entries.iter().filter(|e| e.kind == "character") {
        if e.is_major() {
            let last_seen = written.iter().rposition(|c| mentions(e, &c.content));
            let gap = match last_seen {
                Some(i) => written.len() - 1 - i,
                None => written.len(),
            };
            let f = match last_seen {
                None if written.len() >= 3 => Some(finding("warn", "人物未出场", format!("{}「{}」到现在还没出场", if e.role.is_empty() { "主要人物" } else { e.role.as_str() }, e.name))),
                Some(_) if gap >= ABSENT_MAJOR_CHAPTERS => Some(finding("warn", "人物缺席", format!("「{}」已经 {} 章没出场了", e.name, gap))),
                _ => None,
            };
            if let Some(mut f) = f {
                f.entry_id = Some(e.id);
                out.push(f);
            }
            let missing: Vec<&str> = [("设定描述", e.description.trim().is_empty()), ("不可改变的特征", e.immutable.trim().is_empty())]
                .into_iter()
                .filter_map(|(label, empty)| empty.then_some(label))
                .collect();
            if !missing.is_empty() {
                let mut f = finding("info", "人物设定单薄", format!("「{}」缺少{}，AI 容易把人写崩", e.name, missing.join("和")));
                f.entry_id = Some(e.id);
                out.push(f);
            }
        }
        // 已死亡人物在之后的章节出现
        let death = d
            .states
            .iter()
            .filter(|s| s.entry_id == e.id && is_dead(&vital(&s.fields, &s.state)))
            .filter_map(|s| s.chapter_id.and_then(|cid| d.chapters.iter().position(|c| c.id == cid)))
            .min();
        let after: Vec<&Chapter> = match death {
            Some(pos) => d.chapters[pos + 1..].iter().collect(),
            None if is_dead(&vital(&e.fields, &e.state)) => current.into_iter().collect(),
            None => Vec::new(),
        };
        if let Some((c, quote)) = after.into_iter().find_map(|c| appearance(e, &c.content).map(|q| (c, q))) {
            let mut f = finding("warn", "已死亡人物出场", format!("「{}」已经死亡，但在{}出现了，请确认是回忆、尸体还是复活", e.name, d.label(c)));
            f.entry_id = Some(e.id);
            f.chapter_id = Some(c.id);
            f.quote = quote;
            out.push(f);
        }
    }

    // 章节
    let last_written = d.chapters.iter().rposition(|c| !c.content.trim().is_empty());
    for (i, c) in d.chapters.iter().enumerate() {
        let f = if c.status == "done" && c.summary.trim().is_empty() && !c.content.trim().is_empty() {
            Some(finding("warn", "缺少摘要", format!("{}已定稿但没有摘要，后面的章节看不到它的前情", d.label(c))))
        } else if c.content.trim().is_empty() && last_written.map(|l| i < l).unwrap_or(false) {
            Some(finding("warn", "空章节", format!("{}正文为空，但后面的章节已经写了", d.label(c))))
        } else {
            None
        };
        if let Some(mut f) = f {
            f.chapter_id = Some(c.id);
            out.push(f);
        }
    }

    // 节奏（需要定稿时做过张力分析）
    let measured: Vec<(&Chapter, &serde_json::Value)> = d.chapters.iter().filter(|c| c.metrics.is_object()).map(|c| (c, &c.metrics)).collect();
    let mut streak: Vec<&Chapter> = Vec::new();
    let flush_low = |streak: &mut Vec<&Chapter>, out: &mut Vec<Finding>| {
        if streak.len() >= LOW_TENSION_STREAK {
            let mut f = finding("warn", "张力偏低", format!("{} 到 {} 连续 {} 章张力偏低，读者容易流失", d.label(streak[0]), d.label(streak[streak.len() - 1]), streak.len()));
            f.chapter_id = Some(streak[streak.len() - 1].id);
            out.push(f);
        }
        streak.clear();
    };
    for (c, m) in &measured {
        match m["tension"].as_f64() {
            Some(t) if t <= LOW_TENSION => streak.push(c),
            _ => flush_low(&mut streak, &mut out),
        }
    }
    flush_low(&mut streak, &mut out);
    let mut dry: Vec<&Chapter> = Vec::new();
    let flush_dry = |dry: &mut Vec<&Chapter>, out: &mut Vec<Finding>| {
        if dry.len() >= NO_COOL_POINT_STREAK {
            let mut f = finding("warn", "爽点断档", format!("{} 到 {} 连续 {} 章没有爽点", d.label(dry[0]), d.label(dry[dry.len() - 1]), dry.len()));
            f.chapter_id = Some(dry[dry.len() - 1].id);
            out.push(f);
        }
        dry.clear();
    };
    for (c, m) in &measured {
        if m["cool_points"].as_array().map(|a| a.is_empty()).unwrap_or(false) {
            dry.push(c);
        } else {
            flush_dry(&mut dry, &mut out);
        }
    }
    flush_dry(&mut dry, &mut out);

    out.extend(crate::logic::check(d));
    out.extend(crate::timeline::check(d));

    let rank = |l: &str| match l {
        "critical" => 0,
        "warn" => 1,
        _ => 2,
    };
    out.sort_by_key(|f| rank(&f.level));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ch(id: i64, content: &str) -> Chapter {
        Chapter { id, book_id: 1, sort: id, title: format!("章{id}"), content: content.into(), ..Default::default() }
    }

    #[test]
    fn finds_continuity_problems() {
        let mut chapters: Vec<Chapter> = (1..=8).map(|i| ch(i, if i == 1 { "林凡和苏雨出发。" } else { "林凡继续赶路。" })).collect();
        chapters[2].status = "done".into();
        chapters[3].content.clear();
        for c in chapters.iter_mut().skip(4) {
            c.metrics = json!({ "tension": 3, "cool_points": [] });
        }
        chapters[7].content = "张三忽然站了起来。".into();
        let entries = vec![
            Entry { id: 1, kind: "character".into(), name: "林凡".into(), role: "主角".into(), description: "少年".into(), immutable: "重情义".into(), ..Default::default() },
            Entry { id: 2, kind: "character".into(), name: "苏雨".into(), role: "重要配角".into(), ..Default::default() },
            Entry { id: 3, kind: "character".into(), name: "张三".into(), ..Default::default() },
        ];
        let knows_death = [
            ("knows".to_string(), "知道张三已死亡".to_string()),
            ("body".to_string(), "无伤".to_string()),
            ("power".to_string(), "炼气五层（旁人议论他「死了师父，境界跌了」）".to_string()),
        ]
        .into_iter()
        .collect();
        let states = vec![
            EntryState { id: 1, entry_id: 3, chapter_id: Some(2), phase: "end".into(), state: "被林凡所杀，已死亡".into(), ..Default::default() },
            EntryState { id: 2, entry_id: 1, chapter_id: Some(2), phase: "end".into(), fields: knows_death, ..Default::default() },
        ];
        let threads = vec![
            Thread { id: 1, title: "玉佩".into(), status: "open".into(), planted_chapter_id: Some(1), target_chapter: Some(5), ..Default::default() },
            Thread { id: 2, title: "身世".into(), status: "resolved".into(), ..Default::default() },
        ];
        let d = BookData::new(Book::default(), vec![], chapters, entries, threads, states);
        let found = check(&d, None);
        let kinds: Vec<&str> = found.iter().map(|f| f.kind.as_str()).collect();
        assert_eq!(found[0].kind, "伏笔逾期");
        assert!(kinds.contains(&"人物缺席"), "{kinds:?}");
        assert!(kinds.contains(&"人物设定单薄"));
        assert!(kinds.contains(&"已死亡人物出场"));
        assert!(kinds.contains(&"缺少摘要"));
        assert!(kinds.contains(&"空章节"));
        assert!(kinds.contains(&"张力偏低"));
        assert!(!kinds.contains(&"爽点断档"), "只有 4 章没有爽点，不到阈值");
        assert!(found.iter().all(|f| !f.message.contains("林凡」已经")), "主角一直在出场");
    }

    #[test]
    fn dead_character_remains_are_not_appearances() {
        let person = |id: i64, name: &str| Entry { id, kind: "character".into(), name: name.into(), ..Default::default() };
        let entries = vec![person(1, "徐平安"), person(2, "赵黑虎"), person(3, "王奎"), person(4, "赵陵")];
        // 《大梦长生》第 3、5、9 章里的原句
        let chapters = vec![
            ch(1, "王奎、赵陵、徐平安、赵黑虎都在矿上。"),
            ch(2, "镜面飞快地转，光斑扫过刑架上的徐平安和地上赵黑虎的尸首，又停在青黑巨石那道刺眼的断轨剑印上。"),
            ch(3, "又从王奎那包避毒草叶里抽出几片，塞进回旋的风眼当作气闸。刀上还沾着赵陵的血，没有干透。赵陵死了，赵崇山跌了半境。"),
            ch(4, "头顶上，赵陵心口迸出的那道血柱直贯夜空。"),
        ];
        let states = (1..=4).map(|id| EntryState { id, entry_id: id, chapter_id: Some(1), phase: "end".into(), state: "已死亡".into(), ..Default::default() }).collect();
        let d = BookData::new(Book::default(), vec![], chapters, entries, vec![], states);
        let dead: Vec<Finding> = check(&d, None).into_iter().filter(|f| f.kind == "已死亡人物出场").collect();
        assert_eq!(dead.len(), 1, "尸首、遗物、血和死讯都不算出场：{dead:?}");
        assert_eq!((dead[0].entry_id, dead[0].chapter_id), (Some(4), Some(4)));
        assert_eq!(dead[0].quote, "头顶上，赵陵心口迸出的那道血柱直贯夜空。");
    }
}
