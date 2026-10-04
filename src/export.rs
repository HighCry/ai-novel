//! 导出：番茄/起点投稿用 TXT、Markdown、EPUB，以及投稿前的格式检查。

use crate::models::{Book, Chapter};
use crate::text::{cn_number, count_words, has_chapter_prefix, iso_datetime, is_special_title, paragraphs};
use anyhow::Result;
use serde::Serialize;
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

#[derive(Debug, Clone)]
pub struct ExportOpts {
    pub chinese_numerals: bool,
    pub indent: bool,
    pub blank_line: bool,
    /// 投稿格式自动规范标点（英文引号、半角标点、省略号）
    pub normalize: bool,
}

pub fn opts_for(format: &str) -> ExportOpts {
    match format {
        "qidian" => ExportOpts { chinese_numerals: true, indent: true, blank_line: true, normalize: true },
        "fanqie" => ExportOpts { chinese_numerals: false, indent: true, blank_line: true, normalize: true },
        _ => ExportOpts { chinese_numerals: false, indent: true, blank_line: false, normalize: false },
    }
}

/// 生成章节标题：普通章节加“第 N 章”，序章、番外等保持原样。
pub fn heading(number: Option<i64>, title: &str, chinese: bool) -> String {
    let title = title.trim();
    match number {
        Some(n) if !has_chapter_prefix(title) => {
            let num = if chinese { cn_number(n) } else { n.to_string() };
            if title.is_empty() {
                format!("第{num}章")
            } else {
                format!("第{num}章 {title}")
            }
        }
        _ => title.to_string(),
    }
}

pub fn body(content: &str, o: &ExportOpts) -> String {
    let sep = if o.blank_line { "\n\n" } else { "\n" };
    let normalized;
    let content = if o.normalize {
        normalized = crate::text::normalize_punct(content);
        normalized.as_str()
    } else {
        content
    };
    paragraphs(content)
        .into_iter()
        .filter(|p| !o.normalize || !(p.chars().count() >= 3 && p.chars().all(|c| matches!(c, '-' | '*' | '_' | '=' | '※'))))
        .map(|p| if o.indent { format!("　　{p}") } else { p.to_string() })
        .collect::<Vec<_>>()
        .join(sep)
}

pub fn export_txt(book: &Book, chapters: &[(Option<i64>, &Chapter)], o: &ExportOpts, with_header: bool) -> String {
    let mut out = String::new();
    if with_header {
        out += &format!("《{}》\n\n", book.title.trim());
        if !book.synopsis.trim().is_empty() {
            out += &format!("简介：\n{}\n\n", body(&book.synopsis, o));
        }
    }
    let parts: Vec<String> = chapters
        .iter()
        .map(|(n, ch)| format!("{}\n\n{}", heading(*n, &ch.title, o.chinese_numerals), body(&ch.content, o)))
        .collect();
    out += &parts.join("\n\n\n");
    out.push('\n');
    out
}

pub fn export_markdown(book: &Book, chapters: &[(Option<i64>, &Chapter)]) -> String {
    let mut out = format!("# {}\n\n", book.title.trim());
    if !book.synopsis.trim().is_empty() {
        out += &format!("> {}\n\n", book.synopsis.trim().replace('\n', "\n> "));
    }
    for (n, ch) in chapters {
        out += &format!("## {}\n\n{}\n\n", heading(*n, &ch.title, false), paragraphs(&ch.content).join("\n\n"));
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn xhtml_page(title: &str, inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"zh-CN\">\n<head><meta charset=\"UTF-8\"/><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>\n<body>\n{inner}\n</body>\n</html>\n",
        xml_escape(title)
    )
}

pub fn export_epub(book: &Book, chapters: &[(Option<i64>, &Chapter)], modified: i64) -> Result<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored)?;
    zip.write_all(b"application/epub+zip")?;
    zip.start_file("META-INF/container.xml", deflated)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>
"#,
    )?;
    zip.start_file("OEBPS/style.css", deflated)?;
    zip.write_all("body{font-family:serif;line-height:1.8;margin:1em;}h1,h2{text-align:center;}p{text-indent:2em;margin:0.4em 0;}".as_bytes())?;

    let title = xml_escape(book.title.trim());
    let mut pages: Vec<(String, String)> = Vec::new();
    if !book.synopsis.trim().is_empty() {
        let paras: String = paragraphs(&book.synopsis).iter().map(|p| format!("<p>{}</p>", xml_escape(p))).collect();
        pages.push(("intro".into(), "简介".into()));
        zip.start_file("OEBPS/intro.xhtml", deflated)?;
        zip.write_all(xhtml_page("简介", &format!("<h1>{title}</h1>\n<h2>简介</h2>\n{paras}")).as_bytes())?;
    }
    for (i, (n, ch)) in chapters.iter().enumerate() {
        let h = heading(*n, &ch.title, true);
        let paras: String = paragraphs(&ch.content).iter().map(|p| format!("<p>{}</p>\n", xml_escape(p))).collect();
        let id = format!("c{}", i + 1);
        zip.start_file(format!("OEBPS/{id}.xhtml"), deflated)?;
        zip.write_all(xhtml_page(&h, &format!("<h2>{}</h2>\n{paras}", xml_escape(&h))).as_bytes())?;
        pages.push((id, h));
    }

    let toc: String = pages
        .iter()
        .map(|(id, h)| format!("<li><a href=\"{id}.xhtml\">{}</a></li>\n", xml_escape(h)))
        .collect();
    zip.start_file("OEBPS/nav.xhtml", deflated)?;
    zip.write_all(xhtml_page("目录", &format!("<nav epub:type=\"toc\" id=\"toc\"><h1>目录</h1><ol>\n{toc}</ol></nav>")).as_bytes())?;

    let manifest: String = pages
        .iter()
        .map(|(id, _)| format!("<item id=\"{id}\" href=\"{id}.xhtml\" media-type=\"application/xhtml+xml\"/>\n"))
        .collect();
    let spine: String = pages.iter().map(|(id, _)| format!("<itemref idref=\"{id}\"/>\n")).collect();
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"zh-CN\">\n<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:identifier id=\"bookid\">urn:ai-novel:{}-{}</dc:identifier>\n<dc:title>{title}</dc:title>\n<dc:language>zh-CN</dc:language>\n<meta property=\"dcterms:modified\">{}</meta>\n</metadata>\n<manifest>\n<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n{manifest}</manifest>\n<spine>\n{spine}</spine>\n</package>\n",
        book.id,
        book.created_at,
        iso_datetime(modified)
    );
    zip.start_file("OEBPS/content.opf", deflated)?;
    zip.write_all(opf.as_bytes())?;
    Ok(zip.finish()?.into_inner())
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckItem {
    pub chapter_id: i64,
    pub label: String,
    pub level: String,
    pub message: String,
}

