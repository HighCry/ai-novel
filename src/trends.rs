//! 热点和榜单：微博、抖音、B站、百度的热搜，起点手机版男女频榜单（前 60 本），番茄分类榜（前 50 本）；
//! 统计题材分布、书名高频词和简介里的卖点标签；AI 从热搜里挑能写进网文的梗，按榜单推断读者偏好。
//! 只在作者打开时抓，缓存 6 小时。
//! 番茄榜单里的书名、作者、简介是加密字体（私用区字符），内置了当前字体的完整对照表；
//! 字体换了先下载新字体和内置的比字形（fanqie_font），比不出来的再对照书页里的明文学，学到的存起来。

use crate::ai::{run_json, AiRequest};
use crate::api::{bad_request, require_book, ApiResult, AppError};
use crate::db::{now, Db};
use crate::models::Book;
use crate::state::AppState;
use anyhow::{anyhow, Context};
use axum::extract::{Query, State};
use axum::Json;
use futures_util::future::join_all;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

const CACHE_SECS: i64 = 6 * 3600;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
/// 每份榜单取前多少本：起点一页 20 本，番茄的接口一次能取 50 本
const QIDIAN_DEPTH: usize = 60;
const FANQIE_DEPTH: usize = 50;
/// 番茄短时间内请求太多会整站限流（444），学字要克制：每次抓取最多开几个书页、每轮同时开几页、
/// 每 10 分钟所有抓取合计最多开几页。对照表会存下来，学全以后基本不用再开书页
const FANQIE_LEARN_PAGES: usize = 12;
const FANQIE_LEARN_BATCH: usize = 1;
const FANQIE_PAGE_BUDGET: (i64, usize) = (600, 24);
/// 书页被限流时榜单页和接口往往还能用，单独记
const FANQIE_PAGE_KEY: &str = "fanqienovel.com/page";
/// 还有没认出来的字时，缓存只留这么久，下次打开接着学
const UNDECODED_CACHE_SECS: i64 = 600;
/// 被网站限流后这么久不再发请求，免得越刷越久
const BLOCK_SECS: i64 = 30 * 60;
/// 简介开头这么多字和书页的描述对得上，能学；表格里显示的、卖点标签也都在这一段
const DESC_HEAD: usize = 60;
const GLYPH_KEY: &str = "fanqie_glyphs";

/// 起点手机版榜单：(网址里的名字, 翻页接口, 名称)
pub const QIDIAN_RANKS: &[(&str, &str, &str)] = &[
    ("yuepiao", "yuepiaolist", "月票榜"),
    ("hotsales", "hotsaleslist", "畅销榜"),
    ("readindex", "readIndexlist", "阅读指数榜"),
    ("newfans", "newfanslist", "书友榜"),
    ("newauthor", "newauthorlist", "新人榜"),
];

/// 番茄的分类（榜单页会带最新的，没抓到时用这份）：(男频 1 / 女频 0, 分类 id, 名称)
const FANQIE_CATS: &[(u8, &str, &str)] = &[
    (1, "258", "传统玄幻"), (1, "257", "玄幻脑洞"), (1, "1140", "东方仙侠"), (1, "1141", "西方奇幻"), (1, "261", "都市日常"),
    (1, "124", "都市修真"), (1, "1014", "都市高武"), (1, "262", "都市脑洞"), (1, "263", "都市种田"), (1, "273", "历史古代"),
    (1, "272", "历史脑洞"), (1, "8", "科幻末世"), (1, "539", "悬疑脑洞"), (1, "751", "悬疑灵异"), (1, "27", "战神赘婿"),
    (1, "504", "抗战谍战"), (1, "746", "游戏体育"), (1, "718", "动漫衍生"), (1, "1016", "男频衍生"),
    (0, "1139", "古风世情"), (0, "253", "古言脑洞"), (0, "246", "宫斗宅斗"), (0, "248", "玄幻言情"), (0, "267", "现言脑洞"),
    (0, "749", "青春甜宠"), (0, "750", "职场婚恋"), (0, "79", "年代"), (0, "23", "种田"), (0, "24", "快穿"),
    (0, "745", "星光璀璨"), (0, "747", "女频悬疑"), (0, "8", "科幻末世"), (0, "746", "游戏体育"), (0, "1015", "女频衍生"),
];
const FEMALE_HINTS: &[&str] = &["言情", "女频", "古言", "宫斗", "宅斗", "甜宠", "婚恋", "快穿", "年代", "现言"];

fn is_female(genre: &str) -> bool {
    FEMALE_HINTS.iter().any(|h| genre.contains(h))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotItem {
    pub source: String,
    pub word: String,
    pub heat: i64,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RankBook {
    pub rank: usize,
    pub title: String,
    pub author: String,
    pub category: String,
    pub words: String,
    /// 榜单上的热度：月票数、在读人数
    pub metric: String,
    pub status: String,
    pub desc: String,
    /// 简介里的卖点标签
    #[serde(default)]
    pub tags: Vec<String>,
}

// ---------- 抓取 ----------

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    // 国内站点直连，不走系统代理
    C.get_or_init(|| reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(12)).build().expect("HTTP 客户端"))
}

fn host_of(url: &str) -> &str {
    url.split("://").nth(1).and_then(|s| s.split('/').next()).unwrap_or(url)
}

/// 各网站限流到什么时候
fn blocks() -> std::sync::MutexGuard<'static, HashMap<String, i64>> {
    static B: OnceLock<std::sync::Mutex<HashMap<String, i64>>> = OnceLock::new();
    B.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn block(key: &str) {
    blocks().insert(key.to_string(), now() + BLOCK_SECS);
}

/// 还要停几分钟；没在限流期内是 None
fn blocked_minutes(key: &str) -> Option<i64> {
    blocks().get(key).copied().filter(|t| *t > now()).map(|until| (until - now() + 59) / 60)
}

fn check_key(key: &str) -> anyhow::Result<()> {
    match blocked_minutes(key) {
        Some(m) => Err(anyhow!("对方网站限制了访问（请求太频繁），{m} 分钟后再试")),
        None => Ok(()),
    }
}

/// 这个网站还在限流期内就不发请求
fn check_blocked(url: &str) -> anyhow::Result<()> {
    check_key(host_of(url))
}

/// 444、429、403 是被限流了：记下来，限流期内不再请求
fn check_status(url: &str, status: reqwest::StatusCode) -> anyhow::Result<()> {
    if matches!(status.as_u16(), 403 | 429 | 444) {
        block(host_of(url));
        return Err(anyhow!("对方网站限制了访问（请求太频繁），{} 分钟后再试", BLOCK_SECS / 60));
    }
    if !status.is_success() {
        return Err(anyhow!("{url} 返回 {status}"));
    }
    Ok(())
}

async fn get(url: &str, ua: &str, referer: Option<&str>) -> anyhow::Result<reqwest::Response> {
    check_blocked(url)?;
    let mut req = client().get(url).header("User-Agent", ua).header("Accept-Language", "zh-CN,zh;q=0.9");
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    let resp = req.send().await.with_context(|| format!("连不上 {url}"))?;
    check_status(url, resp.status())?;
    Ok(resp)
}

async fn get_text(url: &str, ua: &str, referer: Option<&str>) -> anyhow::Result<String> {
    Ok(get(url, ua, referer).await?.text().await?)
}

/// 从番茄书页的额度里拿 n 页，返回实际拿到的页数
fn take_page_budget(n: usize) -> usize {
    static B: OnceLock<std::sync::Mutex<(i64, usize)>> = OnceLock::new();
    let mut b = B.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let t = now();
    if t - b.0 >= FANQIE_PAGE_BUDGET.0 {
        *b = (t, 0);
    }
    let granted = n.min(FANQIE_PAGE_BUDGET.1.saturating_sub(b.1));
    b.1 += granted;
    granted
}

