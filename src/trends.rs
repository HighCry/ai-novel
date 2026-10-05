//! 热点和榜单：微博、抖音、B站、百度的热搜，起点手机版榜单，番茄分类榜；AI 从热搜里挑能写进网文的梗，
//! 按榜单推断读者偏好。只在作者打开时抓，缓存 6 小时。
//! 番茄榜单里的书名、作者、简介是加密字体（私用区字符），书页的标题、关键词和描述是明文：
//! 对照着学会每个私用区字符对应的字，学到的对照表存起来，字体换了就重新学。

use crate::ai::{run_json, AiRequest};
use crate::api::{bad_request, require_book, ApiResult, AppError};
use crate::db::{now, Db};
use crate::models::Book;
use crate::state::AppState;
use anyhow::{anyhow, Context};
use axum::extract::{Query, State};
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::time::Duration;

const CACHE_SECS: i64 = 6 * 3600;
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0 Safari/537.36";
const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
/// 每次刷新番茄榜单最多打开几本书的书页来学字；一页榜单 10 本，第一次打开就能认全
const FANQIE_LEARN_PAGES: usize = 10;
const GLYPH_KEY: &str = "fanqie_glyphs";

pub const QIDIAN_RANKS: &[(&str, &str)] = &[("yuepiao", "月票榜"), ("hotsales", "畅销榜"), ("readindex", "阅读指数榜"), ("newfans", "书友榜"), ("newauthor", "新人榜")];

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
}

// ---------- 抓取 ----------

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    // 国内站点直连，不走系统代理
    C.get_or_init(|| reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(12)).build().expect("HTTP 客户端"))
}

async fn get_text(url: &str, ua: &str, referer: Option<&str>) -> anyhow::Result<String> {
    let mut req = client().get(url).header("User-Agent", ua).header("Accept-Language", "zh-CN,zh;q=0.9");
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    let resp = req.send().await.with_context(|| format!("连不上 {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("{url} 返回 {status}"));
    }
    Ok(resp.text().await?)
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

pub fn parse_qidian(html: &str) -> Vec<RankBook> {
    let Some(ctx) = json_from_script(html) else { return vec![] };
    ctx["pageContext"]["pageProps"]["pageData"]["records"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, r)| {
                    let cat = [str_of(&r["cat"]), str_of(&r["subCat"])].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("·");
                    RankBook {
                        rank: r["rankNum"].as_u64().map_or(i + 1, |n| n as usize),
                        title: str_of(&r["bName"]),
                        author: str_of(&r["bAuth"]),
                        category: cat,
                        words: str_of(&r["cnt"]),
                        metric: str_of(&r["rankCnt"]),
                        status: String::new(),
                        desc: str_of(&r["desc"]),
                    }
                })
                .filter(|b| !b.title.is_empty())
                .collect()
        })
        .unwrap_or_default()
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

