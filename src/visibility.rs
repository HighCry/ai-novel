//! 可见性：写到第几章时，哪些资料可以给模型看。
//! 组装上下文时先按可见性剔除不该看见的资料（红线），再按预算分配字数（配额）。
//!
//! 世界观、总纲、卷纲按 Markdown 标题分节，作者在标题末尾写〔第2卷起〕〔第91章起〕〔仅规划〕〔对AI隐藏〕〔公开〕控制这一节。
//! 没写时按标题推断：含「第N卷」的从第 N 卷起开放；含「结局」「终局」「核心主线」「真相」「底牌」「幕后」的只给规划用；
//! 含「作者备注」「仅作者」的对 AI 隐藏。上级标题不可见时，下面的小节一律不可见。
//! 思路参考 Novelcrafter 的 Progressions 和马良写作的「设定可见性」，见 docs/叙事逻辑架构方案.md。

use crate::text::parse_cn_number;
use regex::Regex;
use serde::{Serialize, Serializer};
use std::sync::OnceLock;

/// 一份资料什么时候可以给模型看。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// 写哪一章都可以看
    Public,
    /// 写到第几卷（按卷的先后数）起才可以看
    Volume(i64),
    /// 写到第几章起才可以看
    Chapter(i64),
    /// 作者层：只给章纲规划、起名、推演这类没有当前章节的任务看，写正文时不给
    Planning,
    /// 作者自己的备注，任何任务都不发给模型
    Hidden,
}

/// 正在写的位置。规划类任务没有位置，看到的是作者层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    /// 第几卷，从 1 数；章节没分卷时算第 1 卷
    pub volume: i64,
    /// 第几章；序章、番外没有编号
    pub chapter: Option<i64>,
}

const NUM: &str = "[0-9０-９零〇一二两三四五六七八九十百千万]+";

fn gate_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"^第?({NUM})([卷章])(?:起|开始|开放|以后|之后)?$")).unwrap())
}

fn volume_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"第({NUM})卷")).unwrap())
}

impl Gate {
    /// at 为 None 表示规划类任务（作者层）。
    pub fn open_at(self, at: Option<Point>) -> bool {
        match (self, at) {
            (Gate::Hidden, _) => false,
            (Gate::Public, _) | (_, None) => true,
            (Gate::Planning, Some(_)) => false,
            (Gate::Volume(n), Some(p)) => p.volume >= n,
            (Gate::Chapter(n), Some(p)) => p.chapter.is_some_and(|c| c >= n),
        }
    }

    /// 解析作者写的可见性：空 / 公开 / 第2卷起 / 第91章起 / 仅规划 / 对AI隐藏。写法不认识时返回 None。
    pub fn parse(s: &str) -> Option<Gate> {
        let t: String = s.chars().filter(|c| !c.is_whitespace() && !matches!(c, '〔' | '〕')).collect::<String>().to_lowercase();
        match t.as_str() {
            "" | "公开" => return Some(Gate::Public),
            "仅规划" | "规划可见" | "作者层" => return Some(Gate::Planning),
            "对ai隐藏" | "隐藏" | "作者备注" => return Some(Gate::Hidden),
            _ => {}
        }
        let c = gate_re().captures(&t)?;
        let n = parse_cn_number(&c[1])?.max(1);
        Some(if &c[2] == "卷" { Gate::Volume(n) } else { Gate::Chapter(n) })
    }

    pub fn label(self) -> String {
        match self {
            Gate::Public => "公开".into(),
            Gate::Volume(n) => format!("第{n}卷起"),
            Gate::Chapter(n) => format!("第{n}章起"),
            Gate::Planning => "仅规划".into(),
            Gate::Hidden => "对AI隐藏".into(),
        }
    }
}

impl Serialize for Gate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.label())
    }
}

/// 设定条目的可见性统一写法后再存：公开存空字符串。写法不认识时返回 None。
pub fn normalize(s: &str) -> Option<String> {
    Gate::parse(s).map(|g| if g == Gate::Public { String::new() } else { g.label() })
}

/// 世界观、总纲、卷纲里的一节：标题行和它下面直到下一个标题之前的文字。
#[derive(Debug, Clone)]
pub struct Part {
    /// 标题级别：# 的个数，整行【】标题算 6 级；开头没有标题的文字为 0
    pub level: usize,
    /// 标题文字，不含 # 和可见性标记
    pub title: String,
    /// 作者在标题末尾写的可见性
    pub marked: Option<Gate>,
    /// 按标题推断的可见性
    pub inferred: Gate,
    pub body: String,
    heading: String,
    marker: String,
}