async fn get_json(url: &str, referer: Option<&str>) -> anyhow::Result<Value> {
    let text = get_text(url, UA, referer).await?;
    serde_json::from_str(&text).with_context(|| format!("{url} 返回的不是 JSON"))
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").trim().to_string()
}

/// 番茄的数字是字符串
fn num_of(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0)
}

fn items(v: &Value, source: &str, word: &str, heat: &str, tag: &str) -> Vec<HotItem> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["is_ad"].as_i64().unwrap_or(0) == 0 && !x["isTop"].as_bool().unwrap_or(false))
                .map(|x| HotItem { source: source.into(), word: str_of(&x[word]), heat: x[heat].as_i64().unwrap_or(0), tag: str_of(&x[tag]) })
                .filter(|h| !h.word.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_weibo(v: &Value) -> Vec<HotItem> {
    items(&v["data"]["realtime"], "微博", "word", "num", "label_name")
}

pub fn parse_douyin(v: &Value) -> Vec<HotItem> {
    items(&v["word_list"], "抖音", "word", "hot_value", "label_name")
}

pub fn parse_bili(v: &Value) -> Vec<HotItem> {
    items(&v["list"], "B站", "show_name", "score", "label_name")
}

/// 百度的列表有时多套一层
pub fn parse_baidu(v: &Value) -> Vec<HotItem> {
    let content = &v["data"]["cards"][0]["content"];
    let list = if content[0]["content"].is_array() { &content[0]["content"] } else { content };
    items(list, "百度", "word", "hotScore", "label_name")
}

/// 四个热搜榜一起抓，抓不到的记下来源；按敏感词去掉高风险的，同一个词只留一次
async fn fetch_hot(st: &AppState) -> (Vec<HotItem>, Vec<String>) {
    let (w, d, b, bd) = tokio::join!(
        get_json("https://weibo.com/ajax/side/hotSearch", Some("https://weibo.com/")),
        get_json("https://www.iesdouyin.com/web/api/v2/hotsearch/billboard/word/", None),
        get_json("https://s.search.bilibili.com/main/hotword", Some("https://www.bilibili.com/")),
        get_json("https://top.baidu.com/api/board?platform=wise&tab=realtime", None),
    );
    let mut all = Vec::new();
    let mut errors = Vec::new();
    for (name, r, parse) in [("微博", w, parse_weibo as fn(&Value) -> Vec<HotItem>), ("抖音", d, parse_douyin), ("B站", b, parse_bili), ("百度", bd, parse_baidu)] {
        match r {
            Ok(v) => {
                let got = parse(&v);
                if got.is_empty() {
                    errors.push(format!("{name}：没解析出内容"));
                }
                all.extend(got);
            }
            Err(e) => errors.push(format!("{name}：{e:#}")),
        }
    }
    let settings = st.db.get_settings().unwrap_or_default();
    let mut seen = Vec::new();
    all.retain(|h| {
        let risky = crate::sensitive::scan(&h.word, &settings.sensitive_words, &settings.sensitive_ignore).iter().any(|x| x.level == "high");
        let fresh = !seen.contains(&h.word);
        seen.push(h.word.clone());
        fresh && !risky
    });
    (all, errors)
}

fn json_from_script(html: &str) -> Option<Value> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?s)<script[^>]*id="vite-plugin-ssr_pageContext"[^>]*>(.*?)</script>"#).expect("正则"));
    serde_json::from_str(re.captures(html)?.get(1)?.as_str()).ok()
}

fn qidian_records(records: &Value) -> Vec<RankBook> {
    records
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, r)| {
            let desc = str_of(&r["desc"]);
            RankBook {
                rank: r["rankNum"].as_u64().map_or(i + 1, |n| n as usize),
                title: str_of(&r["bName"]),
                author: str_of(&r["bAuth"]),
                category: [str_of(&r["cat"]), str_of(&r["subCat"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("·"),
                words: str_of(&r["cnt"]),
                metric: str_of(&r["rankCnt"]),
                status: String::new(),
                tags: sell_tags(&desc),
                desc,
            }
        })
        .filter(|b| !b.title.is_empty())
        .collect()
}

/// 榜单网页里带的第一页
pub fn parse_qidian(html: &str) -> Vec<RankBook> {
    json_from_script(html).map(|ctx| qidian_records(&ctx["pageContext"]["pageProps"]["pageData"]["records"])).unwrap_or_default()
}

/// 翻页接口返回的一页：(书, 是不是最后一页)
pub fn parse_qidian_page(v: &Value) -> Option<(Vec<RankBook>, bool)> {
    (v["code"].as_i64() == Some(0)).then(|| (qidian_records(&v["data"]["records"]), v["data"]["isLast"].as_i64() == Some(1)))
}

/// 起点的翻页接口要带 _csrfToken：打开榜单页时它在 Set-Cookie 里发下来，请求时 cookie 和参数各带一份
fn csrf_token(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.trim().strip_prefix("_csrfToken="))
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .filter(|t| !t.is_empty())
}

/// 起点榜单前 QIDIAN_DEPTH 本：男频的榜单网页就是第一页；女频没有网页，全走翻页接口
async fn fetch_qidian(kind: &str, female: bool) -> anyhow::Result<Vec<RankBook>> {
    let endpoint = QIDIAN_RANKS.iter().find(|(k, ..)| *k == kind).map_or("yuepiaolist", |(_, e, _)| e);
    let page_url = format!("https://m.qidian.com/rank/{kind}/");
    check_blocked(&page_url)?;
    let resp = client().get(&page_url).header("User-Agent", MOBILE_UA).send().await.context("连不上起点")?;
    check_status(&page_url, resp.status())?;
    let token = csrf_token(resp.headers());
    let html = resp.text().await?;
    let mut books = if female { vec![] } else { parse_qidian(&html) };
    let gender = if female { "female" } else { "male" };
    let mut page = if books.is_empty() { 1 } else { 2 };
    while let Some(token) = token.as_deref().filter(|_| books.len() < QIDIAN_DEPTH) {
        let url = format!("https://m.qidian.com/webcommon/rank/{endpoint}?gender={gender}&pageNum={page}&catId=-1&_csrfToken={token}");
        let got: anyhow::Result<(Vec<RankBook>, bool)> = async {
            check_blocked(&url)?;
            let req = client().get(&url).header("User-Agent", MOBILE_UA).header("Referer", &page_url).header("Cookie", format!("_csrfToken={token}"));
            let resp = req.send().await?;
            check_status(&url, resp.status())?;
            let v: Value = resp.json().await?;
            parse_qidian_page(&v).ok_or_else(|| anyhow!("起点翻页接口返回 {}：{}", v["code"], str_of(&v["msg"])))
        }
        .await;
        match got {
            Ok((list, last)) => {
                let done = last || list.is_empty();
                books.extend(list);
                if done {
                    break;
                }
            }
            // 有第一页就先用着
            Err(e) if !books.is_empty() => {
                tracing::warn!("{e:#}");
                break;
            }
            Err(e) => return Err(e),
        }
        page += 1;
    }
    if books.is_empty() && token.is_none() {
        return Err(anyhow!("起点没给翻页用的校验令牌"));
    }
    let mut seen = HashSet::new();
    books.retain(|b| seen.insert((b.title.clone(), b.author.clone())));
    books.sort_by_key(|b| b.rank);
    books.truncate(QIDIAN_DEPTH);
    Ok(books)
}

