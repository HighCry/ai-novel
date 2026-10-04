//! 真实模型联调：默认跳过。需要设置环境变量后运行：
//!   AI_NOVEL_LIVE_KEY=sk-...（必填）
//!   AI_NOVEL_LIVE_BASE=http://127.0.0.1:8080/v1（默认）
//!   AI_NOVEL_LIVE_WRITER=claude-sonnet-4-6、AI_NOVEL_LIVE_ANALYST=gemini-3-flash（默认）
//!   cargo test --test live_model -- --ignored --nocapture
//! 某一步失败不会中断，所有结果和错误都写到 target/live-report.md。

use ai_novel::{build_router, db::Db, state::AppState};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::time::Instant;

struct Live {
    base: String,
    http: reqwest::Client,
    report: String,
    failures: Vec<String>,
}

impl Live {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = self.http.request(method.clone(), format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{method} {path} → {status}: {text}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    async fn sse(&self, body: Value) -> Result<String, String> {
        let resp = self.http.post(format!("{}/api/ai/stream", self.base)).json(&body).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("stream {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        let text = resp.text().await.map_err(|e| e.to_string())?;
        let mut event = String::new();
        for line in text.lines() {
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim().to_string();
            } else if let Some(d) = line.strip_prefix("data:") {
                let v: Value = serde_json::from_str(d.trim()).map_err(|e| e.to_string())?;
                match event.as_str() {
                    "done" => return Ok(v["text"].as_str().unwrap_or_default().to_string()),
                    "error" => return Err(format!("流式生成报错：{}", v["message"])),
                    _ => {}
                }
            }
        }
        Err("流式响应没有 done 事件".into())
    }

    /// 返回（完整文本, meta 事件里用到的范文和指南）
    async fn sse_meta(&self, body: Value) -> Result<(String, Value), String> {
        let resp = self.http.post(format!("{}/api/ai/stream", self.base)).json(&body).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("stream {}: {}", resp.status(), resp.text().await.unwrap_or_default()));
        }
        let text = resp.text().await.map_err(|e| e.to_string())?;
        let (mut event, mut meta) = (String::new(), Value::Null);
        for line in text.lines() {
            if let Some(e) = line.strip_prefix("event:") {
                event = e.trim().to_string();
            } else if let Some(d) = line.strip_prefix("data:") {
                let v: Value = serde_json::from_str(d.trim()).map_err(|e| e.to_string())?;
                match event.as_str() {
                    "meta" => meta = v,
                    "done" => return Ok((v["text"].as_str().unwrap_or_default().to_string(), meta)),
                    "error" => return Err(format!("流式生成报错：{}", v["message"])),
                    _ => {}
                }
            }
        }
        Err("流式响应没有 done 事件".into())
    }

    fn record(&mut self, title: &str, started: Instant, result: Result<String, String>) -> Option<String> {
        let secs = started.elapsed().as_secs_f64();
        match result {
            Ok(body) => {
                let _ = writeln!(self.report, "\n## {title}（{secs:.1} 秒）\n\n{body}\n");
                println!("[{secs:>5.1}s] ✓ {title}：{}", body.chars().take(50).collect::<String>().replace('\n', " "));
                Some(body)
            }
            Err(e) => {
                let _ = writeln!(self.report, "\n## ❌ {title}（{secs:.1} 秒）\n\n```\n{e}\n```\n");
                println!("[{secs:>5.1}s] ✗ {title}：{}", e.chars().take(300).collect::<String>());
                self.failures.push(format!("{title}：{e}"));
                None
            }
        }
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn check(ok: bool, v: Value, what: &str) -> Result<Value, String> {
    if ok { Ok(v) } else { Err(format!("{what}：{v}")) }
}

#[tokio::test]
#[ignore]
async fn live_model_flow() {
    let Ok(key) = std::env::var("AI_NOVEL_LIVE_KEY") else {
        eprintln!("没有设置 AI_NOVEL_LIVE_KEY，跳过");
        return;
    };
    let base = std::env::var("AI_NOVEL_LIVE_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".into());
    let writer = std::env::var("AI_NOVEL_LIVE_WRITER").unwrap_or_else(|_| "claude-sonnet-4-6".into());
    let analyst = std::env::var("AI_NOVEL_LIVE_ANALYST").unwrap_or_else(|_| "gemini-3-flash".into());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await.unwrap() });
    let mut c = Live { base: format!("http://{addr}"), http: reqwest::Client::new(), report: format!("# 真实模型联调报告\n\n写作模型：{writer}；分析模型：{analyst}\n"), failures: vec![] };

    c.call(
        reqwest::Method::PUT,
        "/api/settings",
        Some(json!({
            "providers": [{ "id": "live", "name": "sub2api", "base_url": base, "api_key": key }],
            "writer": { "provider_id": "live", "model": writer, "temperature": 0.85, "max_tokens": 8000 },
            "analyst": { "provider_id": "live", "model": analyst, "temperature": 0.3, "max_tokens": 8000 },
            "stream_usage": true
        })),
    )
    .await
    .unwrap();

    let fields = json!({ "genre": "玄幻", "platform": "fanqie", "idea": "山村少年捡到一块黑色玉佩，身边有人说谎时玉佩就会变凉。", "protagonist": "十六岁的林凡，嘴上不饶人，心里重情义" });
    let t = Instant::now();
    let ideas = c.post("/api/ai/json", json!({ "task": "ideas", "fields": fields })).await.and_then(|v| check(v["ideas"][0]["title"].is_string(), v, "格式不对"));
    let idea = ideas.as_ref().map(|v| v["ideas"][0].clone()).unwrap_or(json!({ "title": "黑玉", "logline": "山村少年捡到能感知谎言的玉佩" }));
    c.record("开书方案", t, ideas.map(|v| pretty(&v)));

    let mut fields = fields.clone();
    fields["title"] = idea["title"].clone();
    fields["logline"] = idea["logline"].clone();
    let t = Instant::now();
    let world = c.post("/api/ai/json", json!({ "task": "world", "fields": fields })).await.map(|v| v["text"].as_str().unwrap_or_default().to_string());
    let world = c.record("世界观", t, world).unwrap_or_default();
    fields["worldview"] = json!(world);

    let t = Instant::now();
    let chars = c.post("/api/ai/json", json!({ "task": "characters", "fields": fields })).await.and_then(|v| check(v["characters"].as_array().map(|a| a.len() >= 3).unwrap_or(false), v, "人物太少"));
    let list = chars.as_ref().ok().and_then(|v| v["characters"].as_array().cloned()).unwrap_or_default();
    c.record("主要人物", t, chars.map(|v| pretty(&v)));

    let book = c.post("/api/books", json!({ "title": idea["title"], "genre": "玄幻", "platform": "fanqie", "logline": idea["logline"], "worldview": world, "target_words": 2000 })).await.unwrap();
    let bid = book["id"].as_i64().unwrap();
    let vol = c.post(&format!("/api/books/{bid}/volumes"), json!({ "title": "第一卷" })).await.unwrap();
    for ch in &list {
        let _ = c
            .post(
                &format!("/api/books/{bid}/entries"),
                json!({ "kind": "character", "name": ch["name"], "aliases": ch["aliases"], "role": ch["role"], "description": ch["description"], "immutable": ch["immutable"], "state": ch["state"], "always_include": ch["role"] == "主角" }),
            )
            .await;
    }

    let t = Instant::now();
    let outlines = c.post("/api/ai/json", json!({ "task": "chapter_outlines", "book_id": bid, "volume_id": vol["id"], "count": 3 })).await.and_then(|v| check(v["chapters"].as_array().map(|a| !a.is_empty()).unwrap_or(false), v, "没有章纲"));
    let planned = outlines.as_ref().ok().and_then(|v| v["chapters"].as_array().cloned()).unwrap_or_else(|| vec![json!({ "title": "玉佩", "outline": "林凡捡到黑色玉佩，发现它能感知谎言" }); 3]);
    c.record("前三章章纲", t, outlines.map(|v| pretty(&v)));
    let mut ids = Vec::new();
    for p in planned.iter().take(3) {
        ids.push(c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": p["title"], "outline": p["outline"], "volume_id": vol["id"] })).await.unwrap()["id"].as_i64().unwrap());
    }
    while ids.len() < 3 {
        ids.push(c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "后续", "volume_id": vol["id"] })).await.unwrap()["id"].as_i64().unwrap());
    }

    let t = Instant::now();
    let beats = c.sse(json!({ "task": "beats", "book_id": bid, "chapter_id": ids[0] })).await;
    let beats = c.record("第1章节拍（流式）", t, beats).unwrap_or_default();
    let _ = c.call(reqwest::Method::PATCH, &format!("/api/chapters/{}", ids[0]), Some(json!({ "beats": beats }))).await;

    let t = Instant::now();
    let chapter = c.sse(json!({ "task": "write_chapter", "book_id": bid, "chapter_id": ids[0], "words": 2000 })).await.and_then(|text| {
        let n = text.chars().filter(|c| !c.is_whitespace()).count();
        if n > 800 { Ok(format!("（{n} 字）\n\n{text}")) } else { Err(format!("正文太短：{n} 字\n{text}")) }
    });
    let chapter = c.record("第1章正文（流式）", t, chapter).map(|s| s.split_once("\n\n").map(|(_, b)| b.to_string()).unwrap_or(s)).unwrap_or_default();
    let _ = c.call(reqwest::Method::PATCH, &format!("/api/chapters/{}", ids[0]), Some(json!({ "content": chapter }))).await;

    let t = Instant::now();
    let lint = c.post("/api/lint", json!({ "text": chapter })).await.map(|v| {
        let items: Vec<String> = v["issues"].as_array().map(|a| a.iter().map(|i| format!("{}「{}」×{}", i["kind"].as_str().unwrap_or(""), i["text"].as_str().unwrap_or(""), i["count"])).collect()).unwrap_or_default();
        format!("{} 分\n{}", v["score"], items.join("\n"))
    });
    c.record("本地文字检查", t, lint);

    let with = |task: &str| json!({ "task": task, "book_id": bid, "chapter_id": ids[0] });
    let t = Instant::now();
    let (summary, extract, tension) = tokio::join!(c.post("/api/ai/json", with("summarize")), c.post("/api/ai/json", with("extract")), c.post("/api/ai/json", with("tension")));
    c.record("定稿：摘要", t, summary.map(|v| v["text"].as_str().unwrap_or_default().to_string()));
    let extract_value = extract.clone().ok();
    c.record("定稿：设定变化", t, extract.map(|v| pretty(&v)));
    c.record("定稿：张力分析", t, tension.and_then(|v| check(v["tension"].is_number(), v, "张力格式不对")).map(|v| pretty(&v)));
    if let Some(mut updates) = extract_value {
        updates["chapter_id"] = json!(ids[0]);
        let t = Instant::now();
        let applied = c.post(&format!("/api/books/{bid}/apply_updates"), updates).await.map(|v| v.to_string());
        c.record("写回设定库", t, applied);
    }

    let t = Instant::now();
    let (chk, review) = tokio::join!(c.post("/api/ai/json", with("check")), c.post("/api/ai/json", with("review")));
    c.record("一致性检查", t, chk.and_then(|v| check(v["issues"].is_array(), v, "格式不对")).map(|v| pretty(&v)));
    c.record("编辑审稿", t, review.and_then(|v| check(v["scores"].is_object(), v, "格式不对")).map(|v| pretty(&v)));

    let t = Instant::now();
    let cont = c.post("/api/ai/json", json!({ "task": "continue", "book_id": bid, "chapter_id": ids[1], "words": 500 })).await.map(|v| v["text"].as_str().unwrap_or_default().to_string());
    c.record("第2章续写（500 字）", t, cont);

    let para: String = chapter.lines().filter(|l| l.chars().count() > 30).take(3).collect::<Vec<_>>().join("\n");
    if !para.is_empty() {
        let t = Instant::now();
        let polished = c.post("/api/ai/json", json!({ "task": "polish", "book_id": bid, "chapter_id": ids[0], "selection": para })).await.map(|v| format!("【原文】\n{para}\n\n【润色后】\n{}", v["text"].as_str().unwrap_or_default()));
        c.record("润色", t, polished);
    }

    let t = Instant::now();
    let stats = c.call(reqwest::Method::GET, &format!("/api/books/{bid}/stats"), None).await.map(|v| format!("{}", v["tokens"]));
    c.record("用量", t, stats);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("live-report.md");
    std::fs::write(&path, &c.report).unwrap();
    println!("完整报告：{}", path.display());
    assert!(c.failures.is_empty(), "有 {} 步失败：\n{}", c.failures.len(), c.failures.join("\n"));
}

