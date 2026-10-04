//! 文风库：作者收藏的范文切成片段建索引，写正文时按场景检索几段作为参考；
//! 另外从“AI 原文 → 作者改后”里提取修改样本，供提炼文风指南。全部本地计算，不调用模型。

use crate::models::LibChunk;
use crate::text::{clip, tokenize};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// 片段标签，AI 分析时也只从这些里选
pub const TAGS: &[&str] = &["开篇", "章末钩子", "对话", "打斗", "环境", "心理", "感情", "爽点", "悬念", "日常", "转场", "人物描写", "搞笑", "群像"];

const TARGET: usize = 450;
const MAX: usize = 800;

fn is_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '…' | '；')
}

fn is_close(c: char) -> bool {
    matches!(c, '”' | '’' | '」' | '』' | '"')
}

fn split_sentences(p: &str) -> Vec<String> {
    let chars: Vec<char> = p.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        let next = chars.get(i + 1).copied();
        let next_continues = next.is_some_and(|n| is_end(n) || is_close(n));
        let ends = (is_end(c) || (is_close(c) && i > 0 && is_end(chars[i - 1]))) && !next_continues;
        if ends {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 按段落切片：每片 450 字左右，不拆开段落；超长段落按句子拆。
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    for line in text.lines() {
        let p = line.trim().trim_start_matches('\u{3000}').trim();
        if p.is_empty() {
            continue;
        }
        if p.chars().count() <= MAX {
            pieces.push(p.to_string());
            continue;
        }
        let mut cur = String::new();
        for s in split_sentences(p) {
            if !cur.is_empty() && cur.chars().count() + s.chars().count() > TARGET {
                pieces.push(std::mem::take(&mut cur));
            }
            cur.push_str(&s);
        }
        if !cur.is_empty() {
            pieces.push(cur);
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut len = 0;
    for p in pieces {
        let n = p.chars().count();
        if len > 0 && len + n > MAX {
            out.push(cur.join("\n"));
            cur.clear();
            len = 0;
        }
        cur.push(p);
        len += n;
        if len >= TARGET {
            out.push(cur.join("\n"));
            cur.clear();
            len = 0;
        }
    }
    if !cur.is_empty() {
        let tail = cur.join("\n");
        match out.last_mut() {
            Some(last) if len < TARGET / 3 && last.chars().count() + len <= MAX => {
                last.push('\n');
                last.push_str(&tail);
            }
            _ => out.push(tail),
        }
    }
    out
}

/// 引号里的字占全文的比例
pub fn dialogue_ratio(text: &str) -> f32 {
    let mut depth = 0i32;
    let (mut inside, mut total) = (0usize, 0usize);
    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        match c {
            '“' | '「' | '『' => depth += 1,
            '”' | '」' | '』' => depth = (depth - 1).max(0),
            _ if depth > 0 => inside += 1,
            _ => {}
        }
    }
    if total == 0 { 0.0 } else { inside as f32 / total as f32 }
}

const FIGHT: &[&str] = &["拳", "剑", "刀", "掌", "轰", "砰", "斩", "劈", "杀", "血", "招", "攻", "闪", "挡", "踢", "撞", "枪", "爆"];
const SCENERY: &[&str] = &["风", "雨", "雪", "月", "山", "云", "树", "天色", "夜色", "阳光", "街", "灯", "河", "湖", "雾", "花", "屋檐", "石阶"];
const INNER: &[&str] = &["心想", "心里", "心中", "念头", "觉得", "意识到", "想起", "明白", "犹豫", "后悔", "害怕", "不甘"];
const ROMANCE: &[&str] = &["脸红", "心跳", "温柔", "拥抱", "抱住", "吻", "耳根", "害羞", "眼眶", "喜欢"];
const COOL: &[&str] = &["震惊", "不可能", "目瞪口呆", "倒吸", "跪", "全场", "哗然", "脸色大变", "怎么可能", "秒杀", "打脸"];
const SUSPENSE: &[&str] = &["突然", "竟然", "到底", "秘密", "为什么", "不对劲", "怎么会", "消失", "诡异"];
const FUNNY: &[&str] = &["哈哈", "噗", "笑死", "白眼", "无语", "憋笑", "吐槽"];

/// 按关键词密度粗略打场景标签，AI 分析后会被更准确的标签替换。
pub fn auto_tags(text: &str) -> Vec<String> {
    let n = text.chars().filter(|c| !c.is_whitespace()).count().max(1) as f32;
    let per_k = |words: &[&str]| words.iter().map(|w| text.matches(w).count()).sum::<usize>() as f32 * 1000.0 / n;
    let dialogue = dialogue_ratio(text);
    let mut tags: Vec<(&str, f32)> = Vec::new();
    if dialogue > 0.3 {
        tags.push(("对话", dialogue * 20.0));
    }
    for (tag, words, threshold) in [("打斗", FIGHT, 8.0), ("心理", INNER, 5.0), ("感情", ROMANCE, 3.0), ("爽点", COOL, 3.0), ("悬念", SUSPENSE, 4.0), ("搞笑", FUNNY, 3.0)] {
        let d = per_k(words);
        if d >= threshold {
            tags.push((tag, d / threshold));
        }
    }
    let scenery = per_k(SCENERY);
    if scenery >= 8.0 && dialogue < 0.2 {
        tags.push(("环境", scenery / 8.0));
    }
    tags.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    tags.into_iter().take(3).map(|(t, _)| t.to_string()).collect()
}

/// 从作者的要求里猜想要的场景标签，如“多写对话”→ 对话
pub fn tags_for(instruction: &str) -> Vec<String> {
    const MAP: &[(&[&str], &str)] = &[
        (&["对话", "对白"], "对话"),
        (&["打斗", "战斗", "打架", "动作", "交手"], "打斗"),
        (&["环境", "景色", "氛围", "场景描写"], "环境"),
        (&["心理", "内心", "独白"], "心理"),
        (&["感情", "暧昧", "甜", "恋爱"], "感情"),
        (&["爽", "打脸", "装逼", "高潮"], "爽点"),
        (&["悬念", "悬疑"], "悬念"),
        (&["钩子", "章末", "结尾"], "章末钩子"),
        (&["开篇", "开头"], "开篇"),
        (&["幽默", "搞笑", "轻松"], "搞笑"),
        (&["日常"], "日常"),
        (&["转场", "过渡"], "转场"),
        (&["外貌", "神态", "人物描写"], "人物描写"),
    ];
    let mut out: Vec<String> = Vec::new();
    for (words, tag) in MAP {
        if words.iter().any(|w| instruction.contains(w)) && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// 切片并打上自动标签；按章节导入的范文，最后一片标为章末钩子
pub fn make_chunks(content: &str, chapter_like: bool) -> Vec<(String, Vec<String>)> {
    let pieces = chunk_text(content);
    let last = pieces.len().saturating_sub(1);
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mut tags = auto_tags(&text);
            if chapter_like && i == last && i > 0 && !tags.iter().any(|t| t == "章末钩子") {
                tags.push("章末钩子".into());
            }
            (text, tags)
        })
        .collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

struct Doc {
    chunk: LibChunk,
    title: String,
    genre: String,
    tf: HashMap<String, f32>,
    len: f32,
}

/// 内存里的检索索引，文风库改动后按版本号重建
pub struct LibIndex {
    pub rev: u64,
    docs: Vec<Doc>,
    df: HashMap<String, f32>,
    avg_len: f32,
}

pub struct LibQuery<'a> {
    pub text: &'a str,
    pub tags: &'a [String],
    pub genre: &'a str,
    /// (查询向量, 向量模型名)，只和同一模型生成的片段向量比较
    pub vector: Option<(&'a [f32], &'a str)>,
    pub k: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub chunk_id: i64,
    pub item_id: i64,
    pub title: String,
    pub text: String,
    pub tags: Vec<String>,
    pub score: f32,
    /// 为什么选中它，给检索测试看
    pub why: Vec<String>,
}

impl LibIndex {
    /// rows：(片段, 范文标题, 范文题材)
    pub fn build(rev: u64, rows: Vec<(LibChunk, String, String)>) -> Self {
        let mut df: HashMap<String, f32> = HashMap::new();
        let mut docs = Vec::with_capacity(rows.len());
        let mut total = 0.0;
        for (chunk, title, genre) in rows {
            let mut tf: HashMap<String, f32> = HashMap::new();
            for t in tokenize(&chunk.text).into_iter().filter(|t| t.chars().count() >= 2) {
                *tf.entry(t).or_default() += 1.0;
            }
            let len: f32 = tf.values().sum();
            for k in tf.keys() {
                *df.entry(k.clone()).or_default() += 1.0;
            }
            total += len;
            docs.push(Doc { chunk, title, genre, tf, len });
        }
        let avg_len = if docs.is_empty() { 1.0 } else { (total / docs.len() as f32).max(1.0) };
        Self { rev, docs, df, avg_len }
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn has_vectors(&self, model: &str) -> bool {
        self.docs.iter().any(|d| d.chunk.embedding.is_some() && d.chunk.emb_model == model)
    }

    /// 关键词（BM25）+ 标签 + 题材 + 向量（可选）打分，每篇范文最多取一段。
    pub fn search(&self, q: &LibQuery) -> Vec<Hit> {
        if self.docs.is_empty() || q.k == 0 {
            return Vec::new();
        }
        let n = self.docs.len() as f32;
        let terms: HashSet<String> = tokenize(q.text).into_iter().filter(|t| t.chars().count() >= 2).collect();
        let (k1, b) = (1.2f32, 0.75f32);
        let bm: Vec<f32> = self
            .docs
            .iter()
            .map(|d| {
                terms
                    .iter()
                    .map(|t| {
                        let f = d.tf.get(t).copied().unwrap_or(0.0);
                        if f == 0.0 {
                            return 0.0;
                        }
                        let df = self.df.get(t).copied().unwrap_or(0.0);
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        idf * f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * d.len / self.avg_len))
                    })
                    .sum()
            })
            .collect();
        let bm_max = bm.iter().copied().fold(0.0, f32::max);
        let cos: Vec<f32> = self
            .docs
            .iter()
            .map(|d| match (q.vector, &d.chunk.embedding) {
                (Some((v, model)), Some(e)) if d.chunk.emb_model == model => cosine(v, e).max(0.0),
                _ => 0.0,
            })
            .collect();
        let cos_max = cos.iter().copied().fold(0.0, f32::max);
        let genre = q.genre.trim();
        let mut scored: Vec<(f32, usize, Vec<String>)> = self
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut why = Vec::new();
                let mut s = 0.0;
                if bm_max > 0.0 && bm[i] > 0.0 {
                    s += bm[i] / bm_max;
                    why.push("内容相近".to_string());
                }
                let matched: Vec<&str> = q.tags.iter().filter(|t| d.chunk.tags.contains(t)).map(|t| t.as_str()).collect();
                if !matched.is_empty() {
                    s += 0.5 * matched.len().min(2) as f32;
                    why.push(format!("标签：{}", matched.join("、")));
                }
                let dg = d.genre.trim();
                if !genre.is_empty() && !dg.is_empty() && (dg.contains(genre) || genre.contains(dg)) {
                    s += 0.3;
                    why.push("同题材".to_string());
                }
                if cos_max > 0.0 && cos[i] > 0.0 {
                    s += cos[i] / cos_max;
                    why.push(format!("语义相似 {:.2}", cos[i]));
                }
                (s, i, why)
            })
            .collect();
        // 分数相同时，新收藏的范文优先，同一篇里靠前的片段优先
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then(self.docs[b.1].chunk.item_id.cmp(&self.docs[a.1].chunk.item_id))
                .then(self.docs[a.1].chunk.seq.cmp(&self.docs[b.1].chunk.seq))
        });
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (score, i, why) in scored {
            let d = &self.docs[i];
            if !seen.insert(d.chunk.item_id) {
                continue;
            }
            out.push(Hit {
                chunk_id: d.chunk.id,
                item_id: d.chunk.item_id,
                title: d.title.clone(),
                text: d.chunk.text.clone(),
                tags: d.chunk.tags.clone(),
                score,
                why,
            });
            if out.len() >= q.k {
                break;
            }
        }
        out
    }
}

