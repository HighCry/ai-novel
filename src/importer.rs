//! 导入：TXT（自动识别 UTF-8/GBK/UTF-16）、DOCX、EPUB，以及酒馆（SillyTavern）角色卡和世界书。

use crate::models::Entry;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;

pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(bytes).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

pub fn decode_base64(data: &str) -> Result<Vec<u8>> {
    let payload = match data.find(";base64,") {
        Some(i) => &data[i + 8..],
        None => data,
    };
    base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .map_err(|e| anyhow!("文件数据不是合法的 base64：{e}"))
}

fn unescape_entities(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    re.replace_all(s, |c: &regex::Captures| {
        let e = &c[1];
        let ch = if let Some(hex) = e.strip_prefix("#x") {
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        } else if let Some(dec) = e.strip_prefix('#') {
            dec.parse().ok().and_then(char::from_u32)
        } else {
            match e {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                "hellip" => Some('…'),
                "mdash" => Some('—'),
                _ => None,
            }
        };
        ch.map(|c| c.to_string()).unwrap_or_else(|| c[0].to_string())
    })
    .into_owned()
}

fn normalize_lines(s: &str) -> String {
    s.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

pub fn html_to_text(html: &str) -> String {
    static DROP: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let drop = DROP.get_or_init(|| Regex::new(r"(?is)<head\b.*?</head>|<script\b.*?</script>|<style\b.*?</style>").unwrap());
    let block = BLOCK.get_or_init(|| Regex::new(r"(?i)<(?:/p|br\s*/?|/div|/h[1-6]|/li|/tr|/section|/blockquote)\s*>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let s = drop.replace_all(html, "");
    let s = block.replace_all(&s, "\n");
    let s = tag.replace_all(&s, "");
    normalize_lines(&unescape_entities(&s))
}

fn read_zip_string<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<String> {
    let mut file = zip.by_name(name).with_context(|| format!("压缩包里缺少 {name}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(decode_text(&buf))
}

pub fn docx_to_text(bytes: &[u8]) -> Result<String> {
    static BR: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("不是有效的 docx 文件")?;
    let xml = read_zip_string(&mut zip, "word/document.xml")?;
    let br = BR.get_or_init(|| Regex::new(r"</w:p>|<w:br\s*/>|<w:cr\s*/>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"<[^>]*>").unwrap());
    let s = br.replace_all(&xml, "\n");
    let s = tag.replace_all(&s, "");
    Ok(normalize_lines(&unescape_entities(&s)))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?i)\b{name}\s*=\s*["']([^"']*)["']"#)).ok()?;
    re.captures(tag).map(|c| c[1].to_string())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve_path(base: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub fn epub_to_text(bytes: &[u8]) -> Result<String> {
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static ITEMREF: OnceLock<Regex> = OnceLock::new();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("不是有效的 epub 文件")?;
    let container = read_zip_string(&mut zip, "META-INF/container.xml")?;
    let opf_path = attr(&container, "full-path").ok_or_else(|| anyhow!("epub 缺少 content.opf 路径"))?;
    let opf = read_zip_string(&mut zip, &opf_path)?;
    let base = opf_path.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default();

    let item_re = ITEM.get_or_init(|| Regex::new(r"(?is)<item\b[^>]*>").unwrap());
    let manifest: HashMap<String, String> = item_re
        .find_iter(&opf)
        .filter_map(|m| Some((attr(m.as_str(), "id")?, attr(m.as_str(), "href")?)))
        .collect();
    let itemref_re = ITEMREF.get_or_init(|| Regex::new(r"(?is)<itemref\b[^>]*>").unwrap());
    let mut out = String::new();
    for m in itemref_re.find_iter(&opf) {
        let Some(href) = attr(m.as_str(), "idref").and_then(|id| manifest.get(&id).cloned()) else { continue };
        if let Ok(html) = read_zip_string(&mut zip, &resolve_path(&base, &href)) {
            out.push_str(&html_to_text(&html));
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        bail!("epub 里没有读到正文");
    }
    Ok(out)
}

/// 读取 PNG 的文本块（酒馆角色卡把 base64 JSON 存在 chara / ccv3 里）。
fn png_text_chunks(bytes: &[u8]) -> HashMap<String, String> {
    let mut found = HashMap::new();
    let mut pos = 8;
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let end = start + len;
        if end + 4 > bytes.len() {
            break;
        }
        let data = &bytes[start..end];
        if kind == b"tEXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                let key: String = data[..nul].iter().map(|&b| b as char).collect();
                let val: String = data[nul + 1..].iter().map(|&b| b as char).collect();
                found.insert(key, val);
            }
        } else if kind == b"iTXt" {
            if let Some(nul) = data.iter().position(|&b| b == 0) {
                let key = String::from_utf8_lossy(&data[..nul]).into_owned();
                let rest = &data[nul + 1..];
                if rest.len() > 2 && rest[0] == 0 {
                    let rest = &rest[2..];
                    let mut parts = rest.splitn(3, |&b| b == 0);
                    let (_lang, _translated) = (parts.next(), parts.next());
                    if let Some(text) = parts.next() {
                        found.insert(key, String::from_utf8_lossy(text).into_owned());
                    }
                }
            }
        } else if kind == b"IEND" {
            break;
        }
        pos = end + 4;
    }
    found
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str())).unwrap_or("").trim()
}

fn lorebook_entries(book: &Value) -> Vec<Entry> {
    let list: Vec<&Value> = match &book["entries"] {
        Value::Array(a) => a.iter().collect(),
        Value::Object(m) => m.values().collect(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for e in list {
        let content = str_field(e, &["content"]);
        let disabled = e["disable"].as_bool().unwrap_or(false) || e["enabled"].as_bool() == Some(false);
        if content.is_empty() || disabled {
            continue;
        }
        let keys: Vec<String> = ["key", "keys"]
            .iter()
            .filter_map(|k| e.get(*k).and_then(|x| x.as_array()))
            .flatten()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        let comment = str_field(e, &["comment", "name"]);
        let name = if !comment.is_empty() { comment.to_string() } else { keys.first().cloned().unwrap_or_else(|| "未命名设定".into()) };
        out.push(Entry {
            kind: "concept".into(),
            aliases: keys.into_iter().filter(|k| *k != name).collect::<Vec<_>>().join("、"),
            name,
            description: content.to_string(),
            always_include: e["constant"].as_bool().unwrap_or(false),
            ..Default::default()
        });
    }
    out
}

/// 把酒馆角色卡（V1/V2/V3）或世界书 JSON 转成设定条目。
pub fn entries_from_json(v: &Value) -> Result<Vec<Entry>> {
    let data = if v.get("spec").is_some() && v.get("data").is_some() { &v["data"] } else { v };
    let mut out = Vec::new();
    let name = str_field(data, &["name"]);
    let is_card = !name.is_empty() && ["description", "personality", "first_mes", "scenario"].iter().any(|k| data.get(*k).is_some());
    if is_card {
        let mut desc = Vec::new();
        for (label, key) in [("", "description"), ("性格：", "personality"), ("背景场景：", "scenario"), ("开场白：", "first_mes"), ("对话示例：", "mes_example")] {
            let text = str_field(data, &[key]);
            if !text.is_empty() {
                desc.push(format!("{label}{text}"));
            }
        }
        out.push(Entry {
            kind: "character".into(),
            name: name.to_string(),
            aliases: str_field(data, &["nickname"]).to_string(),
            description: desc.join("\n"),
            ..Default::default()
        });
        if data.get("character_book").is_some() {
            out.extend(lorebook_entries(&data["character_book"]));
        }
    } else if v.get("entries").is_some() {
        out.extend(lorebook_entries(v));
    }
    if out.is_empty() {
        bail!("无法识别的文件：需要酒馆角色卡（PNG/JSON）或世界书 JSON");
    }
    Ok(out)
}

pub fn parse_tavern_file(bytes: &[u8]) -> Result<Vec<Entry>> {
    let json_text = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let chunks = png_text_chunks(bytes);
        let raw = ["ccv3", "chara"]
            .iter()
            .find_map(|k| chunks.get(*k))
            .ok_or_else(|| anyhow!("这张 PNG 里没有角色卡数据"))?;
        let decoded = base64::engine::general_purpose::STANDARD.decode(raw.trim()).context("角色卡数据解码失败")?;
        String::from_utf8_lossy(&decoded).into_owned()
    } else {
        decode_text(bytes)
    };
    let v: Value = serde_json::from_str(json_text.trim()).context("角色卡/世界书不是合法的 JSON")?;
    entries_from_json(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn decodes_gbk() {
        let (gbk, _, _) = encoding_rs::GBK.encode("第一章 少年");
        assert_eq!(decode_text(&gbk), "第一章 少年");
        assert_eq!(decode_text("\u{feff}你好".as_bytes()), "你好");
    }

    #[test]
    fn html_and_paths() {
        assert_eq!(html_to_text("<html><head><title>x</title></head><body><h1>第一章</h1><p>林凡&amp;苏雨</p><p>&#20320;好</p></body></html>"), "第一章\n林凡&苏雨\n你好");
        assert_eq!(resolve_path("OEBPS", "Text/ch%201.xhtml"), "OEBPS/Text/ch 1.xhtml");
        assert_eq!(resolve_path("OEBPS/Text", "../Images/a.png"), "OEBPS/Images/a.png");
    }

    #[test]
    fn reads_docx() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(r#"<w:document><w:body><w:p><w:r><w:t>第一章 开端</w:t></w:r></w:p><w:p><w:r><w:t>他来了。</w:t></w:r></w:p></w:body></w:document>"#.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert_eq!(docx_to_text(&bytes).unwrap(), "第一章 开端\n他来了。");
    }

    fn png_with_text(key: &str, value: &str) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut data = key.as_bytes().to_vec();
        data.push(0);
        data.extend_from_slice(value.as_bytes());
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(b"tEXt");
        png.extend_from_slice(&data);
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0, 0, 0, 0]);
        png
    }

    #[test]
    fn tavern_card_png() {
        let card = r#"{"spec":"chara_card_v2","data":{"name":"苏雨","description":"青云宗大师姐","personality":"外冷内热","first_mes":"你来了。","character_book":{"entries":[{"keys":["青云宗"],"content":"东域第一宗门","constant":true}]}}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(card);
        let entries = parse_tavern_file(&png_with_text("chara", &b64)).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "苏雨");
        assert!(entries[0].description.contains("性格：外冷内热"));
        assert_eq!(entries[1].name, "青云宗");
        assert!(entries[1].always_include);
    }

    #[test]
    fn tavern_world_info() {
        let wi = r#"{"entries":{"0":{"key":["灵气","灵力"],"comment":"灵气体系","content":"天地灵气分九品","constant":false},"1":{"key":["x"],"content":"","constant":false}}}"#;
        let entries = parse_tavern_file(wi.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "灵气体系");
        assert_eq!(entries[0].aliases, "灵气、灵力");
    }
}
