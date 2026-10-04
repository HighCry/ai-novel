//! 写作技能（Skill）：一份 Markdown 说明书，格式与 Claude / Agent Skills 的 SKILL.md 兼容——
//! 开头的 YAML 头信息写 name、description，其余正文是给模型的指令。
//! 正文里「## 写作规则」一节在写正文时注入系统提示；「技能修订」（去 AI 味等）用整份正文。
//! 内置的「网文去AI味」改编自 oh-story-claudecode 的 story-deslop（MIT）等，见 THIRD_PARTY_NOTICES.md。

use crate::db::Db;
use crate::models::{Entry, Settings};
use crate::text::clip;
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

pub const DESLOP_ID: &str = "deslop";

const BUILTIN: &[(&str, &str)] = &[(DESLOP_ID, include_str!("skills/deslop.md"))];

const RULE_TITLES: &[&str] = &["写作规则", "Writing rules", "Writing Rules"];
/// 写正文时每个技能最多注入这么多字的写作规则，合计不超过 RULES_TOTAL
const RULES_EACH: usize = 2500;
const RULES_TOTAL: usize = 5000;
/// 修订时附带的技能正文上限：社区技能常常连参考资料一起写进来，很长
const REVISE_MAX: usize = 16000;

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    /// 原始 Markdown（含 YAML 头信息）
    pub markdown: String,
    pub builtin: bool,
    pub updated_at: i64,
}

impl Skill {
    /// source 为空时用头信息里写的来源
    pub fn from_markdown(id: &str, markdown: &str, source: &str, builtin: bool, updated_at: i64) -> Self {
        let p = parse(markdown);
        let source = if source.trim().is_empty() { p.source } else { source.trim().to_string() };
        Self { id: id.into(), name: p.name, description: p.description, source, markdown: markdown.into(), builtin, updated_at }
    }

    pub fn body(&self) -> String {
        parse(&self.markdown).body
    }

    pub fn write_rules(&self) -> Option<String> {
        section(&self.body(), RULE_TITLES)
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub name: String,
    pub description: String,
    pub source: String,
    pub body: String,
}

/// 解析 SKILL.md。头信息只认 name、description、source（或 homepage），
/// 值可以带引号，也可以是 `|` / `>` 多行块；没有头信息时用第一个标题和第一段话兜底。
pub fn parse(markdown: &str) -> Parsed {
    let text = markdown.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut p = Parsed::default();
    let mut body = text.as_str();
    if let Some(rest) = text.strip_prefix("---\n") {
        let mut offset = 0;
        for line in rest.split_inclusive('\n') {
            if line.trim_end() == "---" {
                read_front(&rest[..offset], &mut p);
                body = &rest[offset + line.len()..];
                break;
            }
            offset += line.len();
        }
    }
    let body = body.trim().to_string();
    if p.name.is_empty() {
        p.name = body
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "未命名技能".into());
    }
    if p.description.is_empty() {
        let first = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("");
        p.description = clip(first, 100);
    }
    p.body = body;
    p
}

fn read_front(front: &str, p: &mut Parsed) {
    let lines: Vec<&str> = front.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        let value = if matches!(value, "|" | "|-" | "|+" | ">" | ">-" | ">+") {
            let mut block = Vec::new();
            while i < lines.len() && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty()) {
                block.push(lines[i].trim());
                i += 1;
            }
            let sep = if value.starts_with('>') { " " } else { "\n" };
            block.join(sep).trim().to_string()
        } else {
            unquote(value)
        };
        match key.trim() {
            "name" => p.name = value,
            "description" => p.description = value,
            "source" | "homepage" if p.source.is_empty() => p.source = value,
            _ => {}
        }
    }
}

fn unquote(v: &str) -> String {
    let quoted = v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    if quoted { v[1..v.len() - 1].replace("\\\"", "\"") } else { v.to_string() }
}

/// 取某个标题下的内容，直到下一个同级或更高级的标题；代码块里以 # 开头的行不算标题。
pub fn section(body: &str, titles: &[&str]) -> Option<String> {
    let mut out: Option<Vec<&str>> = None;
    let mut level = 0;
    let mut fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let heading = !fence && hashes > 0 && line[hashes..].starts_with(' ');
        match out.as_mut() {
            Some(buf) => {
                if heading && hashes <= level {
                    break;
                }
                buf.push(line);
            }
            None if heading => {
                let title = line[hashes..].trim();
                if titles.iter().any(|t| title.starts_with(t)) {
                    out = Some(Vec::new());
                    level = hashes;
                }
            }
            None => {}
        }
    }
    out.map(|b| b.join("\n").trim().to_string()).filter(|s| !s.is_empty())
}

