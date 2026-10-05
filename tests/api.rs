//! 端到端测试：启动应用和一个模拟的 OpenAI 兼容接口，走完整的写作流程。

use ai_novel::{build_router, db::Db, state::AppState};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

const API_KEY: &str = "sk-test-123456789";
const PROSE: &str = "林凡推开门，屋里没人。\n桌上压着一张字条。";
const SUMMARY: &str = "林凡发现字条，决定当晚夜探藏经阁。";
const UNIVERSAL_JSON: &str = r#"{
 "ideas":[{"title":"都市之神级系统","logline":"外卖员获得系统","selling_points":"反差","opening":"送餐途中"}],
 "characters":[{"name":"林凡","aliases":"凡哥","role":"主角","description":"少年","immutable":"重情义","state":"山村"}],
 "chapters":[{"title":"夜探","outline":"林凡夜探藏经阁"},{"title":"对峙","outline":"与黑袍人对峙"}],
 "entries":[{"name":"林凡","kind":"character","is_new":false,"fields":{"power":"炼气四层","body":"左臂受伤","location":""}},{"name":"黑袍人","kind":"character","is_new":true,"description":"神秘刺客","state":"逃走"}],
 "threads_new":[{"title":"字条的笔迹","detail":"像是师父的字"}],
 "threads_progressed":[],
 "threads_resolved":[{"id":1,"note":"玉佩来历揭晓"}],
 "relations":[{"a":"林凡","b":"黑袍人","kind":"敌对","detail":"藏经阁对峙","status":"active"}],
 "tension":7,"emotion":"紧张","cool_points":[{"type":"揭秘","level":"小","desc":"发现字条"}],"hook":{"type":"悬念预知","desc":"字条落款"},"debts":["字条是谁留的"],"comment":"节奏紧凑",
 "issues":[{"severity":"high","type":"能力矛盾","quote":"林凡御剑飞行","problem":"炼气期不能御剑","suggestion":"改成轻功"}],
 "scores":{"hook":8,"pacing":7,"payoff":6,"character":7,"ending":8,"readability":9},
 "summary":"节奏不错","strengths":["开头快"],"problems":[],
 "techniques":["打斗用短句连发","每个动作都有结果"],"rhythm":"短句为主","dialogue":"无","imitate":"动作要有因果",
 "tags":["打斗","悬念","不存在的标签"],"chunk_tags":{"1":["打斗","悬念"]},
 "names":[{"name":"沈砚","note":"砚台的砚，沉稳"},{"name":"顾长夜","note":"有江湖气"}]
}"#;
const GUIDE: &str = "叙事：贴着主角写。\n节奏：打斗用短句。\n禁忌：不写段尾感悟。";

type Seen = Arc<Mutex<Vec<Value>>>;

async fn mock_models(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "data": [{ "id": "mock-writer" }, { "id": "mock-analyst" }] })).into_response()
}

fn authorized(headers: &HeaderMap) -> bool {
    headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {API_KEY}"))
}

async fn mock_chat(State(seen): State<Seen>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if !authorized(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": { "message": "bad key" } }))).into_response();
    }
    seen.lock().unwrap().push(body.clone());
    let prompt = body["messages"].to_string();
    let content = if prompt.contains("整理他自己的《文风指南》") {
        GUIDE.to_string()
    } else if prompt.contains("只输出 JSON") {
        UNIVERSAL_JSON.to_string()
    } else if prompt.contains("剧情摘要") {
        SUMMARY.to_string()
    } else {
        PROSE.to_string()
    };
    if body["stream"].as_bool().unwrap_or(false) {
        let mut out = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "reasoning_content": "先想想" } }] }));
        for piece in content.chars().collect::<Vec<_>>().chunks(4) {
            let s: String = piece.iter().collect();
            out += &format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": s } }] }));
        }
        out += "data: [DONE]\n\n";
        ([(header::CONTENT_TYPE, "text/event-stream")], out).into_response()
    } else {
        Json(json!({ "choices": [{ "message": { "role": "assistant", "content": content } }], "usage": { "prompt_tokens": 120, "completion_tokens": 40 } })).into_response()
    }
}

/// 按字符散列成 16 维向量，相同字多的文本向量更接近
async fn mock_embeddings(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let data: Vec<Value> = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut v = vec![0f32; 16];
            for c in s.as_str().unwrap().chars() {
                v[c as usize % 16] += 1.0;
            }
            json!({ "index": i, "embedding": v })
        })
        .collect();
    Json(json!({ "data": data })).into_response()
}

async fn spawn(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    addr
}

struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.unwrap();
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let (status, v) = self.send(method.clone(), path, body).await;
        assert!(status.is_success(), "{method} {path} → {status}: {v}");
        v
    }

    async fn get(&self, path: &str) -> Value {
        self.ok(reqwest::Method::GET, path, None).await
    }
    async fn post(&self, path: &str, body: Value) -> Value {
        self.ok(reqwest::Method::POST, path, Some(body)).await
    }
    async fn patch(&self, path: &str, body: Value) -> Value {
        self.ok(reqwest::Method::PATCH, path, Some(body)).await
    }
    async fn put(&self, path: &str, body: Value) -> Value {
        self.ok(reqwest::Method::PUT, path, Some(body)).await
    }

    /// 读取完整的 SSE 响应，返回（事件名列表, 拼接后的 delta, done 事件的文本）
    async fn sse(&self, body: Value) -> (Vec<String>, String, String) {
        let resp = self.http.post(format!("{}/api/ai/stream", self.base)).json(&body).send().await.unwrap();
        assert!(resp.status().is_success(), "stream 失败：{}", resp.status());
        let text = resp.text().await.unwrap();
        let (mut events, mut deltas, mut done) = (Vec::new(), String::new(), String::new());
        let mut current = String::new();
        for line in text.lines() {
            if let Some(e) = line.strip_prefix("event:") {
                current = e.trim().to_string();
                events.push(current.clone());
            } else if let Some(d) = line.strip_prefix("data:") {
                let v: Value = serde_json::from_str(d.trim()).unwrap();
                match current.as_str() {
                    "delta" => deltas += v["text"].as_str().unwrap(),
                    "done" => done = v["text"].as_str().unwrap().to_string(),
                    "error" => panic!("流式生成报错：{v}"),
                    _ => {}
                }
            }
        }
        (events, deltas, done)
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test]
async fn full_writing_flow() {
    let seen: Seen = Arc::default();
    let mock = Router::new()
        .route("/v1/models", get(mock_models))
        .route("/v1/chat/completions", post(mock_chat))
        .with_state(seen.clone());
    let mock_addr = spawn(mock).await;
    let app_addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await;
    let c = Client { base: format!("http://{app_addr}"), http: reqwest::Client::new() };

    assert_eq!(c.get("/api/health").await["ok"], true);
    let (status, v) = c.send(reqwest::Method::GET, "/api/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");

    // 设置：密钥打码返回，回传打码值时保留原密钥
    let provider = json!({ "id": "mock", "name": "模拟接口", "base_url": format!("http://{mock_addr}/v1/"), "api_key": API_KEY });
    let saved = c
        .put("/api/settings", json!({ "providers": [provider], "writer": { "provider_id": "mock", "model": "mock-writer", "temperature": 0.9, "max_tokens": 4000 }, "analyst": { "provider_id": "mock", "model": "mock-analyst", "temperature": 0.2, "max_tokens": 2000 } }))
        .await;
    let masked = saved["providers"][0]["api_key"].as_str().unwrap().to_string();
    assert_eq!(masked, "sk-t****6789");
    let mut again = saved.clone();
    again["providers"][0]["name"] = json!("改名后的接口");
    c.put("/api/settings", again).await;
    let test = c.post("/api/settings/test", json!({ "provider": { "id": "mock", "base_url": format!("http://{mock_addr}/v1"), "api_key": masked }, "model": "mock-writer" })).await;
    assert_eq!(test["models"], json!(["mock-analyst", "mock-writer"]));
    assert_eq!(test["chat_ok"], true, "{test}");

    // 作品、卷、章节、设定、伏笔
    let book = c.post("/api/books", json!({ "title": "测试之书", "genre": "玄幻", "platform": "fanqie", "worldview": "灵气复苏的世界" })).await;
    let bid = book["id"].as_i64().unwrap();
    let vol = c.post(&format!("/api/books/{bid}/volumes"), json!({})).await;
    assert_eq!(vol["title"], "第一卷");
    let ch1 = c
        .post(
            &format!("/api/books/{bid}/chapters"),
            json!({ "title": "山村少年", "volume_id": vol["id"], "content": "林凡在山村长大，捡到一块黑色玉佩。", "summary": "林凡捡到黑色玉佩。", "status": "done" }),
        )
        .await;
    let ch2 = c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "夜探", "volume_id": vol["id"], "outline": "林凡夜探藏经阁" })).await;
    let (ch1_id, ch2_id) = (ch1["id"].as_i64().unwrap(), ch2["id"].as_i64().unwrap());
    let lin = c.post(&format!("/api/books/{bid}/entries"), json!({ "kind": "character", "name": "林凡", "aliases": "凡哥", "state": "炼气三层" })).await;
    c.post(&format!("/api/books/{bid}/entries"), json!({ "kind": "item", "name": "黑色玉佩", "description": "刻着古老符文" })).await;
    let thread = c.post(&format!("/api/books/{bid}/threads"), json!({ "title": "玉佩的来历", "planted_chapter_id": ch1_id })).await;
    assert_eq!(thread["id"], 1);

    let metas = c.get(&format!("/api/books/{bid}/chapters")).await;
    assert_eq!(metas[1]["number"], 2);
    assert_eq!(metas[1]["has_outline"], true);

    let ctx = c.get(&format!("/api/books/{bid}/context?chapter_id={ch2_id}")).await;
    let ctx_text = ctx["sections"].to_string();
    assert!(ctx_text.contains("炼气三层") && ctx_text.contains("前情提要") && ctx_text.contains("玉佩的来历"), "{ctx_text}");

    // 流式续写：上下文里要带上设定状态和章纲
    let (events, deltas, done) = c.sse(json!({ "task": "continue", "book_id": bid, "chapter_id": ch2_id })).await;
    assert!(events.contains(&"thinking".to_string()));
    assert_eq!(deltas, PROSE);
    assert_eq!(done, PROSE);
    let last = seen.lock().unwrap().last().cloned().unwrap();
    let prompt = last["messages"].to_string();
    assert!(prompt.contains("炼气三层") && prompt.contains("林凡夜探藏经阁") && prompt.contains("写作原则"));
    assert_eq!(last["model"], "mock-writer");
    assert_eq!(last["max_tokens"], 4000);

    let (status, err) = c.send(reqwest::Method::POST, "/api/ai/stream", Some(json!({ "task": "expand", "book_id": bid, "chapter_id": ch2_id }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"].as_str().unwrap().contains("选中"));

    // 写入正文、记录 AI 参与字数
    let updated = c.patch(&format!("/api/chapters/{ch2_id}"), json!({ "content": PROSE, "client_day": "2026-10-03" })).await;
    assert_eq!(updated["word_count"], 20);
    c.post(&format!("/api/chapters/{ch2_id}/ai_accept"), json!({ "chars": 20 })).await;

    // 定稿：摘要 → 提取设定变化 → 写回
    let s = c.post("/api/ai/json", json!({ "task": "summarize", "book_id": bid, "chapter_id": ch2_id })).await;
    assert_eq!(s["text"], SUMMARY);
    assert_eq!(seen.lock().unwrap().last().unwrap()["model"], "mock-analyst");
    assert_eq!(c.get(&format!("/api/chapters/{ch2_id}")).await["summary"], SUMMARY);

    let mut extracted = c.post("/api/ai/json", json!({ "task": "extract", "book_id": bid, "chapter_id": ch2_id })).await;
    assert_eq!(extracted["entries"].as_array().unwrap().len(), 2);
    extracted["chapter_id"] = json!(ch2_id);
    let applied = c.post(&format!("/api/books/{bid}/apply_updates"), extracted).await;
    assert_eq!(applied, json!({ "created": 1, "updated": 1, "threads_added": 1, "threads_progressed": 0, "threads_resolved": 1, "relations": 1 }));
    let entries = c.get(&format!("/api/books/{bid}/entries")).await;
    let lin_now = entries.as_array().unwrap().iter().find(|e| e["id"] == lin["id"]).unwrap();
    assert_eq!(lin_now["state"], "实力：炼气四层；身体：左臂受伤");
    assert_eq!(lin_now["fields"]["power"], "炼气四层");
    assert_eq!(lin_now["aliases"], "凡哥");

    // 设定随章节演变：第2章仍看到定稿前的状态，第3章才看到第2章结束时的状态
    let states = c.get(&format!("/api/entries/{}/states", lin["id"])).await;
    assert_eq!(states.as_array().unwrap().len(), 2, "{states}");
    let ch3 = c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "对峙", "volume_id": vol["id"] })).await;
    let ch3_id = ch3["id"].as_i64().unwrap();
    let ctx2 = c.get(&format!("/api/books/{bid}/context?chapter_id={ch2_id}")).await["sections"].to_string();
    assert!(ctx2.contains("炼气三层") && !ctx2.contains("炼气四层"), "{ctx2}");
    let ctx3 = c.get(&format!("/api/books/{bid}/context?chapter_id={ch3_id}")).await["sections"].to_string();
    assert!(ctx3.contains("实力：炼气四层"), "{ctx3}");

    // 人物关系：定稿写回，进入上下文；结束后标注
    let rels = c.get(&format!("/api/books/{bid}/relations")).await;
    assert_eq!(rels.as_array().unwrap().len(), 1);
    assert!(c.get(&format!("/api/books/{bid}/context?chapter_id={ch2_id}")).await["sections"].to_string().contains("林凡 与 黑袍人：敌对"));
    let rid = rels[0]["id"].as_i64().unwrap();
    c.patch(&format!("/api/relations/{rid}"), json!({ "status": "ended" })).await;
    let ctx_ended = c.get(&format!("/api/books/{bid}/context?chapter_id={ch2_id}")).await["sections"].to_string();
    assert!(!ctx_ended.contains("林凡 与 黑袍人"), "已结束的关系在另一方不出场时不注入：{ctx_ended}");
    for (task, marker) in [("first_read", "第一次看这本书"), ("revision_plan", "修订计划"), ("simulate", "自己的立场推演")] {
        let p = c.post("/api/ai/preview", json!({ "task": task, "book_id": bid, "chapter_id": ch2_id })).await;
        assert!(p["messages"].to_string().contains(marker), "{task}: {p}");
    }

    // 节拍：先规划再写整章，写整章的提示词要带上节拍
    let preview = c.post("/api/ai/preview", json!({ "task": "beats", "book_id": bid, "chapter_id": ch3_id })).await;
    assert!(preview["messages"][1]["content"].as_str().unwrap().contains("场景节拍"));
    assert!(preview["model"].as_str().unwrap().contains("mock-writer"));
    let (_, _, beats) = c.sse(json!({ "task": "beats", "book_id": bid, "chapter_id": ch3_id })).await;
    c.patch(&format!("/api/chapters/{ch3_id}"), json!({ "beats": beats })).await;
    let preview = c.post("/api/ai/preview", json!({ "task": "write_chapter", "book_id": bid, "chapter_id": ch3_id, "finale": true })).await;
    let prompt = preview["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("本章节拍") && prompt.contains("按节拍顺序推进"), "{prompt}");
    assert!(prompt.contains("【大结局】") && prompt.contains("字条的笔迹"), "大结局要列出未回收伏笔");
    // ch3 只有标题、没有章纲：按前情/总纲合理推进
    assert!(prompt.contains("合理推进本章"), "无章纲时应提示按前情推进：{prompt}");
    // ch2 有章纲「林凡夜探藏经阁」：写整章必须要求严格照本章章纲写，防止 AI 跑偏到别的剧情
    let wc = c.post("/api/ai/preview", json!({ "task": "write_chapter", "book_id": bid, "chapter_id": ch2_id })).await;
    let wcp = wc["messages"][1]["content"].as_str().unwrap();
    assert!(wcp.contains("严格按【本章章纲】") && wcp.contains("林凡夜探藏经阁"), "写整章要求严格按本章章纲：{wcp}");

    // 张力分析写进章节指标
    let tension = c.post("/api/ai/json", json!({ "task": "tension", "book_id": bid, "chapter_id": ch2_id })).await;
    assert_eq!(tension["tension"], 7);
    assert_eq!(c.get(&format!("/api/chapters/{ch2_id}")).await["metrics"]["hook"]["type"], "悬念预知");
    let threads = c.get(&format!("/api/books/{bid}/threads")).await;
    let resolved = threads.as_array().unwrap().iter().find(|t| t["id"] == 1).unwrap();
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["resolved_chapter_id"], ch2_id);

    // 检查、审稿、规划章纲、开书方案、角色对话
    let check = c.post("/api/ai/json", json!({ "task": "check", "book_id": bid, "chapter_id": ch2_id })).await;
    assert_eq!(check["issues"][0]["type"], "能力矛盾");
    let review = c.post("/api/ai/json", json!({ "task": "review", "book_id": bid, "chapter_id": ch2_id })).await;
    assert_eq!(review["scores"]["hook"], 8);
    let outlines = c.post("/api/ai/json", json!({ "task": "chapter_outlines", "book_id": bid, "count": 2 })).await;
    assert_eq!(outlines["chapters"].as_array().unwrap().len(), 2);
    assert!(seen.lock().unwrap().last().unwrap()["messages"].to_string().contains("第 4 章到第 5 章"));
    let ideas = c.post("/api/ai/json", json!({ "task": "ideas", "fields": { "genre": "都市", "platform": "fanqie", "idea": "外卖员获得系统" } })).await;
    assert_eq!(ideas["ideas"][0]["title"], "都市之神级系统");
    let (_, _, reply) = c.sse(json!({ "task": "chat", "book_id": bid, "entry_id": lin["id"], "messages": [{ "role": "user", "content": "你是谁？" }] })).await;
    assert_eq!(reply, PROSE);
    let (_, _, world) = c.sse(json!({ "task": "world", "book_id": bid })).await;
    assert!(!world.is_empty());

    // 本地质量检查
    let lint = c.post("/api/lint", json!({ "text": "他嘴角勾起一抹弧度，空气仿佛凝固了。" })).await;
    assert!(lint["issues"].as_array().unwrap().len() >= 2);

    // 版本快照与恢复
    c.patch(&format!("/api/chapters/{ch2_id}"), json!({ "content": "被 AI 改写后的内容。", "snapshot": "AI 替换前" })).await;
    let versions = c.get(&format!("/api/chapters/{ch2_id}/versions")).await;
    assert_eq!(versions[0]["note"], "AI 替换前");
    let vid = versions[0]["id"].as_i64().unwrap();
    assert_eq!(c.get(&format!("/api/versions/{vid}")).await["content"], PROSE);
    let restored = c.post(&format!("/api/versions/{vid}/restore"), json!({})).await;
    assert_eq!(restored["content"], PROSE);
    assert_eq!(c.get(&format!("/api/chapters/{ch2_id}/versions")).await.as_array().unwrap().len(), 2);

    // 导出与投稿检查
    let resp = c.http.get(format!("{}/api/books/{bid}/export?format=fanqie", c.base)).send().await.unwrap();
    assert!(resp.headers()[header::CONTENT_DISPOSITION.as_str()].to_str().unwrap().contains("UTF-8''"));
    let txt = resp.text().await.unwrap();
    assert!(txt.starts_with("《测试之书》"));
    assert!(txt.contains("第2章 夜探\n\n　　林凡推开门，屋里没人。\n\n　　桌上压着一张字条。"), "{txt}");
    let qidian = c.http.get(format!("{}/api/books/{bid}/export?format=qidian&chapter_id={ch1_id}", c.base)).send().await.unwrap().text().await.unwrap();
    assert!(qidian.starts_with("第一章 山村少年"), "{qidian}");
    let epub = c.http.get(format!("{}/api/books/{bid}/export?format=epub", c.base)).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(&epub[..2], b"PK");
    let check = c.get(&format!("/api/books/{bid}/check")).await;
    assert!(check["items"].as_array().unwrap().iter().any(|i| i["message"].as_str().unwrap().contains("少于 2000")));

    // 导入 GBK 编码的旧稿和酒馆角色卡
    let (gbk, _, _) = encoding_rs::GBK.encode("一个少年的故事。\n第一卷 起\n第一章 开端\n内容一。\n第二章 发展\n内容二。");
    let imported = c.post("/api/import/novel", json!({ "filename": "《旧作》.txt", "data": format!("data:text/plain;base64,{}", b64(&gbk)) })).await;
    assert_eq!(imported["chapters"], 2);
    assert_eq!(imported["volumes"], 1);
    let new_id = imported["book_id"].as_i64().unwrap();
    let new_book = c.get(&format!("/api/books/{new_id}")).await;
    assert_eq!(new_book["title"], "旧作");
    assert_eq!(new_book["synopsis"], "一个少年的故事。");
    let new_chapters = c.get(&format!("/api/books/{new_id}/chapters")).await;
    assert_eq!(new_chapters[0]["title"], "开端");
    assert_eq!(new_chapters[1]["number"], 2);

    let card = json!({ "spec": "chara_card_v2", "data": { "name": "苏雨", "description": "青云宗大师姐", "personality": "外冷内热" } });
    let tavern = c.post(&format!("/api/books/{bid}/import/tavern"), json!({ "filename": "苏雨.json", "data": b64(card.to_string().as_bytes()) })).await;
    assert_eq!(tavern["created"], 1);

    // 统计
    let stats = c.get(&format!("/api/books/{bid}/stats")).await;
    assert_eq!(stats["ai_chars"], 20);
    assert_eq!(stats["daily"][0]["day"], "2026-10-03");
    assert!(stats["ai_usage"].as_array().unwrap().len() >= 5);
    assert_eq!(stats["curve"][0]["tension"], 7);
    assert!(stats["tokens"]["prompt"].as_i64().unwrap() >= 120, "非流式调用记录了接口返回的用量");
    assert_eq!(stats["tokens"]["estimated"], true, "流式调用没有用量时按字数估算");

    // 提示词模板：自定义后生效，恢复默认后还原
    let prompts = c.get("/api/prompts").await;
    assert!(prompts["templates"].as_array().unwrap().len() >= 20);
    assert_eq!(c.put("/api/prompts/task.summarize", json!({ "text": "自定义摘要：{{chapter}}" })).await["custom"], true);
    c.post("/api/ai/json", json!({ "task": "summarize", "book_id": bid, "chapter_id": ch1_id })).await;
    assert!(seen.lock().unwrap().last().unwrap()["messages"].to_string().contains("自定义摘要：第1章 山村少年"));
    assert_eq!(c.put("/api/prompts/task.summarize", json!({ "text": "" })).await["custom"], false);
    let (status, _) = c.send(reqwest::Method::PUT, "/api/prompts/no.such", Some(json!({ "text": "x" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 连续性体检、题材库、写作手册、跨章检查
    let findings = c.get(&format!("/api/books/{bid}/continuity")).await;
    assert!(findings["findings"].is_array());
    assert_eq!(c.get("/api/genres").await.as_array().unwrap().len(), 37);
    let craft = c.get("/api/craft").await;
    assert_eq!(craft.as_array().unwrap().len(), 12);
    assert!(c.get("/api/craft/cool-points").await["text"].as_str().unwrap().contains("爽点"));
    let lint = c.post("/api/lint", json!({ "text": PROSE, "book_id": bid, "chapter_id": ch3_id })).await;
    assert!(lint["score"].is_number());

    // 调整顺序后编号重新计算
    c.post(&format!("/api/books/{bid}/chapters/reorder"), json!([ch2_id, ch1_id])).await;
    let metas = c.get(&format!("/api/books/{bid}/chapters")).await;
    assert_eq!(metas[0]["id"], ch2_id);
    assert_eq!(metas[0]["number"], 1);

    let books = c.get("/api/books").await;
    assert_eq!(books.as_array().unwrap().len(), 2);
    c.ok(reqwest::Method::DELETE, &format!("/api/books/{bid}"), None).await;
    let (status, _) = c.send(reqwest::Method::GET, &format!("/api/books/{bid}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

const FIGHT: &str = "剑光一闪，擂台上的青石被劈开一道深痕。\n林凡侧身让过，反手一掌拍在对方肩头，那人踉跄后退三步，撞翻了兵器架。\n“再来。”他说。\n对方咬牙拔剑，剑尖抖出七朵剑花，直取咽喉。林凡不退反进，两指夹住剑身，轻轻一折，长剑断成两截。";
const TALK: &str = "“你今天怎么来了？”她把咖啡推过来。\n“路过。”\n“路过三条街？”\n他没接话，低头搅着杯子里的糖。窗外下着雨，玻璃上起了一层雾。\n“行吧，”她笑了一下，“那就当你是路过。”";

#[tokio::test]
async fn style_library_flow() {
    let seen: Seen = Arc::default();
    let mock = Router::new()
        .route("/v1/models", get(mock_models))
        .route("/v1/chat/completions", post(mock_chat))
        .route("/v1/embeddings", post(mock_embeddings))
        .with_state(seen.clone());
    let mock_addr = spawn(mock).await;
    let app_addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await;
    let c = Client { base: format!("http://{app_addr}"), http: reqwest::Client::new() };
    let provider = json!({ "id": "mock", "name": "模拟接口", "base_url": format!("http://{mock_addr}/v1"), "api_key": API_KEY });
    let settings = c
        .put(
            "/api/settings",
            json!({ "providers": [provider], "writer": { "provider_id": "mock", "model": "mock-writer" }, "analyst": { "provider_id": "mock", "model": "mock-analyst" }, "embedding": { "provider_id": "mock", "model": "mock-embed" } }),
        )
        .await;
    assert_eq!(settings["library_refs"], 2, "默认参考 2 段范文");
    assert_eq!(settings["use_style_guide"], true);

    let book = c.post("/api/books", json!({ "title": "剑来", "genre": "玄幻" })).await;
    let bid = book["id"].as_i64().unwrap();
    let ch = c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "擂台", "content": "林凡走上擂台。\n台下一片嘘声。" })).await;
    let cid = ch["id"].as_i64().unwrap();

    // 添加范文：自动切片、自动打标签；太短的拒绝
    let (status, _) = c.send(reqwest::Method::POST, "/api/library", Some(json!({ "content": "太短" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let fight = c.post("/api/library", json!({ "content": FIGHT, "genre": "玄幻", "tags": ["打斗", " 打斗 "], "note": "动作干脆" })).await;
    let fight_id = fight["id"].as_i64().unwrap();
    assert_eq!(fight["tags"], json!(["打斗"]));
    assert_eq!(fight["title"], "剑光一闪，擂台上的青石被劈开一道");
    let talk = c.post("/api/library", json!({ "title": "咖啡馆", "content": TALK, "genre": "都市" })).await;
    let detail = c.get(&format!("/api/library/{}", talk["id"])).await;
    assert_eq!(detail["chunks"][0]["tags"][0], "对话", "{detail}");

    // 导入：按章节拆成多篇，最后一片标为章末钩子
    let novel = "第一章 开端\n少年背着剑下山，山路很长。\n他回头看了一眼，师父没有送他。\n第二章 进城\n城门口排着长队，守卫挨个盘查。\n轮到他时，守卫盯着他的剑看了很久。\n第三章 客栈\n客栈里坐满了人，没有空桌。\n他在门口站了一会儿，有人朝他招手。";
    let imported = c.post("/api/library/import", json!({ "filename": "下山.txt", "data": b64(novel.as_bytes()), "genre": "武侠", "split": true })).await;
    assert_eq!(imported["created"], 3, "{imported}");
    let list = c.get("/api/library").await;
    assert_eq!(list.as_array().unwrap().len(), 5);
    assert!(list.as_array().unwrap().iter().any(|i| i["title"] == "进城" && i["source"] == "下山"), "{list}");

    // AI 分析：写法要点、只保留合法标签、片段标签
    let analyzed = c.post(&format!("/api/library/{fight_id}/analyze"), json!({})).await;
    let analysis = analyzed["analysis"].as_str().unwrap();
    assert!(analysis.contains("亮点：节奏不错") && analysis.contains("· 打斗用短句连发") && !analysis.contains("对话："), "{analysis}");
    assert_eq!(analyzed["tags"], json!(["打斗", "悬念"]));
    let detail = c.get(&format!("/api/library/{fight_id}")).await;
    assert_eq!(detail["chunks"][0]["tags"], json!(["打斗", "悬念"]));

    // 检索：标签 + 题材 + 关键词；配置向量模型后生成向量并混合检索
    let found = c.post("/api/library/search", json!({ "text": "擂台上剑光", "tags": ["对话"], "genre": "玄幻", "k": 3 })).await;
    assert_eq!(found["hits"][0]["item_id"], fight_id, "{found}");
    assert!(found["hits"][0]["why"].as_array().unwrap().iter().any(|w| w == "同题材"));
    assert_eq!(found["vector"], false, "还没生成向量");
    let emb = c.post("/api/library/embed", json!({})).await;
    assert!(emb["done"].as_i64().unwrap() >= 5 && emb["remaining"] == 0, "{emb}");
    let found = c.post("/api/library/search", json!({ "text": "咖啡馆里下着雨", "k": 2 })).await;
    assert_eq!(found["vector"], true, "{found}");

    // 文风指南：手写 → AI 提炼 → 恢复旧版本
    let manual = c.put("/api/library/guide", json!({ "genre": "", "content": "手写的指南：多用短句。" })).await;
    let distilled = c.post("/api/library/guide/distill", json!({ "genre": "", "instruction": "强调打斗" })).await;
    assert_eq!(distilled["content"], GUIDE);
    assert!(distilled["note"].as_str().unwrap().contains("5 篇范文"), "{distilled}");
    let last_prompt = seen.lock().unwrap().last().unwrap()["messages"].to_string();
    assert!(last_prompt.contains("手写的指南：多用短句。") && last_prompt.contains("强调打斗") && last_prompt.contains("在现有指南的基础上更新"));
    let state = c.get("/api/library/guide").await;
    assert_eq!(state["current"]["content"], GUIDE);
    assert_eq!(state["counts"]["items"], 5);
    assert_eq!(state["counts"]["analyzed"], 1);
    assert!(state["genres"].as_array().unwrap().contains(&json!("武侠")));
    c.post(&format!("/api/library/guide/{}/restore", manual["id"]), json!({})).await;
    let state = c.get("/api/library/guide").await;
    assert_eq!(state["current"]["content"], "手写的指南：多用短句。");
    assert_eq!(state["versions"].as_array().unwrap().len(), 3);
    c.put("/api/library/guide", json!({ "genre": "", "content": GUIDE })).await;

    // 从作者的修改里学习：采纳 AI 文字后改写一句、删掉一句
    let ai_text = "他缓缓地抬起头，眼中闪过一丝复杂的光芒。\n台下忽然安静下来。\n这一刻，他终于明白了命运的意义。";
    c.post(&format!("/api/chapters/{cid}/ai_accept"), json!({ "chars": 40, "text": ai_text, "task": "ghost", "before": "台下一片嘘声。", "after": "" })).await;
    c.patch(&format!("/api/chapters/{cid}"), json!({ "content": "林凡走上擂台。\n台下一片嘘声。\n他抬起头，盯着对面的人。\n台下忽然安静下来。" })).await;
    let fb = c.get("/api/library/feedback").await;
    assert_eq!(fb["pending"], 1);
    assert_eq!(fb["changed"][0]["final"], "他抬起头，盯着对面的人。", "{fb}");
    assert_eq!(fb["deleted"], json!(["这一刻，他终于明白了命运的意义。"]));
    c.post("/api/library/guide/distill", json!({})).await;
    let last_prompt = seen.lock().unwrap().last().unwrap()["messages"].to_string();
    assert!(last_prompt.contains("作者：他抬起头，盯着对面的人。") && last_prompt.contains("这一刻，他终于明白了命运的意义"), "{last_prompt}");
    assert_eq!(c.get("/api/library/feedback").await["pending"], 0);

    // 灰字续写：先发 meta 事件，系统提示里带上文风指南和检索到的范文
    let (events, deltas, _) = c.sse(json!({ "task": "ghost", "book_id": bid, "chapter_id": cid, "before": "林凡拔剑，", "after": "", "words": 50, "tags": ["打斗"] })).await;
    assert_eq!(events.first().map(String::as_str), Some("meta"));
    assert_eq!(deltas, PROSE);
    let body = seen.lock().unwrap().last().unwrap().clone();
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("【作者的文风指南") && system.contains("节奏：打斗用短句") && system.contains("〔片段1"), "{system}");
    assert!(system.contains("剑光一闪"), "打斗标签应该检索到打斗范文");
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("请从光标处接着写一两句话，约 50 字") && user.contains("林凡拔剑，"), "{user}");

    // 斜杠指令：预览能看到参考了哪些范文
    let preview = c.post("/api/ai/preview", json!({ "task": "insert", "book_id": bid, "chapter_id": cid, "before": "林凡走上擂台。", "goal": "写一段打斗", "words": 300 })).await;
    assert!(preview["messages"][1]["content"].as_str().unwrap().contains("请在光标处写一段：写一段打斗，约 300 字"));
    assert!(preview["style"]["guide"] == true && !preview["style"]["refs"].as_array().unwrap().is_empty(), "{}", preview["style"]);
    let (status, _) = c.send(reqwest::Method::POST, "/api/ai/preview", Some(json!({ "task": "insert", "book_id": bid, "chapter_id": cid }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "斜杠指令必须说明写什么");

    // 起名
    let names = c.post("/api/ai/json", json!({ "task": "names", "book_id": bid, "goal": "主角的师父" })).await;
    assert_eq!(names["names"][0]["name"], "沈砚");

    // 照抄检测：正文里连续 16 字以上和范文相同；从本书收藏的片段不算
    let copied = format!("林凡走上擂台。{}", "林凡侧身让过，反手一掌拍在对方肩头，那人踉跄后退三步");
    let lint = c.post("/api/lint", json!({ "text": copied, "book_id": bid, "chapter_id": cid })).await;
    assert_eq!(lint["issues"][0]["kind"], "疑似照抄", "{lint}");
    c.patch(&format!("/api/library/{fight_id}"), json!({ "enabled": false })).await;
    let lint = c.post("/api/lint", json!({ "text": copied, "book_id": bid })).await;
    assert!(lint["issues"].as_array().unwrap().iter().all(|i| i["kind"] != "疑似照抄"), "停用的范文不参与检测");
    let found = c.post("/api/library/search", json!({ "text": "擂台上剑光", "k": 5 })).await;
    assert!(found["hits"].as_array().unwrap().iter().all(|h| h["item_id"] != fight_id), "停用的范文不参与检索");
    let own = c.post("/api/library", json!({ "content": "林凡侧身让过，反手一掌拍在对方肩头，那人踉跄后退三步", "book_id": bid })).await;
    let lint = c.post("/api/lint", json!({ "text": copied, "book_id": bid })).await;
    assert!(lint["issues"].as_array().unwrap().iter().all(|i| i["kind"] != "疑似照抄"), "从本书收藏的片段不算照抄");

    // 关掉范文参考后只带指南
    let mut s = c.get("/api/settings").await;
    s["library_refs"] = json!(0);
    c.put("/api/settings", s).await;
    let preview = c.post("/api/ai/preview", json!({ "task": "ghost", "book_id": bid, "chapter_id": cid, "before": "林凡" })).await;
    assert!(preview["style"]["refs"].as_array().unwrap().is_empty() && preview["style"]["guide"] == true);

    c.ok(reqwest::Method::DELETE, &format!("/api/library/{}", own["id"]), None).await;
    assert_eq!(c.get("/api/library").await.as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn reports_missing_model_config() {
    let addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await;
    let c = Client { base: format!("http://{addr}"), http: reqwest::Client::new() };
    let book = c.post("/api/books", json!({ "title": "未配置" })).await;
    let (status, v) = c.send(reqwest::Method::POST, "/api/ai/json", Some(json!({ "task": "free", "book_id": book["id"], "instruction": "你好" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("设置"));
}

#[tokio::test]
async fn password_protection() {
    let addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), Some("secret".into())))).await;
    let http = reqwest::Client::new();
    let resp = http.get(format!("http://{addr}/api/health")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
    assert!(resp.headers().contains_key(header::WWW_AUTHENTICATE.as_str()));
    let resp = http.get(format!("http://{addr}/api/health")).basic_auth("me", Some("secret")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = http.get(format!("http://{addr}/api/health")).basic_auth("me", Some("wrong")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn writing_skills_flow() {
    let addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await;
    let c = Client { base: format!("http://{addr}"), http: reqwest::Client::new() };

    // 内置的「网文去AI味」默认启用
    let list = c.get("/api/skills").await;
    assert_eq!(list[0]["id"], "deslop");
    assert_eq!(list[0]["builtin"], true);
    assert_eq!(list[0]["active"], true);
    assert_eq!(list[0]["has_rules"], true);
    assert_eq!(c.get("/api/settings").await["skills"], json!(["deslop"]));

    // 导入一份 SKILL.md 格式的技能并启用
    let md = "---\nname: 对话加强\ndescription: |\n  让人物说话更像人。\n---\n# 对话加强\n\n## 写作规则\n台词里多用省略和反问，少用“说道”。\n\n## 修订流程\n把书面腔的台词改成口语。";
    let created = c.post("/api/skills", json!({ "markdown": md, "source": "对话加强.md" })).await;
    let sid = created["card"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["card"]["name"], "对话加强");
    assert_eq!(created["card"]["description"], "让人物说话更像人。");
    assert_eq!(created["card"]["source"], "对话加强.md");
    assert_eq!(created["card"]["active"], false);
    let active = c.put(&format!("/api/skills/{sid}/active"), json!({ "active": true })).await;
    assert_eq!(active["active"], json!(["deslop", sid]));

    // 内置技能不能改、不能删
    let (status, _) = c.send(reqwest::Method::DELETE, "/api/skills/deslop", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = c.send(reqwest::Method::PUT, "/api/skills/deslop", Some(json!({ "markdown": md }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 写正文时系统提示里带上启用技能的写作规则，不带修订流程
    let book = c.post("/api/books", json!({ "title": "技能测试", "genre": "都市" })).await;
    let bid = book["id"].as_i64().unwrap();
    let ch = c
        .post(&format!("/api/books/{bid}/chapters"), json!({ "title": "第一章", "outline": "主角进城", "content": "他嘴角勾起一抹冷笑，眼中闪过一丝不屑。她不是害怕，而是愤怒。" }))
        .await;
    let cid = ch["id"].as_i64().unwrap();
    let pv = c.post("/api/ai/preview", json!({ "task": "write_chapter", "book_id": bid, "chapter_id": cid })).await;
    let system = pv["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("【写作技能") && system.contains("《网文去AI味》") && system.contains("《对话加强》"), "{system}");
    assert!(system.contains("少用“说道”") && !system.contains("把书面腔的台词改成口语"));

    // 技能修订：系统提示附整份技能，用户提示带本地检测结果
    let pv = c.post("/api/ai/preview", json!({ "task": "deslop", "book_id": bid, "chapter_id": cid })).await;
    let system = pv["messages"][0]["content"].as_str().unwrap();
    let user = pv["messages"][1]["content"].as_str().unwrap();
    assert!(system.contains("【本次修订使用的技能：《网文去AI味》】") && system.contains("三遍法"), "{system}");
    assert!(user.contains("AI味：") && user.contains("嘴角勾起") && user.contains("不是害怕，而是"), "{user}");
    assert!(user.contains("【需要修订的正文】") && user.contains("眼中闪过一丝不屑"));
    let pv = c.post("/api/ai/preview", json!({ "task": "deslop", "book_id": bid, "chapter_id": cid, "skill": sid, "selection": "“我认为此事不妥。”他说道。" })).await;
    assert!(pv["messages"][0]["content"].as_str().unwrap().contains("《对话加强》】"));
    assert!(pv["messages"][1]["content"].as_str().unwrap().contains("我认为此事不妥"));
    let (status, _) =
        c.send(reqwest::Method::POST, "/api/ai/preview", Some(json!({ "task": "deslop", "book_id": bid, "chapter_id": cid, "skill": "nope" }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 修改保留来源；删除后从启用列表里移除
    let edited = c.put(&format!("/api/skills/{sid}"), json!({ "markdown": md.replace("对话加强\ndescription", "台词口语化\ndescription") })).await;
    assert_eq!(edited["card"]["name"], "台词口语化");
    assert_eq!(edited["card"]["source"], "对话加强.md");
    let del = c.ok(reqwest::Method::DELETE, &format!("/api/skills/{sid}"), None).await;
    assert_eq!(del["active"], json!(["deslop"]));
    assert_eq!(c.get("/api/skills").await.as_array().unwrap().len(), 1);

    // 贴进来的是网页而不是 Markdown 时给出明确提示
    let (status, v) = c.send(reqwest::Method::POST, "/api/skills", Some(json!({ "markdown": "<!DOCTYPE html><html></html>" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");
}

#[tokio::test]
async fn visibility_red_line() {
    let addr = spawn(build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await;
    let c = Client { base: format!("http://{addr}"), http: reqwest::Client::new() };
    let world = "矿奴求生的世界\n### 矿区\n赤炼宗黑灵矿\n### 世界真相\n天轨是伪神铸造的锁链\n### 上界〔第2卷起〕\n九霄天轨";
    let outline = "### 核心主线\n最终成为执秤真皇\n### 第一卷\n矿区求生\n### 第二卷\n灵台点灯";
    let book = c.post("/api/books", json!({ "title": "大梦", "worldview": world, "outline": outline })).await;
    let bid = book["id"].as_i64().unwrap();
    let v1 = c.post(&format!("/api/books/{bid}/volumes"), json!({ "title": "第一卷" })).await;
    let ch1 = c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "矿难", "volume_id": v1["id"], "outline": "陈渊在矿道里遇到姜沉雪" })).await;
    let cid = ch1["id"].as_i64().unwrap();

    // 条目的可见性按统一写法存，看不懂的写法直接报错
    let jiang = c
        .post(&format!("/api/books/{bid}/entries"), json!({ "name": "姜沉雪", "kind": "character", "description": "回春堂的医修", "secret": "逆命司第七席", "visibility": "公开" }))
        .await;
    assert_eq!((jiang["visibility"].as_str(), jiang["secret"].as_str()), (Some(""), Some("逆命司第七席")));
    let god = c.post(&format!("/api/books/{bid}/entries"), json!({ "name": "伪神", "kind": "concept", "always_include": true, "visibility": "3卷" })).await;
    assert_eq!(god["visibility"], "第3卷起");
    let (status, v) = c.send(reqwest::Method::PATCH, &format!("/api/entries/{}", god["id"]), Some(json!({ "visibility": "以后再说" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{v}");

    // 第1章：后期的世界观、总纲、设定和作者底牌都不给模型
    let ctx = c.get(&format!("/api/books/{bid}/context?chapter_id={cid}")).await;
    let text = ctx["sections"].to_string();
    assert!(text.contains("赤炼宗黑灵矿") && text.contains("矿区求生") && text.contains("回春堂的医修"), "{text}");
    for leak in ["伪神", "九霄天轨", "执秤真皇", "灵台点灯", "逆命司第七席", "〔"] {
        assert!(!text.contains(leak), "第1章的上下文不该有「{leak}」：{text}");
    }
    assert_eq!(ctx["withheld"]["worldview"], json!(["世界真相", "上界"]));
    assert_eq!(ctx["withheld"]["outline"], json!(["核心主线", "第二卷"]));
    assert_eq!(ctx["withheld"]["entries"], json!([{ "id": god["id"], "name": "伪神", "visibility": "第3卷起" }]));

    let pv = c.post("/api/ai/preview", json!({ "task": "write_chapter", "book_id": bid, "chapter_id": cid })).await;
    let (system, user) = (pv["messages"][0]["content"].as_str().unwrap(), pv["messages"][1]["content"].as_str().unwrap());
    assert!(system.contains("信息投放") && user.contains("新名词最多三个"), "写作规则和第1章开篇要求带上信息投放");
    assert!(!["伪神", "九霄天轨", "执秤真皇", "逆命司第七席"].iter().any(|w| user.contains(w)), "{user}");

    // 规划看作者层，但对 AI 隐藏的和作者底牌照样不给
    let pv = c.post("/api/ai/preview", json!({ "task": "chapter_outlines", "book_id": bid, "count": 3 })).await;
    let user = pv["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("天轨是伪神铸造的锁链") && user.contains("执秤真皇") && user.contains("设定「伪神」"), "{user}");
    assert!(!user.contains("逆命司第七席"));

    // 分节可见性：列出来、改一节、马上生效
    let vis = c.get(&format!("/api/books/{bid}/visibility")).await;
    let parts = vis["worldview"].as_array().unwrap();
    let gate_of = |title: &str| parts.iter().find(|p| p["title"] == title).map(|p| p["gate"].clone()).unwrap();
    assert_eq!((gate_of("矿区"), gate_of("世界真相"), gate_of("上界")), (json!("公开"), json!("仅规划"), json!("第2卷起")));
    let mine = parts.iter().find(|p| p["title"] == "矿区").unwrap()["index"].clone();
    let vis = c.post(&format!("/api/books/{bid}/visibility"), json!({ "field": "worldview", "index": mine, "visibility": "第2卷起" })).await;
    assert!(vis["worldview"].as_array().unwrap().iter().any(|p| p["title"] == "矿区" && p["marked"] == "第2卷起"));
    assert!(c.get(&format!("/api/books/{bid}")).await["worldview"].as_str().unwrap().contains("### 矿区〔第2卷起〕"));
    assert!(!c.get(&format!("/api/books/{bid}/context?chapter_id={cid}")).await["sections"].to_string().contains("赤炼宗黑灵矿"));
    let (status, _) = c.send(reqwest::Method::POST, &format!("/api/books/{bid}/visibility"), Some(json!({ "field": "worldview", "index": 0, "visibility": "仅规划" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "开头没有标题的部分不能加标记");
}

/// 拿真实书稿跑一遍体检，看规则检查在实际稿子上报了什么。AI_NOVEL_CHECK_DB 指向一份数据库副本（打开时会升级表结构，别指向正在用的库）。
/// AI_NOVEL_CHECK_DB=副本路径 cargo test --test api real_book_checks -- --ignored --nocapture
#[test]
#[ignore]
fn real_book_checks() {
    let Some(path) = std::env::var_os("AI_NOVEL_CHECK_DB") else { return };
    let db = Db::open(std::path::Path::new(&path)).unwrap();
    for b in db.list_books().unwrap() {
        let data = ai_novel::api::load_book_data(&db, b.id).unwrap();
        println!("== 《{}》", b.title);
        for f in ai_novel::continuity::check(&data, None) {
            println!("[{}] {}：{}", f.level, f.kind, f.message);
            if !f.quote.is_empty() {
                println!("    原文：{}", f.quote);
            }
        }
    }
}