/// 放进写作提示词的范文参考
pub fn refs_block(hits: &[Hit]) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut s = String::from("【范文参考：作者收藏的片段。学习它们的节奏、句式、描写和对话方式；不要照抄原句，也不要搬用其中的人名、设定和情节】");
    for (i, h) in hits.iter().enumerate() {
        let tags = if h.tags.is_empty() { String::new() } else { format!("·{}", h.tags.join("、")) };
        s += &format!("\n〔片段{}{}〕\n{}", i + 1, tags, clip(&h.text, 600));
    }
    s
}

pub fn guide_block(guide: &str) -> String {
    if guide.trim().is_empty() {
        return String::new();
    }
    format!("【作者的文风指南（从作者收藏的范文和修改习惯里提炼，和上面的通用规则冲突时以这里为准）】\n{}", clip(guide, 2500))
}

/// 从一段采纳的 AI 文字和它所在章节的现稿里找出作者的修改
#[derive(Debug, Default, Clone, Serialize)]
pub struct Feedback {
    /// (AI 原文, 作者改后)
    pub changed: Vec<(String, String)>,
    /// 被作者整句删掉的 AI 文字
    pub deleted: Vec<String>,
}

fn bigrams(s: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

fn dice(a: &str, b: &str) -> f32 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    2.0 * x.intersection(&y).count() as f32 / (x.len() + y.len()) as f32
}

