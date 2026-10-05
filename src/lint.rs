//! 文字质量检查：套话、句式重复、重复表达、排版和标点。全部本地计算，不调用模型。

use crate::text::{clip, is_cjk, paragraphs};
use regex::Regex;
use serde::Serialize;
use std::cmp::Ordering;
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
    /// 轻度 / 中度 / 重度：按每千字的套话和模板句数分档（阈值来自 story-deslop）
    pub ai_level: String,
    pub ai_density: f32,
    pub ai_hits: usize,
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
    Rule { pattern: r"心中暗(?:道|想|忖)|心中一(?:动|凛|震|紧)|心头一(?:震|紧|颤)", kind: "套话", allow: 1, suggestion: "用动作或对话表现心理" },
    Rule { pattern: r"一股莫名的|一种前所未有的|一种难以言喻的", kind: "AI腔", allow: 0, suggestion: "写清楚具体是什么感受" },
    Rule { pattern: r"空气(?:仿佛|似乎)?(?:都)?(?:凝固|安静)了", kind: "套话", allow: 0, suggestion: "写具体的安静：谁停下了动作、哪种声音消失了" },
    Rule { pattern: r"深吸了?一口气|倒吸了?一口(?:凉气|冷气)", kind: "套话", allow: 1, suggestion: "一章里出现多次会显得模板化" },
    Rule { pattern: r"这一刻|那一刻|就在这时|与此同时", kind: "AI腔", allow: 2, suggestion: "转场词太多，可以直接切场景" },
    Rule {
        pattern: r"命运(?:的)?(?:齿轮|棋局|獠牙)|齿轮开始转动|早已布好的棋局|才刚刚开始|终于明白|这才意识到|从这一刻(?:起|开始)",
        kind: "升华套话",
        allow: 0,
        suggestion: "作者在替读者总结，删掉，用动作或没解决的问题收住",
    },
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
    // 以下规则改编自 oh-story-claudecode 的 story-deslop（MIT）：一级禁用词和高频模板句
    Rule { pattern: r"一丝|一抹", kind: "禁用词", allow: 2, suggestion: "“一丝、一抹”是 AI 最爱的量词，删掉或者写具体" },
    Rule { pattern: r"些许|几分(?:无奈|笑意|冷意|寒意|得意|戏谑|玩味|慵懒|不屑)", kind: "禁用词", allow: 1, suggestion: "删掉修饰，直接写动作或台词" },
    Rule { pattern: r"毫无征兆|几不可闻|微不可察|难以察觉|不易察觉", kind: "禁用词", allow: 0, suggestion: "AI 特有的修饰，删掉，直接写发生了什么" },
    Rule { pattern: r"眉头微皱|眉眼低垂|指节泛白|眼神锐利|目光锐利|眸光|眸色", kind: "禁用词", allow: 0, suggestion: "模板化的神态描写，换成具体的动作或台词" },
    Rule { pattern: r"心下了然|心底泛起|不由得", kind: "禁用词", allow: 1, suggestion: "用动作或对话表现心理" },
    Rule { pattern: r"不容置疑|不容置喙|显而易见", kind: "禁用词", allow: 0, suggestion: "书面化的判断词，用具体事实说话" },
    Rule { pattern: r"狡黠|深邃|凛冽", kind: "禁用词", allow: 1, suggestion: "AI 高频形容词，换成具体描写" },
    Rule { pattern: r"情不自禁|自然而然|话锋一转", kind: "禁用词", allow: 0, suggestion: "删掉，直接写动作或下一句话" },
    Rule { pattern: r"取而代之的[，,]?是", kind: "句式套路", allow: 0, suggestion: "AI 的过渡模板，直接写新的状态" },
    Rule { pattern: r"散发着(?:一股|一种)", kind: "句式套路", allow: 0, suggestion: "万能的气场描写，写旁人的反应" },
    Rule { pattern: r"心(?:里|底|中)(?:某个地方|某处|最柔软的地方)", kind: "句式套路", allow: 0, suggestion: "言情套句，写动作" },
    Rule { pattern: r"淬(?:了|着)(?:毒|冰|寒)", kind: "句式套路", allow: 0, suggestion: "AI 的通感套路，写动作或台词" },
    Rule {
        pattern: r"有的[^。！？\n]{1,15}有的[^。！？\n]{1,15}有的|一边[^。！？\n]{1,15}一边[^。！？\n]{1,15}一边",
        kind: "排比",
        allow: 0,
        suggestion: "AI 爱凑三个一组，只留最有力的一条",
    },
    Rule { pattern: r"(?m)(?:说|问|笑|喝|叹|怒)道(?:[：:，,。“]|$)", kind: "对话标签", allow: 3, suggestion: "“说道、问道”这类标签太多，用动作引出台词，或者直接省略" },
];

