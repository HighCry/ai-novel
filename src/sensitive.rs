//! 敏感词 / 屏蔽词自查：平台会把这些词屏蔽成「口口」，严重的整章封禁。全部本地匹配，不调用模型。
//! 内置词库只收网文里常见、平台普遍屏蔽的说法，按类别给风险等级和改法；作者可以在设置里追加自己的词库
//! （比如社区维护的敏感词表），也可以把不想再被提示的词加进忽略列表。结果只是风险提示，以平台审核为准。

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub word: String,
    pub category: &'static str,
    /// high / medium / low
    pub level: &'static str,
    pub suggestion: String,
    /// 字节偏移
    pub spans: Vec<(usize, usize)>,
}

struct Group {
    category: &'static str,
    level: &'static str,
    suggestion: &'static str,
    words: &'static [&'static str],
}

const GROUPS: &[Group] = &[
    Group {
        category: "涉政",
        level: "high",
        suggestion: "现实政治话题一律避开，架空设定也别影射",
        words: &["台独", "藏独", "疆独", "港独", "东突", "法轮功", "法轮大法", "全能神", "六四事件", "八九学潮"],
    },
    Group {
        category: "涉政",
        level: "medium",
        suggestion: "涉及敏感历史，架空处理或删掉",
        words: &["文化大革命", "文革"],
    },
    Group {
        category: "色情",
        level: "high",
        suggestion: "删掉，或用省略号和场景转换带过",
        words: &[
            "做爱", "性交", "口交", "肛交", "乳交", "群交", "性器", "阴茎", "阴道", "阴蒂", "阴唇", "龟头", "睾丸", "精液", "射精", "勃起",
            "自慰", "手淫", "处女膜", "乳头", "奶头", "强奸", "轮奸", "迷奸", "诱奸", "卖淫", "嫖娼", "约炮", "一夜情", "援交", "黄片", "毛片",
            "裸照",
        ],
    },
    Group {
        category: "色情",
        level: "medium",
        suggestion: "擦边描写容易被判低俗，收着写",
        words: &["娇喘", "酥胸", "春药", "媚药", "春宫", "床戏", "欲火焚身"],
    },
    Group {
        category: "色情",
        level: "low",
        suggestion: "单独出现一般没事，和身体接触的动作连写时要小心",
        words: &["裸体", "胴体"],
    },
    Group {
        category: "血腥暴力",
        level: "medium",
        suggestion: "弱化过程，写结果和人物的反应",
        words: &["脑浆", "碎尸", "分尸", "肢解", "开膛破肚", "剥皮", "挖眼", "凌迟", "虐杀", "血肉模糊", "肠子流", "内脏流", "吃人肉", "割腕"],
    },
    Group {
        category: "毒品",
        level: "high",
        suggestion: "不写真实毒品的名称和过程，可以模糊成「禁药」之类",
        words: &["冰毒", "海洛因", "摇头丸", "K粉", "可卡因", "吸毒", "贩毒", "制毒"],
    },
    Group {
        category: "毒品",
        level: "medium",
        suggestion: "不写真实毒品的名称和过程，可以模糊成「禁药」之类",
        words: &["大麻"],
    },
    Group {
        category: "赌博",
        level: "low",
        suggestion: "别细写真实赌法、赔率和平台",
        words: &["网赌", "六合彩", "赌球", "百家乐", "老虎机"],
    },
    Group {
        category: "枪支爆炸物",
        level: "medium",
        suggestion: "型号和制作方法模糊处理，比如「自动步枪」「高爆炸药」",
        words: &["AK47", "AK-47", "C4炸药", "TNT炸药", "雷管", "炸药配方", "土制炸弹", "制作炸弹", "自制枪支", "仿制枪"],
    },
    Group {
        category: "引流广告",
        level: "high",
        suggestion: "正文里不能留联系方式、外链和引流话术，平台会直接判违规",
        words: &["公众号", "二维码", "扫码关注", "淘宝店铺", "加群", "群号"],
    },
    Group {
        category: "低俗辱骂",
        level: "low",
        suggestion: "换个不带脏字的骂法，或者只写「他骂了一句」",
        words: &["操你妈", "草你妈", "草泥马", "肏", "傻逼", "煞笔", "傻B", "婊子", "贱货", "狗娘养的", "鸡巴"],
    },
];

