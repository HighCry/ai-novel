//! 文字质量检查：套话、句式重复、重复表达、排版和标点。全部本地计算，不调用模型。

use crate::text::{is_cjk, paragraphs};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize)]
pub struct LintIssue {
    pub kind: String,
    pub severity: String,
    pub text: String,
    pub count: usize,
    /// UTF-16 偏移（和浏览器 textarea 的选区下标一致）
    pub positions: Vec<[usize; 2]>,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LintStats {
    pub chars: usize,
    pub paragraphs: usize,
    pub sentences: usize,
    pub dialogue_ratio: f32,
    pub avg_sentence: f32,
    pub sentence_stdev: f32,
    pub long_paragraphs: usize,
    pub max_paragraph: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintReport {
    pub score: i32,
    pub stats: LintStats,
    pub issues: Vec<LintIssue>,
}

struct Rule {
    pattern: &'static str,
    kind: &'static str,
    /// 每 3000 字允许出现的次数，超过才提示
    allow: usize,
    suggestion: &'static str,
}

const RULES: &[Rule] = &[
    Rule { pattern: r"嘴角(?:微微)?(?:勾起|上扬|扬起)(?:一抹|一丝)?(?:\S{0,6}?的)?(?:弧度|笑意|笑容)?", kind: "套话", allow: 0, suggestion: "换成具体的表情或动作，或者直接删掉" },
    Rule { pattern: r"(?:眼中|眼里|眸中|眼底|眸子里)(?:闪过|划过|掠过)一(?:丝|抹)", kind: "套话", allow: 0, suggestion: "用具体的反应代替：一个动作、一次停顿或一句台词" },
    Rule { pattern: r"一(?:抹|丝)(?:不易察觉|难以察觉)的", kind: "套话", allow: 0, suggestion: "删掉修饰，直接写动作" },
    Rule { pattern: r"不禁", kind: "AI腔", allow: 1, suggestion: "大多可以直接删掉" },
    Rule { pattern: r"仿佛|宛如|犹如|宛若", kind: "比喻过多", allow: 3, suggestion: "比喻控制数量，优先写实" },
    Rule { pattern: r"心中暗(?:道|想|忖)|心中一(?:动|凛|震|紧)|心头一(?:震|紧|颤)", kind: "套话", allow: 1, suggestion: "用动作或对话表现心理" },
    Rule { pattern: r"一股莫名的|一种前所未有的|一种难以言喻的", kind: "AI腔", allow: 0, suggestion: "写清楚具体是什么感受" },
    Rule { pattern: r"空气(?:仿佛|似乎)?(?:都)?(?:凝固|安静)了", kind: "套话", allow: 0, suggestion: "写具体的安静：谁停下了动作、哪种声音消失了" },
    Rule { pattern: r"深吸了?一口气|倒吸了?一口(?:凉气|冷气)", kind: "套话", allow: 1, suggestion: "一章里出现多次会显得模板化" },
    Rule { pattern: r"这一刻|那一刻|就在这时|与此同时", kind: "AI腔", allow: 2, suggestion: "转场词太多，可以直接切场景" },
    Rule { pattern: r"命运的齿轮|齿轮开始转动|故事才刚刚开始|一切才刚刚开始", kind: "升华套话", allow: 0, suggestion: "删掉，章末用具体的悬念代替" },
    Rule { pattern: r"毫无疑问|不可否认|值得一提的是|总而言之|总的来说|综上所述", kind: "AI腔", allow: 0, suggestion: "说明文用语，不适合小说正文" },
    Rule { pattern: r"五味杂陈|百感交集|难以言喻|无法用言语(?:来)?形容", kind: "AI腔", allow: 0, suggestion: "把感受拆成具体的动作和念头" },
    Rule { pattern: r"(?:他|她|我)知道，", kind: "AI腔", allow: 1, suggestion: "解释性的心理句，常常可以删去" },
    Rule { pattern: r"然而，", kind: "AI腔", allow: 2, suggestion: "转折词偏多，口语化的表达更自然" },
    Rule { pattern: r"瞳孔(?:猛地|骤然|微微)?(?:一缩|收缩|地震)", kind: "套话", allow: 1, suggestion: "网文高频套话，注意频率" },
    Rule { pattern: r"(?:沉声|冷声|淡淡|缓缓|轻声)(?:地)?(?:道|说|开口)", kind: "对话标签", allow: 4, suggestion: "对话标签重复，可以用动作代替或直接省略" },
    Rule { pattern: r"冷笑一声|冷哼一声", kind: "套话", allow: 2, suggestion: "注意频率" },
    Rule { pattern: r"意味深长(?:地|的)", kind: "套话", allow: 1, suggestion: "写出具体的意味" },
    // 以下规则改编自 novel-studio《去AI味完整指南》（Apache-2.0）
    Rule { pattern: r"映入眼帘|目光如炬|此时此刻", kind: "套话", allow: 0, suggestion: "删掉，直接写看到了什么" },
    Rule { pattern: r"脸色一变|不由自主", kind: "套话", allow: 1, suggestion: "用具体的表情或动作代替" },
    Rule { pattern: r"意义深远|具有重大意义|可谓", kind: "意义膨胀", allow: 0, suggestion: "写具体后果，不要替读者拔高" },
    Rule { pattern: r"未来可期|前途无量|充满了?希望", kind: "万能结论", allow: 0, suggestion: "用未解决的紧张感或具体的下一步动作收尾" },
    Rule { pattern: r"不难看出|由此可见|事实上", kind: "论文体", allow: 0, suggestion: "说明文用语，小说里直接删" },
    Rule { pattern: r"于是乎|从而|因而|诚然", kind: "书面连词", allow: 1, suggestion: "口语化，或者直接删掉连接词" },
    Rule {
        pattern: r"殊不知|(?:她|他|我|他们|她们)不知道的是|冥冥之中|仿佛预示着|之所以.{0,20}?是因为|这意味着",
        kind: "解释腔",
        allow: 0,
        suggestion: "上帝视角在解释或剧透，删掉，让读者从动作和对话里自己拼",
    },
    Rule { pattern: r"恰到好处|恰如其分|不多不少", kind: "解释腔", allow: 0, suggestion: "作者在替读者下判断，只写动作，判断留给读者" },
];

const WEAK_ADVERBS: &str = r"微微|淡淡|缓缓|轻轻";

fn compiled() -> &'static Vec<(Regex, &'static Rule)> {
    static C: OnceLock<Vec<(Regex, &'static Rule)>> = OnceLock::new();
    C.get_or_init(|| RULES.iter().map(|r| (Regex::new(r.pattern).expect(r.pattern), r)).collect())
}

/// 字节下标 → UTF-16 下标
struct Utf16Map(Vec<usize>);

impl Utf16Map {
    fn new(text: &str) -> Self {
        let mut map = vec![0; text.len() + 1];
        let mut u = 0;
        for (i, c) in text.char_indices() {
            map[i] = u;
            u += c.len_utf16();
        }
        map[text.len()] = u;
        Self(map)
    }

    fn at(&self, byte: usize) -> usize {
        self.0[byte]
    }
}

fn is_quote(c: char) -> bool {
    matches!(c, '“' | '”' | '"' | '「' | '」' | '‘' | '’')
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '…' | '\n')
}

pub fn stats(text: &str) -> LintStats {
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let para_lens: Vec<usize> = paragraphs(text).iter().map(|p| p.chars().count()).collect();
    let mut sentence_lens = Vec::new();
    let mut cur = 0;
    for c in text.chars() {
        if is_sentence_end(c) {
            if cur > 0 {
                sentence_lens.push(cur as f32);
                cur = 0;
            }
        } else if !c.is_whitespace() && !is_quote(c) {
            cur += 1;
        }
    }
    if cur > 0 {
        sentence_lens.push(cur as f32);
    }
    let mut in_quote = false;
    let mut dialogue = 0;
    for c in text.chars() {
        match c {
            '“' | '「' => in_quote = true,
            '”' | '」' | '\n' => in_quote = false,
            '"' => in_quote = !in_quote,
            _ if in_quote && !c.is_whitespace() => dialogue += 1,
            _ => {}
        }
    }
    let n = sentence_lens.len() as f32;
    let avg = if n > 0.0 { sentence_lens.iter().sum::<f32>() / n } else { 0.0 };
    let var = if n > 1.0 { sentence_lens.iter().map(|x| (x - avg).powi(2)).sum::<f32>() / n } else { 0.0 };
    LintStats {
        chars,
        paragraphs: para_lens.len(),
        sentences: sentence_lens.len(),
        dialogue_ratio: dialogue as f32 / chars.max(1) as f32,
        avg_sentence: avg,
        sentence_stdev: var.sqrt(),
        long_paragraphs: para_lens.iter().filter(|&&l| l > 250).count(),
        max_paragraph: para_lens.iter().copied().max().unwrap_or(0),
    }
}

fn issue(kind: &str, severity: &str, text: String, count: usize, positions: Vec<[usize; 2]>, suggestion: &str) -> LintIssue {
    LintIssue { kind: kind.into(), severity: severity.into(), text, count, positions, suggestion: suggestion.into() }
}

fn long_paragraphs(text: &str, map: &Utf16Map, out: &mut Vec<LintIssue>) {
    let mut positions = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        if line.trim().chars().count() > 250 {
            positions.push([map.at(offset), map.at(offset + line.len())]);
        }
        offset += line.len() + 1;
    }
    if !positions.is_empty() {
        let n = positions.len();
        out.push(issue("排版", "low", format!("{n} 个段落超过 250 字"), n, positions, "手机阅读建议每段 150 字以内，把长段落拆开"));
    }
}

