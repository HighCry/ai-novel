//! 模拟的 OpenAI 兼容接口，用于本地联调和演示界面，不消耗真实模型额度。
//! 运行：cargo run --example mock_openai -- 18080
//! 然后在设置里添加接口 http://127.0.0.1:18080/v1，模型随便填（比如 mock-writer）。

use axum::body::{Body, Bytes};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;

const PROSE: &str = "夜色压下来的时候，林凡才走到藏经阁后墙。\n墙根的青苔湿得发黑，他贴着墙站了一会儿，听见里面有翻书的声音。\n“这个时辰，谁还在？”他压低声音嘀咕了一句，伸手摸了摸怀里的黑色玉佩。\n玉佩是凉的，凉得像一块刚从井里捞出来的石头。\n他踩着墙缝翻上去，落地时膝盖一软，差点撞上窗台。窗纸透出一点昏黄的光，一个穿黑袍的人背对着窗，正把一卷竹简往袖子里塞。\n林凡屏住呼吸。\n那人忽然停了手，侧过半张脸——左眼下面有一道旧疤，一直拉到嘴角。\n“出来吧。”黑袍人说，“从你翻墙的时候我就听见了。”";

const SUMMARY: &str = "林凡夜探藏经阁，在后墙听见有人翻书。他翻墙潜入，看到一名左眼带疤的黑袍人正在偷竹简。黑袍人早已察觉林凡，点破了他的行踪。黑色玉佩在此时发凉，暗示与黑袍人或藏经阁有关。";

const PLAN_JSON: &str = r#"{
 "ideas":[
  {"title":"差评返还系统","logline":"外卖员林凡绑定差评返还系统，每个差评都能十倍返还成能力，从此专挑最难缠的顾客送餐。","selling_points":"反向爽点：越被刁难越强；都市职场+脑洞","opening":"暴雨夜，林凡给一位出了名难缠的客户送餐"},
  {"title":"我在藏经阁扫了十年地","logline":"杂役弟子林凡在藏经阁扫地十年，每扫一层灰就领悟一门失传功法。","selling_points":"苟道流+厚积薄发，扫地僧逆袭","opening":"宗门大比当天，林凡还在扫地"},
  {"title":"黑玉","logline":"山村少年捡到一块会变凉的黑色玉佩，每次变凉，身边就会有人说谎。","selling_points":"测谎金手指+悬疑解谜","opening":"村长家丢了东西，玉佩突然变凉"}
 ],
 "characters":[
  {"name":"林凡","aliases":"凡哥、小林","role":"主角","description":"山村少年，心细胆大，嘴上不饶人，重情义。","immutable":"不对弱者出手；遇事先观察再动手","state":"青云宗外门杂役，炼气三层"},
  {"name":"苏雨","aliases":"大师姐","role":"配角","description":"青云宗内门大师姐，外冷内热，剑道天才。","immutable":"从不说谎","state":"内门首席，筑基中期"},
  {"name":"黑袍人","aliases":"疤脸","role":"反派","description":"身份不明的刺客，左眼下有一道旧疤，目标是藏经阁里的禁术竹简。","immutable":"左眼下有疤","state":"潜伏在青云宗"}
 ],
 "chapters":[
  {"title":"夜探藏经阁","outline":"林凡为查玉佩来历夜探藏经阁，撞见黑袍人偷竹简，被对方察觉。章末黑袍人点破林凡行踪。"},
  {"title":"疤脸","outline":"林凡与黑袍人短暂交手，靠玉佩发凉判断对方的谎话，侥幸脱身，但被认出了外门服饰。"},
  {"title":"大师姐的剑","outline":"苏雨巡夜发现藏经阁异动，林凡被当成嫌疑人。他用细节证明自己不是窃贼，苏雨半信半疑。"}
 ],
 "entries":[
  {"name":"林凡","kind":"character","is_new":false,"fields":{"location":"藏经阁二楼","power":"炼气三层","mind":"紧张但冷静","recent":"潜入藏经阁后被黑袍人发现","knows":"有人在偷禁术竹简"}},
  {"name":"黑袍人","kind":"character","is_new":true,"aliases":"疤脸","description":"左眼下有旧疤的神秘人，在藏经阁偷竹简。","state":"察觉林凡，正在对峙"},
  {"name":"藏经阁","kind":"location","is_new":true,"description":"青云宗存放功法典籍的楼阁，夜间无人值守。","state":"失窃"}
 ],
 "threads_new":[{"title":"黑袍人偷走的竹简","detail":"竹简内容未知，可能与禁术或玉佩有关"}],
 "threads_progressed":[],
 "threads_resolved":[],
 "relations":[{"a":"林凡","b":"黑袍人","kind":"敌对","detail":"藏经阁对峙","status":"active"},{"a":"林凡","b":"苏雨","kind":"师姐弟","detail":"苏雨暗中照顾林凡","status":"active"}],
 "tension":7,"emotion":"紧张","cool_points":[{"type":"揭秘","level":"小","desc":"撞破黑袍人偷竹简"}],"hook":{"type":"截断","desc":"黑袍人点破林凡的行踪"},"debts":["黑袍人是谁","竹简里写了什么"],"comment":"悬念足，爽点偏少",
 "issues":[{"severity":"medium","type":"能力前后不一致","quote":"他踩着墙缝翻上去，落地时膝盖一软","problem":"设定里林凡擅长轻身功夫，这里的狼狈和设定略有出入","suggestion":"可以改成故意放轻落地，或交代刚受过伤"}],
 "scores":{"hook":8,"pacing":7,"payoff":6,"character":7,"ending":9,"readability":8},
 "summary":"悬念足，章末钩子强；爽点偏少，可以给主角一个小的主动行为。",
 "strengths":["开场直接进入场景","黑袍人登场有画面感","章末反转抓人"],
 "problems":[{"quote":"玉佩是凉的，凉得像一块刚从井里捞出来的石头。","problem":"比喻和前文“湿得发黑”的氛围重复","suggestion":"换成触感以外的反应，比如林凡下意识松手"}],
 "techniques":["动作用三到六字的短句连发，一个动作一句","先写结果再补原因，制造冲击感","对话只留最有火药味的半句"],
 "rhythm":"短句推进，关键处用单句成段","dialogue":"对话极短，靠动作补充情绪","imitate":"每个动作都要有对手的反应",
 "tags":["打斗","爽点"],"chunk_tags":{"1":["打斗"],"2":["爽点"]},
 "names":[{"name":"沈砚","note":"砚台的砚，沉稳内敛"},{"name":"顾长夜","note":"有江湖气，适合剑客"},{"name":"云栖山","note":"地名，清冷出尘"}]
}"#;

