use regex::Regex;
use std::sync::OnceLock;

/// 平台口径的字数：不含空白的字符数。
pub fn count_words(s: &str) -> i64 {
    s.chars().filter(|c| !c.is_whitespace()).count() as i64
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

/// 中文按字二元组切分，英文和数字按词切分，用于 BM25 检索。
pub fn tokenize(s: &str) -> Vec<String> {
    fn flush_run(run: &mut Vec<char>, out: &mut Vec<String>) {
        match run.len() {
            0 => {}
            1 => out.push(run[0].to_string()),
            _ => out.extend(run.windows(2).map(|w| w.iter().collect::<String>())),
        }
        run.clear();
    }
    fn flush_word(word: &mut String, out: &mut Vec<String>) {
        if !word.is_empty() {
            out.push(word.to_lowercase());
            word.clear();
        }
    }
    let mut out = Vec::new();
    let mut run = Vec::new();
    let mut word = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            flush_word(&mut word, &mut out);
            run.push(c);
        } else if c.is_alphanumeric() {
            flush_run(&mut run, &mut out);
            word.push(c);
        } else {
            flush_run(&mut run, &mut out);
            flush_word(&mut word, &mut out);
        }
    }
    flush_run(&mut run, &mut out);
    flush_word(&mut word, &mut out);
    out
}

pub fn head_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        s.to_string()
    } else {
        s.chars().skip(total - n).collect()
    }
}

/// 截到 n 字，超出部分用省略号表示。
pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}……", head_chars(s, n))
    }
}

pub fn paragraphs(s: &str) -> Vec<&str> {
    s.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect()
}

const CN_DIGITS: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

pub fn cn_number(n: i64) -> String {
    if n <= 0 {
        return CN_DIGITS[0].to_string();
    }
    if n < 10 {
        return CN_DIGITS[n as usize].to_string();
    }
    if n < 20 {
        let ones = if n % 10 == 0 { "" } else { CN_DIGITS[(n % 10) as usize] };
        return format!("十{ones}");
    }
    if n >= 10000 {
        let (high, low) = (n / 10000, n % 10000);
        let mut s = format!("{}万", cn_number(high));
        if low > 0 {
            if low < 1000 {
                s.push('零');
            }
            s.push_str(&cn_under_10000(low));
        }
        return s;
    }
    cn_under_10000(n)
}

fn cn_under_10000(n: i64) -> String {
    const UNITS: [&str; 4] = ["千", "百", "十", ""];
    let digits = [n / 1000 % 10, n / 100 % 10, n / 10 % 10, n % 10];
    let mut s = String::new();
    let (mut started, mut zero) = (false, false);
    for (i, &d) in digits.iter().enumerate() {
        if d == 0 {
            zero = started;
            continue;
        }
        if zero {
            s.push('零');
            zero = false;
        }
        s.push_str(CN_DIGITS[d as usize]);
        s.push_str(UNITS[i]);
        started = true;
    }
    s
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn utc_day(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn iso_datetime(ts: i64) -> String {
    let secs = ts.rem_euclid(86_400);
    format!("{}T{:02}:{:02}:{:02}Z", utc_day(ts), secs / 3600, secs % 3600 / 60, secs % 60)
}

pub fn is_valid_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn full_width(c: char) -> char {
    match c {
        ',' => '，',
        ';' => '；',
        ':' => '：',
        '?' => '？',
        '!' => '！',
        _ => c,
    }
}

/// 规范中文正文的标点：英文双引号按段成对换成“”，汉字后的半角 , ; : ? ! 换成全角，... 换成 ……
pub fn normalize_punct(text: &str) -> String {
    static DOTS: OnceLock<Regex> = OnceLock::new();
    if !text.chars().any(is_cjk) {
        return text.to_string();
    }
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let mut out = String::with_capacity(line.len());
            let mut open = true;
            for (i, &c) in chars.iter().enumerate() {
                let after_cjk = i > 0 && (is_cjk(chars[i - 1]) || matches!(chars[i - 1], '”' | '）' | '》' | '」'));
                match c {
                    '"' => {
                        out.push(if open { '“' } else { '”' });
                        open = !open;
                    }
                    ',' | ';' | ':' | '?' | '!' if after_cjk => out.push(full_width(c)),
                    _ => out.push(c),
                }
            }
            out
        })
        .collect();
    let dots = DOTS.get_or_init(|| Regex::new(r"\.{3,}|。{2,}").unwrap());
    dots.replace_all(&lines.join("\n"), "……").into_owned()
}

const SPECIAL_TITLES: [&str; 12] =
    ["序章", "序言", "楔子", "引子", "前言", "尾声", "终章", "后记", "番外", "完本感言", "上架感言", "作品相关"];

/// 序章、番外这类章节不参与“第 N 章”编号。
pub fn is_special_title(title: &str) -> bool {
    let t = title.trim();
    SPECIAL_TITLES.iter().any(|s| t.starts_with(s))
}

pub fn number_chapters<'a>(titles: impl Iterator<Item = &'a str>) -> Vec<Option<i64>> {
    let mut n = 0;
    titles
        .map(|t| {
            if is_special_title(t) {
                None
            } else {
                n += 1;
                Some(n)
            }
        })
        .collect()
}

const NUM_CLASS: &str = "[0-9０-９零〇一二两三四五六七八九十百千万]+";

const SEP_CLASS: &str = r"[\s:：、.．—\-]";

// “第一节课下课后”“第一部手机”这类正文开头不能当成标题，所以“节/部/集”后面必须是分隔符或行尾。
fn chapter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"^第\s*{NUM_CLASS}\s*(?:章|节(?:$|{SEP_CLASS})){SEP_CLASS}*(.*)$")).unwrap()
    })
}

