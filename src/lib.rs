pub mod ai;
pub mod api;
pub mod backup;
pub mod declare;
pub mod replace;
pub mod teardown;
pub mod timeline;
pub mod trends;
pub mod fanqie_font;
pub mod continuity;
pub mod db;
pub mod export;
pub mod importer;
pub mod io;
pub mod libapi;
pub mod library;
pub mod lint;
pub mod llm;
pub mod logic;
pub mod memory;
pub mod models;
pub mod prompts;
pub mod sensitive;
pub mod skillapi;
pub mod skills;
pub mod state;
pub mod stylelib;
pub mod text;
pub mod visibility;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use base64::Engine;
use state::AppState;

#[derive(rust_embed::RustEmbed)]
#[folder = "web/"]
struct Assets;

pub fn build_router(state: AppState) -> Router {
    use api::*;
    let api = Router::new()
        .route("/health", get(health))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/settings/test", post(test_provider))
        .route("/backups", get(backup::list_backups).post(backup::create_backup))
        .route("/backups/{name}", axum::routing::delete(backup::delete_backup))
        .route("/backups/{name}/restore", post(backup::restore_backup))
        .route("/books", get(list_books).post(create_book))
        .route("/books/{id}", get(get_book).patch(patch_book).delete(delete_book))
        .route("/books/{id}/stats", get(book_stats))
        .route("/books/{id}/publish", post(publish_chapters))
        .route("/books/{id}/declaration", get(declare::declaration))
        .route("/books/{id}/creation_log", get(declare::creation_log))
        .route("/books/{id}/search", post(replace::search))
        .route("/books/{id}/replace", post(replace::replace))
        .route("/books/{id}/timeline", get(timeline::get_timeline))
        .route("/books/{id}/events", post(timeline::create_event))
        .route("/events/{id}", patch(timeline::patch_event).delete(timeline::delete_event))
        .route("/refbooks", get(teardown::list))
        .route("/refbooks/import", post(teardown::import))
        .route("/refbooks/{id}", get(teardown::get).patch(teardown::patch).delete(teardown::delete))
        .route("/refbooks/{id}/compare", get(teardown::compare))
        .route("/refchapters/{id}/analyze", post(teardown::analyze))
        .route("/trends/hot", get(trends::hot))
        .route("/trends/rank", get(trends::rank))
        .route("/trends/memes", post(trends::memes))
        .route("/trends/preference", post(trends::preference))
        .route("/books/{id}/volumes", get(list_volumes).post(create_volume))
        .route("/volumes/{id}", patch(patch_volume).delete(delete_volume))
        .route("/books/{id}/chapters", get(list_chapters).post(create_chapter))
        .route("/books/{id}/chapters/reorder", post(reorder_chapters))
        .route("/chapters/{id}", get(get_chapter).patch(patch_chapter).delete(delete_chapter))
        .route("/chapters/{id}/ai_accept", post(ai_accept))
        .route("/chapters/{id}/split", post(split_chapter))
        .route("/chapters/{id}/merge_next", post(merge_next_chapter))
        .route("/chapters/{id}/versions", get(list_versions).post(create_version))
        .route("/versions/{id}", get(get_version))
        .route("/versions/{id}/restore", post(restore_version))
        .route("/books/{id}/entries", get(list_entries).post(create_entry))
        .route("/entries/{id}", patch(patch_entry).delete(delete_entry))
        .route("/entries/{id}/states", get(entry_states))
        .route("/entry_states/{id}", axum::routing::delete(delete_entry_state))
        .route("/entries/{id}/progressions", get(list_progressions).post(create_progression))
        .route("/progressions/{id}", patch(patch_progression).delete(delete_progression))
        .route("/books/{id}/reveals", get(list_reveals).post(create_reveal))
        .route("/books/{id}/reveals/plan", post(apply_reveal_plan))
        .route("/reveals/{id}", patch(patch_reveal).delete(delete_reveal))
        .route("/reveals/{id}/events", post(add_reveal_event))
        .route("/reveal_events/{id}", axum::routing::delete(delete_reveal_event))
        .route("/books/{id}/knowledge", post(create_knowledge))
        .route("/knowledge/{id}", patch(patch_knowledge).delete(delete_knowledge))
        .route("/chapters/{id}/logic", get(chapter_logic))
        .route("/chapters/{id}/contract", get(chapter_contract))
        .route("/books/{id}/continuity", get(continuity))
        .route("/books/{id}/relations", get(list_relations).post(create_relation))
        .route("/relations/{id}", patch(patch_relation).delete(delete_relation))
        .route("/prompts", get(list_prompts))
        .route("/prompts/{id}", axum::routing::put(put_prompt))
        .route("/genres", get(genres))
        .route("/craft", get(craft_list))
        .route("/craft/{id}", get(craft_doc))
        .route("/ai/preview", post(ai::preview))
        .route("/books/{id}/threads", get(list_threads).post(create_thread))
        .route("/threads/{id}", patch(patch_thread).delete(delete_thread))
        .route("/books/{id}/apply_updates", post(apply_updates))
        .route("/books/{id}/context", get(context_preview))
        .route("/books/{id}/visibility", get(get_visibility).post(set_visibility))
        .route("/lint", post(lint))
        .route("/tools/normalize", post(normalize_text))
        .route("/ai/stream", post(ai::stream))
        .route("/ai/json", post(ai::json_task))
        .route("/books/{id}/export", get(io::export))
        .route("/books/{id}/check", get(io::submission_check))
        .route("/books/{id}/import/tavern", post(io::import_tavern))
        .route("/import/novel", post(io::import_novel))
        .route("/library", get(libapi::list_items).post(libapi::create_item))
        .route("/library/import", post(libapi::import_items))
        .route("/library/search", post(libapi::search))
        .route("/library/feedback", get(libapi::feedback_list))
        .route("/library/embed", post(libapi::embed_pending))
        .route("/library/guide", get(libapi::guide_state).put(libapi::save_guide))
        .route("/library/guide/distill", post(libapi::distill_guide))
        .route("/library/guide/{id}/restore", post(libapi::restore_guide))
        .route("/library/{id}", get(libapi::get_item).patch(libapi::patch_item).delete(libapi::delete_item))
        .route("/library/{id}/analyze", post(libapi::analyze_item))
        .route("/skills", get(skillapi::list_skills).post(skillapi::create_skill))
        .route("/skills/{id}", get(skillapi::get_skill).put(skillapi::update_skill).delete(skillapi::delete_skill))
        .route("/skills/{id}/active", axum::routing::put(skillapi::set_active));

    Router::new()
        .nest("/api", api)
        .fallback(static_file)
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), require_password))
        .with_state(state)
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "接口不存在" }))).into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    let (file, path) = match Assets::get(path) {
        Some(f) => (f, path),
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = if mime.type_() == "text" || mime.subtype() == "javascript" {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    };
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache".to_string())], file.data).into_response()
}

async fn require_password(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let Some(password) = st.password.as_deref() else {
        return next.run(req).await;
    };
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.trim()).ok())
        .and_then(|d| String::from_utf8(d).ok())
        .and_then(|s| s.split_once(':').map(|(_, p)| p == password))
        .unwrap_or(false);
    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"ai-novel\", charset=\"UTF-8\"")],
            "需要密码",
        )
            .into_response()
    }
}