fn sentences(text: &str) -> Vec<String> {
    text.lines().flat_map(split_sentences).map(|s| s.trim().to_string()).filter(|s| s.chars().count() >= 4).collect()
}

/// 对比采纳的 AI 文字和章节现稿。before/after 是采纳时插入点前后的原文，用来在现稿里框出这段文字的位置；
/// 框内再以没改动的句子为锚点分段：两头都有锚点的段落直接成对，开放的段落按相似度配对，找不到的算作删掉。
pub fn feedback(accepted: &str, before: &str, after: &str, final_text: &str) -> Feedback {
    let mut start = 0;
    let mut end = final_text.len();
    let mut start_fixed = false;
    let mut end_fixed = false;
    let before = before.trim();
    if !before.is_empty() {
        if let Some(p) = final_text.find(before) {
            start = p + before.len();
            start_fixed = true;
        }
    }
    let after = after.trim();
    if !after.is_empty() {
        if let Some(p) = final_text[start..].find(after) {
            end = start + p;
            end_fixed = true;
        }
    }
    let ai = sentences(accepted);
    let fin = sentences(&final_text[start..end]);
    // 原样保留下来的句子作为锚点：(AI 句序号, 现稿句序号)
    let mut anchors: Vec<(usize, usize)> = Vec::new();
    let mut next = 0;
    for (i, a) in ai.iter().enumerate() {
        if let Some(j) = (next..fin.len()).find(|&j| fin[j].contains(a.as_str())) {
            anchors.push((i, j));
            next = j + 1;
        }
    }
    let mut out = Feedback::default();
    let mut gaps: Vec<(std::ops::Range<usize>, std::ops::Range<usize>, bool, bool)> = Vec::new();
    let mut prev: Option<(usize, usize)> = None;
    for &(i, j) in anchors.iter().chain(std::iter::once(&(ai.len(), fin.len()))) {
        let (ai_from, fin_from) = prev.map(|(pi, pj)| (pi + 1, pj + 1)).unwrap_or((0, 0));
        let closed_start = prev.is_some() || start_fixed;
        let closed_end = i < ai.len() || end_fixed;
        if ai_from < i {
            gaps.push((ai_from..i, fin_from..j.max(fin_from), closed_start, closed_end));
        }
        prev = Some((i, j));
    }
    for (a_range, f_range, closed_start, closed_end) in gaps {
        let a_text = ai[a_range.clone()].join("");
        if closed_start && closed_end {
            let f_text = fin[f_range].join("");
            if f_text.is_empty() {
                out.deleted.extend(ai[a_range].iter().cloned());
            } else if dice(&a_text, &f_text) < 0.98 {
                out.changed.push((a_text, f_text));
            }
            continue;
        }
        // 只有一头确定的段落：按位置取紧挨着确定那头的同样句数，作者在这个位置写了别的就算改写
        if closed_start || closed_end {
            let n = a_range.len();
            let f = &fin[f_range];
            let picked = if closed_start { &f[..n.min(f.len())] } else { &f[f.len().saturating_sub(n)..] };
            let f_text = picked.join("");
            if f_text.is_empty() {
                out.deleted.extend(ai[a_range].iter().cloned());
            } else if dice(&a_text, &f_text) < 0.98 {
                out.changed.push((a_text, f_text));
            }
            continue;
        }
        // 完全没有锚点：按相似度配对
        let candidates: Vec<&String> = fin[f_range].iter().collect();
        for a in &ai[a_range] {
            let best = candidates.iter().map(|f| (dice(a, f), *f)).max_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(Ordering::Equal));
            match best {
                Some((d, _)) if d >= 0.98 => {}
                Some((d, f)) if d >= 0.35 => out.changed.push((a.clone(), f.clone())),
                _ => out.deleted.push(a.clone()),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: i64, item_id: i64, text: &str, tags: &[&str]) -> LibChunk {
        LibChunk { id, item_id, seq: 0, text: text.into(), tags: tags.iter().map(|s| s.to_string()).collect(), embedding: None, emb_model: String::new() }
    }

    #[test]
    fn chunks_keep_paragraphs() {
        let para = "他推门进去，屋里没有点灯。".repeat(10);
        let text = (0..12).map(|_| para.clone()).collect::<Vec<_>>().join("\n\n");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        for c in &chunks {
            let n = c.chars().count();
            assert!(n <= MAX + TARGET / 3, "片段太长：{n}");
            assert!(c.lines().all(|l| l == para), "段落被拆开了");
        }
        let long = "这是一句很长的话。".repeat(150);
        let pieces = chunk_text(&long);
        assert!(pieces.len() >= 2 && pieces.iter().all(|p| p.ends_with('。')));
        assert!(chunk_text("  \n\n ").is_empty());
    }

    #[test]
    fn tags_from_density() {
        let talk = "“你来了？”“来了。”“吃了吗？”“还没有，等你一起。”“那走吧。”";
        assert_eq!(auto_tags(talk).first().map(String::as_str), Some("对话"));
        let fight = "他一拳轰出，对方横剑去挡，剑身被砸得嗡嗡作响。第二拳紧跟着砸在胸口，血从嘴角溢出来，他借势一脚踢在对方膝弯。";
        assert!(auto_tags(fight).contains(&"打斗".to_string()), "{:?}", auto_tags(fight));
        assert_eq!(tags_for("多写对话，章末留钩子"), vec!["对话", "章末钩子"]);
        let made = make_chunks(&format!("{}\n{}", "第一段。".repeat(130), "最后一段。".repeat(130)), true);
        assert!(made.len() >= 2 && made.last().unwrap().1.contains(&"章末钩子".to_string()));
    }

    #[test]
    fn search_uses_tags_genre_and_diversity() {
        let rows = vec![
            (chunk(1, 1, "剑光一闪，宗门大比的擂台上血花飞溅。", &["打斗"]), "甲".to_string(), "玄幻".to_string()),
            (chunk(2, 1, "剑气纵横，宗门弟子纷纷后退。", &["打斗"]), "甲".to_string(), "玄幻".to_string()),
            (chunk(3, 2, "咖啡馆里，她低头搅着杯子，没有说话。", &["对话"]), "乙".to_string(), "都市".to_string()),
            (chunk(4, 3, "雨下了一夜，山门前的石阶湿漉漉的。", &["环境"]), "丙".to_string(), "玄幻".to_string()),
        ];
        let ix = LibIndex::build(1, rows);
        let tags = vec!["对话".to_string()];
        let hits = ix.search(&LibQuery { text: "宗门擂台上剑光", tags: &[], genre: "玄幻", vector: None, k: 3 });
        assert_eq!(hits[0].item_id, 1);
        assert_eq!(hits.iter().filter(|h| h.item_id == 1).count(), 1, "同一篇范文只取一段");
        assert!(hits[0].why.iter().any(|w| w == "内容相近") && hits[0].why.iter().any(|w| w == "同题材"));
        let hits = ix.search(&LibQuery { text: "", tags: &tags, genre: "", vector: None, k: 1 });
        assert_eq!(hits[0].item_id, 2);
        assert!(ix.search(&LibQuery { text: "随便", tags: &[], genre: "", vector: None, k: 0 }).is_empty());
    }

    #[test]
    fn search_with_vectors() {
        let mut a = chunk(1, 1, "甲", &[]);
        a.embedding = Some(vec![1.0, 0.0]);
        a.emb_model = "m".into();
        let mut b = chunk(2, 2, "乙", &[]);
        b.embedding = Some(vec![0.0, 1.0]);
        b.emb_model = "m".into();
        let ix = LibIndex::build(1, vec![(a, "甲".into(), String::new()), (b, "乙".into(), String::new())]);
        assert!(ix.has_vectors("m") && !ix.has_vectors("other"));
        let q = [0.1f32, 0.9];
        let hits = ix.search(&LibQuery { text: "", tags: &[], genre: "", vector: Some((&q, "m")), k: 1 });
        assert_eq!(hits[0].item_id, 2);
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn finds_edits_and_deletions() {
        let ai = "他缓缓地抬起头，眼中闪过一丝复杂的光芒。\n门外传来脚步声，越来越近。\n这一刻，他终于明白了命运的意义。";
        let fin = "林凡推开门，屋里很暗。\n他抬起头，盯着她看了很久。\n门外传来脚步声，越来越近。\n后来作者自己写的一段，和 AI 无关。";
        let f = feedback(ai, "林凡推开门，屋里很暗。", "后来作者自己写的", fin);
        assert_eq!(f.changed, vec![("他缓缓地抬起头，眼中闪过一丝复杂的光芒。".to_string(), "他抬起头，盯着她看了很久。".to_string())], "{f:?}");
        assert_eq!(f.deleted, vec!["这一刻，他终于明白了命运的意义。"]);

        // 没有定位锚点时，开放的段落按相似度配对，前面的旧文不会被误配
        let f = feedback("门外传来脚步声，越来越近。\n她转身就跑，头也不回地冲进雨里。", "", "", "很早以前写的一句话，毫不相干。\n门外传来脚步声，越来越近。\n她转身就跑，头也没回，冲进了雨里。");
        assert_eq!(f.changed.len(), 1, "{f:?}");
        assert!(f.deleted.is_empty());

        let f = feedback("完全没有改动的一句话。", "前文。", "后文", "前文。\n完全没有改动的一句话。\n后文。");
        assert!(f.changed.is_empty() && f.deleted.is_empty());

        // 插在末尾的 AI 文字：最后一句被作者换成了自己的话，按位置算改写，而不是删除
        let ai = "凉得像被人攥着扔进冰水里。\n林凡低头按了按胸口，抬起眼：“管事说亲眼看见——是哪个时辰？”";
        let fin = "黑玉忽然一凉，凉得像被人攥着扔进冰水里。\n林凡抬起头，直直看着王管事：“你再说一遍？”";
        let f = feedback(ai, "黑玉忽然一凉，", "", fin);
        assert_eq!(f.changed, vec![("林凡低头按了按胸口，抬起眼：“管事说亲眼看见——是哪个时辰？”".to_string(), "林凡抬起头，直直看着王管事：“你再说一遍？”".to_string())], "{f:?}");
        assert!(f.deleted.is_empty());

        // 完全找不到位置时，不相干的句子不会被硬配成一对
        let f = feedback("他转身走进了雨里。", "", "", "完全无关的另一段文字，讲的是别的事。");
        assert!(f.changed.is_empty() && f.deleted.len() == 1, "{f:?}");
    }

    #[test]
    fn blocks() {
        assert!(refs_block(&[]).is_empty() && guide_block("  ").is_empty());
        let hit = Hit { chunk_id: 1, item_id: 1, title: "甲".into(), text: "正文".into(), tags: vec!["对话".into()], score: 1.0, why: vec![] };
        let block = refs_block(&[hit]);
        assert!(block.contains("〔片段1·对话〕\n正文") && block.contains("不要照抄"));
    }
}
