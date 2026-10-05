use crate::models::*;
use crate::skills::Skill;
use crate::text::{count_words, number_chapters};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS books (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  genre TEXT NOT NULL DEFAULT '',
  platform TEXT NOT NULL DEFAULT '',
  logline TEXT NOT NULL DEFAULT '',
  synopsis TEXT NOT NULL DEFAULT '',
  worldview TEXT NOT NULL DEFAULT '',
  outline TEXT NOT NULL DEFAULT '',
  style_guide TEXT NOT NULL DEFAULT '',
  style_sample TEXT NOT NULL DEFAULT '',
  target_words INTEGER NOT NULL DEFAULT 2500,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS volumes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  outline TEXT NOT NULL DEFAULT '',
  summary TEXT NOT NULL DEFAULT '',
  sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS chapters (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  volume_id INTEGER REFERENCES volumes(id) ON DELETE SET NULL,
  sort INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL DEFAULT '',
  outline TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL DEFAULT '',
  summary TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'draft',
  word_count INTEGER NOT NULL DEFAULT 0,
  ai_chars INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chapters_book ON chapters(book_id, sort);
CREATE TABLE IF NOT EXISTS versions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  chapter_id INTEGER NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
  content TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  word_count INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_versions_chapter ON versions(chapter_id, id);
CREATE TABLE IF NOT EXISTS entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  kind TEXT NOT NULL DEFAULT 'character',
  name TEXT NOT NULL,
  aliases TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL DEFAULT '',
  immutable TEXT NOT NULL DEFAULT '',
  always_include INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entries_book ON entries(book_id);
CREATE TABLE IF NOT EXISTS threads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'open',
  planted_chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  resolved_chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_threads_book ON threads(book_id);
CREATE TABLE IF NOT EXISTS daily_words (
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  day TEXT NOT NULL,
  words INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (book_id, day)
);
CREATE TABLE IF NOT EXISTS ai_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
  chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  task TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '',
  input_chars INTEGER NOT NULL DEFAULT 0,
  output_chars INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ailog_book ON ai_log(book_id, chapter_id);
"#;

const SCHEMA_V2: &str = r#"
ALTER TABLE chapters ADD COLUMN beats TEXT NOT NULL DEFAULT '';
ALTER TABLE chapters ADD COLUMN metrics TEXT NOT NULL DEFAULT '';
ALTER TABLE entries ADD COLUMN role TEXT NOT NULL DEFAULT '';
ALTER TABLE entries ADD COLUMN fields TEXT NOT NULL DEFAULT '';
ALTER TABLE threads ADD COLUMN target_chapter INTEGER;
ALTER TABLE threads ADD COLUMN last_chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL;
ALTER TABLE ai_log ADD COLUMN prompt_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ai_log ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ai_log ADD COLUMN estimated INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS entry_states (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  chapter_id INTEGER REFERENCES chapters(id) ON DELETE CASCADE,
  phase TEXT NOT NULL DEFAULT 'end',
  state TEXT NOT NULL DEFAULT '',
  fields TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entry_states ON entry_states(entry_id);
"#;

const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS relations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  a_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  b_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  kind TEXT NOT NULL DEFAULT '',
  detail TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'active',
  since_chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_relations_book ON relations(book_id);
"#;

const SCHEMA_V4: &str = r#"
CREATE TABLE IF NOT EXISTS library_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT '',
  genre TEXT NOT NULL DEFAULT '',
  tags TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL DEFAULT '',
  analysis TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 1,
  book_id INTEGER REFERENCES books(id) ON DELETE SET NULL,
  word_count INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS library_chunks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id INTEGER NOT NULL REFERENCES library_items(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  text TEXT NOT NULL,
  tags TEXT NOT NULL DEFAULT '',
  embedding BLOB,
  emb_model TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_library_chunks ON library_chunks(item_id);
CREATE TABLE IF NOT EXISTS style_guides (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  genre TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ai_accepts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
  chapter_id INTEGER REFERENCES chapters(id) ON DELETE CASCADE,
  task TEXT NOT NULL DEFAULT '',
  text TEXT NOT NULL,
  before_ctx TEXT NOT NULL DEFAULT '',
  after_ctx TEXT NOT NULL DEFAULT '',
  learned INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ai_accepts ON ai_accepts(chapter_id);
"#;

const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS skills (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT '',
  markdown TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
"#;

const SCHEMA_V6: &str = r#"
ALTER TABLE entries ADD COLUMN visibility TEXT NOT NULL DEFAULT '';
ALTER TABLE entries ADD COLUMN secret TEXT NOT NULL DEFAULT '';
"#;

const SCHEMA_V7: &str = r#"
CREATE TABLE IF NOT EXISTS reveals (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  truth TEXT NOT NULL DEFAULT '',
  misread TEXT NOT NULL DEFAULT '',
  gap TEXT NOT NULL DEFAULT '',
  terms TEXT NOT NULL DEFAULT '',
  exceptions TEXT NOT NULL DEFAULT '',
  entry_ids TEXT NOT NULL DEFAULT '',
  seed_at INTEGER,
  clue_at INTEGER,
  reveal_at INTEGER,
  seed_note TEXT NOT NULL DEFAULT '',
  clue_note TEXT NOT NULL DEFAULT '',
  payoff TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'active',
  sort INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_reveals_book ON reveals(book_id);
CREATE TABLE IF NOT EXISTS reveal_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  reveal_id INTEGER NOT NULL REFERENCES reveals(id) ON DELETE CASCADE,
  chapter_id INTEGER NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
  step TEXT NOT NULL DEFAULT 'clue',
  quote TEXT NOT NULL DEFAULT '',
  note TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_reveal_events ON reveal_events(reveal_id);
CREATE TABLE IF NOT EXISTS progressions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  gate TEXT NOT NULL DEFAULT '',
  mode TEXT NOT NULL DEFAULT 'add',
  text TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_progressions ON progressions(entry_id);
"#;

/// 存稿和发布、设定名排除词、开篇体检用的金手指名
const SCHEMA_V8: &str = r#"
ALTER TABLE chapters ADD COLUMN published_at INTEGER;
ALTER TABLE entries ADD COLUMN exclude TEXT NOT NULL DEFAULT '';
ALTER TABLE books ADD COLUMN update_target INTEGER NOT NULL DEFAULT 0;
ALTER TABLE books ADD COLUMN golden_finger TEXT NOT NULL DEFAULT '';
"#;

const SCHEMA_V9: &str = r#"
ALTER TABLE chapters ADD COLUMN story_time TEXT NOT NULL DEFAULT '';
ALTER TABLE chapters ADD COLUMN story_day INTEGER;
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  kind TEXT NOT NULL DEFAULT 'event',
  title TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '',
  who TEXT NOT NULL DEFAULT '',
  story_time TEXT NOT NULL DEFAULT '',
  day INTEGER,
  status TEXT NOT NULL DEFAULT '',
  done_chapter_id INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_book ON events(book_id);
"#;

const MIGRATIONS: [&str; 8] = [SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9];

const LIB_COLS: &str = "id, title, source, genre, tags, note, content, analysis, enabled, book_id, word_count, created_at, updated_at";

fn lib_item_row(r: &Row) -> rusqlite::Result<LibItem> {
    Ok(LibItem {
        id: r.get(0)?,
        title: r.get(1)?,
        source: r.get(2)?,
        genre: r.get(3)?,
        tags: json_vec(r.get(4)?),
        note: r.get(5)?,
        content: r.get(6)?,
        analysis: r.get(7)?,
        enabled: r.get(8)?,
        book_id: r.get(9)?,
        word_count: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

fn lib_chunk_row(r: &Row) -> rusqlite::Result<LibChunk> {
    let blob: Option<Vec<u8>> = r.get(5)?;
    Ok(LibChunk {
        id: r.get(0)?,
        item_id: r.get(1)?,
        seq: r.get(2)?,
        text: r.get(3)?,
        tags: json_vec(r.get(4)?),
        embedding: blob.filter(|b| !b.is_empty()).map(|b| blob_vec(&b)),
        emb_model: r.get(6)?,
    })
}

fn guide_row(r: &Row) -> rusqlite::Result<StyleGuide> {
    Ok(StyleGuide { id: r.get(0)?, genre: r.get(1)?, content: r.get(2)?, note: r.get(3)?, created_at: r.get(4)? })
}

fn json_vec(raw: String) -> Vec<String> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn vec_json(v: &[String]) -> String {
    let cleaned: Vec<&str> = v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if cleaned.is_empty() { String::new() } else { serde_json::to_string(&cleaned).unwrap_or_default() }
}

pub fn vec_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn blob_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// 作者采纳过的一段 AI 文字，before/after 是采纳时插入点前后的原文
#[derive(Debug, Clone, Default)]
pub struct Accepted {
    pub id: i64,
    pub chapter_id: i64,
    pub task: String,
    pub text: String,
    pub before: String,
    pub after: String,
    pub learned: bool,
}

const REL_COLS: &str = "id, book_id, a_id, b_id, kind, detail, status, since_chapter_id, updated_at";

fn relation_row(r: &Row) -> rusqlite::Result<Relation> {
    Ok(Relation {
        id: r.get(0)?,
        book_id: r.get(1)?,
        a_id: r.get(2)?,
        b_id: r.get(3)?,
        kind: r.get(4)?,
        detail: r.get(5)?,
        status: r.get(6)?,
        since_chapter_id: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn json_map(raw: String) -> BTreeMap<String, String> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn json_value(raw: String) -> serde_json::Value {
    if raw.trim().is_empty() { serde_json::Value::Null } else { serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null) }
}

fn map_json(m: &BTreeMap<String, String>) -> String {
    let cleaned: BTreeMap<&String, &String> = m.iter().filter(|(_, v)| !v.trim().is_empty()).collect();
    if cleaned.is_empty() { String::new() } else { serde_json::to_string(&cleaned).unwrap_or_default() }
}

fn value_json(v: &serde_json::Value) -> String {
    if v.is_null() { String::new() } else { v.to_string() }
}

/// 一次 AI 调用的记录
pub struct AiLog<'a> {
    pub book_id: Option<i64>,
    pub chapter_id: Option<i64>,
    pub task: &'a str,
    pub model: &'a str,
    pub input_chars: i64,
    pub output_chars: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub estimated: bool,
}

const BOOK_COLS: &str =
    "id, title, genre, platform, logline, synopsis, worldview, outline, style_guide, style_sample, target_words, created_at, updated_at, golden_finger, update_target";
const VOL_COLS: &str = "id, book_id, title, outline, summary, sort";
const CH_COLS: &str =
    "id, book_id, volume_id, sort, title, outline, content, summary, status, word_count, ai_chars, created_at, updated_at, beats, metrics, published_at, story_time, story_day";
const EVENT_COLS: &str = "id, book_id, chapter_id, kind, title, detail, who, story_time, day, status, done_chapter_id, updated_at";
const ENTRY_COLS: &str = "id, book_id, kind, name, aliases, description, state, immutable, always_include, updated_at, role, fields, visibility, secret, exclude";
const THREAD_COLS: &str = "id, book_id, title, detail, status, planted_chapter_id, resolved_chapter_id, updated_at, target_chapter, last_chapter_id";

fn book_row(r: &Row) -> rusqlite::Result<Book> {
    Ok(Book {
        id: r.get(0)?,
        title: r.get(1)?,
        genre: r.get(2)?,
        platform: r.get(3)?,
        logline: r.get(4)?,
        synopsis: r.get(5)?,
        worldview: r.get(6)?,
        outline: r.get(7)?,
        style_guide: r.get(8)?,
        style_sample: r.get(9)?,
        target_words: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        golden_finger: r.get(13)?,
        update_target: r.get(14)?,
    })
}

fn volume_row(r: &Row) -> rusqlite::Result<Volume> {
    Ok(Volume { id: r.get(0)?, book_id: r.get(1)?, title: r.get(2)?, outline: r.get(3)?, summary: r.get(4)?, sort: r.get(5)? })
}

fn chapter_row(r: &Row) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: r.get(0)?,
        book_id: r.get(1)?,
        volume_id: r.get(2)?,
        sort: r.get(3)?,
        title: r.get(4)?,
        outline: r.get(5)?,
        content: r.get(6)?,
        summary: r.get(7)?,
        status: r.get(8)?,
        word_count: r.get(9)?,
        ai_chars: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        beats: r.get(13)?,
        metrics: json_value(r.get(14)?),
        published_at: r.get(15)?,
        story_time: r.get(16)?,
        story_day: r.get(17)?,
    })
}

fn event_row(r: &Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        book_id: r.get(1)?,
        chapter_id: r.get(2)?,
        kind: r.get(3)?,
        title: r.get(4)?,
        detail: r.get(5)?,
        who: r.get(6)?,
        story_time: r.get(7)?,
        day: r.get(8)?,
        status: r.get(9)?,
        done_chapter_id: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

fn entry_row(r: &Row) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        book_id: r.get(1)?,
        kind: r.get(2)?,
        name: r.get(3)?,
        aliases: r.get(4)?,
        description: r.get(5)?,
        state: r.get(6)?,
        immutable: r.get(7)?,
        always_include: r.get(8)?,
        updated_at: r.get(9)?,
        role: r.get(10)?,
        fields: json_map(r.get(11)?),
        visibility: r.get(12)?,
        secret: r.get(13)?,
        exclude: r.get(14)?,
    })
}

fn thread_row(r: &Row) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: r.get(0)?,
        book_id: r.get(1)?,
        title: r.get(2)?,
        detail: r.get(3)?,
        status: r.get(4)?,
        planted_chapter_id: r.get(5)?,
        resolved_chapter_id: r.get(6)?,
        updated_at: r.get(7)?,
        target_chapter: r.get(8)?,
        last_chapter_id: r.get(9)?,
    })
}

const REVEAL_COLS: &str = "id, book_id, title, truth, misread, gap, terms, exceptions, entry_ids, seed_at, clue_at, reveal_at, seed_note, clue_note, payoff, status, sort, updated_at";

fn reveal_row(r: &Row) -> rusqlite::Result<Reveal> {
    let ids: String = r.get(8)?;
    Ok(Reveal {
        id: r.get(0)?,
        book_id: r.get(1)?,
        title: r.get(2)?,
        truth: r.get(3)?,
        misread: r.get(4)?,
        gap: r.get(5)?,
        terms: r.get(6)?,
        exceptions: r.get(7)?,
        entry_ids: serde_json::from_str(&ids).unwrap_or_default(),
        seed_at: r.get(9)?,
        clue_at: r.get(10)?,
        reveal_at: r.get(11)?,
        seed_note: r.get(12)?,
        clue_note: r.get(13)?,
        payoff: r.get(14)?,
        status: r.get(15)?,
        sort: r.get(16)?,
        updated_at: r.get(17)?,
    })
}

fn reveal_event_row(r: &Row) -> rusqlite::Result<RevealEvent> {
    Ok(RevealEvent { id: r.get(0)?, reveal_id: r.get(1)?, chapter_id: r.get(2)?, step: r.get(3)?, quote: r.get(4)?, note: r.get(5)?, created_at: r.get(6)? })
}

fn progression_row(r: &Row) -> rusqlite::Result<Progression> {
    Ok(Progression { id: r.get(0)?, entry_id: r.get(1)?, gate: r.get(2)?, mode: r.get(3)?, text: r.get(4)?, created_at: r.get(5)? })
}

fn state_row(r: &Row) -> rusqlite::Result<EntryState> {
    Ok(EntryState {
        id: r.get(0)?,
        entry_id: r.get(1)?,
        chapter_id: r.get(2)?,
        phase: r.get(3)?,
        state: r.get(4)?,
        fields: json_map(r.get(5)?),
        created_at: r.get(6)?,
    })
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    /// 文风库每次改动加一，检索索引据此判断是否需要重建
    lib_rev: Arc<AtomicU64>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0));
        migrate(&conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)), lib_rev: Arc::new(AtomicU64::new(1)) })
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---------- 备份 ----------

    /// 数据库文件路径；内存库返回 None
    pub fn file_path(&self) -> Option<std::path::PathBuf> {
        self.c().path().filter(|p| !p.is_empty()).map(std::path::PathBuf::from)
    }

    /// 本机时间，形如 20261005-233313（借 SQLite 的 localtime，不另引日期库）
    pub fn local_stamp(&self) -> Result<String> {
        Ok(self.c().query_row("SELECT strftime('%Y%m%d-%H%M%S', 'now', 'localtime')", [], |r| r.get(0))?)
    }

    /// VACUUM INTO 拍一份一致的快照，WAL 里还没写回的改动也在里面
    pub fn snapshot_to(&self, dst: &Path) -> Result<()> {
        self.c().execute("VACUUM INTO ?1", [dst.to_string_lossy()])?;
        Ok(())
    }

    /// 用 SQLite 在线备份接口把快照整份写回当前库；快照来自旧版本时顺带升级表结构
    pub fn restore_from(&self, src: &Path) -> Result<()> {
        let from = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut c = self.c();
        rusqlite::backup::Backup::new(&from, &mut c)?.run_to_completion(256, std::time::Duration::ZERO, None)?;
        migrate(&c)?;
        drop(c);
        self.lib_rev.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self.c().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

/// 按 user_version 补齐表结构；打开库和从备份还原后都要跑一遍
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.execute_batch("PRAGMA user_version = 1;")?;
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 2;
        if version < v {
            conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {v}; COMMIT;"))?;
        }
    }
    Ok(())
}