/// 把 GitHub 网页地址换成原始文件地址；仓库或目录地址默认取其中的 SKILL.md。
pub fn raw_url(url: &str) -> String {
    let u = url.trim();
    let u = u.split(['?', '#']).next().unwrap_or(u);
    if let Some(rest) = u.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        match parts.as_slice() {
            [owner, repo] => return format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/SKILL.md"),
            [owner, repo, "blob", path @ ..] if !path.is_empty() => {
                return format!("https://raw.githubusercontent.com/{owner}/{repo}/{}", path.join("/"));
            }
            [owner, repo, "tree", path @ ..] if !path.is_empty() => {
                return format!("https://raw.githubusercontent.com/{owner}/{repo}/{}/SKILL.md", path.join("/"));
            }
            _ => {}
        }
    }
    u.to_string()
}

pub fn builtins() -> Vec<Skill> {
    BUILTIN.iter().map(|(id, md)| Skill::from_markdown(id, md, "", true, 0)).collect()
}

/// 内置技能在前，作者导入的在后
pub fn all(db: &Db) -> Result<Vec<Skill>> {
    let mut list = builtins();
    list.extend(db.list_skills()?);
    Ok(list)
}

pub fn find(db: &Db, id: &str) -> Result<Option<Skill>> {
    match builtins().into_iter().find(|s| s.id == id) {
        Some(s) => Ok(Some(s)),
        None => db.get_skill(id),
    }
}

/// 设置里启用的技能，按启用顺序；已经删掉的 id 跳过
pub fn active(db: &Db, settings: &Settings) -> Vec<Skill> {
    let all = match all(db) {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("读取写作技能失败：{e:#}");
            return Vec::new();
        }
    };
    settings.skills.iter().filter_map(|id| all.iter().find(|s| &s.id == id).cloned()).collect()
}

/// 写正文时追加到系统提示里的写作规则
pub fn rules_block(skills: &[Skill]) -> String {
    let mut parts = Vec::new();
    let mut total = 0;
    for s in skills {
        let Some(rules) = s.write_rules() else { continue };
        let rules = clip(&rules, RULES_EACH);
        total += rules.chars().count();
        if total > RULES_TOTAL {
            break;
        }
        parts.push(format!("《{}》\n{}", s.name, rules));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("【写作技能：作者启用的写作规则，写正文时必须遵守】\n{}", parts.join("\n\n"))
}

/// 修订时必须原样保留的词：正文里出现的设定名和别名、书名号里的名字、境界等级、关键数量。
/// 写进提示词；修订完前端再核对哪些在修改稿里不见了（模型改写时常顺手把这些改掉或删掉）。
pub fn keep_terms(entries: &[Entry], text: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"《[^》\n]{1,20}》",
            r"|[\p{Han}--[的了在是第这那上下着过和与把被到从如同像似般]]{2}境?[一二三四五六七八九十]{1,2}[重层阶](?:圆满|巅峰|大成|后期|中期|初期)?",
            r"|[\p{Han}--[的了在是第这那上下着过和与把被到从如同像似般]]{2}境(?:圆满|巅峰|大成|后期|中期|初期)",
            r"|(?:一[一二三四五六七八九十百千万两]+|[二三四五六七八九十百千万两][一二三四五六七八九十百千万两]*)(?:斤|枚|颗)",
            r"|[一二三四五六七八九十百千万两]+\p{Han}{0,3}灵石",
        ))
        .unwrap()
    });
    let mut found: Vec<(usize, String)> = Vec::new();
    for e in entries {
        for k in e.keywords() {
            let k = k.trim();
            if k.chars().count() >= 2 {
                if let Some(i) = text.find(k) {
                    found.push((i, k.to_string()));
                }
            }
        }
    }
    found.extend(re.find_iter(text).map(|m| (m.start(), m.as_str().to_string())));
    found.sort();
    let mut out: Vec<String> = Vec::new();
    for (_, t) in found {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(40);
    out
}

