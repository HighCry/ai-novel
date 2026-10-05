//! 自动备份：每天第一次打开、之后每满一天，用 VACUUM INTO 给整库拍一份快照，放进数据目录下的 backups，
//! 只保留最近几份；设置里可以立即备份、还原，也可以每次再复制一份到网盘同步文件夹。
//! 桌面版和手机访问的后台服务可能同时开着，「上次自动备份」记在库里，两边加起来一天只拍一次。

use crate::api::{bad_request, not_found, ApiResult, AppError};
use crate::db::{now, Db};
use crate::state::AppState;
use anyhow::{Context, Result};
use axum::extract::{Path as UrlPath, State};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const LAST_AUTO: &str = "backup_last_auto";
/// 比一天略短：每天差不多同一时间打开也能赶上
const AUTO_INTERVAL: i64 = 20 * 3600;
const KINDS: [&str; 3] = ["auto", "manual", "before-restore"];
/// 还原前自动留的快照只留最近这么多份；手动备份不删
const KEEP_BEFORE_RESTORE: usize = 5;

#[derive(Debug, Clone, Serialize)]
pub struct BackupFile {
    pub name: String,
    /// auto 自动 / manual 手动 / before-restore 还原前
    pub kind: String,
    pub bytes: u64,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct Made {
    #[serde(flatten)]
    pub file: BackupFile,
    /// 复制到同步文件夹失败的原因；本地备份本身已经成功
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror_error: Option<String>,
}

pub fn dir(db: &Db) -> Option<PathBuf> {
    Some(db.file_path()?.parent()?.join("backups"))
}

fn kind_of(name: &str) -> Option<&'static str> {
    KINDS.iter().copied().find(|k| name.starts_with(&format!("{k}-")) && name.ends_with(".db"))
}

fn files_in(dir: &Path) -> Vec<BackupFile> {
    let mut out: Vec<BackupFile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let kind = kind_of(&name)?;
            let meta = e.metadata().ok().filter(|m| m.is_file())?;
            let created_at = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            Some(BackupFile { name, kind: kind.into(), bytes: meta.len(), created_at })
        })
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.name.cmp(&a.name)));
    out
}

/// 新的在前
pub fn list(db: &Db) -> Vec<BackupFile> {
    dir(db).map(|d| files_in(&d)).unwrap_or_default()
}

/// 自动备份只留最近 keep 份，还原前的快照留 KEEP_BEFORE_RESTORE 份
fn prune(dir: &Path, keep: usize) {
    let files = files_in(dir);
    for (kind, limit) in [("auto", keep.max(1)), ("before-restore", KEEP_BEFORE_RESTORE)] {
        for f in files.iter().filter(|f| f.kind == kind).skip(limit) {
            if let Err(e) = std::fs::remove_file(dir.join(&f.name)) {
                tracing::warn!("删除旧备份 {} 失败：{e}", f.name);
            }
        }
    }
}

fn valid_name(name: &str) -> bool {
    kind_of(name).is_some() && !name.contains(['/', '\\']) && !name.contains("..")
}

pub fn make(db: &Db, kind: &str) -> Result<Made> {
    let d = dir(db).context("内存数据库不能备份")?;
    std::fs::create_dir_all(&d).with_context(|| format!("创建备份目录 {} 失败", d.display()))?;
    let name = format!("{kind}-{}.db", db.local_stamp()?);
    let path = d.join(&name);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    db.snapshot_to(&path).context("拍快照失败")?;
    let settings = db.get_settings()?;
    prune(&d, settings.backup_keep);
    let mirror = settings.backup_mirror.trim();
    let mirror_error = (!mirror.is_empty())
        .then(|| {
            let target = Path::new(mirror);
            std::fs::create_dir_all(target)
                .and_then(|_| std::fs::copy(&path, target.join(&name)))
                .map(|_| prune(target, settings.backup_keep))
                .err()
                .map(|e| format!("复制到 {mirror} 失败：{e}"))
        })
        .flatten();
    if let Some(e) = &mirror_error {
        tracing::warn!("{e}");
    }
    let file = files_in(&d).into_iter().find(|f| f.name == name).context("备份文件没写出来")?;
    Ok(Made { file, mirror_error })
}

/// 先给当前库留一份「还原前」快照，再把选中的备份整份写回；返回那份快照
pub fn restore(db: &Db, name: &str) -> Result<Made, AppError> {
    if !valid_name(name) {
        return Err(bad_request("备份文件名不对"));
    }
    let src = dir(db).map(|d| d.join(name)).filter(|p| p.is_file()).ok_or_else(|| not_found("备份"))?;
    let safety = make(db, "before-restore")?;
    db.restore_from(&src)?;
    db.set_kv(LAST_AUTO, &now().to_string())?;
    Ok(safety)
}