fn sentence_starts(text: &str, map: &Utf16Map, out: &mut Vec<LintIssue>) {
    let mut starts: Vec<(usize, char)> = Vec::new();
    let mut at_start = true;
    for (i, c) in text.char_indices() {
        if is_sentence_end(c) {
            at_start = true;
        } else if at_start && !c.is_whitespace() && !is_quote(c) {
            starts.push((i, c));
            at_start = false;
        }
    }
    let mut positions = Vec::new();
    let mut longest = (0, ' ');
    let mut i = 0;
    while i < starts.len() {
        let c = starts[i].1;
        let mut j = i + 1;
        while j < starts.len() && starts[j].1 == c {
            j += 1;
        }
        if j - i >= 4 && "他她我你它这那".contains(c) {
            let last = starts[j - 1].0;
            positions.push([map.at(starts[i].0), map.at(last + c.len_utf8())]);
            if j - i > longest.0 {
                longest = (j - i, c);
            }
        }
        i = j;
    }
    if !positions.is_empty() {
        let n = positions.len();
        out.push(issue(
            "句式重复",
            "medium",
            format!("连续 {} 句以「{}」开头", longest.0, longest.1),
            n,
            positions,
            "换个主语，或者用动作、对话、环境开头",
        ));
    }
}

fn repeated_phrases(text: &str, map: &Utf16Map, out: &mut Vec<LintIssue>) {
    const N: usize = 8;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
    for w in chars.windows(N) {
        if w.iter().all(|(_, c)| is_cjk(*c)) {
            seen.entry(w.iter().map(|(_, c)| *c).collect()).or_default().push(w[0].0);
        }
    }
    let mut reps: Vec<(String, Vec<usize>)> = seen.into_iter().filter(|(_, v)| v.len() >= 3).collect();
    reps.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.1[0].cmp(&b.1[0])));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (phrase, starts) in reps {
        if taken.len() >= 3 {
            break;
        }
        let (first, len) = (starts[0], phrase.len());
        if taken.iter().any(|(s, e)| first < *e + N * 3 && first + len + N * 3 > *s) {
            continue;
        }
        taken.push((first, first + len));
        let severity = if starts.len() >= 5 { "medium" } else { "low" };
        let positions = starts.iter().map(|&s| [map.at(s), map.at(s + len)]).collect();
        out.push(issue("重复表达", severity, phrase, starts.len(), positions, "同一个表达反复出现，换个说法或者删掉"));
    }
}