const GUIDE: &str = "【叙事】贴着主角的眼睛写，只写他此刻看到、听到、想到的东西。\n【节奏】打斗用短句，一个动作一句；情绪爆发前先压一段。\n【对话】对话短，带火药味，少用“说道”。\n【描写】只写一两个最抓人的细节，不堆形容词。\n【禁忌】不写段尾感悟，不用“这一刻”“命运的齿轮”。";
const GHOST: &str = "他没有回头，只是把玉佩攥得更紧了些。";
const INSERT: &str = "檐角的风铃响了一声。月光从窗棂缝里漏进来，落在青砖地上，碎成一格一格的白。远处传来打更的梆子声，三更了。";

const WORLD: &str = "【时代与地图】\n东域九州，宗门林立。青云宗坐镇云州，下辖外门、内门与藏经阁。\n\n【修炼体系】\n炼气（一至九层）→ 筑基（初、中、后期）→ 金丹 → 元婴。每个大境界之间战力差距明显，越阶而战极少见。\n\n【主要势力】\n青云宗：正道大宗，内部分为传功、执法两派，暗中不和。\n黑市“无面楼”：收钱办事的刺客组织，黑袍人的来历与此有关。\n\n【金手指】\n黑色玉佩：持有者身边有人说谎时会变凉；只对说话人有效，对文字无效；连续使用会让持有者头痛。";

fn pick(prompt: &str, stream: bool) -> &'static str {
    if prompt.contains("整理他自己的《文风指南》") {
        GUIDE
    } else if prompt.contains("只输出 JSON") {
        PLAN_JSON
    } else if prompt.contains("请从光标处接着写") {
        GHOST
    } else if prompt.contains("请在光标处写一段") {
        INSERT
    } else if prompt.contains("剧情摘要") || prompt.contains("卷摘要") {
        SUMMARY
    } else if prompt.contains("世界观") && prompt.contains("设计") && !stream {
        WORLD
    } else if prompt.contains("请为这部") || prompt.contains("世界观。只写") {
        WORLD
    } else if prompt.contains("连接测试") {
        "收到"
    } else {
        PROSE
    }
}

async fn models() -> Json<Value> {
    Json(json!({ "data": [{ "id": "mock-writer" }, { "id": "mock-analyst" }] }))
}

async fn chat(Json(body): Json<Value>) -> Response {
    let prompt = body["messages"].to_string();
    let stream = body["stream"].as_bool().unwrap_or(false);
    let content = pick(&prompt, stream);
    if !stream {
        tokio::time::sleep(Duration::from_millis(400)).await;
        return Json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }],
            "usage": { "prompt_tokens": prompt.chars().count() * 4 / 5, "completion_tokens": content.chars().count() * 4 / 5 }
        }))
        .into_response();
    }
    let mut events = vec![format!("data: {}\n\n", json!({ "choices": [{ "delta": { "reasoning_content": "先梳理一下前情……" } }] }))];
    let chars: Vec<char> = content.chars().collect();
    for piece in chars.chunks(6) {
        events.push(format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": piece.iter().collect::<String>() } }] })));
    }
    events.push("data: [DONE]\n\n".to_string());
    let body = futures_util::stream::iter(events).then(|e| async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok::<_, Infallible>(Bytes::from(e))
    });
    ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(body)).into_response()
}

/// 按字符散列成 32 维向量，只用于演示向量检索
async fn embeddings(Json(body): Json<Value>) -> Json<Value> {
    let inputs = body["input"].as_array().cloned().unwrap_or_default();
    let data: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut v = vec![0f32; 32];
            for c in s.as_str().unwrap_or("").chars() {
                v[c as usize % 32] += 1.0;
            }
            json!({ "index": i, "embedding": v })
        })
        .collect();
    Json(json!({ "data": data }))
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(18080);
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/embeddings", post(embeddings));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("端口被占用");
    println!("模拟 OpenAI 接口：http://127.0.0.1:{port}/v1");
    axum::serve(listener, app).await.unwrap();
}
