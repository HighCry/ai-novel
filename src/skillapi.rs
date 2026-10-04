//! 写作技能接口：列表、导入（粘贴 / 文件 / 网址）、修改、删除、启用。

use crate::api::{bad_request, not_found, ApiResult, AppError};
use crate::db::now;
use crate::skills::{self, Skill};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

/// 技能说明书再长也不会超过这个大小；超过多半是贴错了东西
const MAX_BYTES: usize = 512 * 1024;

#[derive(Serialize)]
pub struct SkillCard {
    id: String,
    name: String,
    description: String,
    source: String,
    builtin: bool,
    active: bool,
    has_rules: bool,
    chars: usize,
}

fn card(s: &Skill, active: &[String]) -> SkillCard {
    SkillCard {
        id: s.id.clone(),
        name: s.name.clone(),
        description: s.description.clone(),
        source: s.source.clone(),
        builtin: s.builtin,
        active: active.contains(&s.id),
        has_rules: s.write_rules().is_some(),
        chars: s.markdown.chars().count(),
    }
}

pub async fn list_skills(State(st): State<AppState>) -> ApiResult<Vec<SkillCard>> {
    let active = st.db.get_settings()?.skills;
    Ok(Json(skills::all(&st.db)?.iter().map(|s| card(s, &active)).collect()))
}

pub async fn get_skill(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let s = skills::find(&st.db, &id)?.ok_or_else(|| not_found("技能"))?;
    let active = st.db.get_settings()?.skills;
    Ok(Json(json!({ "card": card(&s, &active), "markdown": s.markdown, "rules": s.write_rules() })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SkillInput {
    pub markdown: String,
    /// 网址导入：GitHub 的文件、目录或仓库地址都行
    pub url: String,
    /// 来源说明，比如上传的文件名
    pub source: String,
}

async fn fetch_markdown(url: &str) -> Result<(String, String), AppError> {
    let raw = skills::raw_url(url);
    if !(raw.starts_with("https://") || raw.starts_with("http://")) {
        return Err(bad_request("请填 http:// 或 https:// 开头的网址"));
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?;
    let resp = client.get(&raw).send().await.map_err(|e| bad_request(format!("下载失败：{e}（{raw}）")))?;
    if !resp.status().is_success() {
        return Err(bad_request(format!("下载失败：{}（{raw}）", resp.status())));
    }
    let bytes = resp.bytes().await.map_err(|e| bad_request(format!("下载失败：{e}")))?;
    if bytes.len() > MAX_BYTES {
        return Err(bad_request("文件超过 512KB，不像是技能说明书"));
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| bad_request("下载到的不是 UTF-8 文本"))?;
    Ok((text, raw))
}

fn check_markdown(md: &str) -> Result<(), AppError> {
    if md.trim().is_empty() {
        return Err(bad_request("技能内容是空的"));
    }
    if md.len() > MAX_BYTES {
        return Err(bad_request("技能内容超过 512KB"));
    }
    let head = md.trim_start().to_ascii_lowercase();
    if head.starts_with("<!doctype") || head.starts_with("<html") {
        return Err(bad_request("拿到的是网页而不是 Markdown；GitHub 上请复制文件的地址，或者点「Raw」后再复制"));
    }
    Ok(())
}

pub async fn create_skill(State(st): State<AppState>, Json(input): Json<SkillInput>) -> ApiResult<Value> {
    let (markdown, source) = if input.url.trim().is_empty() {
        let source = if input.source.trim().is_empty() { "粘贴".to_string() } else { input.source.trim().to_string() };
        (input.markdown, source)
    } else {
        fetch_markdown(&input.url).await?
    };
    check_markdown(&markdown)?;
    let base = format!("my-{}", now());
    let mut id = base.clone();
    let mut n = 1;
    while skills::find(&st.db, &id)?.is_some() {
        n += 1;
        id = format!("{base}-{n}");
    }
    let skill = Skill::from_markdown(&id, &markdown, &source, false, now());
    st.db.save_skill(&skill)?;
    let active = st.db.get_settings()?.skills;
    Ok(Json(json!({ "card": card(&skill, &active), "markdown": skill.markdown })))
}

pub async fn update_skill(State(st): State<AppState>, Path(id): Path<String>, Json(input): Json<SkillInput>) -> ApiResult<Value> {
    let old = st.db.get_skill(&id)?.ok_or_else(|| {
        if skills::builtins().iter().any(|s| s.id == id) { bad_request("内置技能不能直接修改，可以先复制一份再改") } else { not_found("技能") }
    })?;
    check_markdown(&input.markdown)?;
    let skill = Skill::from_markdown(&id, &input.markdown, &old.source, false, now());
    st.db.save_skill(&skill)?;
    let active = st.db.get_settings()?.skills;
    Ok(Json(json!({ "card": card(&skill, &active), "markdown": skill.markdown })))
}

pub async fn delete_skill(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if skills::builtins().iter().any(|s| s.id == id) {
        return Err(bad_request("内置技能不能删除，不想用可以停用"));
    }
    if !st.db.delete_skill(&id)? {
        return Err(not_found("技能"));
    }
    let mut s = st.db.get_settings()?;
    s.skills.retain(|x| x != &id);
    st.db.save_settings(&s)?;
    Ok(Json(json!({ "ok": true, "active": s.skills })))
}

#[derive(Deserialize)]
pub struct ActiveInput {
    pub active: bool,
}

/// 返回最新的启用列表，前端拿它更新缓存的设置，免得之后保存设置时用旧值覆盖
pub async fn set_active(State(st): State<AppState>, Path(id): Path<String>, Json(input): Json<ActiveInput>) -> ApiResult<Value> {
    skills::find(&st.db, &id)?.ok_or_else(|| not_found("技能"))?;
    let mut s = st.db.get_settings()?;
    s.skills.retain(|x| x != &id);
    if input.active {
        s.skills.push(id);
    }
    st.db.save_settings(&s)?;
    Ok(Json(json!({ "active": s.skills })))
}