// ---------- 番茄：加密字体 ----------

pub fn is_pua(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

#[derive(Default, Serialize, Deserialize)]
pub struct Glyphs {
    /// 字体文件名里的编号，换了字体对照表就作废
    pub font: String,
    pub map: BTreeMap<u32, char>,
    /// 已经下载这个字体和内置的比过字形（不管认出多少），不用再比
    #[serde(default)]
    pub compared: bool,
}

/// 对照加密文字和明文学字：去掉空白后逐字对齐，私用区字符记下对应的字；
/// whole 为 true 时要求两边一样长（书名、作者），否则只对齐到第一个对不上的普通字（简介开头）
pub fn learn(map: &mut BTreeMap<u32, char>, obf: &str, plain: &str, whole: bool) -> usize {
    let o: Vec<char> = obf.chars().filter(|c| !c.is_whitespace()).collect();
    let p: Vec<char> = plain.chars().filter(|c| !c.is_whitespace()).collect();
    if whole && o.len() != p.len() {
        return 0;
    }
    let mut n = 0;
    for (a, b) in o.iter().zip(p.iter()) {
        if is_pua(*a) {
            if !is_pua(*b) && map.insert(*a as u32, *b).is_none() {
                n += 1;
            }
        } else if a != b {
            break;
        }
    }
    n
}

/// 认得的私用区字符换成原字，不认得的换成 □
pub fn decode(map: &BTreeMap<u32, char>, s: &str) -> String {
    s.chars().map(|c| if is_pua(c) { map.get(&(c as u32)).copied().unwrap_or('□') } else { c }).collect()
}

/// 页面里 `window.__INITIAL_STATE__=` 后面的 JSON（按括号配对截出来，跳过字符串里的括号）
fn initial_state(html: &str) -> Option<Value> {
    let start = html.find("window.__INITIAL_STATE__")? + "window.__INITIAL_STATE__".len();
    let rest = html[start..].trim_start().strip_prefix('=')?.trim_start();
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, c) in rest.char_indices() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return serde_json::from_str(&rest[..=i].replace(":undefined", ":null")).ok();
                }
            }
            _ => {}
        }
    }
    None
}

pub struct FanqieRaw {
    pub id: String,
    pub title: String,
    pub author: String,
    pub desc: String,
    pub read: i64,
    pub words: i64,
    pub finished: bool,
}

/// 番茄榜单里的书（加密的）：榜单页的 book_list 和接口的 data.book_list 是同一种格式
pub fn fanqie_books(list: &Value) -> Vec<FanqieRaw> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|b| FanqieRaw {
            id: b["bookId"].as_str().map(String::from).unwrap_or_else(|| b["bookId"].to_string()),
            title: str_of(&b["bookName"]),
            author: str_of(&b["author"]),
            desc: str_of(&b["abstract"]),
            // 在读人数是 read_count；readCount 是另一个数，大多是 0
            read: num_of(&b["read_count"]),
            words: num_of(&b["wordNumber"]),
            finished: b["creationStatus"].as_i64() == Some(0) || b["creationStatus"].as_str() == Some("0"),
        })
        .collect()
}

/// 榜单页：书（加密的，只有前 10 本）、分类表、字体编号
pub fn parse_fanqie(html: &str) -> Option<(Vec<FanqieRaw>, Value, String)> {
    static FONT: OnceLock<Regex> = OnceLock::new();
    let font = FONT.get_or_init(|| Regex::new(r"awesome-font/c/([0-9a-z]+)\.woff2").expect("正则"));
    let state = initial_state(html)?;
    let rank = &state["rank"];
    rank["book_list"].as_array()?;
    let font_id = font.captures(html).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    Some((fanqie_books(&rank["book_list"]), rank["rankCategoryTypeList"].clone(), font_id))
}