fn punctuation(text: &str, map: &Utf16Map, out: &mut Vec<LintIssue>) {
    static HALF: OnceLock<Regex> = OnceLock::new();
    static DOTS: OnceLock<Regex> = OnceLock::new();
    let half = HALF.get_or_init(|| Regex::new(r"\p{Han}[,;:?!]").unwrap());
    let dots = DOTS.get_or_init(|| Regex::new(r"\.{3,}|。{2,}").unwrap());
    let positions: Vec<[usize; 2]> = half.find_iter(text).map(|m| [map.at(m.end() - 1), map.at(m.end())]).collect();
    if !positions.is_empty() {
        let n = positions.len();
        out.push(issue("标点", "medium", format!("{n} 处半角标点"), n, positions, "中文正文请用全角标点：，；：？！"));
    }
    let positions: Vec<[usize; 2]> = dots.find_iter(text).map(|m| [map.at(m.start()), map.at(m.end())]).collect();
    if !positions.is_empty() {
        let n = positions.len();
        out.push(issue("标点", "low", format!("{n} 处省略号不规范"), n, positions, "省略号请用“……”"));
    }
    let quotes: Vec<[usize; 2]> = text.match_indices('"').map(|(i, _)| [map.at(i), map.at(i + 1)]).collect();
    if !quotes.is_empty() && text.chars().any(is_cjk) {
        let n = quotes.len();
        out.push(issue("标点", "low", format!("{n} 个英文引号"), n, quotes, "中文对话请用“”"));
    }
}