struct Pattern {
    re: &'static str,
    category: &'static str,
    level: &'static str,
    suggestion: &'static str,
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        re: r"(?i)(?:qq|扣扣)[号群]?[:：\s]*\d{5,11}",
        category: "引流广告",
        level: "high",
        suggestion: "正文里不能留 QQ 号",
    },
    Pattern {
        re: r"(?i)(?:微信|vx|wx|v信|薇信|威信)号?[:：\s]*[a-z][a-z0-9_-]{5,19}",
        category: "引流广告",
        level: "high",
        suggestion: "正文里不能留微信号",
    },
    Pattern { re: r"(?:^|[^0-9])(1[3-9][0-9]{9})(?:[^0-9]|$)", category: "引流广告", level: "high", suggestion: "正文里不能留手机号" },
    Pattern { re: r"(?i)(?:https?://|www\.)[a-z0-9./?=&%_-]+", category: "引流广告", level: "high", suggestion: "正文里不能留网址" },
    Pattern {
        re: r"[嗯啊哦唔哈]{4,}",
        category: "色情",
        level: "medium",
        suggestion: "连着一串语气词容易被机器判成低俗，缩短或者改成动作描写",
    },
];

fn compiled() -> &'static [(Regex, &'static Pattern)] {
    static RE: OnceLock<Vec<(Regex, &'static Pattern)>> = OnceLock::new();
    RE.get_or_init(|| PATTERNS.iter().map(|p| (Regex::new(p.re).expect("敏感词正则"), p)).collect())
}

/// 自定义词一行一个，「词=改法」可以顺带写上改法
fn parse_custom(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (word, tip) = line.split_once(['=', '＝']).map_or((line, ""), |(w, t)| (w.trim(), t.trim()));
    (!word.is_empty()).then(|| (word.to_string(), tip.to_string()))
}

/// 按出现顺序返回命中的词；同一个词合并成一条
pub fn scan(text: &str, extra: &[String], ignore: &[String]) -> Vec<Hit> {
    let ignored = |w: &str| ignore.iter().any(|i| i.trim() == w);
    let mut hits: Vec<Hit> = Vec::new();
    let mut add = |word: &str, category: &'static str, level: &'static str, suggestion: &str, span: (usize, usize)| {
        if ignored(word) {
            return;
        }
        match hits.iter_mut().find(|h| h.word == word) {
            Some(h) => h.spans.push(span),
            None => hits.push(Hit { word: word.into(), category, level, suggestion: suggestion.into(), spans: vec![span] }),
        }
    };
    for g in GROUPS {
        for w in g.words {
            for (i, m) in text.match_indices(w) {
                add(w, g.category, g.level, g.suggestion, (i, i + m.len()));
            }
        }
    }
    // 有捕获组时只标组里那段（手机号两边的边界字符不算）
    for (re, p) in compiled() {
        for caps in re.captures_iter(text) {
            if let Some(m) = caps.get(1).or_else(|| caps.get(0)) {
                add(m.as_str(), p.category, p.level, p.suggestion, (m.start(), m.end()));
            }
        }
    }
    for (word, tip) in extra.iter().filter_map(|l| parse_custom(l)) {
        let tip = if tip.is_empty() { "这是你在设置里添加的敏感词".to_string() } else { format!("改成：{tip}") };
        for (i, m) in text.match_indices(word.as_str()) {
            add(&word, "自定义", "medium", &tip, (i, i + m.len()));
        }
    }
    hits.sort_by_key(|h| h.spans.iter().map(|s| s.0).min().unwrap_or(0));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_builtin_patterns_and_custom_words() {
        let text = "他扯开她的衣服做爱。加我微信号：abc_12345，或者QQ：123456789。嗯嗯啊啊啊，脑浆迸裂。青云宗的禁药。";
        let hits = scan(text, &["禁药=违禁丹药".into()], &[]);
        let words: Vec<&str> = hits.iter().map(|h| h.word.as_str()).collect();
        assert!(words.contains(&"做爱") && words.contains(&"脑浆"), "{words:?}");
        assert!(hits.iter().any(|h| h.category == "引流广告" && h.word.contains("abc_12345")));
        assert!(hits.iter().any(|h| h.word.starts_with("QQ")));
        assert!(hits.iter().any(|h| h.word == "嗯嗯啊啊啊"));
        let custom = hits.iter().find(|h| h.word == "禁药").unwrap();
        assert_eq!((custom.category, custom.suggestion.as_str()), ("自定义", "改成：违禁丹药"));
        let first = &hits[0];
        assert_eq!(&text[first.spans[0].0..first.spans[0].1], first.word);
    }

    #[test]
    fn plot_words_and_ignored_words_are_quiet() {
        let text = "故事迎来高潮，他赤裸上身冲进赌坊，一刀斩下妖兽头颅，痛苦地呻吟。";
        assert!(scan(text, &[], &[]).is_empty(), "{:?}", scan(text, &[], &[]));
        assert!(scan("文革时期", &[], &["文革".into()]).is_empty());
        assert!(scan("电话13812345678，打给他。", &[], &[]).iter().any(|h| h.word == "13812345678"));
        assert!(scan("去www.abc.com看看", &[], &[]).iter().any(|h| h.word == "www.abc.com"));
    }
}