/// 只查引号外的叙述：这些句式在台词里常常是人物的正常说法
const NARRATION_RULES: &[Rule] = &[
    Rule { pattern: r"不是[^，。！？；“”\n]{1,16}[，,](?:而|反而)?是", kind: "句式套路", allow: 0, suggestion: "“不是A，而是B”是最典型的 AI 句式，直接写 B" },
    Rule {
        pattern: r"[，,]带着(?:一丝|一抹|几分|些许|一股|淡淡的|浓浓的)",
        kind: "句式套路",
        allow: 0,
        suggestion: "“……，带着一丝……”是万能状语，删掉状语留主句，或者换成具体动作",
    },
    Rule {
        pattern: r"声音不大[，,]?(?:却|但)|语气(?:里)?(?:毫无|没有一丝)(?:波澜|情绪)|平静无波|听不出(?:半点|任何|一丝)?(?:波澜|情绪)",
        kind: "句式套路",
        allow: 0,
        suggestion: "AI 最爱的声音描写，直接写台词内容或说话时的动作",
    },
    Rule { pattern: r"(?:他|她|我)(?:感到|感觉到|意识到)", kind: "告知心理", allow: 2, suggestion: "直接把心理告诉读者，换成动作、反应或后果" },
    Rule { pattern: r"没有[^，。！？“”\n]{1,20}[，,](?:只有|唯有)", kind: "句式套路", allow: 0, suggestion: "“没有X，只有Y”是 AI 爱用的对照句，直接写 Y" },
    Rule { pattern: r"——|—", kind: "破折号", allow: 2, suggestion: "叙述里用破折号制造停顿是 AI 的习惯，改用句号、逗号或动作断句" },
];

/// 按密度提示：少量出现很正常，堆多了才是 AI 味。超出部分计入 AI 味密度
struct Density {
    pattern: &'static str,
    kind: &'static str,
    /// 每千字允许的次数
    per_k: f32,
    suggestion: &'static str,
}

const DENSITY: &[Density] = &[
    Density {
        pattern: r"如同|如[^，。！？“”\n]{1,8}般|像是|仿佛|宛如|犹如|宛若|好似|恍若|似的",
        kind: "比喻过多",
        per_k: 2.0,
        suggestion: "比喻太密：保留最能传达信息的一两个，其余改回具体的动作、声音和后果；不要换成新的比喻",
    },
    Density {
        pattern: r"极其|极为|猛地|猛然|死死|狠狠|疯狂|彻底|精准|瞬间|骤然|剧烈",
        kind: "强调词过多",
        per_k: 4.0,
        suggestion: "“极其、猛地、死死、狠狠、精准”这类强调词堆在一起反而没力；删掉大半，让动词自己用力",
    },
];

/// 计入 AI 味密度的问题类别
const AI_KINDS: &[&str] = &[
    "套话", "AI腔", "升华套话", "解释腔", "意义膨胀", "万能结论", "论文体", "书面连词", "比喻过多", "对话标签", "禁用词", "句式套路", "告知心理", "排比", "破折号",
    "章末升华", "弱化副词", "强调词过多", "拟声独段",
];