impl Part {
    /// 实际生效的可见性：作者写了就按作者的；书没分卷时，按「第N卷」推断出来的不算数。
    pub fn gate(&self, uses_volumes: bool) -> Gate {
        match (self.marked, self.inferred) {
            (Some(g), _) => g,
            (None, Gate::Volume(_)) if !uses_volumes => Gate::Public,
            (None, g) => g,
        }
    }

    fn clean_heading(&self) -> String {
        if self.marker.is_empty() { self.heading.trim_end().to_string() } else { self.heading.replacen(&self.marker, "", 1).trim_end().to_string() }
    }
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes > 0 {
        let rest = &t[hashes..];
        let title = rest.trim().trim_end_matches('#').trim_end();
        return ((1..=6).contains(&hashes) && rest.starts_with(char::is_whitespace) && !title.is_empty()).then_some((hashes, title));
    }
    let core = match t.rfind('〔') {
        Some(i) if t.ends_with('〕') => t[..i].trim_end(),
        _ => t,
    };
    (core.starts_with('【') && core.ends_with('】') && core.matches('【').count() == 1 && core.chars().count() <= 42).then_some((6, t))
}

fn take_marker(raw: &str) -> (String, String, Option<Gate>) {
    let raw = raw.trim();
    if let Some(start) = raw.rfind('〔').filter(|_| raw.ends_with('〕')) {
        let marker = &raw[start..];
        let inner = &marker['〔'.len_utf8()..marker.len() - '〕'.len_utf8()];
        if let Some(g) = Some(inner).filter(|s| !s.trim().is_empty()).and_then(Gate::parse) {
            return (raw[..start].trim().to_string(), marker.to_string(), Some(g));
        }
    }
    (raw.to_string(), String::new(), None)
}

fn infer(title: &str) -> Gate {
    const HIDDEN: [&str; 2] = ["作者备注", "仅作者"];
    const PLANNING: [&str; 6] = ["结局", "终局", "核心主线", "真相", "底牌", "幕后"];
    if HIDDEN.iter().any(|w| title.contains(w)) {
        return Gate::Hidden;
    }
    if let Some(n) = volume_re().captures(title).and_then(|c| parse_cn_number(&c[1])) {
        return Gate::Volume(n.max(1));
    }
    if PLANNING.iter().any(|w| title.contains(w)) {
        return Gate::Planning;
    }
    Gate::Public
}

/// 按标题拆成若干节；第一项总是开头没有标题的文字（可能为空），下标和 set_marker、describe 对应。
pub fn split(text: &str) -> Vec<Part> {
    let mut parts = vec![Part { level: 0, title: String::new(), marked: None, inferred: Gate::Public, body: String::new(), heading: String::new(), marker: String::new() }];
    for line in text.lines() {
        match heading(line) {
            Some((level, raw)) => {
                let (title, marker, marked) = take_marker(raw);
                let inferred = infer(&title);
                parts.push(Part { level, title, marked, inferred, body: String::new(), heading: line.to_string(), marker });
            }
            None => {
                let body = &mut parts.last_mut().expect("至少有开头部分").body;
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    parts
}

/// 每一节在 at 处是否可见：自己可见，并且所有上级标题都可见。
fn visible(parts: &[Part], at: Option<Point>, uses_volumes: bool) -> Vec<bool> {
    let mut stack: Vec<(usize, bool)> = Vec::new();
    parts
        .iter()
        .map(|p| {
            while stack.last().is_some_and(|(l, _)| *l >= p.level) {
                stack.pop();
            }
            let ok = stack.last().map_or(true, |(_, v)| *v) && p.gate(uses_volumes).open_at(at);
            if p.level > 0 {
                stack.push((p.level, ok));
            }
            ok
        })
        .collect()
}

/// 去掉删节后留下的多余空行和分隔线。
fn tidy(s: &str) -> String {
    let is_rule = |l: &str| {
        let t = l.trim();
        t.chars().filter(|c| !c.is_whitespace()).count() >= 3 && t.chars().all(|c| matches!(c, '-' | '*' | '_') || c.is_whitespace())
    };
    let mut out: Vec<&str> = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(line);
            }
            continue;
        }
        if is_rule(line) && out.iter().rev().find(|l| !l.trim().is_empty()).map_or(true, |l| is_rule(l)) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty() || is_rule(l)) {
        out.pop();
    }
    out.join("\n")
}