impl Db {
    // ---------- 作品 ----------

    pub fn list_books(&self) -> Result<Vec<BookCard>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT b.id, b.title, b.genre, b.platform, b.logline, b.updated_at, COUNT(ch.id), COALESCE(SUM(ch.word_count), 0)
             FROM books b LEFT JOIN chapters ch ON ch.book_id = b.id GROUP BY b.id ORDER BY b.updated_at DESC, b.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(BookCard {
                id: r.get(0)?,
                title: r.get(1)?,
                genre: r.get(2)?,
                platform: r.get(3)?,
                logline: r.get(4)?,
                updated_at: r.get(5)?,
                chapter_count: r.get(6)?,
                word_count: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_book(&self, id: i64) -> Result<Option<Book>> {
        let sql = format!("SELECT {BOOK_COLS} FROM books WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], book_row).optional()?)
    }

    pub fn create_book(&self, b: &Book) -> Result<Book> {
        let t = now();
        let id = {
            let c = self.c();
            c.execute(
                "INSERT INTO books (title, genre, platform, logline, synopsis, worldview, outline, style_guide, style_sample, target_words, created_at, updated_at,
                 golden_finger, update_target)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13)",
                params![
                    b.title.trim(),
                    b.genre,
                    b.platform,
                    b.logline,
                    b.synopsis,
                    b.worldview,
                    b.outline,
                    b.style_guide,
                    b.style_sample,
                    if b.target_words > 0 { b.target_words } else { 2500 },
                    t,
                    b.golden_finger.trim(),
                    b.update_target.max(0)
                ],
            )?;
            c.last_insert_rowid()
        };
        Ok(self.get_book(id)?.expect("刚插入的作品"))
    }

    pub fn update_book(&self, b: &Book) -> Result<()> {
        self.c().execute(
            "UPDATE books SET title = ?2, genre = ?3, platform = ?4, logline = ?5, synopsis = ?6, worldview = ?7, outline = ?8,
             style_guide = ?9, style_sample = ?10, target_words = ?11, updated_at = ?12, golden_finger = ?13, update_target = ?14 WHERE id = ?1",
            params![
                b.id,
                b.title.trim(),
                b.genre,
                b.platform,
                b.logline,
                b.synopsis,
                b.worldview,
                b.outline,
                b.style_guide,
                b.style_sample,
                b.target_words,
                now(),
                b.golden_finger.trim(),
                b.update_target.max(0)
            ],
        )?;
        Ok(())
    }

    pub fn touch_book(&self, id: i64) -> Result<()> {
        self.c().execute("UPDATE books SET updated_at = ?2 WHERE id = ?1", params![id, now()])?;
        Ok(())
    }

    pub fn delete_book(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM books WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 卷 ----------

    pub fn list_volumes(&self, book_id: i64) -> Result<Vec<Volume>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {VOL_COLS} FROM volumes WHERE book_id = ?1 ORDER BY sort, id"))?;
        let rows = st.query_map([book_id], volume_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_volume(&self, id: i64) -> Result<Option<Volume>> {
        let sql = format!("SELECT {VOL_COLS} FROM volumes WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], volume_row).optional()?)
    }

    pub fn create_volume(&self, v: &Volume) -> Result<Volume> {
        let id = {
            let c = self.c();
            let sort: i64 = if v.sort > 0 {
                v.sort
            } else {
                c.query_row("SELECT COALESCE(MAX(sort), 0) + 1 FROM volumes WHERE book_id = ?1", [v.book_id], |r| r.get(0))?
            };
            c.execute(
                "INSERT INTO volumes (book_id, title, outline, summary, sort) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![v.book_id, v.title.trim(), v.outline, v.summary, sort],
            )?;
            c.last_insert_rowid()
        };
        Ok(self.get_volume(id)?.expect("刚插入的卷"))
    }

    pub fn update_volume(&self, v: &Volume) -> Result<()> {
        self.c().execute(
            "UPDATE volumes SET title = ?2, outline = ?3, summary = ?4, sort = ?5 WHERE id = ?1",
            params![v.id, v.title.trim(), v.outline, v.summary, v.sort],
        )?;
        Ok(())
    }

    pub fn delete_volume(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM volumes WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 章节 ----------

    pub fn list_chapter_metas(&self, book_id: i64) -> Result<Vec<ChapterMeta>> {
        let mut metas = {
            let c = self.c();
            let mut st = c.prepare(
                "SELECT id, volume_id, sort, title, status, word_count, ai_chars, length(outline) > 0, length(summary) > 0, updated_at, length(beats) > 0, published_at,
                        story_time, story_day
                 FROM chapters WHERE book_id = ?1 ORDER BY sort, id",
            )?;
            let rows = st.query_map([book_id], |r| {
                Ok(ChapterMeta {
                    id: r.get(0)?,
                    volume_id: r.get(1)?,
                    sort: r.get(2)?,
                    number: None,
                    title: r.get(3)?,
                    status: r.get(4)?,
                    word_count: r.get(5)?,
                    ai_chars: r.get(6)?,
                    has_outline: r.get(7)?,
                    has_summary: r.get(8)?,
                    updated_at: r.get(9)?,
                    has_beats: r.get(10)?,
                    published_at: r.get(11)?,
                    story_time: r.get(12)?,
                    story_day: r.get(13)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let numbers = number_chapters(metas.iter().map(|m| m.title.as_str()));
        for (m, n) in metas.iter_mut().zip(numbers) {
            m.number = n;
        }
        Ok(metas)
    }

    pub fn list_chapters(&self, book_id: i64) -> Result<Vec<Chapter>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {CH_COLS} FROM chapters WHERE book_id = ?1 ORDER BY sort, id"))?;
        let rows = st.query_map([book_id], chapter_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_chapter(&self, id: i64) -> Result<Option<Chapter>> {
        let sql = format!("SELECT {CH_COLS} FROM chapters WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], chapter_row).optional()?)
    }

    fn insert_chapter(c: &Connection, ch: &Chapter, t: i64) -> Result<i64> {
        let sort: i64 = if ch.sort > 0 {
            ch.sort
        } else {
            c.query_row("SELECT COALESCE(MAX(sort), 0) + 1 FROM chapters WHERE book_id = ?1", [ch.book_id], |r| r.get(0))?
        };
        let status = if ch.status.is_empty() { "draft" } else { ch.status.as_str() };
        c.execute(
            "INSERT INTO chapters (book_id, volume_id, sort, title, outline, content, summary, status, word_count, ai_chars, created_at, updated_at, beats, metrics, story_time, story_day)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13, ?14, ?15)",
            params![
                ch.book_id,
                ch.volume_id,
                sort,
                ch.title.trim(),
                ch.outline,
                ch.content,
                ch.summary,
                status,
                count_words(&ch.content),
                ch.ai_chars,
                t,
                ch.beats,
                value_json(&ch.metrics),
                ch.story_time.trim(),
                ch.story_day
            ],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn create_chapter(&self, ch: &Chapter) -> Result<Chapter> {
        let id = Self::insert_chapter(&self.c(), ch, now())?;
        Ok(self.get_chapter(id)?.expect("刚插入的章节"))
    }

    /// 批量导入章节（一个事务），返回插入数量。
    pub fn bulk_create_chapters(&self, chapters: &[Chapter]) -> Result<usize> {
        let mut c = self.c();
        let tx = c.transaction()?;
        let t = now();
        for ch in chapters {
            Self::insert_chapter(&tx, ch, t)?;
        }
        tx.commit()?;
        Ok(chapters.len())
    }

    pub fn update_chapter(&self, ch: &Chapter) -> Result<()> {
        self.c().execute(
            "UPDATE chapters SET volume_id = ?2, sort = ?3, title = ?4, outline = ?5, content = ?6, summary = ?7, status = ?8,
             word_count = ?9, updated_at = ?10, beats = ?11, metrics = ?12, published_at = ?13, story_time = ?14, story_day = ?15 WHERE id = ?1",
            params![
                ch.id,
                ch.volume_id,
                ch.sort,
                ch.title.trim(),
                ch.outline,
                ch.content,
                ch.summary,
                ch.status,
                count_words(&ch.content),
                now(),
                ch.beats,
                value_json(&ch.metrics),
                ch.published_at,
                ch.story_time.trim(),
                ch.story_day
            ],
        )?;
        Ok(())
    }

    /// 连载用：published 时把这一章和前面所有有正文的章节标成已发布（已发布的保留原时间），
    /// 否则把这一章和后面的取消发布。返回改了几章。
    pub fn publish_through(&self, book_id: i64, chapter_id: i64, published: bool) -> Result<usize> {
        let c = self.c();
        let sort: i64 = c.query_row("SELECT sort FROM chapters WHERE id = ?1 AND book_id = ?2", params![chapter_id, book_id], |r| r.get(0))?;
        let n = if published {
            c.execute(
                "UPDATE chapters SET published_at = ?3 WHERE book_id = ?1 AND sort <= ?2 AND published_at IS NULL AND word_count > 0",
                params![book_id, sort, now()],
            )?
        } else {
            c.execute("UPDATE chapters SET published_at = NULL WHERE book_id = ?1 AND sort >= ?2 AND published_at IS NOT NULL", params![book_id, sort])?
        };
        Ok(n)
    }

    pub fn delete_chapter(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM chapters WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 合并章节、删掉被合并的那章之前，把指向它的记录都改成指向合并后的章节：
    /// 历史版本、伏笔、AI 调用、设定状态、人物关系、采纳记录、揭示进度
    pub fn repoint_chapter(&self, from: i64, to: i64) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for sql in [
            "UPDATE versions SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE threads SET planted_chapter_id = ?2 WHERE planted_chapter_id = ?1",
            "UPDATE threads SET resolved_chapter_id = ?2 WHERE resolved_chapter_id = ?1",
            "UPDATE threads SET last_chapter_id = ?2 WHERE last_chapter_id = ?1",
            "UPDATE ai_log SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE entry_states SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE relations SET since_chapter_id = ?2 WHERE since_chapter_id = ?1",
            "UPDATE ai_accepts SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE reveal_events SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE events SET chapter_id = ?2 WHERE chapter_id = ?1",
            "UPDATE events SET done_chapter_id = ?2 WHERE done_chapter_id = ?1",
        ] {
            tx.execute(sql, params![from, to])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn reorder_chapters(&self, book_id: i64, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE chapters SET sort = ?1 WHERE id = ?2 AND book_id = ?3",
                params![i as i64 + 1, id, book_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_ai_chars(&self, id: i64, n: i64) -> Result<()> {
        self.c().execute("UPDATE chapters SET ai_chars = ai_chars + ?2 WHERE id = ?1", params![id, n])?;
        Ok(())
    }

    // ---------- 版本快照 ----------

    pub fn list_versions(&self, chapter_id: i64) -> Result<Vec<Version>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, chapter_id, note, word_count, created_at FROM versions WHERE chapter_id = ?1 ORDER BY id DESC LIMIT 100",
        )?;
        let rows = st.query_map([chapter_id], |r| {
            Ok(Version {
                id: r.get(0)?,
                chapter_id: r.get(1)?,
                content: String::new(),
                note: r.get(2)?,
                word_count: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_version(&self, id: i64) -> Result<Option<Version>> {
        Ok(self
            .c()
            .query_row(
                "SELECT id, chapter_id, content, note, word_count, created_at FROM versions WHERE id = ?1",
                [id],
                |r| {
                    Ok(Version {
                        id: r.get(0)?,
                        chapter_id: r.get(1)?,
                        content: r.get(2)?,
                        note: r.get(3)?,
                        word_count: r.get(4)?,
                        created_at: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn create_version(&self, chapter_id: i64, content: &str, note: &str) -> Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO versions (chapter_id, content, note, word_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chapter_id, content, note, count_words(content), now()],
        )?;
        let id = c.last_insert_rowid();
        c.execute(
            "DELETE FROM versions WHERE chapter_id = ?1 AND id NOT IN (SELECT id FROM versions WHERE chapter_id = ?1 ORDER BY id DESC LIMIT 60)",
            [chapter_id],
        )?;
        Ok(id)
    }

    // ---------- 设定库 ----------

    pub fn list_entries(&self, book_id: i64) -> Result<Vec<Entry>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {ENTRY_COLS} FROM entries WHERE book_id = ?1
             ORDER BY CASE kind WHEN 'character' THEN 0 WHEN 'faction' THEN 1 WHEN 'location' THEN 2 WHEN 'item' THEN 3 WHEN 'concept' THEN 4 ELSE 5 END, id"
        ))?;
        let rows = st.query_map([book_id], entry_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_entry(&self, id: i64) -> Result<Option<Entry>> {
        let sql = format!("SELECT {ENTRY_COLS} FROM entries WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], entry_row).optional()?)
    }

    pub fn create_entry(&self, e: &Entry) -> Result<Entry> {
        let id = {
            let c = self.c();
            c.execute(
                "INSERT INTO entries (book_id, kind, name, aliases, description, state, immutable, always_include, updated_at, role, fields, visibility, secret, exclude)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    e.book_id,
                    normalize_kind(&e.kind),
                    e.name.trim(),
                    e.aliases.trim(),
                    e.description,
                    e.state,
                    e.immutable,
                    e.always_include,
                    now(),
                    e.role.trim(),
                    map_json(&e.fields),
                    e.visibility.trim(),
                    e.secret,
                    e.exclude.trim()
                ],
            )?;
            c.last_insert_rowid()
        };
        Ok(self.get_entry(id)?.expect("刚插入的设定"))
    }

    pub fn update_entry(&self, e: &Entry) -> Result<()> {
        self.c().execute(
            "UPDATE entries SET kind = ?2, name = ?3, aliases = ?4, description = ?5, state = ?6, immutable = ?7, always_include = ?8, updated_at = ?9,
             role = ?10, fields = ?11, visibility = ?12, secret = ?13, exclude = ?14 WHERE id = ?1",
            params![
                e.id,
                normalize_kind(&e.kind),
                e.name.trim(),
                e.aliases.trim(),
                e.description,
                e.state,
                e.immutable,
                e.always_include,
                now(),
                e.role.trim(),
                map_json(&e.fields),
                e.visibility.trim(),
                e.secret,
                e.exclude.trim()
            ],
        )?;
        Ok(())
    }

    pub fn delete_entry(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM entries WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 伏笔 ----------

    pub fn list_threads(&self, book_id: i64) -> Result<Vec<Thread>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {THREAD_COLS} FROM threads WHERE book_id = ?1 ORDER BY CASE status WHEN 'open' THEN 0 ELSE 1 END, id"
        ))?;
        let rows = st.query_map([book_id], thread_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_thread(&self, id: i64) -> Result<Option<Thread>> {
        let sql = format!("SELECT {THREAD_COLS} FROM threads WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], thread_row).optional()?)
    }

    pub fn create_thread(&self, t: &Thread) -> Result<Thread> {
        let id = {
            let c = self.c();
            let status = if t.status.is_empty() { "open" } else { t.status.as_str() };
            c.execute(
                "INSERT INTO threads (book_id, title, detail, status, planted_chapter_id, resolved_chapter_id, updated_at, target_chapter, last_chapter_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    t.book_id,
                    t.title.trim(),
                    t.detail,
                    status,
                    t.planted_chapter_id,
                    t.resolved_chapter_id,
                    now(),
                    t.target_chapter,
                    t.last_chapter_id
                ],
            )?;
            c.last_insert_rowid()
        };
        Ok(self.get_thread(id)?.expect("刚插入的伏笔"))
    }

    pub fn update_thread(&self, t: &Thread) -> Result<()> {
        self.c().execute(
            "UPDATE threads SET title = ?2, detail = ?3, status = ?4, planted_chapter_id = ?5, resolved_chapter_id = ?6, updated_at = ?7,
             target_chapter = ?8, last_chapter_id = ?9 WHERE id = ?1",
            params![
                t.id,
                t.title.trim(),
                t.detail,
                t.status,
                t.planted_chapter_id,
                t.resolved_chapter_id,
                now(),
                t.target_chapter,
                t.last_chapter_id
            ],
        )?;
        Ok(())
    }

    // ---------- 人物关系 ----------

    pub fn list_relations(&self, book_id: i64) -> Result<Vec<Relation>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {REL_COLS} FROM relations WHERE book_id = ?1 ORDER BY id"))?;
        let rows = st.query_map([book_id], relation_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_relation(&self, id: i64) -> Result<Option<Relation>> {
        let sql = format!("SELECT {REL_COLS} FROM relations WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], relation_row).optional()?)
    }

    pub fn save_relation(&self, r: &Relation) -> Result<Relation> {
        let status = if r.status.is_empty() { "active" } else { r.status.as_str() };
        let id = {
            let c = self.c();
            if r.id > 0 {
                c.execute(
                    "UPDATE relations SET a_id = ?2, b_id = ?3, kind = ?4, detail = ?5, status = ?6, since_chapter_id = ?7, updated_at = ?8 WHERE id = ?1",
                    params![r.id, r.a_id, r.b_id, r.kind.trim(), r.detail, status, r.since_chapter_id, now()],
                )?;
                r.id
            } else {
                c.execute(
                    "INSERT INTO relations (book_id, a_id, b_id, kind, detail, status, since_chapter_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![r.book_id, r.a_id, r.b_id, r.kind.trim(), r.detail, status, r.since_chapter_id, now()],
                )?;
                c.last_insert_rowid()
            }
        };
        Ok(self.get_relation(id)?.expect("刚保存的关系"))
    }

    pub fn delete_relation(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM relations WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 文风库 ----------

    pub fn lib_rev(&self) -> u64 {
        self.lib_rev.load(Ordering::SeqCst)
    }

    fn bump_lib(&self) {
        self.lib_rev.fetch_add(1, Ordering::SeqCst);
    }

    /// full 为 false 时正文只取前 200 字，用于列表
    pub fn list_lib_items(&self, full: bool) -> Result<Vec<LibItem>> {
        let cols = if full { LIB_COLS.to_string() } else { LIB_COLS.replace("content,", "substr(content, 1, 200),") };
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {cols} FROM library_items ORDER BY id DESC"))?;
        let rows = st.query_map([], lib_item_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_lib_item(&self, id: i64) -> Result<Option<LibItem>> {
        let sql = format!("SELECT {LIB_COLS} FROM library_items WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], lib_item_row).optional()?)
    }

    /// 新建（id 为 0）或更新范文；chunks 不为 None 时整体替换片段
    pub fn save_lib_item(&self, item: &LibItem, chunks: Option<&[(String, Vec<String>)]>) -> Result<LibItem> {
        let id = {
            let mut c = self.c();
            let tx = c.transaction()?;
            let words = count_words(&item.content);
            let id = if item.id > 0 {
                tx.execute(
                    "UPDATE library_items SET title = ?2, source = ?3, genre = ?4, tags = ?5, note = ?6, content = ?7, analysis = ?8, enabled = ?9, book_id = ?10, word_count = ?11, updated_at = ?12 WHERE id = ?1",
                    params![item.id, item.title.trim(), item.source.trim(), item.genre.trim(), vec_json(&item.tags), item.note, item.content, item.analysis, item.enabled, item.book_id, words, now()],
                )?;
                item.id
            } else {
                tx.execute(
                    "INSERT INTO library_items (title, source, genre, tags, note, content, analysis, enabled, book_id, word_count, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
                    params![item.title.trim(), item.source.trim(), item.genre.trim(), vec_json(&item.tags), item.note, item.content, item.analysis, item.enabled, item.book_id, words, now()],
                )?;
                tx.last_insert_rowid()
            };
            if let Some(chunks) = chunks {
                tx.execute("DELETE FROM library_chunks WHERE item_id = ?1", [id])?;
                let mut ins = tx.prepare("INSERT INTO library_chunks (item_id, seq, text, tags) VALUES (?1, ?2, ?3, ?4)")?;
                for (i, (text, tags)) in chunks.iter().enumerate() {
                    ins.execute(params![id, i as i64, text, vec_json(tags)])?;
                }
            }
            tx.commit()?;
            id
        };
        self.bump_lib();
        Ok(self.get_lib_item(id)?.expect("刚保存的范文"))
    }

    pub fn delete_lib_item(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM library_items WHERE id = ?1", [id])?;
        self.bump_lib();
        Ok(())
    }

    pub fn list_lib_chunks(&self, item_id: i64) -> Result<Vec<LibChunk>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, item_id, seq, text, tags, embedding, emb_model FROM library_chunks WHERE item_id = ?1 ORDER BY seq")?;
        let rows = st.query_map([item_id], lib_chunk_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 启用中的范文的所有片段，附带范文的标题和题材，用来建检索索引
    pub fn lib_index_rows(&self) -> Result<Vec<(LibChunk, String, String)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT c.id, c.item_id, c.seq, c.text, c.tags, c.embedding, c.emb_model, i.title, i.genre
             FROM library_chunks c JOIN library_items i ON i.id = c.item_id WHERE i.enabled = 1 ORDER BY c.item_id, c.seq",
        )?;
        let rows = st.query_map([], |r| Ok((lib_chunk_row(r)?, r.get::<_, String>(7)?, r.get::<_, String>(8)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 检查照抄用：启用中、且不是从这本书收藏的范文 (标题, 正文)
    pub fn lib_sources(&self, exclude_book: Option<i64>) -> Result<Vec<(String, String)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT title, content FROM library_items WHERE enabled = 1 AND (book_id IS NULL OR book_id IS NOT ?1)")?;
        let rows = st.query_map([exclude_book], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_chunk_tags(&self, updates: &[(i64, Vec<String>)]) -> Result<()> {
        {
            let mut c = self.c();
            let tx = c.transaction()?;
            for (id, tags) in updates {
                tx.execute("UPDATE library_chunks SET tags = ?2 WHERE id = ?1", params![id, vec_json(tags)])?;
            }
            tx.commit()?;
        }
        self.bump_lib();
        Ok(())
    }

    /// 还没有用 model 生成向量的片段 (id, 正文)
    pub fn chunks_without_embedding(&self, model: &str, limit: usize) -> Result<(Vec<(i64, String)>, i64)> {
        let c = self.c();
        let remaining: i64 = c.query_row(
            "SELECT COUNT(*) FROM library_chunks c JOIN library_items i ON i.id = c.item_id WHERE i.enabled = 1 AND c.emb_model != ?1",
            [model],
            |r| r.get(0),
        )?;
        let mut st = c.prepare(
            "SELECT c.id, c.text FROM library_chunks c JOIN library_items i ON i.id = c.item_id WHERE i.enabled = 1 AND c.emb_model != ?1 ORDER BY c.id LIMIT ?2",
        )?;
        let rows = st.query_map(params![model, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((rows.collect::<rusqlite::Result<Vec<_>>>()?, remaining))
    }

    pub fn set_chunk_embeddings(&self, model: &str, items: &[(i64, Vec<f32>)]) -> Result<()> {
        {
            let mut c = self.c();
            let tx = c.transaction()?;
            for (id, v) in items {
                tx.execute("UPDATE library_chunks SET embedding = ?2, emb_model = ?3 WHERE id = ?1", params![id, vec_blob(v), model])?;
            }
            tx.commit()?;
        }
        self.bump_lib();
        Ok(())
    }

    /// (范文数, 已分析, 片段数, 总字数, 已生成向量的片段数)
    pub fn lib_counts(&self, model: &str) -> Result<(i64, i64, i64, i64, i64)> {
        let c = self.c();
        let (items, analyzed, words): (i64, i64, i64) = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(analysis) > 0), 0), COALESCE(SUM(word_count), 0) FROM library_items",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let (chunks, embedded): (i64, i64) = c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(emb_model = ?1 AND ?1 != ''), 0) FROM library_chunks",
            [model],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((items, analyzed, chunks, words, embedded))
    }

    pub fn list_guides(&self) -> Result<Vec<StyleGuide>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, genre, content, note, created_at FROM style_guides ORDER BY id DESC")?;
        let rows = st.query_map([], guide_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_guide(&self, id: i64) -> Result<Option<StyleGuide>> {
        Ok(self.c().query_row("SELECT id, genre, content, note, created_at FROM style_guides WHERE id = ?1", [id], guide_row).optional()?)
    }

    /// 某个题材的最新指南（genre 为空即通用指南）
    pub fn latest_guide(&self, genre: &str) -> Result<Option<StyleGuide>> {
        Ok(self
            .c()
            .query_row(
                "SELECT id, genre, content, note, created_at FROM style_guides WHERE genre = ?1 ORDER BY id DESC LIMIT 1",
                [genre.trim()],
                guide_row,
            )
            .optional()?)
    }

    /// 写作时用的指南：有本题材的专用指南就用它，否则用通用指南
    pub fn guide_for(&self, genre: &str) -> Result<Option<StyleGuide>> {
        if !genre.trim().is_empty() {
            if let Some(g) = self.latest_guide(genre)? {
                return Ok(Some(g));
            }
        }
        self.latest_guide("")
    }

    pub fn add_guide(&self, genre: &str, content: &str, note: &str) -> Result<StyleGuide> {
        let id = {
            let c = self.c();
            c.execute(
                "INSERT INTO style_guides (genre, content, note, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![genre.trim(), content.trim(), note, now()],
            )?;
            c.last_insert_rowid()
        };
        Ok(self.get_guide(id)?.expect("刚保存的指南"))
    }

    pub fn add_accept(&self, book_id: i64, a: &Accepted) -> Result<()> {
        self.c().execute(
            "INSERT INTO ai_accepts (book_id, chapter_id, task, text, before_ctx, after_ctx, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![book_id, a.chapter_id, a.task, a.text, a.before, a.after, now()],
        )?;
        Ok(())
    }

    /// 最近采纳的 AI 文字（新的在前）
    pub fn recent_accepts(&self, limit: usize) -> Result<Vec<Accepted>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, chapter_id, task, text, before_ctx, after_ctx, learned FROM ai_accepts WHERE chapter_id IS NOT NULL ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit as i64], |r| {
            Ok(Accepted { id: r.get(0)?, chapter_id: r.get(1)?, task: r.get(2)?, text: r.get(3)?, before: r.get(4)?, after: r.get(5)?, learned: r.get(6)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn mark_accepts_learned(&self, ids: &[i64]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for id in ids {
            tx.execute("UPDATE ai_accepts SET learned = 1 WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---------- 设定状态快照 ----------

    pub fn list_book_states(&self, book_id: i64) -> Result<Vec<EntryState>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT s.id, s.entry_id, s.chapter_id, s.phase, s.state, s.fields, s.created_at
             FROM entry_states s JOIN entries e ON e.id = s.entry_id WHERE e.book_id = ?1 ORDER BY s.id",
        )?;
        let rows = st.query_map([book_id], state_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_entry_states(&self, entry_id: i64) -> Result<Vec<EntryState>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, entry_id, chapter_id, phase, state, fields, created_at FROM entry_states WHERE entry_id = ?1 ORDER BY id",
        )?;
        let rows = st.query_map([entry_id], state_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 记录状态快照；同一设定在同一章同一阶段只保留最新一条。
    pub fn put_entry_state(&self, entry_id: i64, chapter_id: Option<i64>, phase: &str, state: &str, fields: &BTreeMap<String, String>) -> Result<()> {
        let c = self.c();
        c.execute(
            "DELETE FROM entry_states WHERE entry_id = ?1 AND chapter_id IS ?2 AND phase = ?3",
            params![entry_id, chapter_id, phase],
        )?;
        c.execute(
            "INSERT INTO entry_states (entry_id, chapter_id, phase, state, fields, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![entry_id, chapter_id, phase, state, map_json(fields), now()],
        )?;
        Ok(())
    }

    // ---------- 时间线：事件和时限 ----------

    pub fn list_events(&self, book_id: i64) -> Result<Vec<Event>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {EVENT_COLS} FROM events WHERE book_id = ?1 ORDER BY id"))?;
        let rows = st.query_map([book_id], event_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_event(&self, id: i64) -> Result<Option<Event>> {
        let sql = format!("SELECT {EVENT_COLS} FROM events WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], event_row).optional()?)
    }

    /// id 为 0 时新建，否则更新
    pub fn save_event(&self, e: &Event) -> Result<Event> {
        let kind = if e.kind == "deadline" { "deadline" } else { "event" };
        let status = match (kind, e.status.as_str()) {
            ("deadline", "done") => "done",
            ("deadline", _) => "open",
            _ => "",
        };
        let id = {
            let c = self.c();
            if e.id == 0 {
                c.execute(
                    "INSERT INTO events (book_id, chapter_id, kind, title, detail, who, story_time, day, status, done_chapter_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![e.book_id, e.chapter_id, kind, e.title.trim(), e.detail.trim(), e.who.trim(), e.story_time.trim(), e.day, status, e.done_chapter_id, now()],
                )?;
                c.last_insert_rowid()
            } else {
                c.execute(
                    "UPDATE events SET book_id = ?1, chapter_id = ?2, kind = ?3, title = ?4, detail = ?5, who = ?6, story_time = ?7, day = ?8,
                     status = ?9, done_chapter_id = ?10, updated_at = ?11 WHERE id = ?12",
                    params![e.book_id, e.chapter_id, kind, e.title.trim(), e.detail.trim(), e.who.trim(), e.story_time.trim(), e.day, status, e.done_chapter_id, now(), e.id],
                )?;
                e.id
            }
        };
        Ok(self.get_event(id)?.expect("刚保存的事件"))
    }

    pub fn delete_event(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM events WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 重新定稿时先清掉这一章上次提取的事件（时限不清，可能已经被后面的章节了结）
    pub fn delete_chapter_events(&self, chapter_id: i64) -> Result<usize> {
        Ok(self.c().execute("DELETE FROM events WHERE chapter_id = ?1 AND kind = 'event'", [chapter_id])?)
    }

    /// 改一条状态快照的文字（查找替换用），不动它属于哪一章
    pub fn update_entry_state(&self, s: &EntryState) -> Result<()> {
        self.c().execute("UPDATE entry_states SET state = ?2, fields = ?3 WHERE id = ?1", params![s.id, s.state, map_json(&s.fields)])?;
        Ok(())
    }

    pub fn delete_entry_state(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM entry_states WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 设定进展 ----------

    pub fn list_book_progressions(&self, book_id: i64) -> Result<Vec<Progression>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT p.id, p.entry_id, p.gate, p.mode, p.text, p.created_at FROM progressions p JOIN entries e ON e.id = p.entry_id WHERE e.book_id = ?1 ORDER BY p.id",
        )?;
        let rows = st.query_map([book_id], progression_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_entry_progressions(&self, entry_id: i64) -> Result<Vec<Progression>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, entry_id, gate, mode, text, created_at FROM progressions WHERE entry_id = ?1 ORDER BY id")?;
        let rows = st.query_map([entry_id], progression_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_progression(&self, id: i64) -> Result<Option<Progression>> {
        Ok(self
            .c()
            .query_row("SELECT id, entry_id, gate, mode, text, created_at FROM progressions WHERE id = ?1", [id], progression_row)
            .optional()?)
    }

    pub fn save_progression(&self, p: &Progression) -> Result<Progression> {
        let mode = if p.mode == "replace" { "replace" } else { "add" };
        let id = {
            let c = self.c();
            if p.id > 0 {
                c.execute("UPDATE progressions SET gate = ?2, mode = ?3, text = ?4 WHERE id = ?1", params![p.id, p.gate.trim(), mode, p.text.trim()])?;
                p.id
            } else {
                c.execute(
                    "INSERT INTO progressions (entry_id, gate, mode, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![p.entry_id, p.gate.trim(), mode, p.text.trim(), now()],
                )?;
                c.last_insert_rowid()
            }
        };
        Ok(self.get_progression(id)?.expect("刚保存的设定进展"))
    }

    pub fn delete_progression(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM progressions WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 秘密台账 ----------

    pub fn list_reveals(&self, book_id: i64) -> Result<Vec<Reveal>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {REVEAL_COLS} FROM reveals WHERE book_id = ?1 ORDER BY CASE status WHEN 'dropped' THEN 1 ELSE 0 END, COALESCE(reveal_at, 1000000000), sort, id"
        ))?;
        let rows = st.query_map([book_id], reveal_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_reveal(&self, id: i64) -> Result<Option<Reveal>> {
        let sql = format!("SELECT {REVEAL_COLS} FROM reveals WHERE id = ?1");
        Ok(self.c().query_row(&sql, [id], reveal_row).optional()?)
    }

    /// 新建（id 为 0）或更新一条秘密。
    pub fn save_reveal(&self, r: &Reveal) -> Result<Reveal> {
        let status = if r.status == "dropped" { "dropped" } else { "active" };
        let ids = if r.entry_ids.is_empty() { String::new() } else { serde_json::to_string(&r.entry_ids)? };
        let id = {
            let c = self.c();
            if r.id > 0 {
                c.execute(
                    "UPDATE reveals SET title = ?2, truth = ?3, misread = ?4, gap = ?5, terms = ?6, exceptions = ?7, entry_ids = ?8, seed_at = ?9, clue_at = ?10,
                     reveal_at = ?11, seed_note = ?12, clue_note = ?13, payoff = ?14, status = ?15, sort = ?16, updated_at = ?17 WHERE id = ?1",
                    params![
                        r.id, r.title.trim(), r.truth, r.misread, r.gap.trim(), r.terms.trim(), r.exceptions.trim(), ids, r.seed_at, r.clue_at,
                        r.reveal_at, r.seed_note, r.clue_note, r.payoff, status, r.sort, now()
                    ],
                )?;
                r.id
            } else {
                c.execute(
                    "INSERT INTO reveals (book_id, title, truth, misread, gap, terms, exceptions, entry_ids, seed_at, clue_at, reveal_at, seed_note, clue_note, payoff, status, sort, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        r.book_id, r.title.trim(), r.truth, r.misread, r.gap.trim(), r.terms.trim(), r.exceptions.trim(), ids, r.seed_at, r.clue_at,
                        r.reveal_at, r.seed_note, r.clue_note, r.payoff, status, r.sort, now()
                    ],
                )?;
                c.last_insert_rowid()
            }
        };
        Ok(self.get_reveal(id)?.expect("刚保存的秘密"))
    }

    pub fn delete_reveal(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM reveals WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn delete_book_reveals(&self, book_id: i64) -> Result<usize> {
        Ok(self.c().execute("DELETE FROM reveals WHERE book_id = ?1", [book_id])?)
    }

    pub fn list_book_reveal_events(&self, book_id: i64) -> Result<Vec<RevealEvent>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT e.id, e.reveal_id, e.chapter_id, e.step, e.quote, e.note, e.created_at FROM reveal_events e JOIN reveals r ON r.id = e.reveal_id
             WHERE r.book_id = ?1 ORDER BY e.id",
        )?;
        let rows = st.query_map([book_id], reveal_event_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 记一步揭示进度；同一章对同一个秘密的同一步只留最新一条。
    pub fn add_reveal_event(&self, e: &RevealEvent) -> Result<i64> {
        let c = self.c();
        c.execute("DELETE FROM reveal_events WHERE reveal_id = ?1 AND chapter_id = ?2 AND step = ?3", params![e.reveal_id, e.chapter_id, e.step])?;
        c.execute(
            "INSERT INTO reveal_events (reveal_id, chapter_id, step, quote, note, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.reveal_id, e.chapter_id, e.step, e.quote.trim(), e.note.trim(), now()],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn delete_reveal_event(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM reveal_events WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_chapter_metrics(&self, id: i64, metrics: &serde_json::Value) -> Result<()> {
        self.c().execute("UPDATE chapters SET metrics = ?2 WHERE id = ?1", params![id, value_json(metrics)])?;
        Ok(())
    }

    // ---------- 提示词覆盖 ----------

    pub fn prompt_overrides(&self) -> Result<HashMap<String, String>> {
        let raw: Option<String> =
            self.c().query_row("SELECT value FROM settings WHERE key = 'prompts'", [], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    pub fn set_prompt_override(&self, id: &str, text: &str) -> Result<()> {
        let mut all = self.prompt_overrides()?;
        if text.trim().is_empty() {
            all.remove(id);
        } else {
            all.insert(id.to_string(), text.to_string());
        }
        self.c().execute(
            "INSERT INTO settings (key, value) VALUES ('prompts', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(&all)?],
        )?;
        Ok(())
    }

    pub fn delete_thread(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM threads WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- 设置 ----------

    pub fn get_settings(&self) -> Result<Settings> {
        let raw: Option<String> =
            self.c().query_row("SELECT value FROM settings WHERE key = 'app'", [], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.c().execute(
            "INSERT INTO settings (key, value) VALUES ('app', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(s)?],
        )?;
        Ok(())
    }

    // ---------- 写作技能（作者导入的；内置技能不进数据库） ----------

    pub fn list_skills(&self) -> Result<Vec<Skill>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, markdown, source, updated_at FROM skills ORDER BY created_at, id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, md, source, updated) = row?;
            out.push(Skill::from_markdown(&id, &md, &source, false, updated));
        }
        Ok(out)
    }

    pub fn get_skill(&self, id: &str) -> Result<Option<Skill>> {
        let row: Option<(String, String, i64)> = self
            .c()
            .query_row("SELECT markdown, source, updated_at FROM skills WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        Ok(row.map(|(md, source, updated)| Skill::from_markdown(id, &md, &source, false, updated)))
    }

    pub fn save_skill(&self, s: &Skill) -> Result<()> {
        let t = now();
        self.c().execute(
            "INSERT INTO skills (id, name, description, source, markdown, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, description = excluded.description, source = excluded.source,
             markdown = excluded.markdown, updated_at = excluded.updated_at",
            params![s.id, s.name, s.description, s.source, s.markdown, t],
        )?;
        Ok(())
    }

    pub fn delete_skill(&self, id: &str) -> Result<bool> {
        Ok(self.c().execute("DELETE FROM skills WHERE id = ?1", [id])? > 0)
    }

    // ---------- 统计 ----------

    pub fn add_daily_words(&self, book_id: i64, day: &str, delta: i64) -> Result<()> {
        if delta <= 0 {
            return Ok(());
        }
        self.c().execute(
            "INSERT INTO daily_words (book_id, day, words) VALUES (?1, ?2, ?3)
             ON CONFLICT(book_id, day) DO UPDATE SET words = words + excluded.words",
            params![book_id, day, delta],
        )?;
        Ok(())
    }

    pub fn daily_words(&self, book_id: i64, limit: i64) -> Result<Vec<(String, i64)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT day, words FROM daily_words WHERE book_id = ?1 ORDER BY day DESC LIMIT ?2")?;
        let rows = st.query_map(params![book_id, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn log_ai(&self, l: &AiLog) -> Result<()> {
        self.c().execute(
            "INSERT INTO ai_log (book_id, chapter_id, task, model, input_chars, output_chars, created_at, prompt_tokens, completion_tokens, estimated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                l.book_id,
                l.chapter_id,
                l.task,
                l.model,
                l.input_chars,
                l.output_chars,
                now(),
                l.prompt_tokens,
                l.completion_tokens,
                l.estimated
            ],
        )?;
        Ok(())
    }

    /// 按任务汇总：(任务, 次数, 输入字数, 输出字数, 输入 token, 输出 token)
    pub fn ai_usage(&self, book_id: i64) -> Result<Vec<(String, i64, i64, i64, i64, i64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT task, COUNT(*), SUM(input_chars), SUM(output_chars), SUM(prompt_tokens), SUM(completion_tokens)
             FROM ai_log WHERE book_id = ?1 GROUP BY task ORDER BY COUNT(*) DESC",
        )?;
        let rows = st.query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 按模型汇总 token：(模型, 输入 token, 输出 token, 是否含估算)
    pub fn ai_tokens_by_model(&self, book_id: i64) -> Result<Vec<(String, i64, i64, bool)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT model, SUM(prompt_tokens), SUM(completion_tokens), MAX(estimated) FROM ai_log WHERE book_id = ?1 GROUP BY model",
        )?;
        let rows = st.query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 按章节和任务数 AI 调用次数：(章节 id, 任务, 次数)；不属于哪一章的调用章节 id 为空
    pub fn ai_usage_by_chapter(&self, book_id: i64) -> Result<Vec<(Option<i64>, String, i64)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT chapter_id, task, COUNT(*) FROM ai_log WHERE book_id = ?1 GROUP BY chapter_id, task")?;
        let rows = st.query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 各章的创建、最后修改时间，历史版本数和最早、最晚一个版本的日期（本地时间）
    pub fn chapter_history(&self, book_id: i64) -> Result<HashMap<i64, ChapterHistory>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT c.id,
                    strftime('%Y-%m-%d %H:%M', c.created_at, 'unixepoch', 'localtime'),
                    strftime('%Y-%m-%d %H:%M', c.updated_at, 'unixepoch', 'localtime'),
                    COUNT(v.id),
                    COALESCE(strftime('%Y-%m-%d', MIN(v.created_at), 'unixepoch', 'localtime'), ''),
                    COALESCE(strftime('%Y-%m-%d', MAX(v.created_at), 'unixepoch', 'localtime'), '')
             FROM chapters c LEFT JOIN versions v ON v.chapter_id = c.id
             WHERE c.book_id = ?1 GROUP BY c.id",
        )?;
        let rows = st.query_map([book_id], |r| {
            Ok((r.get(0)?, ChapterHistory { created: r.get(1)?, updated: r.get(2)?, versions: r.get(3)?, first_version: r.get(4)?, last_version: r.get(5)? }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 按本地时区格式化时间戳，fmt 是 SQLite strftime 的格式
    pub fn local_time(&self, ts: i64, fmt: &str) -> Result<String> {
        Ok(self.c().query_row("SELECT strftime(?2, ?1, 'unixepoch', 'localtime')", params![ts, fmt], |r| r.get(0))?)
    }
}

pub struct ChapterHistory {
    pub created: String,
    pub updated: String,
    pub versions: i64,
    pub first_version: String,
    pub last_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let book = db.create_book(&Book { title: "测试书".into(), ..Default::default() }).unwrap();
        assert_eq!(book.target_words, 2500);
        let vol = db.create_volume(&Volume { book_id: book.id, title: "第一卷".into(), ..Default::default() }).unwrap();
        let c1 = db
            .create_chapter(&Chapter { book_id: book.id, volume_id: Some(vol.id), title: "开端".into(), content: "林凡醒了。".into(), ..Default::default() })
            .unwrap();
        let c2 = db.create_chapter(&Chapter { book_id: book.id, title: "序章".into(), ..Default::default() }).unwrap();
        assert_eq!(c1.word_count, 5);
        assert_eq!(c2.sort, 2);
        db.reorder_chapters(book.id, &[c2.id, c1.id]).unwrap();
        let metas = db.list_chapter_metas(book.id).unwrap();
        assert_eq!(metas[0].id, c2.id);
        assert_eq!(metas[0].number, None);
        assert_eq!(metas[1].number, Some(1));

        db.create_version(c1.id, "旧内容", "测试").unwrap();
        assert_eq!(db.list_versions(c1.id).unwrap().len(), 1);

        let cards = db.list_books().unwrap();
        assert_eq!(cards[0].chapter_count, 2);
        assert_eq!(cards[0].word_count, 5);

        db.add_daily_words(book.id, "2026-10-03", 100).unwrap();
        db.add_daily_words(book.id, "2026-10-03", 50).unwrap();
        assert_eq!(db.daily_words(book.id, 7).unwrap(), vec![("2026-10-03".to_string(), 150)]);

        db.delete_volume(vol.id).unwrap();
        assert_eq!(db.get_chapter(c1.id).unwrap().unwrap().volume_id, None);
        db.delete_book(book.id).unwrap();
        assert!(db.get_chapter(c1.id).unwrap().is_none());
    }
}