/// 按密度检查，超出允许次数的命中计入 AI 味密度（返回这些超出的命中）
fn density_checks(text: &str, chars: usize, map: &Utf16Map, out: &mut Vec<LintIssue>) -> Vec<(usize, usize)> {
    static C: OnceLock<Vec<(Regex, &'static Density)>> = OnceLock::new();
    let compiled = C.get_or_init(|| DENSITY.iter().map(|d| (Regex::new(d.pattern).expect(d.pattern), d)).collect());
    let mut excess = Vec::new();
    for (re, d) in compiled {
        let found: Vec<_> = re.find_iter(text).collect();
        let allowed = ((chars as f32 / 1000.0) * d.per_k).ceil().max(2.0) as usize;
        if found.len() > allowed {
            let positions = found.iter().map(|m| [map.at(m.start()), map.at(m.end())]).collect();
            out.push(issue(d.kind, "medium", found[0].as_str().to_string(), found.len(), positions, d.suggestion));
            excess.extend(found[allowed..].iter().map(|m| (m.start(), m.end())));
        }
    }
    excess
}

/// 叙述段落（不以引号开头）：拟声词单独成段、一句一段太多、精确数字太密。返回计入 AI 味的命中
fn paragraph_checks(text: &str, map: &Utf16Map, out: &mut Vec<LintIssue>) -> Vec<(usize, usize)> {
    static SFX: OnceLock<Regex> = OnceLock::new();
    static NUM: OnceLock<Regex> = OnceLock::new();
    let sfx = SFX.get_or_init(|| Regex::new(r"^[嗤噗咔嚓当啷啪嗒轰砰哗嗡呼咚铛叮咣嘭嘶吱嘎刷唰锵铮]{1,4}[。！!…—～~]*$").unwrap());
    let num = NUM.get_or_init(|| Regex::new(r"[一二三四五六七八九十百千两半几]+(?:丈|寸|尺|息|步|斤|里)").unwrap());
    let (mut sfx_hits, mut short, mut total) = (Vec::new(), Vec::new(), 0);
    let mut offset = 0;
    for line in text.split('\n') {
        let t = line.trim();
        let start = offset + (line.len() - line.trim_start().len());
        offset += line.len() + 1;
        if t.is_empty() {
            continue;
        }
        total += 1;
        if t.starts_with(is_quote) {
            continue;
        }
        if sfx.is_match(t) {
            sfx_hits.push((start, start + t.len()));
        } else if t.chars().filter(|c| is_cjk(*c)).count() <= 8 {
            short.push((start, start + t.len()));
        }
    }
    let mut ai = Vec::new();
    if sfx_hits.len() >= 3 {
        let positions = sfx_hits.iter().map(|&(s, e)| [map.at(s), map.at(e)]).collect();
        out.push(issue("拟声独段", "medium", format!("{} 个拟声词单独成段", sfx_hits.len()), sfx_hits.len(), positions, "“噗。”“咔嚓。”这样单独成段是 AI 打斗戏的模板，并进前后的动作句里"));
        ai.extend(sfx_hits.iter().skip(2).copied());
    }
    let allowed = (total as f32 * 0.2).ceil() as usize;
    if short.len() >= 8 && short.len() > allowed {
        let positions = short.iter().map(|&(s, e)| [map.at(s), map.at(e)]).collect();
        out.push(issue("碎段", "low", format!("{} 段只有一句短话", short.len()), short.len(), positions, "一句一段太多，读起来像分镜脚本；把同一个镜头里的短句并成一段"));
        ai.extend(short.iter().skip(allowed).copied());
    }
    let nums: Vec<_> = num.find_iter(text).collect();
    let allowed = ((text.chars().count() as f32 / 1000.0) * 3.0).ceil().max(3.0) as usize;
    if nums.len() > allowed {
        let positions = nums.iter().map(|m| [map.at(m.start()), map.at(m.end())]).collect();
        out.push(issue("数字过密", "low", format!("“{}”等精确数字 {} 处", nums[0].as_str(), nums.len()), nums.len(), positions, "处处精确到几寸、几息像说明书；只留推动剧情的一两个，其余写动作和感受"));
    }
    ai
}

const WEAK_ADVERBS: &str = r"微微|淡淡|缓缓|轻轻";

fn compiled() -> &'static Vec<(Regex, &'static Rule)> {
    static C: OnceLock<Vec<(Regex, &'static Rule)>> = OnceLock::new();
    C.get_or_init(|| RULES.iter().map(|r| (Regex::new(r.pattern).expect(r.pattern), r)).collect())
}

fn compiled_narration() -> &'static Vec<(Regex, &'static Rule)> {
    static C: OnceLock<Vec<(Regex, &'static Rule)>> = OnceLock::new();
    C.get_or_init(|| NARRATION_RULES.iter().map(|r| (Regex::new(r.pattern).expect(r.pattern), r)).collect())
}

/// 引号里的台词（字节范围）。遇到换行就结束，免得漏掉的右引号把后文都当成台词
fn dialogue_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match c {
            '“' | '「' if start.is_none() => start = Some(i),
            '”' | '」' => {
                if let Some(s) = start.take() {
                    spans.push((s, i + c.len_utf8()));
                }
            }
            '\n' => start = None,
            _ => {}
        }
    }
    spans
}