const LIVE_FIGHT: &str = "刀来得很快。\n陈默没躲，左肩一沉，让刀锋贴着锁骨擦过去，右拳已经砸进对方肋下。骨头响了一声，很脆。\n那人弯下腰，刀还没落地，陈默的膝盖已经顶上他的下巴。\n第二个人从背后扑上来。陈默往前一扑，滚过半张桌子，抄起条凳回身就抡。条凳断成两截，那人也趴下了。\n巷子里忽然安静。\n剩下的三个人互相看了一眼，谁都没动。\n陈默把断凳扔到脚边，抹了一把嘴角的血：“还来不来？”";
const LIVE_RAIN: &str = "雨是半夜下起来的。\n先是瓦片上响了几声，像有人在屋顶上来回走。后来声音密了，连成一片，把巷子里的狗叫都盖住了。\n沈青没睡。她坐在灯下数铜板，数到第三遍，还是差七个。\n窗纸被风顶得一鼓一鼓的，灯苗跟着晃。她伸手去护，手背碰到灯罩，烫得一缩。\n铜板滚了一地。\n她没去捡，就那么坐着，听雨从屋檐上往下砸，砸进门口那只缺了口的水缸里，一声，又一声。";
const LIVE_OPENING: &str = "天还没亮，林凡就被冻醒了。\n柴房的门板漏风，他缩在稻草堆里，怀里揣着那块捡来的黑玉。玉是凉的，凉得像刚从井里捞出来。\n外头有人在喊他的名字，是王管事的声音，又尖又急。\n林凡爬起来，拍掉身上的草屑，推开门。\n王管事站在院子里，脸色比天色还难看：“昨晚库房丢了一袋米。我亲眼看见你半夜进了库房！”\n林凡还没开口，怀里的黑玉忽然一凉，";