/// 榜单页样式表里字体文件的完整地址
pub fn fanqie_font_url(html: &str) -> Option<String> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let re = URL.get_or_init(|| Regex::new(r#"(?:https?:)?//[^\s"'()]+/awesome-font/c/[0-9a-z]+\.woff2"#).expect("正则"));
    let url = re.find(html)?.as_str();
    Some(if url.starts_with("//") { format!("https:{url}") } else { url.to_string() })
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
}

/// 书页里的明文：书名（标题）、作者（关键词里的「作者小说书名」）、简介开头（描述）
pub fn parse_fanqie_page(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static META: OnceLock<Regex> = OnceLock::new();
    let title_re = TITLE.get_or_init(|| Regex::new(r"(?s)<title>(.*?)</title>").expect("正则"));
    let meta_re = META.get_or_init(|| Regex::new(r#"<meta name="(description|keywords)" content="([^"]*)""#).expect("正则"));
    let title = title_re.captures(html).and_then(|c| c.get(1)).and_then(|m| m.as_str().split("完整版在线免费阅读").next()).map(|s| unescape(s.trim())).filter(|s| !s.is_empty());
    let mut author = None;
    let mut desc = None;
    for c in meta_re.captures_iter(html) {
        let content = unescape(&c[2]);
        match &c[1] {
            "keywords" => {
                // 其中一条是「作者小说书名」；书名可能带逗号，不能先按逗号切开
                if let Some(end) = title.as_ref().and_then(|t| content.find(&format!("小说{t}"))) {
                    let before = &content[..end];
                    let start = before.char_indices().filter(|(_, c)| matches!(c, ',' | '，')).last().map_or(0, |(i, c)| i + c.len_utf8());
                    author = Some(before[start..].trim().to_string()).filter(|a| !a.is_empty());
                }
            }
            _ => {
                desc = content.split_once("精彩小说尽在番茄小说网。").map(|(_, d)| d.trim_end_matches('.').trim_end_matches('…').trim().to_string()).filter(|d| !d.is_empty());
            }
        }
    }
    (title, author, desc)
}

/// 存下来的对照表；内置的对照表是同一个字体时，用来补上还没学过的字
fn load_glyphs(db: &Db, font: &str) -> Glyphs {
    let saved: Glyphs = db.get_kv(GLYPH_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut g = if saved.font == font { saved } else { Glyphs { font: font.to_string(), ..Default::default() } };
    if let Some(seed) = seed_glyphs().filter(|s| s.font == font) {
        for (k, v) in &seed.map {
            g.map.entry(*k).or_insert(*v);
        }
    }
    g
}

/// 内置的番茄字体对照表（362 个字全认得）：番茄的书页限流很严，现学要很久才能认全；
/// 番茄换了字体就对不上了，这时拿它和对应的字体文件去认新字体（fanqie_font）
pub(crate) fn seed_glyphs() -> Option<&'static Glyphs> {
    static SEED: OnceLock<Option<Glyphs>> = OnceLock::new();
    SEED.get_or_init(|| serde_json::from_str(include_str!("fanqie_glyphs.json")).ok()).as_ref()
}

fn wan(n: i64, unit: &str) -> String {
    if n >= 10_000 { format!("{:.1}万{unit}", n as f64 / 10_000.0) } else { format!("{n}{unit}") }
}

/// 番茄换了字体、榜单里还有不认得的字，并且还没下载这个字体比过字形
fn wants_font(glyphs: &Glyphs, raws: &[FanqieRaw]) -> bool {
    !glyphs.compared
        && !glyphs.font.is_empty()
        && raws.iter().flat_map(|r| r.title.chars().chain(r.author.chars()).chain(r.desc.chars())).any(|c| is_pua(c) && !glyphs.map.contains_key(&(c as u32)))
}

/// 下载番茄的新字体和内置的比字形，认出来的字补进对照表；下载失败下次再试
async fn compare_font(glyphs: &mut Glyphs, url: &str, referer: &str) {
    let Some(seed) = seed_glyphs() else { return };
    let found = async {
        let bytes = get(url, UA, Some(referer)).await?.bytes().await?;
        tokio::task::spawn_blocking(move || crate::fanqie_font::recognize_font(&bytes, &seed.map)).await?
    };
    match found.await {
        Ok(found) => {
            let before = glyphs.map.len();
            for (k, v) in found {
                glyphs.map.entry(k).or_insert(v);
            }
            glyphs.compared = true;
            tracing::info!("番茄换了字体 {}，比字形认出 {} 个字", glyphs.font, glyphs.map.len() - before);
        }
        Err(e) => tracing::warn!("番茄新字体 {}：{e:#}", glyphs.font),
    }
}

/// 打开书页对照明文学字：每轮挑生字最多的几本同时打开，学到的字马上用来重新挑，直到认全或用完名额。
/// 书名、作者里的生字优先，其次是简介开头；简介后面的字书页上没有，不算
async fn learn_glyphs(map: &mut BTreeMap<u32, char>, raws: &[FanqieRaw]) {
    let unknown = |map: &BTreeMap<u32, char>, s: &str, limit: usize| {
        s.chars().filter(|c| !c.is_whitespace()).take(limit).filter(|c| is_pua(*c) && !map.contains_key(&(*c as u32))).count()
    };
    let mut tried: HashSet<&str> = HashSet::new();
    while tried.len() < FANQIE_LEARN_PAGES && check_blocked("https://fanqienovel.com/").is_ok() && check_key(FANQIE_PAGE_KEY).is_ok() {
        let known: &BTreeMap<u32, char> = map;
        let mut picks: Vec<(usize, usize, &FanqieRaw)> = raws
            .iter()
            .filter(|r| !tried.contains(r.id.as_str()))
            .map(|r| (unknown(known, &r.title, usize::MAX) + unknown(known, &r.author, usize::MAX), unknown(known, &r.desc, DESC_HEAD), r))
            .filter(|(name, head, _)| name + head > 0)
            .collect();
        if picks.is_empty() {
            break;
        }
        picks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let n = take_page_budget(FANQIE_LEARN_BATCH.min(FANQIE_LEARN_PAGES - tried.len()).min(picks.len()));
        if n == 0 {
            break;
        }
        let batch: Vec<&FanqieRaw> = picks.into_iter().take(n).map(|(.., r)| r).collect();
        if !tried.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tried.extend(batch.iter().map(|r| r.id.as_str()));
        let urls: Vec<String> = batch.iter().map(|r| format!("https://fanqienovel.com/page/{}", r.id)).collect();
        let pages = join_all(urls.iter().map(|u| get_text(u, UA, None))).await;
        for (r, page) in batch.iter().zip(pages) {
            let Ok(page) = page else { continue };
            // 书页被悄悄限流时返回 200 和空内容
            if page.trim().is_empty() {
                block(FANQIE_PAGE_KEY);
                break;
            }
            let (title, author, desc) = parse_fanqie_page(&page);
            if let Some(t) = title {
                learn(map, &r.title, &t, true);
            }
            if let Some(a) = author {
                learn(map, &r.author, &a, true);
            }
            if let Some(d) = desc {
                learn(map, &r.desc, &d, false);
            }
        }
    }
}

/// 番茄分类榜前 FANQIE_DEPTH 本：list 2 阅读榜、1 新书榜；gender 1 男频、0 女频。
/// 榜单页给字体编号和分类表（书只有前 10 本），书从接口一次取够；接口不通就用榜单页的 10 本
async fn fetch_fanqie(db: &Db, gender: u8, list: u8, category: &str) -> anyhow::Result<(Vec<RankBook>, Value, usize)> {
    let page_url = format!("https://fanqienovel.com/rank/{gender}_{list}_{category}");
    let html = get_text(&page_url, UA, None).await?;
    let (mut raws, cats, font) = parse_fanqie(&html).ok_or_else(|| anyhow!("番茄榜单页面格式变了，暂时拿不到"))?;
    let api = format!(
        "https://fanqienovel.com/api/rank/category/list?app_id=2503&rank_list_type=3&offset=0&limit={FANQIE_DEPTH}&category_id={category}&rank_version=&gender={gender}&rankMold={list}"
    );
    match get_json(&api, Some(&page_url)).await.map(|v| fanqie_books(&v["data"]["book_list"])) {
        Ok(more) if more.len() > raws.len() => raws = more,
        Ok(_) => tracing::warn!("番茄榜单接口没有返回更多的书"),
        Err(e) => tracing::warn!("番茄榜单接口：{e:#}"),
    }
    let mut glyphs = load_glyphs(db, &font);
    if let Some(url) = fanqie_font_url(&html).filter(|_| wants_font(&glyphs, &raws)) {
        compare_font(&mut glyphs, &url, &page_url).await;
    }
    learn_glyphs(&mut glyphs.map, &raws).await;
    if let Ok(s) = serde_json::to_string(&glyphs) {
        let _ = db.set_kv(GLYPH_KEY, &s);
    }
    let cat_name = fanqie_categories(&cats).into_iter().find(|(g, id, _)| *g == gender && id == category).map(|(_, _, n)| n).unwrap_or_default();
    let books: Vec<RankBook> = raws
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let desc = decode(&glyphs.map, &r.desc);
            RankBook {
                rank: i + 1,
                title: decode(&glyphs.map, &r.title),
                author: decode(&glyphs.map, &r.author),
                category: cat_name.clone(),
                words: wan(r.words, "字"),
                metric: if r.read > 0 { wan(r.read, "人在读") } else { String::new() },
                status: if r.finished { "完结".into() } else { "连载".into() },
                tags: sell_tags(&desc),
                desc,
            }
        })
        .collect();
    let undecoded = books.iter().map(|b| b.title.chars().chain(b.author.chars()).filter(|c| *c == '□').count()).sum();
    Ok((books, cats, undecoded))
}

/// 榜单页带的分类表；没有就用内置的
pub fn fanqie_categories(cats: &Value) -> Vec<(u8, String, String)> {
    let mut out = Vec::new();
    for (gender, key) in [(1u8, "male"), (0u8, "female")] {
        for c in cats[key].as_array().into_iter().flatten() {
            let (id, name) = (str_of(&c["id"]), str_of(&c["name"]));
            if !id.is_empty() && !name.is_empty() {
                out.push((gender, id, name));
            }
        }
    }
    if out.is_empty() {
        out = FANQIE_CATS.iter().map(|(g, id, n)| (*g, id.to_string(), n.to_string())).collect();
    }
    out
}

/// 按题材挑番茄分类：先定男频女频，再按名称里共有的字打分
pub fn match_fanqie(genre: &str, cats: &[(u8, String, String)]) -> Vec<(u8, String, String)> {
    let gender = if is_female(genre) { 0 } else { 1 };
    let mut scored: Vec<(usize, usize, &(u8, String, String))> = cats
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0 == gender)
        .map(|(i, c)| (genre.chars().filter(|ch| !"脑洞衍生".contains(*ch) && c.2.contains(*ch)).count(), i, c))
        .filter(|(s, _, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let picked: Vec<(u8, String, String)> = scored.into_iter().take(2).map(|(_, _, c)| c.clone()).collect();
    if picked.is_empty() {
        cats.iter().filter(|c| c.0 == gender).take(1).cloned().collect()
    } else {
        picked
    }
}

// ---------- 缓存 ----------

#[derive(Serialize, Deserialize)]
struct Cached<T> {
    at: i64,
    data: T,
}

fn cache_get<T: for<'de> Deserialize<'de>>(db: &Db, key: &str) -> Option<Cached<T>> {
    db.get_kv(&format!("trend:{key}")).ok().flatten().and_then(|s| serde_json::from_str(&s).ok())
}

fn cache_put<T: Serialize>(db: &Db, key: &str, data: &T) -> i64 {
    let at = now();
    if let Ok(s) = serde_json::to_string(&Cached { at, data }) {
        let _ = db.set_kv(&format!("trend:{key}"), &s);
    }
    at
}

async fn hot_cached(st: &AppState, refresh: bool) -> (Vec<HotItem>, i64, Vec<String>) {
    let old: Option<Cached<Vec<HotItem>>> = cache_get(&st.db, "hot");
    if let Some(c) = old.as_ref().filter(|c| !refresh && now() - c.at < CACHE_SECS) {
        return (c.data.clone(), c.at, vec![]);
    }
    let (items, errors) = fetch_hot(st).await;
    if items.is_empty() {
        if let Some(c) = old {
            return (c.data, c.at, errors);
        }
        return (vec![], 0, errors);
    }
    let at = cache_put(&st.db, "hot", &items);
    (items, at, errors)
}

async fn qidian_cached(db: &Db, kind: &str, female: bool, refresh: bool) -> Result<(Vec<RankBook>, i64), AppError> {
    // 键里带上深度：改了抓多少本，旧缓存自然作废
    let key = format!("qidian:{}:{kind}:{QIDIAN_DEPTH}", if female { "f" } else { "m" });
    let old: Option<Cached<Vec<RankBook>>> = cache_get(db, &key);
    if let Some(c) = old.as_ref().filter(|c| !refresh && now() - c.at < CACHE_SECS) {
        return Ok((c.data.clone(), c.at));
    }
    match fetch_qidian(kind, female).await {
        Ok(books) if !books.is_empty() => {
            let at = cache_put(db, &key, &books);
            Ok((books, at))
        }
        other => match old {
            Some(c) => Ok((c.data, c.at)),
            None => Err(bad_request(match other {
                Err(e) => format!("起点榜单暂时拿不到：{e:#}"),
                Ok(_) => "起点榜单页面格式变了，暂时拿不到".into(),
            })),
        },
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct FanqieList {
    books: Vec<RankBook>,
    categories: Value,
    undecoded: usize,
}

async fn fanqie_cached(db: &Db, gender: u8, list: u8, category: &str, refresh: bool) -> Result<(FanqieList, i64), AppError> {
    let key = format!("fanqie:{gender}_{list}_{category}:{FANQIE_DEPTH}");
    let old: Option<Cached<FanqieList>> = cache_get(db, &key);
    let ttl = |c: &Cached<FanqieList>| if c.data.undecoded > 0 { UNDECODED_CACHE_SECS } else { CACHE_SECS };
    if let Some(c) = old.as_ref().filter(|c| !refresh && now() - c.at < ttl(c)) {
        return Ok((c.data.clone(), c.at));
    }
    match fetch_fanqie(db, gender, list, category).await {
        Ok((books, categories, undecoded)) if !books.is_empty() => {
            let data = FanqieList { books, categories, undecoded };
            let at = cache_put(db, &key, &data);
            Ok((data, at))
        }
        other => match old {
            Some(c) => Ok((c.data, c.at)),
            None => Err(bad_request(match other {
                Err(e) => format!("番茄榜单暂时拿不到：{e:#}"),
                Ok(_) => "番茄这个分类暂时没有数据".into(),
            })),
        },
    }
}

/// 简介开头的卖点标签：【无系统+爽文】、『传统玄幻』、（无系统，亦正亦邪），以及用 + 或 & 连起来的「父子&重生&团宠」。
/// 从头读到正文开始为止，正文里的【洪荒】【叮！系统提示】不算；宣传语、太长的句子和没认出来的字也不算
pub fn sell_tags(desc: &str) -> Vec<String> {
    // 整段都是宣传、说明的括号
    const PROMO: &[&str] = &[
        "实体书", "出版", "版权", "影视", "短剧", "漫剧", "动画", "有声", "广播剧", "可看", "购买", "预售", "发售", "上架", "原名", "书名", "改编",
        "同名", "联系", "群号", "读者群", "QQ", "微信", "公众号", "评分", "开分", "排雷", "避雷", "预警", "注意", "简介", "文案",
    ];
    // 混在标签里的宣传词，只去掉这一项
    const NOISE: &[&str] = &[
        "新书", "完结", "完本", "番外", "月票", "推荐", "强推", "必看", "神作", "好评", "更新", "爆更", "日更", "万字", "签约", "已售", "全网", "首发", "收藏",
        "追读", "读者", "放心",
    ];
    let chars: Vec<char> = desc.chars().take(200).collect();
    let mut segments: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let close = match chars[i] {
            '【' => Some('】'),
            '『' => Some('』'),
            '〖' => Some('〗'),
            '（' => Some('）'),
            '(' => Some(')'),
            _ => None,
        };
        if let Some(end) = close.and_then(|c| chars[i + 1..].iter().take(80).position(|x| *x == c)) {
            segments.push(chars[i + 1..i + 1 + end].iter().collect());
            i += end + 2;
            continue;
        }
        // 括号外的一段，到下一个左括号或换行为止：用 + & 连起来的是标签，成句的就是正文开始了
        let start = i;
        while i < chars.len() && chars[i] != '\n' && !(i > start && "【『〖（(".contains(chars[i])) {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        if run.chars().filter(|c| "+＋&＆".contains(*c)).count() >= 2 {
            segments.push(run);
        } else if run.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count() >= 8 {
            break;
        }
        if chars.get(i) == Some(&'\n') {
            i += 1;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for seg in segments.iter().filter(|s| !PROMO.iter().any(|w| s.contains(w))) {
        // 「标签：系统+爽文」这种带说明的只要冒号后面
        let seg = seg.rsplit(['：', ':']).next().unwrap_or(seg);
        // □ 要在去标点之前排除，不然「无系□」会变成「无系」
        let pieces = seg.split(|c: char| "+＋&＆、，,|｜/／·".contains(c) || c.is_whitespace()).filter(|t| !t.contains('□') && !NOISE.iter().any(|w| t.contains(w)));
        for t in pieces.map(normalize_tag) {
            let n = t.chars().count();
            if (2..=6).contains(&n) && t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) && !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// 去掉两头的标点，全角字母数字换成半角，字母大写：无ｃｐ、无cp 都算「无CP」
fn normalize_tag(t: &str) -> String {
    t.trim_matches(|c: char| !c.is_alphanumeric())
        .chars()
        .map(|c| if ('\u{FF01}'..='\u{FF5E}').contains(&c) { char::from_u32(c as u32 - 0xFEE0).unwrap_or(c) } else { c })
        .collect::<String>()
        .to_uppercase()
}

/// 榜单的题材分布、书名高频词和卖点标签（标签数的是出现在几本书里）
pub fn rank_stats(books: &[RankBook]) -> Value {
    let mut cats: BTreeMap<String, usize> = BTreeMap::new();
    for b in books {
        let c = b.category.split('·').last().unwrap_or("").trim();
        if !c.is_empty() {
            *cats.entry(c.to_string()).or_default() += 1;
        }
    }
    const STOP: &str = "的了我你他她之是在一不这那与和";
    let mut words: HashMap<String, usize> = HashMap::new();
    for b in books {
        let chars: Vec<char> = b.title.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).collect();
        let mut seen = Vec::new();
        for w in chars.windows(2) {
            let s: String = w.iter().collect();
            if !w.iter().any(|c| STOP.contains(*c)) && !seen.contains(&s) {
                seen.push(s.clone());
                *words.entry(s).or_default() += 1;
            }
        }
    }
    let mut cats: Vec<(String, usize)> = cats.into_iter().collect();
    cats.sort_by(|a, b| b.1.cmp(&a.1));
    let mut words: Vec<(String, usize)> = words.into_iter().filter(|(_, n)| *n >= 2).collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    words.truncate(12);
    let mut tags: HashMap<&str, usize> = HashMap::new();
    for t in books.iter().flat_map(|b| &b.tags) {
        *tags.entry(t).or_default() += 1;
    }
    let mut tags: Vec<(&str, usize)> = tags.into_iter().filter(|(_, n)| *n >= 2).collect();
    tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    tags.truncate(16);
    json!({ "count": books.len(), "categories": cats, "keywords": words, "tags": tags })
}

// ---------- 接口 ----------

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct HotQuery {
    pub refresh: bool,
}

pub async fn hot(State(st): State<AppState>, Query(q): Query<HotQuery>) -> ApiResult<Value> {
    let (items, at, errors) = hot_cached(&st, q.refresh).await;
    Ok(Json(json!({ "items": items, "fetched_at": at, "errors": errors })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RankQuery {
    pub platform: String,
    pub kind: String,
    pub gender: Option<u8>,
    pub list: Option<u8>,
    pub category: String,
    pub refresh: bool,
}

pub async fn rank(State(st): State<AppState>, Query(q): Query<RankQuery>) -> ApiResult<Value> {
    if q.platform == "fanqie" {
        let gender = q.gender.unwrap_or(1).min(1);
        let list = q.list.filter(|l| *l == 1).unwrap_or(2);
        let category = if q.category.trim().is_empty() { "258".to_string() } else { q.category.trim().to_string() };
        let (data, at) = fanqie_cached(&st.db, gender, list, &category, q.refresh).await?;
        let cats: Vec<Value> = fanqie_categories(&data.categories).into_iter().map(|(g, id, name)| json!({ "gender": g, "id": id, "name": name })).collect();
        return Ok(Json(json!({
            "books": data.books, "fetched_at": at, "categories": cats, "undecoded": data.undecoded,
            "stats": rank_stats(&data.books), "limited": limited("https://fanqienovel.com/"), "paused": blocked_minutes(FANQIE_PAGE_KEY),
        })));
    }
    let kind = QIDIAN_RANKS.iter().find(|(k, ..)| *k == q.kind).map_or("yuepiao", |(k, ..)| k);
    let (books, at) = qidian_cached(&st.db, kind, q.gender == Some(0), q.refresh).await?;
    let kinds: Vec<Value> = QIDIAN_RANKS.iter().map(|(k, _, n)| json!({ "id": k, "name": n })).collect();
    Ok(Json(json!({ "books": books, "fetched_at": at, "kinds": kinds, "stats": rank_stats(&books), "limited": limited("https://m.qidian.com/") })))
}

/// 正在被限流时的说明：这时给的是上次抓到的榜单
fn limited(url: &str) -> Option<String> {
    check_blocked(url).err().map(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct BookRequest {
    pub book_id: i64,
    #[serde(default)]
    pub refresh: bool,
}

/// AI 从热搜里挑能写进这本书的梗
pub async fn memes(State(st): State<AppState>, Json(r): Json<BookRequest>) -> ApiResult<Value> {
    require_book(&st.db, r.book_id)?;
    let (items, at, _) = hot_cached(&st, r.refresh).await;
    if items.is_empty() {
        return Err(bad_request("热搜暂时拿不到，稍后再试"));
    }
    let hot = items.iter().take(150).map(|h| format!("{}｜{}", h.source, h.word)).collect::<Vec<_>>().join("\n");
    let req = AiRequest { task: "memes".into(), book_id: Some(r.book_id), selection: hot, ..Default::default() };
    let v = run_json(&st, &req).await?;
    let memes = v["memes"].as_array().cloned().unwrap_or_default();
    Ok(Json(json!({ "memes": memes, "fetched_at": at })))
}

fn rank_line(b: &RankBook) -> String {
    let mut parts = vec![format!("{}. 《{}》", b.rank, b.title)];
    for s in [&b.category, &b.words, &b.metric, &b.status] {
        if !s.is_empty() {
            parts.push(s.clone());
        }
    }
    let desc: String = b.desc.chars().filter(|c| !c.is_whitespace()).take(60).collect();
    if !desc.is_empty() {
        parts.push(desc);
    }
    parts.join("｜")
}

/// 一份榜单给 AI 看的内容：整份榜单的统计，加上前 take 本的明细
fn rank_block(label: &str, books: &[RankBook], take: usize) -> String {
    let stats = rank_stats(books);
    let mut s = format!("【{label}（共 {} 本）】", books.len());
    // 番茄一次只看一个分类，题材分布只有一项，不用列
    for (key, name, min) in [("categories", "题材分布", 2), ("keywords", "书名高频词", 1), ("tags", "卖点标签", 1)] {
        let pairs: Vec<String> = stats[key].as_array().into_iter().flatten().filter_map(|p| Some(format!("{} {}", p[0].as_str()?, p[1]))).collect();
        if pairs.len() >= min {
            s += &format!("\n{name}：{}", pairs.join("、"));
        }
    }
    s += &format!("\n前 {} 本：\n{}", books.len().min(take), books.iter().take(take).map(rank_line).collect::<Vec<_>>().join("\n"));
    s
}

/// 按平台和题材取几份榜单：(说明, 正文, 最早的抓取时间)
async fn preference_basis(st: &AppState, book: &Book, refresh: bool) -> Result<(String, String, i64), AppError> {
    let mut basis = Vec::new();
    let mut text = Vec::new();
    let mut oldest = i64::MAX;
    let female = is_female(&book.genre);
    if book.platform != "fanqie" {
        for (kind, name, take) in [("yuepiao", "月票榜", 20), ("readindex", "阅读指数榜", 15), ("newauthor", "新人榜", 10)] {
            if let Ok((books, at)) = qidian_cached(&st.db, kind, female, refresh).await {
                let label = format!("起点{}{name}", if female { "女频" } else { "" });
                basis.push(format!("{label}前{}本", books.len()));
                text.push(rank_block(&label, &books, take));
                oldest = oldest.min(at);
            }
        }
    }
    if book.platform != "qidian" {
        let cats = fanqie_categories(&Value::Null);
        for (gender, id, name) in match_fanqie(&book.genre, &cats) {
            for (list, list_name) in [(2u8, "阅读榜"), (1u8, "新书榜")] {
                if let Ok((data, at)) = fanqie_cached(&st.db, gender, list, &id, refresh).await {
                    let label = format!("番茄{name}{list_name}");
                    basis.push(format!("{label}前{}本", data.books.len()));
                    text.push(rank_block(&label, &data.books, 10));
                    oldest = oldest.min(at);
                }
            }
        }
    }
    if text.is_empty() {
        return Err(bad_request("榜单暂时拿不到，稍后再试"));
    }
    Ok((basis.join("、"), text.join("\n\n"), oldest))
}

/// 读者偏好预测：按榜单推断当下读者偏好，评估这本书的契合度
pub async fn preference(State(st): State<AppState>, Json(r): Json<BookRequest>) -> ApiResult<Value> {
    let book = require_book(&st.db, r.book_id)?;
    let (basis, ranks, at) = preference_basis(&st, &book, r.refresh).await?;
    let req = AiRequest { task: "preference".into(), book_id: Some(r.book_id), selection: ranks, instruction: basis.clone(), ..Default::default() };
    let mut v = run_json(&st, &req).await?;
    if let Some(s) = v["score"].as_f64().or_else(|| v["score"].as_str().and_then(|s| s.trim().parse().ok())) {
        v["score"] = json!(s.round().clamp(0.0, 100.0) as i64);
    }
    Ok(Json(json!({ "result": v, "basis": basis, "fetched_at": at })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hot_lists() {
        let w = json!({ "data": { "realtime": [{ "word": "城市不城市", "num": 900, "label_name": "热" }, { "word": "广告", "num": 1, "is_ad": 1 }] } });
        assert_eq!(parse_weibo(&w), vec![HotItem { source: "微博".into(), word: "城市不城市".into(), heat: 900, tag: "热".into() }]);
        assert_eq!(parse_douyin(&json!({ "word_list": [{ "word": "班味", "hot_value": 5 }] }))[0].word, "班味");
        assert_eq!(parse_bili(&json!({ "list": [{ "show_name": "抽象", "keyword": "x" }] }))[0].word, "抽象");
        let nested = json!({ "data": { "cards": [{ "content": [{ "content": [{ "word": "置顶", "isTop": true }, { "word": "情绪价值", "hotScore": 7 }] }] }] } });
        assert_eq!(parse_baidu(&nested).iter().map(|h| h.word.as_str()).collect::<Vec<_>>(), vec!["情绪价值"], "置顶的不要");
        assert_eq!(parse_baidu(&json!({ "data": { "cards": [{ "content": [{ "word": "平铺" }] }] } }))[0].word, "平铺");
    }

    #[test]
    fn parses_qidian_rank_page() {
        let ctx = json!({ "pageContext": { "pageProps": { "pageData": { "records": [{ "bName": "玄鉴仙族", "bAuth": "季越人", "cat": "仙侠", "subCat": "古典仙侠", "cnt": "637.28万字", "rankCnt": "6.05万月票", "rankNum": 1, "desc": "陆江仙熬夜猝死" }] } } } });
        let html = format!(r#"<html><script id="vite-plugin-ssr_pageContext" type="application/json">{ctx}</script></html>"#);
        let books = parse_qidian(&html);
        assert_eq!(books[0].title, "玄鉴仙族");
        assert_eq!((books[0].category.as_str(), books[0].metric.as_str(), books[0].rank), ("仙侠·古典仙侠", "6.05万月票", 1));
        assert!(parse_qidian("<html>出错了</html>").is_empty());
    }

    #[test]
    fn learns_fanqie_glyphs_from_plain_text() {
        let state = json!({ "rank": { "book_list": [{ "bookId": "7001", "bookName": "惹\u{e49c}枝", "author": "空留", "abstract": "时不虞是笑\u{e40c}出生的。那一刻{电闪雷鸣}", "read_count": "224535", "readCount": "0", "wordNumber": "1163967", "creationStatus": "0", "x": null }], "rankCategoryTypeList": { "male": [{ "id": "258", "name": "传统玄幻" }], "female": [{ "id": "1139", "name": "古风世情" }] } } });
        let html = format!(r#"<style>src:url(https://x/obj/awesome-font/c/dc027189e0ba4cd.woff2)</style><script>window.__INITIAL_STATE__={}</script>"#, state.to_string().replace("null", "undefined"));
        let (books, cats, font) = parse_fanqie(&html).unwrap_or_else(|| panic!("解析失败"));
        assert_eq!((books[0].id.as_str(), books[0].read, books[0].finished, font.as_str()), ("7001", 224535, true, "dc027189e0ba4cd"));
        assert_eq!(fanqie_categories(&cats).len(), 2);

        let page = r#"<title>惹金枝完整版在线免费阅读_惹金枝小说_番茄小说官网</title><meta name="description" content="番茄小说提供惹金枝完整版在线免费阅读，精彩小说尽在番茄小说网。时不虞是笑着出生的。那一刻电闪雷鸣..."><meta name="keywords" content="惹金枝,惹金枝免费阅读,空留小说惹金枝">"#;
        let (title, author, desc) = parse_fanqie_page(page);
        assert_eq!((title.as_deref(), author.as_deref()), (Some("惹金枝"), Some("空留")));
        let comma = r#"<title>灵墟，剑棺完整版在线免费阅读_灵墟，剑棺小说_番茄小说官网</title><meta name="keywords" content="灵墟，剑棺,灵墟，剑棺免费阅读,煮熟的来福鸽小说灵墟，剑棺,灵墟，剑棺全本免费下载">"#;
        assert_eq!(parse_fanqie_page(comma).1.as_deref(), Some("煮熟的来福鸽"), "书名带逗号也要找到作者");
        let mut map = BTreeMap::new();
        assert_eq!(learn(&mut map, &books[0].title, title.as_deref().unwrap_or(""), true), 1);
        assert_eq!(learn(&mut map, &books[0].desc, desc.as_deref().unwrap_or(""), false), 1, "简介对齐到不一样的地方为止");
        assert_eq!(decode(&map, &books[0].title), "惹金枝");
        assert_eq!(decode(&map, "\u{e40c}\u{e000}"), "着□");
        assert_eq!(learn(&mut map, "\u{e111}", "长了", true), 0, "书名长度对不上不学");
    }

    #[test]
    fn matches_fanqie_categories_and_counts_keywords() {
        let cats = fanqie_categories(&Value::Null);
        let names = |g: &str| match_fanqie(g, &cats).into_iter().map(|c| c.2).collect::<Vec<_>>();
        assert_eq!(names("玄幻")[0], "传统玄幻");
        assert_eq!(names("宫斗")[0], "宫斗宅斗");
        assert_eq!(names("古言")[0], "古言脑洞", "古言是女频");
        assert_eq!(names("都市")[0], "都市日常");
        assert_eq!(names("无关题材"), vec!["传统玄幻".to_string()], "对不上时给男频第一个");
        let books: Vec<RankBook> = ["开局签到系统", "我的系统能开局", "仙途"].iter().map(|t| RankBook { title: t.to_string(), category: "玄幻·东方玄幻".into(), ..Default::default() }).collect();
        let stats = rank_stats(&books);
        assert_eq!(stats["categories"][0], json!(["东方玄幻", 3]));
        let kw: Vec<&str> = stats["keywords"].as_array().into_iter().flatten().map(|k| k[0].as_str().unwrap_or("")).collect();
        assert!(kw.contains(&"开局") && kw.contains(&"系统") && !kw.iter().any(|k| k.contains('的')), "{kw:?}");
    }

    #[test]
    fn extracts_sell_tags() {
        assert_eq!(sell_tags("【无系统+无敌+爽文+无女主+玄幻】\n陈观楼获得长生后"), ["无系统", "无敌", "爽文", "无女主", "玄幻"]);
        assert_eq!(sell_tags("【红果短剧：《山海藏墟》可看！】【无系统】【天才剑道】【热血】他瞎，背棺"), ["无系统", "天才剑道", "热血"], "宣传语不算");
        assert_eq!(sell_tags("【实体书已全网发售】\n『传统玄幻』『非后宫』九州大地"), ["传统玄幻", "非后宫"]);
        assert_eq!(sell_tags("【子不类父则父厌之，子若类父则父疑之】父子&重生&救赎&团宠\n前世"), ["父子", "重生", "救赎", "团宠"], "长句不算，一行里用 & 连起来的算");
        assert_eq!(sell_tags("【穿越＋争霸＋无ｃｐ＋大女主】【标签：基建、种田】"), ["穿越", "争霸", "无CP", "大女主", "基建", "种田"], "全角和大小写统一，冒号前的说明不要");
        assert_eq!(sell_tags("【无系□+爽文】"), ["爽文"], "没认出来的字不算");
        assert!(sell_tags("陆江仙熬夜猝死，残魂附在一面镜子上。他说：「1+1=2」。").is_empty());
        assert!(sell_tags(&format!("{}【系统提示】", "长".repeat(220))).is_empty(), "只看开头");
        assert!(sell_tags("赵子轩穿越了，发现这个世界中有被称为【洪荒】的秘境。").is_empty(), "正文里的括号不算");
        assert_eq!(sell_tags("【文字游戏】【高武】\n陈玄无意之间接触了一款游戏，得到【神农枯木桩】"), ["文字游戏", "高武"], "正文开始后就不再找");
        assert_eq!(sell_tags("（评分刚出）【无敌、热血、快节奏、爆更、亿万读者强推！】十万年前"), ["无敌", "热血", "快节奏"], "整段宣传跳过，混在标签里的只去掉那一项");
        assert_eq!(sell_tags("（无系统，多女主，亦正亦邪）\n拥有混沌圣体的方凌"), ["无系统", "多女主", "亦正亦邪"]);
    }

    #[test]
    fn reads_qidian_pages_and_token() {
        let page = json!({ "code": 0, "data": { "isLast": 1, "records": [{ "bName": "我在永夜打造庇护所", "bAuth": "中世纪的兔子", "cat": "玄幻", "subCat": "异世大陆", "rankNum": 21, "rankCnt": "1.59万月票", "desc": "【基建+领主】陈凡穿越" }] } });
        let (books, last) = parse_qidian_page(&page).unwrap();
        assert_eq!((books[0].rank, books[0].category.as_str(), books[0].tags.clone(), last), (21, "玄幻·异世大陆", vec!["基建".to_string(), "领主".into()], true));
        assert!(parse_qidian_page(&json!({ "code": 1403, "msg": "CSRF 不合法" })).is_none());
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(reqwest::header::SET_COOKIE, "newstatisticUUID=1; path=/".parse().unwrap());
        headers.append(reqwest::header::SET_COOKIE, "_csrfToken=4d37b6a4; domain=.qidian.com; path=/".parse().unwrap());
        assert_eq!(csrf_token(&headers).as_deref(), Some("4d37b6a4"));
        assert_eq!(csrf_token(&reqwest::header::HeaderMap::new()), None);
    }

    #[test]
    fn backs_off_after_being_limited() {
        let url = "https://limit.example.com/page/1";
        assert!(check_blocked(url).is_ok());
        assert!(check_status(url, reqwest::StatusCode::from_u16(444).unwrap()).is_err());
        let e = check_blocked("https://limit.example.com/rank").unwrap_err().to_string();
        assert!(e.contains("限制了访问") && e.contains("30 分钟"), "{e}");
        assert!(check_blocked("https://other.example.com/").is_ok(), "只停被限流的网站");
        assert!(check_status("https://ok.example.com/", reqwest::StatusCode::NOT_FOUND).is_err());
        assert!(check_blocked("https://ok.example.com/").is_ok(), "404 不算限流");
        assert_eq!(take_page_budget(FANQIE_PAGE_BUDGET.1 + 5), FANQIE_PAGE_BUDGET.1, "一次最多给到额度");
        assert_eq!(take_page_budget(1), 0, "额度用完就不给了");
    }

    #[test]
    fn seeds_glyphs_for_the_same_font_only() {
        let seed = seed_glyphs().expect("内置对照表能解析");
        assert!(seed.map.len() >= 100 && seed.map.iter().all(|(k, v)| is_pua(char::from_u32(*k).unwrap_or(' ')) && !is_pua(*v)), "{}", seed.map.len());
        let db = Db::open_in_memory().unwrap();
        assert!(load_glyphs(&db, "换了的字体").map.is_empty(), "字体对不上不用内置的");
        db.set_kv(GLYPH_KEY, &json!({ "font": seed.font, "map": { "57344": "学" } }).to_string()).unwrap();
        let g = load_glyphs(&db, &seed.font);
        assert_eq!((g.map.get(&0xE000), g.map.len()), (Some(&'学'), seed.map.len() + 1), "存下来的和内置的合在一起");
    }

    #[test]
    fn finds_fanqie_font_and_compares_it_once() {
        let css = r#"@font-face{font-family:DNMrHsV173Pd4pgy;src:url(https://lf6-awef.bytetos.com/obj/awesome-font/c/dc027189e0ba4cd.woff2)format("woff2"),url(https://x/a.woff)}"#;
        assert_eq!(fanqie_font_url(css).as_deref(), Some("https://lf6-awef.bytetos.com/obj/awesome-font/c/dc027189e0ba4cd.woff2"));
        assert_eq!(fanqie_font_url("src:url(//cdn.example.com/obj/awesome-font/c/abc123.woff2)").as_deref(), Some("https://cdn.example.com/obj/awesome-font/c/abc123.woff2"), "省略协议的地址补上 https");
        assert_eq!(fanqie_font_url("<html>没有字体</html>"), None);

        let raws = fanqie_books(&json!([{ "bookId": "1", "bookName": "\u{e3e8}局", "author": "沙茶", "abstract": "" }]));
        let seed = seed_glyphs().expect("内置对照表能解析");
        let db = Db::open_in_memory().unwrap();
        assert!(!wants_font(&load_glyphs(&db, &seed.font), &raws), "内置的字体全认得，不用比");
        let mut g = load_glyphs(&db, "换了的字体");
        assert!(wants_font(&g, &raws), "换了字体、有不认得的字，要比");
        g.compared = true;
        assert!(!wants_font(&g, &raws), "比过了不再比");
        db.set_kv(GLYPH_KEY, &serde_json::to_string(&g).unwrap()).unwrap();
        assert!(load_glyphs(&db, "换了的字体").compared, "比过的记下来，重启也不再比");
        assert!(!load_glyphs(&db, "又换了一个").compared, "再换字体要重新比");
    }

    #[test]
    fn reads_fanqie_api_books() {
        let v = json!({ "code": 0, "data": { "book_list": [{ "bookId": "7202777894500699188", "bookName": "\u{e4ff}局", "author": "沙茶", "abstract": "（无系统）", "read_count": "218510", "wordNumber": "5092500", "creationStatus": "1" }] } });
        let books = fanqie_books(&v["data"]["book_list"]);
        assert_eq!((books.len(), books[0].read, books[0].words, books[0].finished), (1, 218510, 5092500, false));
        assert!(fanqie_books(&Value::Null).is_empty());
    }

    #[test]
    fn counts_tags_and_builds_rank_block() {
        let book = |title: &str, tags: &[&str]| RankBook { rank: 1, title: title.into(), category: "传统玄幻".into(), tags: tags.iter().map(|t| t.to_string()).collect(), ..Default::default() };
        let books = [book("开局系统", &["无系统", "爽文"]), book("开局长生", &["无系统"]), book("仙途", &["种田"])];
        let stats = rank_stats(&books);
        assert_eq!((stats["tags"].clone(), stats["count"].clone()), (json!([["无系统", 2]]), json!(3)), "只出现在一本书里的标签不列");
        let s = rank_block("番茄传统玄幻阅读榜", &books, 1);
        assert!(s.starts_with("【番茄传统玄幻阅读榜（共 3 本）】"), "{s}");
        assert!(s.contains("书名高频词：开局 2") && s.contains("卖点标签：无系统 2") && !s.contains("题材分布"), "只有一个分类时不列题材分布：{s}");
        assert_eq!(s.matches('《').count(), 1, "明细只列前 take 本");
    }
}