/// 只保留 at 处可见的节，去掉标题里的可见性标记。at 为 None 时只去掉对 AI 隐藏的节。
pub fn visible_text(text: &str, at: Option<Point>, uses_volumes: bool) -> String {
    let parts = split(text);
    let flags = visible(&parts, at, uses_volumes);
    let mut out = String::new();
    for (p, ok) in parts.iter().zip(flags) {
        if !ok {
            continue;
        }
        if p.level > 0 {
            out.push_str(&p.clean_heading());
            out.push('\n');
        }
        out.push_str(&p.body);
    }
    tidy(&out)
}

/// at 处不给模型看的节的标题；上级标题已经列出的，下面的小节不再重复。
pub fn withheld(text: &str, at: Option<Point>, uses_volumes: bool) -> Vec<String> {
    let parts = split(text);
    let flags = visible(&parts, at, uses_volumes);
    let mut out = Vec::new();
    let mut hidden_level: Option<usize> = None;
    for (p, ok) in parts.iter().zip(flags) {
        if hidden_level.is_some_and(|l| p.level > l) {
            continue;
        }
        hidden_level = None;
        if !ok && p.level > 0 {
            out.push(p.title.clone());
            hidden_level = Some(p.level);
        }
    }
    out
}

/// 把第 index 节标题末尾的可见性改成 gate；None 表示去掉标记、按标题推断。返回改好的全文，下标不对时返回 None。
pub fn set_marker(text: &str, index: usize, gate: Option<Gate>) -> Option<String> {
    let parts = split(text);
    let target = parts.get(index).filter(|p| p.level > 0)?;
    let base = target.clean_heading();
    let new_heading = match gate {
        Some(g) => format!("{base}〔{}〕", g.label()),
        None => base,
    };
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if p.level > 0 {
            out.push_str(if i == index { &new_heading } else { &p.heading });
            out.push('\n');
        }
        out.push_str(&p.body);
    }
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

/// 给界面看的分节列表。
#[derive(Debug, Clone, Serialize)]
pub struct PartInfo {
    pub index: usize,
    pub level: usize,
    pub title: String,
    /// 作者写的可见性，没写为 null
    pub marked: Option<Gate>,
    /// 不写标记时按标题推断出的可见性
    pub auto: Gate,
    /// 实际生效的可见性
    pub gate: Gate,
    /// 最近一个有限制的上级标题：它没开放时，这一节也不会给模型
    pub parent: Option<String>,
    pub chars: usize,
}