/// 弱化副词每千字超过 3 个就提示（novel-studio 的阈值）。
fn weak_adverbs(text: &str, chars: usize, map: &Utf16Map, out: &mut Vec<LintIssue>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(WEAK_ADVERBS).unwrap());
    let found: Vec<_> = re.find_iter(text).collect();
    let allowed = ((chars as f32 / 1000.0) * 3.0).ceil().max(3.0) as usize;
    if found.len() > allowed {
        let positions = found.iter().map(|m| [map.at(m.start()), map.at(m.end())]).collect();
        out.push(issue(
            "弱化副词",
            "medium",
            format!("“微微、淡淡、缓缓、轻轻”出现 {} 次", found.len()),
            found.len(),
            positions,
            "每千字超过 3 个就显得软；删掉大部分，用更准确的动词",
        ));
    }
}

fn first_sentence(text: &str) -> String {
    text.trim_start()
        .chars()
        .skip_while(|c| is_quote(*c) || c.is_whitespace())
        .take_while(|c| !is_sentence_end(*c))
        .collect()
}

fn last_sentence(text: &str) -> String {
    let trimmed = text.trim_end().trim_end_matches(|c: char| is_sentence_end(c) || is_quote(c));
    let start = trimmed.rfind(|c: char| is_sentence_end(c)).map(|i| i + trimmed[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)).unwrap_or(0);
    trimmed[start..].trim().trim_start_matches(is_quote).to_string()
}

fn prefix(s: &str, n: usize) -> String {
    s.chars().filter(|c| !c.is_whitespace()).take(n).collect()
}