/// 文风库和编辑器新功能的真实模型联调，报告写到 target/live-report-v4.md
#[tokio::test]
#[ignore]
async fn live_style_library() {
    let Ok(key) = std::env::var("AI_NOVEL_LIVE_KEY") else {
        eprintln!("没有设置 AI_NOVEL_LIVE_KEY，跳过");
        return;
    };
    let base = std::env::var("AI_NOVEL_LIVE_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080/v1".into());
    let writer = std::env::var("AI_NOVEL_LIVE_WRITER").unwrap_or_else(|_| "claude-sonnet-4-6".into());
    let analyst = std::env::var("AI_NOVEL_LIVE_ANALYST").unwrap_or_else(|_| "gemini-3-flash".into());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, build_router(AppState::new(Db::open_in_memory().unwrap(), None))).await.unwrap() });
    let mut c = Live { base: format!("http://{addr}"), http: reqwest::Client::new(), report: format!("# 文风库与编辑器联调报告\n\n写作模型：{writer}；分析模型：{analyst}\n"), failures: vec![] };
    c.call(
        reqwest::Method::PUT,
        "/api/settings",
        Some(json!({
            "providers": [{ "id": "live", "name": "sub2api", "base_url": base, "api_key": key }],
            "writer": { "provider_id": "live", "model": writer, "temperature": 0.85, "max_tokens": 8000 },
            "analyst": { "provider_id": "live", "model": analyst, "temperature": 0.3, "max_tokens": 8000 },
            "stream_usage": true, "library_refs": 2
        })),
    )
    .await
    .unwrap();

    let book = c.post("/api/books", json!({ "title": "黑玉", "genre": "玄幻", "platform": "fanqie", "worldview": "黑玉：持有者身边有人说谎时，玉会变凉；说谎越重，越凉。" })).await.unwrap();
    let bid = book["id"].as_i64().unwrap();
    for (name, role, desc) in [("林凡", "主角", "十六岁的外门杂役，嘴上不饶人，心里重情义"), ("王管事", "配角", "外门杂役管事，贪财，爱找杂役的麻烦")] {
        c.post(&format!("/api/books/{bid}/entries"), json!({ "kind": "character", "name": name, "role": role, "description": desc })).await.unwrap();
    }
    let ch = c.post(&format!("/api/books/{bid}/chapters"), json!({ "title": "黑玉", "outline": "王管事诬陷林凡偷米，林凡靠黑玉发现他在说谎，当众反咬回去", "content": LIVE_OPENING })).await.unwrap();
    let cid = ch["id"].as_i64().unwrap();

    let fight = c.post("/api/library", json!({ "title": "巷战", "content": LIVE_FIGHT, "genre": "武侠", "tags": ["打斗"], "note": "动作干脆，一招一个结果" })).await.unwrap();
    let rain = c.post("/api/library", json!({ "title": "雨夜", "content": LIVE_RAIN, "genre": "古言", "tags": ["环境"], "note": "环境和人物心事揉在一起" })).await.unwrap();
    for item in [&fight, &rain] {
        let t = Instant::now();
        let r = c.post(&format!("/api/library/{}/analyze", item["id"]), json!({})).await.and_then(|v| check(!v["analysis"].as_str().unwrap_or("").is_empty(), v, "没有分析结果"));
        let title = format!("范文分析：《{}》", item["title"].as_str().unwrap_or(""));
        c.record(&title, t, r.map(|v| format!("标签：{}\n\n{}", v["tags"], v["analysis"].as_str().unwrap_or(""))));
    }

    let t = Instant::now();
    let guide = c.post("/api/library/guide/distill", json!({ "genre": "" })).await.map(|v| v["content"].as_str().unwrap_or_default().to_string());
    c.record("提炼文风指南（第一版）", t, guide);

    let t = Instant::now();
    let ghost = c.sse_meta(json!({ "task": "ghost", "book_id": bid, "chapter_id": cid, "before": LIVE_OPENING, "after": "", "words": 50 })).await;
    let ghost_text = ghost.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
    c.record("灰字续写（约 50 字）", t, ghost.and_then(|(text, meta)| {
        let n = text.chars().filter(|c| !c.is_whitespace()).count();
        let repeats = text.trim_start().starts_with("林凡还没开口");
        if n == 0 || n > 300 || repeats { Err(format!("长度 {n}，重复前文：{repeats}\n{text}")) } else { Ok(format!("（{n} 字，参考：{}）\n\n{LIVE_OPENING}【{text}】", meta["refs"])) }
    }));

    let t = Instant::now();
    let insert = c.sse_meta(json!({ "task": "insert", "book_id": bid, "chapter_id": cid, "before": LIVE_OPENING, "goal": "写一段环境或场景描写，用具体的感官细节烘托此刻的气氛，不推进剧情", "tags": ["环境"], "words": 150 })).await;
    c.record("斜杠指令：环境描写（约 150 字）", t, insert.map(|(text, meta)| format!("（参考：{}）\n\n{text}", meta["refs"])));

    let t = Instant::now();
    let names = c.post("/api/ai/json", json!({ "task": "names", "book_id": bid, "goal": "林凡所在宗门的名字" })).await.and_then(|v| check(v["names"].as_array().map(|a| !a.is_empty()).unwrap_or(false), v, "没有名字"));
    c.record("起名：宗门", t, names.map(|v| pretty(&v)));

    // 作者采纳灰字后按自己的口味改：删掉解释性的心理句
    if !ghost_text.trim().is_empty() {
        let accepted = ghost_text.trim().to_string();
        c.post(&format!("/api/chapters/{cid}/ai_accept"), json!({ "chars": accepted.chars().count(), "text": accepted, "task": "ghost", "before": LIVE_OPENING.chars().rev().take(30).collect::<String>().chars().rev().collect::<String>(), "after": "" })).await.unwrap();
        let first = accepted.split(['。', '！', '？']).next().unwrap_or(&accepted).to_string();
        let edited = format!("{LIVE_OPENING}{first}。\n林凡抬起头，直直看着王管事：“你再说一遍？”");
        c.call(reqwest::Method::PATCH, &format!("/api/chapters/{cid}"), Some(json!({ "content": edited }))).await.unwrap();
        let t = Instant::now();
        let fb = c.call(reqwest::Method::GET, "/api/library/feedback", None).await.map(|v| pretty(&v));
        c.record("修改样本", t, fb);
        let t = Instant::now();
        let guide2 = c.post("/api/library/guide/distill", json!({ "genre": "" })).await.map(|v| v["content"].as_str().unwrap_or_default().to_string());
        c.record("提炼文风指南（加入作者修改后）", t, guide2);
    }

    let t = Instant::now();
    let stats = c.call(reqwest::Method::GET, &format!("/api/books/{bid}/stats"), None).await.map(|v| format!("{}", v["tokens"]));
    c.record("用量", t, stats);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("live-report-v4.md");
    std::fs::write(&path, &c.report).unwrap();
    println!("完整报告：{}", path.display());
    assert!(c.failures.is_empty(), "有 {} 步失败：\n{}", c.failures.len(), c.failures.join("\n"));
}