/// 距上次自动备份超过 AUTO_INTERVAL 才拍；先记时间再拍，另一个进程同时检查时不会重复
pub fn auto_once(db: &Db) -> Result<Option<Made>> {
    if db.file_path().is_none() {
        return Ok(None);
    }
    let last = db.get_kv(LAST_AUTO)?.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    if now() - last < AUTO_INTERVAL {
        return Ok(None);
    }
    db.set_kv(LAST_AUTO, &now().to_string())?;
    let made = make(db, "auto")?;
    tracing::info!("自动备份：{}", made.file.name);
    Ok(Some(made))
}

/// 启动时检查一次，之后每小时检查一次
pub async fn auto_loop(db: Db) {
    loop {
        let d = db.clone();
        match tokio::task::spawn_blocking(move || auto_once(&d)).await {
            Ok(Err(e)) => tracing::warn!("自动备份失败：{e:#}"),
            Err(e) => tracing::warn!("自动备份任务异常：{e}"),
            Ok(Ok(_)) => {}
        }
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

// ---------- 接口 ----------

pub async fn list_backups(State(st): State<AppState>) -> ApiResult<Value> {
    let s = st.db.get_settings()?;
    Ok(Json(json!({
        "dir": dir(&st.db).map(|d| d.display().to_string()),
        "files": list(&st.db),
        "last_auto": st.db.get_kv(LAST_AUTO)?.and_then(|v| v.parse::<i64>().ok()),
        "keep": s.backup_keep,
        "mirror": s.backup_mirror,
    })))
}

pub async fn create_backup(State(st): State<AppState>) -> ApiResult<Made> {
    Ok(Json(make(&st.db, "manual")?))
}

pub async fn restore_backup(State(st): State<AppState>, UrlPath(name): UrlPath<String>) -> ApiResult<Value> {
    let safety = restore(&st.db, &name)?;
    Ok(Json(json!({ "ok": true, "safety": safety.file.name })))
}

pub async fn delete_backup(State(st): State<AppState>, UrlPath(name): UrlPath<String>) -> ApiResult<Value> {
    if !valid_name(&name) {
        return Err(bad_request("备份文件名不对"));
    }
    let path = dir(&st.db).map(|d| d.join(&name)).filter(|p| p.is_file()).ok_or_else(|| not_found("备份"))?;
    std::fs::remove_file(path)?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Book;

    fn book(db: &Db, title: &str) -> i64 {
        db.create_book(&Book { title: title.into(), ..Default::default() }).unwrap().id
    }

    #[test]
    fn snapshot_restore_and_prune() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("ai-novel.db")).unwrap();
        book(&db, "第一本");
        let made = make(&db, "manual").unwrap();
        assert!(made.file.bytes > 0 && made.mirror_error.is_none());

        book(&db, "第二本");
        let safety = restore(&db, &made.file.name).unwrap();
        assert_eq!(safety.file.kind, "before-restore");
        let titles: Vec<String> = db.list_books().unwrap().into_iter().map(|b| b.title).collect();
        assert_eq!(titles, vec!["第一本"], "还原后回到备份时的样子");

        let mut s = db.get_settings().unwrap();
        s.backup_keep = 2;
        s.backup_mirror = tmp.path().join("网盘").display().to_string();
        db.save_settings(&s).unwrap();
        for i in 0..4 {
            std::fs::write(dir(&db).unwrap().join(format!("auto-2026010{i}-000000.db")), b"x").unwrap();
        }
        let made = make(&db, "auto").unwrap();
        assert!(made.mirror_error.is_none());
        let autos = list(&db).into_iter().filter(|f| f.kind == "auto").count();
        assert_eq!(autos, 2, "自动备份只留最近 2 份");
        assert!(list(&db).iter().any(|f| f.kind == "manual"), "手动备份不删");
        assert!(tmp.path().join("网盘").join(&made.file.name).is_file(), "同步文件夹里也有一份");

        assert!(restore(&db, "../ai-novel.db").is_err());
    }

    #[test]
    fn auto_backup_once_a_day() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("ai-novel.db")).unwrap();
        assert!(auto_once(&db).unwrap().is_some());
        assert!(auto_once(&db).unwrap().is_none(), "一天之内不重复备份");
        assert!(auto_once(&Db::open_in_memory().unwrap()).unwrap().is_none(), "内存库不备份");
    }
}