/// 跨章节检查：和前面几章逐字重复的长句、雷同的开头和结尾（参考 novel-studio 的跨章文风记忆）。
pub fn cross_chapter(text: &str, previous: &[(String, String)]) -> Vec<LintIssue> {
    const N: usize = 10;
    let map = Utf16Map::new(text);
    let mut out = Vec::new();
    if previous.is_empty() {
        return out;
    }
    let mut seen: HashMap<String, std::collections::HashSet<usize>> = HashMap::new();
    for (idx, (_, content)) in previous.iter().enumerate() {
        let chars: Vec<char> = content.chars().collect();
        for w in chars.windows(N) {
            if w.iter().all(|c| is_cjk(*c)) {
                seen.entry(w.iter().collect()).or_default().insert(idx);
            }
        }
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut hits: Vec<(usize, usize, usize)> = Vec::new(); // (字节起点, 字节终点, 出现在几章)
    for w in chars.windows(N) {
        if !w.iter().all(|(_, c)| is_cjk(*c)) {
            continue;
        }
        let key: String = w.iter().map(|(_, c)| *c).collect();
        if let Some(chs) = seen.get(&key).filter(|s| s.len() >= 2) {
            let end = w[N - 1].0 + w[N - 1].1.len_utf8();
            match hits.last_mut() {
                Some(last) if w[0].0 < last.1 => last.1 = end,
                _ => hits.push((w[0].0, end, chs.len())),
            }
        }
    }
    hits.sort_by(|a, b| b.2.cmp(&a.2).then((b.1 - b.0).cmp(&(a.1 - a.0))));
    for (s, e, n) in hits.into_iter().take(5) {
        out.push(issue(
            "跨章重复",
            "low",
            text[s..e].to_string(),
            n,
            vec![[map.at(s), map.at(e)]],
            "这句话在前面几章里也出现过，换个说法",
        ));
    }
    let same = |mine: String, pick: fn(&str) -> String| -> usize {
        if mine.chars().count() < 4 {
            return 0;
        }
        previous.iter().filter(|(_, c)| prefix(&pick(c), 4) == mine).count()
    };
    let opening = prefix(&first_sentence(text), 4);
    let n = same(opening.clone(), first_sentence);
    if n >= 2 {
        out.push(issue("开头雷同", "medium", format!("开头“{opening}……”和前面 {n} 章相同"), n, vec![], "换一种开篇方式：动作、对话、悬念或环境"));
    }
    let ending = prefix(&last_sentence(text), 4);
    let n = same(ending.clone(), last_sentence);
    if n >= 2 {
        out.push(issue("结尾雷同", "medium", format!("结尾“{ending}……”和前面 {n} 章相同"), n, vec![], "换一种章末钩子"));
    }
    out
}

/// 和文风库范文连续重复的片段：去掉标点和空白后连续 16 个字相同，就当作照抄提示出来。
pub fn source_overlap(text: &str, sources: &[(String, String)]) -> Vec<LintIssue> {
    const N: usize = 16;
    fn norm(s: &str) -> Vec<(usize, char)> {
        s.char_indices().filter(|(_, c)| c.is_alphanumeric()).collect()
    }
    fn hash(w: &[(usize, char)]) -> u64 {
        w.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, (_, c)| (h ^ *c as u64).wrapping_mul(0x0100_0000_01b3))
    }
    let mine = norm(text);
    if mine.len() < N || sources.is_empty() {
        return Vec::new();
    }
    let mut windows: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, w) in mine.windows(N).enumerate() {
        windows.entry(hash(w)).or_default().push(i);
    }
    let mut matched: Vec<Option<usize>> = vec![None; mine.len()];
    for (si, (_, src)) in sources.iter().enumerate() {
        let theirs = norm(src);
        for w in theirs.windows(N) {
            let Some(starts) = windows.get(&hash(w)) else { continue };
            for &i in starts {
                if matched[i].is_none() && mine[i..i + N].iter().map(|x| x.1).eq(w.iter().map(|x| x.1)) {
                    matched[i] = Some(si);
                }
            }
        }
    }
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    for (i, m) in matched.iter().enumerate() {
        let Some(si) = *m else { continue };
        match spans.last_mut() {
            Some(last) if i <= last.1 && last.2 == si => last.1 = last.1.max(i + N),
            _ => spans.push((i, i + N, si)),
        }
    }
    spans.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)));
    let map = Utf16Map::new(text);
    spans
        .into_iter()
        .take(8)
        .map(|(s, e, si)| {
            let start = mine[s].0;
            let (last, c) = mine[e - 1];
            let end = last + c.len_utf8();
            issue(
                "疑似照抄",
                "high",
                text[start..end].to_string(),
                1,
                vec![[map.at(start), map.at(end)]],
                &format!("和文风库里《{}》的原文连续 {} 字相同，投稿前请改写", sources[si].0, e - s),
            )
        })
        .collect()
}