fn volume_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r"^第\s*{NUM_CLASS}\s*(?:卷.*|[部集](?:$|{SEP_CLASS}.*))$")).unwrap()
    })
}

/// 标题是否已经自带“第 N 章”前缀。
pub fn has_chapter_prefix(title: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"^第\s*{NUM_CLASS}\s*[章节]")).unwrap()).is_match(title.trim())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedChapter {
    pub volume: Option<String>,
    pub title: String,
    pub content: String,
}

fn match_chapter(line: &str) -> Option<String> {
    if line.chars().count() > 50 {
        return None;
    }
    if let Some(c) = chapter_re().captures(line) {
        return Some(c.get(1).map(|m| m.as_str().trim().to_string()).unwrap_or_default());
    }
    if is_special_title(line) && line.chars().count() <= 30 {
        return Some(line.to_string());
    }
    None
}

/// 按“第X章”“第X卷”等标题拆分整本小说；返回（正文前的内容, 章节列表）。
pub fn split_chapters(text: &str) -> (String, Vec<ParsedChapter>) {
    let mut preface = String::new();
    let mut chapters: Vec<ParsedChapter> = Vec::new();
    let mut volume: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().count() <= 40 && volume_re().is_match(line) {
            volume = Some(line.to_string());
            continue;
        }
        if let Some(title) = match_chapter(line) {
            chapters.push(ParsedChapter { volume: volume.clone(), title, content: String::new() });
            continue;
        }
        let target = match chapters.last_mut() {
            Some(ch) => &mut ch.content,
            None => &mut preface,
        };
        target.push_str(line);
        target.push('\n');
    }
    if chapters.is_empty() && !preface.trim().is_empty() {
        chapters = split_by_length(&preface, 3000);
        preface.clear();
    }
    for ch in &mut chapters {
        ch.content = ch.content.trim_end().to_string();
    }
    chapters.retain(|c| !c.content.is_empty() || !c.title.is_empty());
    (preface.trim().to_string(), chapters)
}

fn split_by_length(text: &str, target: usize) -> Vec<ParsedChapter> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut len = 0;
    for p in paragraphs(text) {
        buf.push_str(p);
        buf.push('\n');
        len += p.chars().count();
        if len >= target {
            out.push(ParsedChapter { volume: None, title: String::new(), content: std::mem::take(&mut buf) });
            len = 0;
        }
    }
    if !buf.trim().is_empty() {
        out.push(ParsedChapter { volume: None, title: String::new(), content: buf });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_numbers() {
        assert_eq!(count_words("你好， world\n　测试"), 10);
        assert_eq!(cn_number(1), "一");
        assert_eq!(cn_number(10), "十");
        assert_eq!(cn_number(15), "十五");
        assert_eq!(cn_number(20), "二十");
        assert_eq!(cn_number(105), "一百零五");
        assert_eq!(cn_number(110), "一百一十");
        assert_eq!(cn_number(1010), "一千零一十");
        assert_eq!(cn_number(12345), "一万二千三百四十五");
    }

    #[test]
    fn tokenizes_cjk_bigrams() {
        assert_eq!(tokenize("林凡拔剑"), vec!["林凡", "凡拔", "拔剑"]);
        assert_eq!(tokenize("AI 写作"), vec!["ai", "写作"]);
        assert_eq!(tokenize("剑，"), vec!["剑"]);
    }

    #[test]
    fn dates() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(1_790_000_000), "2026-09-21");
        assert!(is_valid_day("2026-10-03"));
        assert!(!is_valid_day("2026/10/03"));
    }

    #[test]
    fn splits_novel() {
        let text = "作品简介：一个少年的故事。\n\n第一卷 风起\n第一章 少年\n林凡睁开眼。\n\n他看见了天。\n第一节课下课后，他走了。\n第一部手机是师父给的。\n第2章：出山\n下山了。\n番外 师父\n师父的故事。";
        let (preface, chs) = split_chapters(text);
        assert_eq!(preface, "作品简介：一个少年的故事。");
        assert_eq!(chs.len(), 3);
        assert!(chs[0].content.contains("第一节课下课后"));
        assert!(chs[0].content.contains("第一部手机"));
        assert_eq!(chs[0].title, "少年");
        assert_eq!(chs[0].volume.as_deref(), Some("第一卷 风起"));
        assert!(chs[0].content.starts_with("林凡睁开眼。\n他看见了天。\n"));
        assert_eq!(match_chapter("第三章少年归来").as_deref(), Some("少年归来"));
        assert_eq!(match_chapter("第一节").as_deref(), Some(""));
        assert_eq!(chs[1].title, "出山");
        assert_eq!(chs[2].title, "番外 师父");
        let nums = number_chapters(chs.iter().map(|c| c.title.as_str()));
        assert_eq!(nums, vec![Some(1), Some(2), None]);
    }

    #[test]
    fn normalizes_punctuation() {
        assert_eq!(normalize_punct("\"你好,林凡!\"他说...\n\"走吧?\""), "“你好，林凡！”他说……\n“走吧？”");
        assert_eq!(normalize_punct("version 1.2, ok?"), "version 1.2, ok?");
    }

    #[test]
    fn splits_by_length_without_headings() {
        let text = "甲".repeat(2000) + "\n" + &"乙".repeat(2000) + "\n" + &"丙".repeat(100);
        let (_, chs) = split_chapters(&text);
        assert_eq!(chs.len(), 2);
        assert!(chs.iter().all(|c| c.title.is_empty()));
    }
}
