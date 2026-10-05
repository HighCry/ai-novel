use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 人物的结构化状态字段（参考 StoryForge 的角色动态状态，另加知识边界）。
pub const CHAR_FIELDS: [(&str, &str); 8] = [
    ("location", "位置"),
    ("power", "实力"),
    ("body", "身体"),
    ("mind", "心理"),
    ("items", "关键物品"),
    ("recent", "近期经历"),
    ("knows", "知道的秘密"),
    ("unaware", "还不知道的事"),
];

pub fn render_fields(fields: &BTreeMap<String, String>) -> String {
    let label = |k: &str| CHAR_FIELDS.iter().find(|(key, _)| *key == k).map(|(_, l)| *l);
    let mut parts: Vec<String> = CHAR_FIELDS
        .iter()
        .filter_map(|(k, l)| fields.get(*k).map(|v| v.trim()).filter(|v| !v.is_empty()).map(|v| format!("{l}：{v}")))
        .collect();
    parts.extend(
        fields
            .iter()
            .filter(|(k, v)| label(k).is_none() && !v.trim().is_empty())
            .map(|(k, v)| format!("{k}：{}", v.trim())),
    );
    parts.join("；")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub genre: String,
    pub platform: String,
    pub logline: String,
    pub synopsis: String,
    pub worldview: String,
    pub outline: String,
    pub style_guide: String,
    pub style_sample: String,
    pub target_words: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BookCard {
    pub id: i64,
    pub title: String,
    pub genre: String,
    pub platform: String,
    pub logline: String,
    pub chapter_count: i64,
    pub word_count: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Volume {
    pub id: i64,
    pub book_id: i64,
    pub title: String,
    pub outline: String,
    pub summary: String,
    pub sort: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Chapter {
    pub id: i64,
    pub book_id: i64,
    pub volume_id: Option<i64>,
    pub sort: i64,
    pub title: String,
    pub outline: String,
    pub content: String,
    pub summary: String,
    pub status: String,
    pub word_count: i64,
    pub ai_chars: i64,
    /// 本章节拍：先规划场景，再写正文
    pub beats: String,
    /// 定稿时的张力/追读力分析（JSON）
    pub metrics: Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChapterMeta {
    pub id: i64,
    pub volume_id: Option<i64>,
    pub sort: i64,
    pub number: Option<i64>,
    pub title: String,
    pub status: String,
    pub word_count: i64,
    pub ai_chars: i64,
    pub has_outline: bool,
    pub has_summary: bool,
    pub has_beats: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Version {
    pub id: i64,
    pub chapter_id: i64,
    pub content: String,
    pub note: String,
    pub word_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entry {
    pub id: i64,
    pub book_id: i64,
    pub kind: String,
    pub name: String,
    pub aliases: String,
    pub description: String,
    pub state: String,
    pub immutable: String,
    pub always_include: bool,
    /// 主角 / 重要配角 / 配角 / 反派 / 路人，用于连续性检查判断谁不该长期缺席
    pub role: String,
    /// 人物的结构化状态（最新），键见 CHAR_FIELDS
    pub fields: BTreeMap<String, String>,
    pub updated_at: i64,
    /// 什么时候可以给模型看：空为公开，或「第2卷起」「第91章起」「仅规划」「对AI隐藏」
    pub visibility: String,
    /// 作者底牌：真实身份、后期反转这类读者暂时不能知道的设定，任何任务都不发给模型
    pub secret: String,
}

/// 设定在某一章的状态快照。phase = start 表示从这一章开始生效（手动修改），
/// end 表示这一章结束时的状态（定稿提取）。chapter_id 为空表示初始状态。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EntryState {
    pub id: i64,
    pub entry_id: i64,
    pub chapter_id: Option<i64>,
    pub phase: String,
    pub state: String,
    pub fields: BTreeMap<String, String>,
    pub created_at: i64,
}

impl EntryState {
    pub fn text(&self) -> String {
        let f = render_fields(&self.fields);
        if f.is_empty() { self.state.trim().to_string() } else { f }
    }
}

impl Entry {
    /// 最新状态文字：有结构化字段时用字段，否则用自由文本。
    pub fn state_text(&self) -> String {
        let f = render_fields(&self.fields);
        if f.is_empty() { self.state.trim().to_string() } else { f }
    }

    pub fn is_major(&self) -> bool {
        self.kind == "character" && (self.always_include || matches!(self.role.as_str(), "主角" | "重要配角" | "反派"))
    }

    /// 名称和别名，用于在正文、章纲里匹配出场的设定。
    pub fn keywords(&self) -> Vec<String> {
        let mut out = vec![self.name.trim().to_string()];
        let seps = |c: char| matches!(c, ',' | '，' | '、' | ';' | '；' | '/' | '|') || c.is_whitespace();
        for alias in self.aliases.split(seps) {
            let alias = alias.trim();
            if !alias.is_empty() && !out.iter().any(|k| k == alias) {
                out.push(alias.to_string());
            }
        }
        out.retain(|k| !k.is_empty());
        out
    }

    pub fn kind_label(&self) -> &'static str {
        kind_label(&self.kind)
    }

    /// 写法不认识的可见性按对 AI 隐藏处理，宁可少给也不泄露。
    pub fn gate(&self) -> crate::visibility::Gate {
        crate::visibility::Gate::parse(&self.visibility).unwrap_or(crate::visibility::Gate::Hidden)
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "character" => "人物",
        "location" => "地点",
        "item" => "物品",
        "faction" => "势力",
        "concept" => "设定",
        _ => "其他",
    }
}

pub fn normalize_kind(kind: &str) -> String {
    let k = kind.trim().to_lowercase();
    let k = match k.as_str() {
        "character" | "人物" | "角色" => "character",
        "location" | "地点" | "地图" => "location",
        "item" | "物品" | "道具" => "item",
        "faction" | "势力" | "组织" | "宗门" => "faction",
        "concept" | "设定" | "体系" | "规则" => "concept",
        _ => "other",
    };
    k.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Thread {
    pub id: i64,
    pub book_id: i64,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub planted_chapter_id: Option<i64>,
    pub resolved_chapter_id: Option<i64>,
    /// 计划在第几章回收
    pub target_chapter: Option<i64>,
    /// 最近一次推进这条伏笔的章节
    pub last_chapter_id: Option<i64>,
    pub updated_at: i64,
}

/// 两个设定条目之间的关系（参考 wenmai 的关系图谱）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Relation {
    pub id: i64,
    pub book_id: i64,
    pub a_id: i64,
    pub b_id: i64,
    /// 师徒、敌对、恋人、盟友、亲属、上下级……
    pub kind: String,
    pub detail: String,
    /// active / ended
    pub status: String,
    pub since_chapter_id: Option<i64>,
    pub updated_at: i64,
}

/// 文风库里的一篇范文
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibItem {
    pub id: i64,
    pub title: String,
    /// 出处或作者
    pub source: String,
    pub genre: String,
    pub tags: Vec<String>,
    /// 作者觉得它好在哪里
    pub note: String,
    pub content: String,
    /// AI 分析出的写法要点
    pub analysis: String,
    pub enabled: bool,
    /// 从自己作品里收藏的片段记下来源，检查照抄时跳过这本书
    pub book_id: Option<i64>,
    pub word_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for LibItem {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            source: String::new(),
            genre: String::new(),
            tags: vec![],
            note: String::new(),
            content: String::new(),
            analysis: String::new(),
            enabled: true,
            book_id: None,
            word_count: 0,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// 范文切出来的检索片段
#[derive(Debug, Clone, Serialize, Default)]
pub struct LibChunk {
    pub id: i64,
    pub item_id: i64,
    pub seq: i64,
    pub text: String,
    pub tags: Vec<String>,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
    pub emb_model: String,
}

/// 文风指南的一个版本，genre 为空表示通用
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StyleGuide {
    pub id: i64,
    pub genre: String,
    pub content: String,
    pub note: String,
    pub created_at: i64,
}

/// 可选的向量检索接口（OpenAI 兼容的 /embeddings）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EmbedRole {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRole {
    pub provider_id: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    /// 每百万输入 token 的价格（元），用于费用统计
    pub input_price: f64,
    /// 每百万输出 token 的价格（元）
    pub output_price: f64,
}

impl Default for ModelRole {
    fn default() -> Self {
        Self { provider_id: String::new(), model: String::new(), temperature: 0.8, max_tokens: 8192, input_price: 0.0, output_price: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Writer,
    Analyst,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub providers: Vec<Provider>,
    /// 写正文、续写、润色用的模型
    pub writer: ModelRole,
    /// 摘要、提取设定、一致性检查用的模型，可以选便宜的
    pub analyst: ModelRole,
    /// 每次生成时注入的上下文上限（字）
    pub context_budget: usize,
    pub json_mode: bool,
    pub extra_cliches: Vec<String>,
    pub min_chapter_words: i64,
    pub max_chapter_words: i64,
    /// 流式请求时附带 stream_options.include_usage，让接口返回真实 token 用量
    pub stream_usage: bool,
    /// 写正文时从文风库检索几段范文作为参考，0 表示不参考
    pub library_refs: usize,
    /// 写正文时带上文风指南
    pub use_style_guide: bool,
    /// 向量检索（可选）：没配置时文风库只用关键词、标签和题材检索
    pub embedding: EmbedRole,
    /// 启用的写作技能 id，按顺序注入写作规则（内置的「网文去AI味」默认启用）
    pub skills: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            providers: vec![],
            writer: ModelRole { temperature: 0.85, max_tokens: 8192, ..Default::default() },
            analyst: ModelRole { temperature: 0.3, max_tokens: 4096, ..Default::default() },
            context_budget: 12000,
            json_mode: false,
            extra_cliches: vec![],
            min_chapter_words: 2000,
            max_chapter_words: 6000,
            stream_usage: false,
            library_refs: 2,
            use_style_guide: true,
            embedding: EmbedRole::default(),
            skills: vec![crate::skills::DESLOP_ID.to_string()],
        }
    }
}

pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    match chars.len() {
        0 => String::new(),
        1..=8 => "****".to_string(),
        n => format!(
            "{}****{}",
            chars[..4].iter().collect::<String>(),
            chars[n - 4..].iter().collect::<String>()
        ),
    }
}

impl Settings {
    /// 返回给前端时隐藏密钥。
    pub fn masked(&self) -> Self {
        let mut s = self.clone();
        for p in &mut s.providers {
            p.api_key = mask_key(&p.api_key);
        }
        s
    }

    /// 前端回传的是打码后的密钥时，沿用原来保存的密钥。
    pub fn restore_masked_keys(&mut self, old: &Settings) {
        for p in &mut self.providers {
            if p.api_key.contains("****") {
                p.api_key = old
                    .providers
                    .iter()
                    .find(|o| o.id == p.id)
                    .map(|o| o.api_key.clone())
                    .unwrap_or_default();
            }
        }
    }

    pub fn resolve(&self, role: Role) -> anyhow::Result<(ModelRole, Provider)> {
        let (primary, fallback) = match role {
            Role::Writer => (&self.writer, &self.analyst),
            Role::Analyst => (&self.analyst, &self.writer),
        };
        for candidate in [primary, fallback] {
            if candidate.model.trim().is_empty() {
                continue;
            }
            let provider = self
                .providers
                .iter()
                .find(|p| p.id == candidate.provider_id)
                .or_else(|| self.providers.first());
            if let Some(p) = provider {
                let mut cfg = candidate.clone();
                cfg.temperature = primary.temperature;
                if primary.max_tokens > 0 || std::ptr::eq(candidate, primary) {
                    cfg.max_tokens = primary.max_tokens;
                }
                return Ok((cfg, p.clone()));
            }
        }
        anyhow::bail!("还没有配置模型：请先在「设置」里添加接口，并选择写作模型")
    }

    /// 配置了向量模型时返回 (模型名, 接口)
    pub fn resolve_embedding(&self) -> Option<(String, Provider)> {
        let model = self.embedding.model.trim();
        if model.is_empty() {
            return None;
        }
        let provider = self.providers.iter().find(|p| p.id == self.embedding.provider_id).or_else(|| self.providers.first())?;
        Some((model.to_string(), provider.clone()))
    }
}