/// 给文风提取用的统计描述。
pub fn style_stats_text(sample: &str) -> String {
    let s = stats(sample);
    let first = sample.matches('我').count();
    let third = sample.matches('他').count() + sample.matches('她').count();
    let pov = if first > third { "第一人称为主" } else { "第三人称为主" };
    format!(
        "字数 {}；{} 段，平均每段 {:.0} 字，最长 {} 字；句子平均 {:.0} 字（波动 {:.0}）；对话占比 {:.0}%；“我”出现 {} 次，“他/她”出现 {} 次，{}",
        s.chars,
        s.paragraphs,
        s.chars as f32 / s.paragraphs.max(1) as f32,
        s.max_paragraph,
        s.avg_sentence,
        s.sentence_stdev,
        s.dialogue_ratio * 100.0,
        first,
        third,
        pov
    )
}

pub fn lint(text: &str, extra: &[String]) -> LintReport {
    let map = Utf16Map::new(text);
    let stats = stats(text);
    let scale = (stats.chars as f32 / 3000.0).ceil().max(1.0) as usize;
    let mut issues = Vec::new();

    for (re, rule) in compiled() {
        let found: Vec<_> = re.find_iter(text).collect();
        let allowed = rule.allow * scale;
        if found.len() > allowed {
            let severity = if found.len() > allowed * 2 + 2 { "high" } else { "medium" };
            let positions = found.iter().map(|m| [map.at(m.start()), map.at(m.end())]).collect();
            issues.push(issue(rule.kind, severity, found[0].as_str().to_string(), found.len(), positions, rule.suggestion));
        }
    }
    for phrase in extra.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let positions: Vec<[usize; 2]> = text.match_indices(phrase).map(|(i, m)| [map.at(i), map.at(i + m.len())]).collect();
        if !positions.is_empty() {
            let n = positions.len();
            issues.push(issue("自定义禁用词", "medium", phrase.to_string(), n, positions, "这是你在设置里标记的词"));
        }
    }
    weak_adverbs(text, stats.chars, &map, &mut issues);
    long_paragraphs(text, &map, &mut issues);
    sentence_starts(text, &map, &mut issues);
    repeated_phrases(text, &map, &mut issues);
    punctuation(text, &map, &mut issues);
    if stats.chars > 1500 && stats.dialogue_ratio < 0.08 {
        issues.push(issue(
            "节奏",
            "low",
            format!("对话占比 {:.0}%", stats.dialogue_ratio * 100.0),
            1,
            vec![],
            "叙述多、对话少，读起来会闷；关键冲突尽量用对话推进",
        ));
    }
    if stats.sentences >= 20 && stats.sentence_stdev < stats.avg_sentence * 0.35 {
        issues.push(issue(
            "节奏",
            "low",
            format!("句子长度过于整齐（平均 {:.0} 字）", stats.avg_sentence),
            1,
            vec![],
            "长短句交错读起来更有呼吸感，紧张处用短句，铺陈处用长句",
        ));
    }
    let penalty: i32 = issues
        .iter()
        .map(|i| match i.severity.as_str() {
            "high" => 6,
            "medium" => 3,
            _ => 1,
        })
        .sum();
    LintReport { score: (100 - penalty).clamp(0, 100), stats, issues }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_copied_passages() {
        let source = "夜色压下来的时候，林凡才走到藏经阁后墙。墙根的青苔湿得发黑，他贴着墙站了一会儿。";
        let text = "😀前面是自己写的。夜色压下来的时候林凡才走到藏经阁后墙，墙根的青苔湿得发黑！后面也是自己写的。";
        let found = source_overlap(text, &[("范文甲".into(), source.into())]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].text.starts_with("夜色压下来") && found[0].text.ends_with("湿得发黑"), "{}", found[0].text);
        assert!(found[0].suggestion.contains("范文甲"));
        let start = text.encode_utf16().collect::<Vec<_>>();
        let [s, e] = found[0].positions[0];
        assert_eq!(String::from_utf16(&start[s..e]).unwrap(), found[0].text);
        assert!(source_overlap("完全不同的一段话，和范文没有任何关系，也不会重复。", &[("甲".into(), source.into())]).is_empty());
    }

    #[test]
    fn finds_cliches_with_utf16_positions() {
        let text = "😀他嘴角勾起一抹冷笑的弧度。空气仿佛凝固了。";
        let r = lint(text, &[]);
        let smile = r.issues.iter().find(|i| i.text.starts_with("嘴角勾起")).expect("应识别嘴角套话");
        // 😀 占两个 UTF-16 单位，“他”一个，所以从 3 开始
        assert_eq!(smile.positions[0][0], 3);
        assert!(r.issues.iter().any(|i| i.text.contains("空气仿佛凝固了")));
        assert!(r.score < 100);
    }

    #[test]
    fn allows_occasional_use() {
        let r = lint("他沉声道：“走。”", &[]);
        assert!(r.issues.iter().all(|i| i.kind != "对话标签"));
    }

    #[test]
    fn structure_checks() {
        let text = "他走了。他笑了。他停下。他回头。她说话了。";
        let r = lint(text, &["回头".to_string()]);
        assert!(r.issues.iter().any(|i| i.kind == "句式重复"));
        assert!(r.issues.iter().any(|i| i.kind == "自定义禁用词"));
        let r = lint("他来了,然后走了...", &[]);
        assert_eq!(r.issues.iter().filter(|i| i.kind == "标点").count(), 2);
        let phrase = "青云宗的大长老来了";
        let r = lint(&format!("{phrase}。{phrase}。{phrase}。"), &[]);
        assert!(r.issues.iter().any(|i| i.kind == "重复表达"));
    }

    #[test]
    fn new_quality_rules() {
        let r = lint("殊不知，这意味着一切都变了。他的笑容恰到好处。未来可期。", &[]);
        assert!(r.issues.iter().any(|i| i.kind == "解释腔"));
        assert!(r.issues.iter().any(|i| i.kind == "万能结论"));
        let soft = "他微微一笑，淡淡地说，缓缓起身，轻轻关门。".repeat(2);
        assert!(lint(&soft, &[]).issues.iter().any(|i| i.kind == "弱化副词"));
    }

    #[test]
    fn cross_chapter_repeats() {
        let repeated = "夜色像一块浸了墨的旧布盖下来";
        let prev = vec![
            ("第1章".to_string(), format!("夜色降临。他走了。{repeated}。他回头。")),
            ("第2章".to_string(), format!("夜色降临。她来了。{repeated}。结尾。")),
        ];
        let cur = format!("夜色降临。林凡到了。{repeated}。");
        let issues = cross_chapter(&cur, &prev);
        let rep = issues.iter().find(|i| i.kind == "跨章重复").expect("应发现跨章重复");
        assert!(rep.text.contains("浸了墨的旧布"));
        assert!(issues.iter().any(|i| i.kind == "开头雷同"));
        assert!(style_stats_text("“你来了。”他说。我笑了。").contains("对话占比"));
    }

    #[test]
    fn dialogue_ratio() {
        let s = stats("“你好。”他说。");
        assert!(s.dialogue_ratio > 0.3);
        assert_eq!(s.sentences, 2);
    }
}
