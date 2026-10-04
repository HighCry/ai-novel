//! 内置资料：题材模板（assets/genres.json）和写作手册（assets/craft/*.md）。
//! 来源和许可证见 THIRD_PARTY_NOTICES.md。

use regex::Regex;
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Builtin;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GenreProfile {
    pub id: String,
    pub genre_name: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub core_tone: String,
    pub pacing_strategy: String,
    pub anti_patterns: Vec<String>,
    pub reference_tables: String,
}

#[derive(Deserialize)]
struct GenreFile {
    profiles: Vec<GenreProfile>,
}

pub fn genres() -> &'static [GenreProfile] {
    static G: OnceLock<Vec<GenreProfile>> = OnceLock::new();
    G.get_or_init(|| {
        Builtin::get("genres.json")
            .and_then(|f| serde_json::from_slice::<GenreFile>(&f.data).ok())
            .map(|g| g.profiles)
            .unwrap_or_default()
    })
}

/// 按作品填写的题材找最接近的模板，比如“都市脑洞”→ 都市，“末世”→ 末世流。
pub fn find_genre(text: &str) -> Option<&'static GenreProfile> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let all = genres();
    if let Some(g) = all.iter().find(|g| g.genre_name == t || g.id.eq_ignore_ascii_case(t) || g.aliases.iter().any(|a| a.eq_ignore_ascii_case(t))) {
        return Some(g);
    }
    let mut hits: Vec<&GenreProfile> = all
        .iter()
        .filter(|g| {
            let core = g.genre_name.trim_end_matches('流');
            !core.is_empty() && (t.contains(core) || core.contains(t))
        })
        .collect();
    hits.sort_by_key(|g| std::cmp::Reverse(g.genre_name.chars().count()));
    hits.into_iter().next()
}

pub fn genre_block(text: &str) -> String {
    match find_genre(text) {
        Some(g) => {
            let mut s = format!("【题材要点：{}】\n{}", g.genre_name, g.core_tone.trim());
            if !g.pacing_strategy.trim().is_empty() {
                s += &format!("\n{}", g.pacing_strategy.trim());
            }
            if !g.anti_patterns.is_empty() {
                s += &format!("\n要避开的套路：{}", g.anti_patterns.join("；"));
            }
            s
        }
        None => String::new(),
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CraftDoc {
    pub id: &'static str,
    pub title: &'static str,
    pub source: &'static str,
    pub license: &'static str,
}

const WW: &str = "webnovel-writer";
const NS: &str = "novel-studio";

pub const CRAFT: &[CraftDoc] = &[
    CraftDoc { id: "cool-points", title: "爽点设计", source: WW, license: "GPL-3.0" },
    CraftDoc { id: "chapter-planning", title: "章节规划", source: WW, license: "GPL-3.0" },
    CraftDoc { id: "conflict-design", title: "冲突设计", source: WW, license: "GPL-3.0" },
    CraftDoc { id: "strand-weave", title: "多线交织", source: WW, license: "GPL-3.0" },
    CraftDoc { id: "polish-guide", title: "润色指南", source: WW, license: "GPL-3.0" },
    CraftDoc { id: "hooks-chapter", title: "章首章尾钩子", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "hooks-suspense", title: "悬念设计", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "natural-prose", title: "自然文笔：AI 腔自查", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "style-craft", title: "文风技法", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "emotion-system", title: "情绪节奏", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "combat-face", title: "战斗与打脸", source: NS, license: "Apache-2.0" },
    CraftDoc { id: "genre-formulas", title: "题材写作公式", source: NS, license: "Apache-2.0" },
];

pub fn craft_text(id: &str) -> Option<String> {
    static COMMENT: OnceLock<Regex> = OnceLock::new();
    CRAFT.iter().find(|c| c.id == id)?;
    let raw = Builtin::get(&format!("craft/{id}.md"))?;
    let text = String::from_utf8_lossy(&raw.data).into_owned();
    let re = COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    Some(re.replace_all(&text, "").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_genres_and_matches() {
        assert_eq!(genres().len(), 37);
        assert_eq!(find_genre("都市脑洞").unwrap().genre_name, "都市");
        assert_eq!(find_genre("末世").unwrap().genre_name, "末世流");
        assert_eq!(find_genre("xuanhuan").unwrap().genre_name, "玄幻");
        assert!(find_genre("").is_none());
        assert!(genre_block("修仙").contains("要避开的套路"));
    }

    #[test]
    fn loads_craft_docs() {
        for c in CRAFT {
            let text = craft_text(c.id).unwrap_or_else(|| panic!("缺少手册 {}", c.id));
            assert!(text.len() > 500, "{} 内容太短", c.id);
            assert!(!text.contains("<!--"));
        }
        assert!(craft_text("../Cargo.toml").is_none());
    }
}