fn inside(spans: &[(usize, usize)], pos: usize) -> bool {
    spans
        .binary_search_by(|&(s, e)| if pos < s { Ordering::Greater } else if pos >= e { Ordering::Less } else { Ordering::Equal })
        .is_ok()
}

/// 重叠的命中只算一处（同一句可能同时命中好几条规则）
fn count_spots(mut spans: Vec<(usize, usize)>) -> usize {
    spans.sort_unstable();
    let mut n = 0;
    let mut end = 0;
    for (s, e) in spans {
        if n == 0 || s >= end {
            n += 1;
            end = e;
        } else {
            end = end.max(e);
        }
    }
    n
}

/// 章末的总结、感悟和预告：只看最后一段；正文太短（多半是选中的片段）时不查
fn trailer(text: &str, map: &Utf16Map) -> Option<(LintIssue, (usize, usize))> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"不知道的是|即将来临|风暴(?:即将|正在)|才刚刚开始|注定(?:要|会|无人|不平凡)|终于明白|这一夜|一切都(?:变了|不一样了)|新的篇章|新的一页").unwrap()
    });
    if text.chars().count() < 800 {
        return None;
    }
    let trimmed = text.trim_end();
    let start = trimmed.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let m = re.find(&trimmed[start..])?;
    let (s, e) = (start + m.start(), start + m.end());
    let found = issue(
        "章末升华",
        "high",
        m.as_str().to_string(),
        1,
        vec![[map.at(s), map.at(e)]],
        "章末在总结、感悟或预告；删掉，用最后一个动作、一句对话或一个具体的悬念收住",
    );
    Some((found, (s, e)))
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

/// 弱化副词每千字超过 3 个就提示（novel-studio 的阈值）。返回全部命中，用来算 AI 味密度
fn weak_adverbs(text: &str, chars: usize, map: &Utf16Map, out: &mut Vec<LintIssue>) -> Vec<(usize, usize)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(WEAK_ADVERBS).unwrap());
    let found: Vec<_> = re.find_iter(text).collect();
    let spans = found.iter().map(|m| (m.start(), m.end())).collect();
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
    spans
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
    let mut ai_spans: Vec<(usize, usize)> = Vec::new();

    let dialogue = dialogue_spans(text);
    let everywhere = compiled().iter().map(|(re, rule)| (re, *rule, false));
    let narration = compiled_narration().iter().map(|(re, rule)| (re, *rule, true));
    for (re, rule, only_narration) in everywhere.chain(narration) {
        let found: Vec<_> = re.find_iter(text).filter(|m| !only_narration || !inside(&dialogue, m.start())).collect();
        if AI_KINDS.contains(&rule.kind) {
            ai_spans.extend(found.iter().map(|m| (m.start(), m.end())));
        }
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
    ai_spans.extend(weak_adverbs(text, stats.chars, &map, &mut issues));
    ai_spans.extend(density_checks(text, stats.chars, &map, &mut issues));
    ai_spans.extend(paragraph_checks(text, &map, &mut issues));
    if let Some((found, span)) = trailer(text, &map) {
        issues.push(found);
        ai_spans.push(span);
    }
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
    let ai_hits = count_spots(ai_spans);
    // 选中的短片段按 500 字算，免得两三处就判成重度
    let ai_density = ai_hits as f32 * 1000.0 / stats.chars.max(500) as f32;
    let ai_level = if ai_density <= 5.0 {
        "轻度"
    } else if ai_density <= 15.0 {
        "中度"
    } else {
        "重度"
    };
    LintReport { score: (100 - penalty).clamp(0, 100), stats, issues, ai_level: ai_level.into(), ai_density, ai_hits }
}

/// 敏感词命中转成文字检查的条目，严重度沿用词库等级
pub fn sensitive_issues(text: &str, extra: &[String], ignore: &[String]) -> Vec<LintIssue> {
    let map = Utf16Map::new(text);
    crate::sensitive::scan(text, extra, ignore)
        .into_iter()
        .map(|h| {
            let positions = h.spans.iter().map(|&(a, b)| [map.at(a), map.at(b)]).collect();
            issue("敏感词", h.level, h.word, h.spans.len(), positions, &format!("{}：{}", h.category, h.suggestion))
        })
        .collect()
}