const LEFTOVER_PHRASES: [&str; 6] = ["好的，", "以下是", "作为AI", "作为一个AI", "（本章完）", "希望你喜欢"];

/// 投稿前检查：空章、字数、标题、标点、超长段落、残留的 AI 回复语。
pub fn submission_check(chapters: &[(Option<i64>, &Chapter)], min: i64, max: i64) -> Vec<CheckItem> {
    let mut out = Vec::new();
    for (n, ch) in chapters {
        let label = heading(*n, &ch.title, false);
        let mut push = |level: &str, message: String| {
            out.push(CheckItem { chapter_id: ch.id, label: label.clone(), level: level.into(), message });
        };
        let words = count_words(&ch.content);
        if words == 0 {
            push("error", "正文为空".into());
            continue;
        }
        if !is_special_title(&ch.title) {
            if min > 0 && words < min {
                push("warn", format!("字数 {words}，少于 {min}"));
            }
            if max > 0 && words > max {
                push("warn", format!("字数 {words}，超过 {max}，可以考虑拆章"));
            }
            if ch.title.trim().is_empty() {
                push("warn", "没有章节标题".into());
            }
        }
        let first = paragraphs(&ch.content).first().map(|p| p.to_string()).unwrap_or_default();
        if has_chapter_prefix(&first) {
            push("warn", "正文第一行是章节标题，导出后会重复".into());
        }
        let half = crate::lint::lint(&ch.content, &[]);
        for i in half.issues.iter().filter(|i| i.kind == "标点" || i.kind == "排版") {
            push("warn", format!("{}：{}", i.text, i.suggestion));
        }
        let head: String = ch.content.chars().take(60).collect();
        let tail: String = {
            let total = ch.content.chars().count();
            ch.content.chars().skip(total.saturating_sub(60)).collect()
        };
        for p in LEFTOVER_PHRASES {
            if head.contains(p) || tail.contains(p) {
                push("warn", format!("开头或结尾疑似残留了 AI 回复语「{p}」"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: i64, title: &str, content: &str) -> Chapter {
        Chapter { id, title: title.into(), content: content.into(), ..Default::default() }
    }

    #[test]
    fn headings() {
        assert_eq!(heading(Some(12), "风起", false), "第12章 风起");
        assert_eq!(heading(Some(12), "风起", true), "第十二章 风起");
        assert_eq!(heading(Some(3), "", false), "第3章");
        assert_eq!(heading(None, "序章 少年", false), "序章 少年");
        assert_eq!(heading(Some(1), "第一章 少年", false), "第一章 少年");
    }

    #[test]
    fn txt_format() {
        let book = Book { title: "测试".into(), ..Default::default() };
        let c = ch(1, "开端", "第一段。\n\n第二段。");
        let txt = export_txt(&book, &[(Some(1), &c)], &opts_for("fanqie"), false);
        assert_eq!(txt, "第1章 开端\n\n　　第一段。\n\n　　第二段。\n");
    }

    #[test]
    fn submission_formats_normalize_punctuation() {
        let book = Book::default();
        let c = ch(1, "开端", "\"走吧,\"他说...\n---\n她没动。");
        let txt = export_txt(&book, &[(Some(1), &c)], &opts_for("fanqie"), false);
        assert!(txt.contains("　　“走吧，”他说……\n\n　　她没动。"), "{txt}");
        let plain = export_txt(&book, &[(Some(1), &c)], &opts_for("txt"), false);
        assert!(plain.contains("\"走吧,\""));
    }

    #[test]
    fn epub_is_valid_zip() {
        let book = Book { id: 1, title: "测试<书>".into(), synopsis: "简介".into(), ..Default::default() };
        let c = ch(1, "开端", "林凡 & 苏雨。");
        let bytes = export_epub(&book, &[(Some(1), &c)], 0).unwrap();
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(z.by_index(0).unwrap().name(), "mimetype");
        let mut s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("OEBPS/c1.xhtml").unwrap(), &mut s).unwrap();
        assert!(s.contains("林凡 &amp; 苏雨。"));
    }

    #[test]
    fn checks() {
        let a = ch(1, "", "好的，以下是第一章。\n他走了,没回头。");
        let b = ch(2, "空", "");
        let items = submission_check(&[(Some(1), &a), (Some(2), &b)], 2000, 6000);
        assert!(items.iter().any(|i| i.message.contains("少于 2000")));
        assert!(items.iter().any(|i| i.message.contains("没有章节标题")));
        assert!(items.iter().any(|i| i.message.contains("AI 回复语")));
        assert!(items.iter().any(|i| i.message.contains("半角标点")));
        assert!(items.iter().any(|i| i.chapter_id == 2 && i.level == "error"));
    }
}
