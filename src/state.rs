use crate::db::Db;
use crate::llm::LlmClient;
use crate::stylelib::LibIndex;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub llm: LlmClient,
    /// 设置后所有请求都需要 HTTP Basic 认证（用户名任意）
    pub password: Option<String>,
    /// 文风库检索索引的缓存
    pub lib_index: Arc<Mutex<Option<Arc<LibIndex>>>>,
}

impl AppState {
    pub fn new(db: Db, password: Option<String>) -> Self {
        Self { db, llm: LlmClient::new(), password: password.filter(|p| !p.is_empty()), lib_index: Arc::default() }
    }

    /// 文风库没改动时复用缓存的索引
    pub fn library_index(&self) -> anyhow::Result<Arc<LibIndex>> {
        let rev = self.db.lib_rev();
        let mut cache = self.lib_index.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ix) = cache.as_ref().filter(|ix| ix.rev == rev) {
            return Ok(ix.clone());
        }
        let ix = Arc::new(LibIndex::build(rev, self.db.lib_index_rows()?));
        *cache = Some(ix.clone());
        Ok(ix)
    }
}