pub fn describe(text: &str, uses_volumes: bool) -> Vec<PartInfo> {
    let parts = split(text);
    let mut stack: Vec<(usize, Option<String>)> = Vec::new();
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        while stack.last().is_some_and(|(l, _)| *l >= p.level) {
            stack.pop();
        }
        let parent = stack.last().and_then(|(_, t)| t.clone());
        let gate = p.gate(uses_volumes);
        if p.level > 0 {
            stack.push((p.level, if gate == Gate::Public { parent.clone() } else { Some(p.title.clone()) }));
        } else if p.body.trim().is_empty() {
            continue;
        }
        let title = if p.level == 0 { "（开头）".to_string() } else { p.title.clone() };
        let auto = Part { marked: None, ..p.clone() }.gate(uses_volumes);
        out.push(PartInfo { index: i, level: p.level, title, marked: p.marked, auto, gate, parent, chars: p.body.trim().chars().count() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTLINE: &str = "这份总纲写给番茄读者。

---

### 一、 核心主线概述
*   **最终结局**：主角成为执秤真皇。

---

### 二、 分卷大纲
#### 【第一卷：矿奴蜕骨】
矿区求生。
#### 【第二卷：灵台点灯】
灵台境大圆满。
#### 【第六卷：太虚真皇（终卷）】〔对AI隐藏〕
诸天禁物尽在一念。

---

### 三、 黄金前三章
第1章矿难。
";

    fn at(volume: i64, chapter: i64) -> Option<Point> {
        Some(Point { volume, chapter: Some(chapter) })
    }

    #[test]
    fn parses_and_labels_gates() {
        for g in [Gate::Public, Gate::Volume(2), Gate::Chapter(91), Gate::Planning, Gate::Hidden] {
            assert_eq!(Gate::parse(&g.label()), Some(g));
        }
        assert_eq!(Gate::parse(""), Some(Gate::Public));
        assert_eq!(Gate::parse("〔第三卷起〕"), Some(Gate::Volume(3)));
        assert_eq!(Gate::parse("第 91 章开始"), Some(Gate::Chapter(91)));
        assert_eq!(Gate::parse("对AI 隐藏"), Some(Gate::Hidden));
        assert_eq!(Gate::parse("下周再说"), None);
        assert_eq!(normalize("公开").as_deref(), Some(""));
        assert_eq!(normalize("2卷").as_deref(), Some("第2卷起"));
        assert_eq!(serde_json::to_string(&Gate::Chapter(5)).unwrap(), "\"第5章起\"");
    }

    #[test]
    fn gates_open_by_position() {
        assert!(Gate::Volume(2).open_at(at(2, 91)));
        assert!(!Gate::Volume(2).open_at(at(1, 90)));
        assert!(Gate::Chapter(91).open_at(at(2, 91)));
        assert!(!Gate::Chapter(91).open_at(Some(Point { volume: 9, chapter: None })), "序章、番外没有编号，按章开放的不给");
        assert!(!Gate::Planning.open_at(at(1, 1)));
        assert!(Gate::Planning.open_at(None) && Gate::Volume(6).open_at(None), "规划看作者层");
        assert!(!Gate::Hidden.open_at(None), "对 AI 隐藏的规划也不给");
    }

    #[test]
    fn filters_sections_by_position() {
        let ch1 = visible_text(OUTLINE, at(1, 1), true);
        assert!(ch1.contains("这份总纲写给番茄读者"));
        assert!(ch1.contains("#### 【第一卷：矿奴蜕骨】\n矿区求生。"));
        assert!(ch1.contains("### 三、 黄金前三章"));
        for leak in ["执秤真皇", "核心主线", "灵台境", "诸天禁物"] {
            assert!(!ch1.contains(leak), "第 1 章不该看到「{leak}」：\n{ch1}");
        }
        assert!(!ch1.contains("---\n\n---") && !ch1.starts_with("---"), "删节后不留成堆的分隔线：\n{ch1}");

        let vol2 = visible_text(OUTLINE, at(2, 91), true);
        assert!(vol2.contains("灵台境大圆满") && !vol2.contains("执秤真皇"));

        let planning = visible_text(OUTLINE, None, true);
        assert!(planning.contains("执秤真皇") && planning.contains("灵台境大圆满"), "规划看作者层");
        assert!(!planning.contains("诸天禁物") && !planning.contains('〔'), "对 AI 隐藏的不给，标记不发给模型");

        let flat = visible_text(OUTLINE, at(1, 1), false);
        assert!(flat.contains("灵台境大圆满"), "书没分卷时，按「第N卷」推断的不算数");
        assert!(!flat.contains("执秤真皇") && !flat.contains("诸天禁物"));
    }

    #[test]
    fn parent_heading_hides_children() {
        let text = "## 设定集〔仅规划〕\n总述\n### 伪神\n天轨是锁链\n## 矿区\n赤炼宗";
        let ch = visible_text(text, at(3, 200), true);
        assert_eq!(ch, "## 矿区\n赤炼宗");
        let text = "【第二卷】\n灵台\n【矿区】〔公开〕\n赤炼宗";
        assert_eq!(visible_text(text, at(1, 1), true), "【矿区】\n赤炼宗", "整行【】也算标题");
    }

    #[test]
    fn sets_markers_and_describes() {
        let parts = describe(OUTLINE, true);
        let titles: Vec<(&str, String)> = parts.iter().map(|p| (p.title.as_str(), p.gate.label())).collect();
        assert_eq!(titles[0], ("（开头）", "公开".to_string()));
        assert!(titles.contains(&("一、 核心主线概述", "仅规划".to_string())));
        assert!(titles.contains(&("【第二卷：灵台点灯】", "第2卷起".to_string())));
        let sixth = parts.iter().find(|p| p.title.contains("第六卷")).unwrap();
        assert_eq!((sixth.marked, sixth.auto, sixth.gate), (Some(Gate::Hidden), Gate::Volume(6), Gate::Hidden));

        let first = parts.iter().find(|p| p.title.contains("第一卷")).unwrap();
        let changed = set_marker(OUTLINE, first.index, Some(Gate::Chapter(3))).unwrap();
        assert!(changed.contains("#### 【第一卷：矿奴蜕骨】〔第3章起〕\n矿区求生。"));
        assert!(!visible_text(&changed, at(1, 2), true).contains("矿区求生"));
        let back = set_marker(&changed, first.index, None).unwrap();
        assert_eq!(back, OUTLINE, "去掉标记后和原文一字不差");
        assert!(set_marker(OUTLINE, 0, Some(Gate::Hidden)).is_none(), "开头没有标题，不能加标记");

        let text = "## 设定集〔仅规划〕\n### 伪神\n天轨是锁链\n## 矿区\n赤炼宗\n## 上界〔第2卷起〕\n九霄";
        assert_eq!(withheld(text, at(1, 1), true), vec!["设定集", "上界"], "上级已经列出的小节不重复");
        assert!(withheld(text, None, true).is_empty());
    }
}