/// 给「技能修订」的检测摘要：AI 味等级和主要问题，严重的在前
pub fn ai_brief(r: &LintReport) -> String {
    let mut lines = vec![format!("AI味：{}（每千字约 {:.1} 处套话和模板句）", r.ai_level, r.ai_density)];
    let mut list: Vec<&LintIssue> =
        r.issues.iter().filter(|i| AI_KINDS.contains(&i.kind.as_str()) || matches!(i.kind.as_str(), "句式重复" | "重复表达" | "节奏" | "碎段" | "数字过密")).collect();
    list.sort_by_key(|i| match i.severity.as_str() {
        "high" => 0,
        "medium" => 1,
        _ => 2,
    });
    for i in list.into_iter().take(24) {
        let times = if i.count > 1 { format!("×{}", i.count) } else { String::new() };
        lines.push(format!("· {}「{}」{}：{}", i.kind, clip(&i.text, 30), times, i.suggestion));
    }
    if lines.len() == 1 {
        lines.push("· 没有发现明显的套话和模板句；按技能要求通读，只改确实别扭的地方".into());
    }
    lines.join("\n")
}

/// 按 AI 味等级允许删减的比例（story-deslop 的上限）
pub fn cut_ratio(level: &str) -> f32 {
    match level {
        "重度" => 0.35,
        "中度" => 0.25,
        _ => 0.15,
    }
}

pub fn max_cut(level: &str) -> String {
    format!("{:.0}%", cut_ratio(level) * 100.0)
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
    fn deslop_patterns_and_level() {
        // 叙述里的“不是A，而是B”算，台词里的不算
        let r = lint("他不是冷漠，而是绝望。“不是我，是他！”她喊。", &[]);
        let hit = r.issues.iter().find(|i| i.kind == "句式套路").expect("应识别不是A而是B");
        assert_eq!(hit.count, 1, "{hit:?}");
        let r = lint("她笑了一下，带着一丝嘲讽。她声音不大，却很清楚。", &[]);
        assert_eq!(r.issues.iter().filter(|i| i.kind == "句式套路").map(|i| i.count).sum::<usize>(), 2);
        // 章末升华只看最后一段
        let body = "他推开门，屋里没人。".repeat(90);
        let r = lint(&format!("{body}\n他不知道的是，更大的风暴即将来临。"), &[]);
        assert!(r.issues.iter().any(|i| i.kind == "章末升华"));
        // 等级：干净的叙述是轻度，套话密集是重度
        assert_eq!(lint(&body, &[]).ai_level, "轻度");
        let slop = "他嘴角勾起一抹冷笑，眼中闪过一丝不屑，仿佛一切尽在掌握。她不禁深吸一口气，心中一凛。".repeat(10);
        let r = lint(&slop, &[]);
        assert_eq!(r.ai_level, "重度", "density {}", r.ai_density);
        assert_eq!(count_spots(vec![(0, 6), (3, 9), (12, 15)]), 2);
        let brief = ai_brief(&r);
        assert!(brief.starts_with("AI味：重度") && brief.contains("套话"), "{brief}");
        assert_eq!(max_cut(&r.ai_level), "35%");
    }

    #[test]
    fn density_and_paragraph_checks() {
        assert!(lint("没有真气的轰鸣，只有刀锋的轻响。", &[]).issues.iter().any(|i| i.kind == "句式套路"));
        let fight = "他出刀。\n\n嗤。\n\n对方倒下。\n\n噗。\n\n血溅出来。\n\n咔嚓。\n\n轰——\n";
        let r = lint(fight, &[]);
        assert_eq!(r.issues.iter().find(|i| i.kind == "拟声独段").map(|i| i.count), Some(4), "{:?}", r.issues);
        assert!(r.issues.iter().all(|i| i.kind != "破折号"), "一两处破折号在允许范围内");
        let purple = "他如同猛虎般扑出，刀光如同闪电，身形如鬼魅般飘忽，气势如山岳般沉重，拳头如铁锤般砸下。".repeat(3);
        assert!(lint(&purple, &[]).issues.iter().any(|i| i.kind == "比喻过多"));
        let hard = "他猛地转身，死死盯住对方，狠狠一拳砸下，精准命中，彻底击溃，极其凶狠。".repeat(4);
        assert!(lint(&hard, &[]).issues.iter().any(|i| i.kind == "强调词过多"));
        // 台词里的破折号和省略号不算
        assert!(lint("“你他娘看什——”他还没说完就倒了。", &[]).issues.iter().all(|i| i.kind != "破折号"));
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