/// 技能修订时追加到系统提示里的整份技能说明
pub fn revise_block(s: &Skill) -> String {
    format!(
        "【本次修订使用的技能：《{}》】\n技能里如果要求输出检测报告、修改清单或分步过程，这里一律省略，只输出修订后的正文；技能里提到的脚本、参考文件和子代理在这里都用不了，忽略即可。\n\n{}",
        s.name,
        clip(&s.body(), REVISE_MAX)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_front_matter_variants() {
        let p = parse("---\nname: humanizer\nversion: 2.8.2\ndescription: |\n  Remove signs of AI writing.\n  Keeps meaning.\nlicense: MIT\nallowed-tools:\n  - Read\n---\n# Humanizer\n\nBody here.");
        assert_eq!(p.name, "humanizer");
        assert_eq!(p.description, "Remove signs of AI writing.\nKeeps meaning.");
        assert!(p.body.starts_with("# Humanizer"));

        let p = parse("\u{feff}---\r\nname: story-deslop\r\ndescription: \"网文去AI味。触发方式：/去AI味\"\r\nmetadata: {\"a\":1}\r\n---\r\n正文");
        assert_eq!(p.name, "story-deslop");
        assert_eq!(p.description, "网文去AI味。触发方式：/去AI味");
        assert_eq!(p.body, "正文");

        let p = parse("# 对话加强\n\n让人物说话更像人。\n\n## 写作规则\n少用说道。");
        assert_eq!(p.name, "对话加强");
        assert_eq!(p.description, "让人物说话更像人。");
    }

    #[test]
    fn extracts_sections() {
        let body = "# 技能\n\n## 写作规则\n1. 少用比喻\n```bash\n# 不是标题\n```\n### 细则\n细则内容\n## 修订流程\n修订";
        let rules = section(body, RULE_TITLES).unwrap();
        assert!(rules.contains("少用比喻") && rules.contains("# 不是标题") && rules.contains("细则内容"));
        assert!(!rules.contains("修订"));
        assert_eq!(section(body, &["不存在"]), None);
    }

    #[test]
    fn builtin_deslop_has_rules_and_process() {
        let s = builtins().into_iter().find(|s| s.id == DESLOP_ID).unwrap();
        assert_eq!(s.name, "网文去AI味");
        assert!(s.source.contains("story-deslop"));
        let rules = s.write_rules().unwrap();
        assert!(rules.contains("不是A，而是B") && !rules.contains("三遍法"));
        assert!(rules_block(&[s.clone()]).starts_with("【写作技能"));
        let revise = revise_block(&s);
        assert!(revise.contains("三遍法") && revise.contains("只输出修订后的正文"));
        assert!(!revise.contains("name: "), "头信息不该发给模型");
    }

    #[test]
    fn keeps_setting_terms() {
        let entry = Entry { name: "六角天平砝码".into(), aliases: "砝码".into(), ..Default::default() };
        let text = "他摸出六角天平砝码，赵陵从炼体六重跌到炼体三重。赵崇山是炼体境九重圆满。三百斤矿，两千下品灵石，三十枚中品灵石，藏在《天元度支簿》里。他走到楼的三层。";
        let keep = keep_terms(&[entry], text);
        for t in ["六角天平砝码", "砝码", "炼体六重", "炼体三重", "炼体境九重圆满", "三百斤", "两千下品灵石", "三十枚", "《天元度支簿》"] {
            assert!(keep.iter().any(|k| k == t), "缺少 {t}：{keep:?}");
        }
        assert!(keep.iter().all(|k| !k.contains("的三层")), "{keep:?}");
        assert_eq!(keep[0], "六角天平砝码", "按出现顺序排");
        let keep = keep_terms(&[], "他掏出一枚铜钱，手掌如同一层铁皮，又拿了一百斤米。");
        assert_eq!(keep, vec!["一百斤".to_string()], "一枚、如同一层不算设定");
    }

    #[test]
    fn github_urls_become_raw() {
        assert_eq!(
            raw_url("https://github.com/blader/humanizer/blob/main/SKILL.md?plain=1"),
            "https://raw.githubusercontent.com/blader/humanizer/main/SKILL.md"
        );
        assert_eq!(raw_url("https://github.com/blader/humanizer"), "https://raw.githubusercontent.com/blader/humanizer/HEAD/SKILL.md");
        assert_eq!(
            raw_url("https://github.com/o/r/tree/main/skills/story-deslop/"),
            "https://raw.githubusercontent.com/o/r/main/skills/story-deslop/SKILL.md"
        );
        assert_eq!(raw_url(" https://example.com/a.md "), "https://example.com/a.md");
    }
}