/// 榜单页：书（加密的）、分类表、字体编号
pub fn parse_fanqie(html: &str) -> Option<(Vec<FanqieRaw>, Value, String)> {
    static FONT: OnceLock<Regex> = OnceLock::new();
    let font = FONT.get_or_init(|| Regex::new(r"awesome-font/c/([0-9a-z]+)\.woff2").expect("正则"));
    let state = initial_state(html)?;
    let rank = &state["rank"];
    let books = rank["book_list"]
        .as_array()?
        .iter()
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
        .collect();
    let font_id = font.captures(html).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    Some((books, rank["rankCategoryTypeList"].clone(), font_id))
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

fn load_glyphs(db: &Db, font: &str) -> Glyphs {
    let g: Glyphs = db.get_kv(GLYPH_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if g.font == font { g } else { Glyphs { font: font.to_string(), map: BTreeMap::new() } }
}

fn wan(n: i64, unit: &str) -> String {
    if n >= 10_000 { format!("{:.1}万{unit}", n as f64 / 10_000.0) } else { format!("{n}{unit}") }
}

/// 番茄分类榜：list 2 阅读榜、1 新书榜；gender 1 男频、0 女频
async fn fetch_fanqie(db: &Db, gender: u8, list: u8, category: &str) -> anyhow::Result<(Vec<RankBook>, Value, usize)> {
    let html = get_text(&format!("https://fanqienovel.com/rank/{gender}_{list}_{category}"), UA, None).await?;
    let (raws, cats, font) = parse_fanqie(&html).ok_or_else(|| anyhow!("番茄榜单页面格式变了，暂时拿不到"))?;
    let mut glyphs = load_glyphs(db, &font);
    let unknown = |map: &BTreeMap<u32, char>, s: &str| s.chars().any(|c| is_pua(c) && !map.contains_key(&(c as u32)));
    // 简介只有开头对得上书页，后面的字学不到；先轮书名、作者里有生字的书，不然名额总被前几本占掉
    let mut order: Vec<&FanqieRaw> = raws.iter().collect();
    order.sort_by_key(|r| !(unknown(&glyphs.map, &r.title) || unknown(&glyphs.map, &r.author)));
    let mut opened = 0;
    for r in order {
        if opened >= FANQIE_LEARN_PAGES {
            break;
        }
        if ![&r.title, &r.author, &r.desc].iter().any(|s| unknown(&glyphs.map, s)) {
            continue;
        }
        opened += 1;
        let Ok(page) = get_text(&format!("https://fanqienovel.com/page/{}", r.id), UA, None).await else { continue };
        let (title, author, desc) = parse_fanqie_page(&page);
        if let Some(t) = title {
            learn(&mut glyphs.map, &r.title, &t, true);
        }
        if let Some(a) = author {
            learn(&mut glyphs.map, &r.author, &a, true);
        }
        if let Some(d) = desc {
            learn(&mut glyphs.map, &r.desc, &d, false);
        }
    }
    if let Ok(s) = serde_json::to_string(&glyphs) {
        let _ = db.set_kv(GLYPH_KEY, &s);
    }
    let cat_name = fanqie_categories(&cats).into_iter().find(|(g, id, _)| *g == gender && id == category).map(|(_, _, n)| n).unwrap_or_default();
    let books: Vec<RankBook> = raws
        .iter()
        .enumerate()
        .map(|(i, r)| RankBook {
            rank: i + 1,
            title: decode(&glyphs.map, &r.title),
            author: decode(&glyphs.map, &r.author),
            category: cat_name.clone(),
            words: wan(r.words, "字"),
            metric: if r.read > 0 { wan(r.read, "人在读") } else { String::new() },
            status: if r.finished { "完结".into() } else { "连载".into() },
            desc: decode(&glyphs.map, &r.desc),
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
    let female = FEMALE_HINTS.iter().any(|h| genre.contains(h));
    let gender = if female { 0 } else { 1 };
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

async fn qidian_cached(db: &Db, kind: &str, refresh: bool) -> Result<(Vec<RankBook>, i64), AppError> {
    let key = format!("qidian:{kind}");
    let old: Option<Cached<Vec<RankBook>>> = cache_get(db, &key);
    if let Some(c) = old.as_ref().filter(|c| !refresh && now() - c.at < CACHE_SECS) {
        return Ok((c.data.clone(), c.at));
    }
    match get_text(&format!("https://m.qidian.com/rank/{kind}/"), MOBILE_UA, None).await.map(|h| parse_qidian(&h)) {
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
    let key = format!("fanqie:{gender}_{list}_{category}");
    let old: Option<Cached<FanqieList>> = cache_get(db, &key);
    // 还有没认出来的字时允许提前刷新，多学几本
    if let Some(c) = old.as_ref().filter(|c| !refresh && now() - c.at < CACHE_SECS) {
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

/// 榜单的题材分布和书名高频词
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
    json!({ "categories": cats, "keywords": words })
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
        return Ok(Json(json!({ "books": data.books, "fetched_at": at, "categories": cats, "undecoded": data.undecoded, "stats": rank_stats(&data.books) })));
    }
    let kind = QIDIAN_RANKS.iter().find(|(k, _)| *k == q.kind).map_or("yuepiao", |(k, _)| k);
    let (books, at) = qidian_cached(&st.db, kind, q.refresh).await?;
    let kinds: Vec<Value> = QIDIAN_RANKS.iter().map(|(k, n)| json!({ "id": k, "name": n })).collect();
    Ok(Json(json!({ "books": books, "fetched_at": at, "kinds": kinds, "stats": rank_stats(&books) })))
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

/// 按平台和题材取几份榜单：(说明, 正文, 最早的抓取时间)
async fn preference_basis(st: &AppState, book: &Book, refresh: bool) -> Result<(String, String, i64), AppError> {
    let mut basis = Vec::new();
    let mut text = Vec::new();
    let mut oldest = i64::MAX;
    if book.platform != "fanqie" {
        for (kind, name, take) in [("yuepiao", "起点月票榜", 20), ("readindex", "起点阅读指数榜", 15), ("newauthor", "起点新人榜", 10)] {
            if let Ok((books, at)) = qidian_cached(&st.db, kind, refresh).await {
                basis.push(format!("{name}前{}", books.len().min(take)));
                text.push(format!("【{name}】\n{}", books.iter().take(take).map(rank_line).collect::<Vec<_>>().join("\n")));
                oldest = oldest.min(at);
            }
        }
    }
    if book.platform != "qidian" {
        let cats = fanqie_categories(&Value::Null);
        for (gender, id, name) in match_fanqie(&book.genre, &cats) {
            for (list, list_name) in [(2u8, "阅读榜"), (1u8, "新书榜")] {
                if let Ok((data, at)) = fanqie_cached(&st.db, gender, list, &id, refresh).await {
                    basis.push(format!("番茄{name}{list_name}前{}", data.books.len()));
                    text.push(format!("【番茄{name}{list_name}】\n{}", data.books.iter().map(rank_line).collect::<Vec<_>>().join("\n")));
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
}
