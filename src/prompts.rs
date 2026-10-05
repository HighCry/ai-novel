//! 提示词模板。每个模板都可以在「设置 → 提示词模板」里修改，留空即恢复默认。
//! 模板里用 {{变量}} 占位，{{craft:手册ID}} 会替换成内置写作手册的全文。
//! 部分规则改编自 webnovel-writer（GPL-3.0）、novel-studio（Apache-2.0）和 webnovel-handbook（MIT，信息投放与开篇规则），见 THIRD_PARTY_NOTICES.md。

use crate::text::clip;
use regex::{Captures, Regex};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub vars: &'static [(&'static str, &'static str)],
    pub text: &'static str,
}

const V_CONTEXT: (&str, &str) = ("context", "自动组装的上下文：作品信息、世界观、大纲、前情、出场设定、伏笔、相关前文");
const V_CHAPTER: (&str, &str) = ("chapter", "章节名，如“第12章 夜探”");
const V_OUTLINE: (&str, &str) = ("outline_block", "本章章纲（带标题）");
const V_BEATS: (&str, &str) = ("beats_block", "本章节拍（有节拍时才有内容）");
const V_WORDS: (&str, &str) = ("words", "目标字数");
const V_INSTRUCTION: (&str, &str) = ("instruction_block", "作者的补充要求（没有时为空）");
const V_GOLDEN: (&str, &str) = ("golden_block", "开篇三章的“黄金三章”要求");
const V_FINALE: (&str, &str) = ("finale_block", "大结局要求和全部未回收伏笔（勾选“大结局”时才有）");
const V_CONTENT: (&str, &str) = ("content", "本章正文");
const V_BRIEF: (&str, &str) = ("brief", "开书资料：书名、题材、平台、创意、主角、梗概、世界观、人物、总纲");
const V_PLATFORM: (&str, &str) = ("platform", "目标平台名称");
const V_GENRE_BLOCK: (&str, &str) = ("genre_block", "匹配到的题材模板要点");

const WRITER_SYSTEM: &str = "你是一位经验丰富的中文网络小说作者，熟悉番茄、起点等平台读者的阅读习惯。你在协助作者写作：你写出的文字会由作者审阅、修改后再放进正文。

【写作原则】
1. 只输出正文本身：不要标题，不要“好的”“以下是”之类的开场白，不要解释，不要在结尾总结、点题或升华。
2. 用场景、动作和对话推进情节，少用概括性叙述；能用一句对话或一个动作表现的，不要写一段心理分析。
3. 句子长短交错，段落以一到四句为主，适合手机阅读；对话单独成段，用中文引号“”，说话人和动作交代清楚；场景切换空一行即可，不要用“---”之类的分隔线。
4. 每个人物说话要符合身份、年龄和性格，不同人物的语气要能区分开。
5. 细节要具体：写看得见、听得见、摸得着的东西，不堆砌形容词，不用空泛的比喻。
6. 不复述前文已经写过的内容，不写提纲式的流水账，不注水。
7. 严格遵守设定：人物的能力等级、身体状态、所在位置、人物关系和世界规则以【相关设定】为准；人物只知道自己知道的事；已死亡或不在场的人物不能无故出现。
8. 只写人物此刻能感知的内容，不站在上帝视角解释、剧透或下结论：不用“殊不知”“她不知道的是”“这意味着”“之所以……是因为”“多年以后”这类说法，因果和潜台词让读者从动作、对话里自己拼出来。
9. 避免套话和 AI 腔：嘴角勾起一抹弧度、眼中闪过一丝、不禁、仿佛、宛如、映入眼帘、心中暗道、脸色一变、一股莫名的、空气仿佛凝固了、深吸一口气、倒吸一口凉气、这一刻、命运的齿轮、与此同时、然而、毫无疑问、不可否认、五味杂陈、难以言喻；“微微、淡淡、缓缓、轻轻”这类弱化副词少用；不要三连排比，不要段尾感悟句。
10. 信息投放：作者知道十成，正文先露一成，只写读者此刻该知道的设定。设定跟着行动、冲突、代价和后果带出来，不写大段介绍，不让人物“如你所知”地互相背常识；新名词出场就绑在一个动作或后果上，不要一口气抛出好几个；不写境界大全、地图介绍、势力图谱和金手指说明书；视角人物不知道的名字、来历和幕后真相，旁白不替他说破，只写他看得到的异常。

{{genre_block}}

{{style_block}}

{{sample_block}}";

const ANALYST_SYSTEM: &str = "你是一位严谨的网络小说责任编辑，擅长梳理剧情、核对设定、发现逻辑漏洞。你的回答要具体、可操作；引用原文时保持原样，不编造正文里没有的内容。";

const EDITOR_SYSTEM: &str = "你是一位资深网文编辑兼策划，熟悉番茄、起点的题材热点、读者口味和签约标准。你在和作者讨论创作：回答要具体、有网文感，给出可以直接采用的方案，不讲空泛的理论。

{{genre_block}}";

const CHAT_SYSTEM: &str = "你现在扮演小说《{{book_title}}》里的人物「{{name}}」，和作者对话，帮作者找到这个人物的声音。

{{profile}}

{{worldview_block}}

{{related_block}}

要求：始终以「{{name}}」的身份、语气、见识和立场说话，只知道这个人物此刻知道的事，不知道自己是小说人物；回答简短自然，像真人说话，可以用括号写简短的动作或神态。";

const CONTINUE: &str = "{{context}}

【本章】{{chapter}}
{{outline_block}}
{{beats_block}}

【正文（截至光标处）】
{{before}}

{{after_block}}

请从光标处接着往下写约 {{words}} 字。{{instruction_block}}
直接输出续写的正文，不要重复光标之前的内容。";

const BEATS: &str = "{{context}}

【要规划的章节】{{chapter}}
{{outline_block}}
{{golden_block}}
{{finale_block}}

请把这一章拆成 5～8 个场景节拍，紧扣【本章章纲】的事件、冲突和出场人物（本章章纲为空时，按前情和总纲合理安排本章），本章计划约 {{words}} 字。{{instruction_block}}
要求：
1. 每个节拍一行，格式：序号. 【场景】地点和出场人物｜这一段谁想要什么｜遇到什么阻碍｜怎么推进、结果如何。节拍之间要有因果，不要平铺流水账。
2. 至少安排一个爽点或同等的情绪兑现（打脸、扮猪吃虎、越级反杀、收获、揭秘、反派翻车、甜蜜超预期等），按“铺垫 → 兑现 → 微反转”组织，并标出它在哪个节拍。
3. 最后一个节拍是章末钩子，从这些类型里选最合适的并注明类型：悬念预知、截断、选择困境、危机升级、身份即将揭露、情感转折、信息差、对话悬念、收获预告、反派逼近。
4. 只规划本章，严格落实【本章章纲】、不要偏离章纲另编情节，也不要提前写后面章节的事；【总纲】只作背景；人物言行符合【相关设定】里的状态和已知信息。
直接输出节拍列表，不要写正文。";

const WRITE_CHAPTER: &str = "{{context}}

【要写的章节】{{chapter}}
{{outline_block}}
{{beats_block}}
{{golden_block}}
{{finale_block}}

请写出本章完整正文，约 {{words}} 字。{{instruction_block}}
要求：{{outline_rule}}开头直接进入场景，不要回顾上一章；{{beats_rule}}章末留一个让读者想看下一章的钩子。直接输出正文。";

const EXPAND: &str = "{{context}}

{{before_block}}

【需要处理的段落】
{{selection}}

{{after_block}}

请把【需要处理的段落】扩写到约 {{words}} 字：补充动作、对话、神态和环境细节，情节走向和结果不变。{{instruction_block}}
只输出处理后的段落，不要输出前文和后文，不要解释。";

const SHORTEN: &str = "{{context}}

{{before_block}}

【需要处理的段落】
{{selection}}

{{after_block}}

请把【需要处理的段落】精简到约 {{words}} 字：保留关键情节、对话和信息，删掉重复和拖沓的部分。{{instruction_block}}
只输出处理后的段落，不要输出前文和后文，不要解释。";

const REWRITE: &str = "{{context}}

{{before_block}}

【需要处理的段落】
{{selection}}

{{after_block}}

请按这个要求改写【需要处理的段落】：{{goal}}
只输出处理后的段落，不要输出前文和后文，不要解释。";

const POLISH: &str = "{{context}}

{{before_block}}

【需要处理的段落】
{{selection}}

{{after_block}}

请润色【需要处理的段落】：修正病句、错别字和不通顺的地方，删掉套话和 AI 腔（解释腔、上帝视角、弱化副词堆砌、三连排比、段尾感悟），让节奏更紧凑；只做必要的修改，不改情节，不改人物的说话风格。{{instruction_block}}
只输出处理后的段落，不要输出前文和后文，不要解释。";

const DESLOP: &str = "{{context}}

{{before_block}}

【需要修订的正文】
{{selection}}

{{after_block}}

【本地检测结果（供参考：按语境判断，有功能的写法可以保留）】
{{issues}}

{{keep_block}}

请按系统提示里【本次修订使用的技能】修订【需要修订的正文】，优先处理上面检测到的问题。{{instruction_block}}
要求：只改“怎么说”，不改“说什么”；改最少的字，没有问题的句子原样保留。原文里发生的每一件事、说的每一句台词都要留下：台词可以改得更口语，不能删；人名、地名、物品和功法的全名、境界等级、数量、地点细节（比如在谁的哪个房间）、事件顺序都属于设定，一个字都不改；不要整段删除。
原文约 {{words}} 字，修订稿控制在 {{min_words}}～{{words}} 字，不要比原文长：要删的只是套话、重复和多余的修饰，不是情节、台词和设定信息；比喻和强调词改成直写，不要连同所在的句子一起删；有问题的句子可以换成一个具体的动作或后果，但不要另外补写原文没有的情节、细节和台词，也不要为了把句子写长往里加东西。不要把句子都压成短句，也不要用同义词轮换来应付。
只输出修订后的完整正文，不要标题、说明或修改清单。";

const FREE: &str = "{{context}}

{{chapter_block}}
{{outline_block}}
{{recent_block}}

【作者的问题】{{question}}";

const IDEAS: &str = "作者想在{{platform}}写一部{{genre}}小说。
{{brief}}
{{genre_block}}

请给出 3 个方向明显不同的开书方案，每个都要有清晰的卖点和差异化。只输出 JSON，格式：
{\"ideas\":[{\"title\":\"书名\",\"logline\":\"一句话梗概（30-60字）\",\"selling_points\":\"核心卖点：金手指、爽点、差异化（50字以内）\",\"opening\":\"第一章的切入场景（50字以内）\"}]}";

const WORLD: &str = "{{brief}}
{{genre_block}}

请为这部{{platform}}小说设计世界观。只写会影响剧情的设定，分小标题、条理清楚，1500 字以内：
1. 时代背景与地图格局
2. 力量/职业/升级体系（等级划分清晰，可以互相比较强弱）
3. 主要势力和它们之间的矛盾
4. 金手指（或核心设定）的规则、代价和限制
5. 其他会推动剧情的特殊规则";

const CHARACTERS: &str = "{{brief}}

请设计本书的主要人物：主角 1 名、重要配角 3-6 名、前期反派 1-2 名。人物之间要有关系和冲突，每个人都要有自己想要的东西。只输出 JSON，格式：
{\"characters\":[{\"name\":\"姓名\",\"aliases\":\"别名、称呼，用顿号分隔\",\"role\":\"主角/重要配角/配角/反派\",\"description\":\"身份、外貌、性格、动机、和主角的关系（100字以内）\",\"immutable\":\"不能写崩的核心特征，比如性格底线、身世、口头禅\",\"state\":\"开篇时的状态：所在位置、实力等级、处境\"}]}";

const OUTLINE: &str = "{{brief}}
{{genre_block}}

请写出这本书的总纲：
1. 先用一段话写清主线：主角想要什么、最大的阻碍是什么、最终结局。
2. 再按卷拆分（建议 4-8 卷），每卷写：卷名、本卷目标、主要冲突和对手、高潮事件、结尾状态、大致章数。
3. 节奏符合{{platform}}读者的习惯：开篇三章内进入主线，每章至少一个爽点或同等兑现，每 5 章一个组合爽点，每卷一个改变主角地位的里程碑高潮，卷与卷之间有升级感。";

const SYNOPSIS: &str = "{{brief}}

请为这本书写平台展示用的作品简介，每版 150-300 字：开头一句抓人，突出主角身份、金手指和核心冲突，结尾留悬念，不剧透结局。写 3 个风格不同的版本，版本之间空一行，不要加编号和说明。";

const VOLUME_OUTLINE: &str = "{{brief}}

请细化「{{volume_title}}」的卷纲：
1. 本卷开始和结束时主角的状态
2. 按顺序列出 8-15 个关键事件，每个事件一两句话
3. 本卷新登场的重要人物
4. 需要埋下和回收的伏笔
5. 本卷的高潮和结尾钩子
{{existing_block}}";

const CHAPTER_OUTLINES: &str = "{{context}}

{{recent_block}}

请接着规划第 {{start}} 章到第 {{end}} 章的章纲，共 {{count}} 章。{{instruction_block}}
要求：每章都有明确的事件和推进；按爽点密度安排节奏：每章至少一个爽点或同等兑现（过渡章可以弱一些），每 5 章至少一个组合爽点，每 10～15 章一个改变主角地位的里程碑；章末尽量留钩子；和已有剧情、未回收的伏笔衔接。只输出 JSON，格式：
{\"chapters\":[{\"title\":\"章节标题（不带“第几章”）\",\"outline\":\"本章章纲：主要事件、冲突、出场人物、爽点、结尾钩子，80-150字\"}]}";

const SUMMARIZE: &str = "请为下面这一章写剧情摘要，供后续写作时回顾前情使用。
要求：200-350 字；按时间顺序写清发生了什么、谁做了什么、结果如何；写明人物状态和关系的变化、新出现的人物/物品/设定、埋下或回收的伏笔；不评价，不修辞，直接输出摘要。

【{{chapter}}】
{{content}}";

const EXTRACT: &str = "下面是小说设定库的现状和最新一章正文。请找出这一章带来的设定变化，用来更新设定库。

【已有设定（名称｜类别｜当前状态）】
{{entries}}

【未回收的伏笔（编号. 标题）】
{{threads}}

【{{chapter}}】
{{content}}

只输出 JSON，格式：
{\"entries\":[{\"name\":\"名称（已有条目用原名）\",\"kind\":\"character/location/item/faction/concept\",\"is_new\":false,\"aliases\":\"别名\",\"description\":\"新条目的设定描述；已有条目留空\",\"fields\":{\"location\":\"\",\"power\":\"\",\"body\":\"\",\"mind\":\"\",\"items\":\"\",\"recent\":\"\",\"knows\":\"\",\"unaware\":\"\"},\"state\":\"非人物条目的最新状态\"}],
 \"threads_new\":[{\"title\":\"本章新埋下的伏笔或悬念\",\"detail\":\"具体内容和可能的回收方向\"}],
 \"threads_progressed\":[{\"id\":1,\"note\":\"本章如何推进了这条伏笔\"}],
 \"threads_resolved\":[{\"id\":1,\"note\":\"本章如何回收\"}],
 \"relations\":[{\"a\":\"人物甲\",\"b\":\"人物乙\",\"kind\":\"师徒/敌对/恋人/盟友/亲属/上下级/交易/暧昧等\",\"detail\":\"一句话说明\",\"status\":\"active/ended\"}]}
relations 只列这一章里新建立、发生变化或结束的关系。
人物用 fields 写这一章结束时的状态：location 位置，power 实力等级，body 身体状况，mind 心理状态，items 关键物品，recent 近期经历，knows 知道了哪些秘密，unaware 还不知道的重要事实；没有变化的字段留空或省略。
只列出确实有变化的条目；状态没有变化的已有条目不要列出；不要编造正文里没有的信息。";

const CHECK: &str = "请核对下面这一章和已有设定、前情是否矛盾。

{{context}}

【待检查：{{chapter}}】
{{content}}

按五个类别逐项检查：
1. 设定：能力等级、物品、世界规则是否和设定冲突
2. 时间线：时间先后、季节、年龄、事件顺序是否冲突
3. 连贯：和前情事实、上一章结尾是否接得上
4. 人物：性格、口吻、动机是否走样；人物是否知道了他还不知道的事；已死亡或不在场的人物是否出现
5. 逻辑：因果是否成立、行为是否合理
只报能拿出证据的问题：必须引用原文，并说明和哪条设定或前情矛盾。不评价文笔好坏，不建议改剧情走向。
只输出 JSON，格式：
{\"issues\":[{\"severity\":\"high/medium/low\",\"type\":\"设定/时间线/连贯/人物/逻辑\",\"quote\":\"原文中有问题的句子（原样摘录）\",\"problem\":\"和哪条设定或前情矛盾\",\"suggestion\":\"修改建议\"}]}
没有问题就返回 {\"issues\":[]}。";

const REVIEW: &str = "请以{{platform}}责任编辑的视角审阅这一章，指出影响读者追读的问题。

{{info}}
{{outline_block}}
【{{chapter}}】
{{content}}

只输出 JSON，格式：
{\"scores\":{\"hook\":7,\"pacing\":7,\"payoff\":7,\"character\":7,\"ending\":7,\"readability\":7},\"summary\":\"一句话总评\",\"strengths\":[\"优点\"],\"problems\":[{\"quote\":\"原文片段\",\"problem\":\"问题\",\"suggestion\":\"改法\"}]}
评分 0-10：hook 开头吸引力，pacing 节奏，payoff 爽点和情绪，character 人物塑造，ending 章末钩子，readability 可读性。";

const TENSION: &str = "请分析下面这一章的节奏和追读力，用来画全书的张力曲线。
{{outline_block}}
【{{chapter}}】
{{content}}

只输出 JSON，格式：
{\"tension\":6,\"emotion\":\"本章主导情绪，如紧张/压抑/爽快/温馨/悬疑\",\"cool_points\":[{\"type\":\"装逼打脸/扮猪吃虎/越级反杀/打脸权威/反派翻车/甜蜜超预期/收获/揭秘/其他\",\"level\":\"小/组合/里程碑\",\"desc\":\"一句话描述\"}],\"hook\":{\"type\":\"悬念预知/截断/选择困境/危机升级/身份即将揭露/情感转折/信息差/对话悬念/收获预告/反派逼近/无\",\"desc\":\"章末钩子是什么\"},\"debts\":[\"本章向读者许下、还没兑现的期待\"],\"comment\":\"一句话点评节奏\"}
tension 是 0-10 的整数：0-3 平淡过渡，4-6 有推进，7-8 紧张或爽快，9-10 全书级高潮。没有爽点时 cool_points 为空数组。";

const VOLUME_SUMMARY: &str = "下面是「{{volume_title}}」各章的摘要。请压缩成 300-500 字的卷摘要：保留主线事件、人物关系和状态的关键变化、尚未回收的伏笔；直接输出摘要。

{{summaries}}";

const STYLE_PROFILE: &str = "下面是作者提供的样章和本地统计的文风数据。请总结这段文字的文风，写成可以直接放进“文风要求”的条目，供 AI 模仿。

【统计】
{{stats}}

【样章】
{{sample}}

要求：分条写，每条一句话，覆盖：叙述视角和人称、句子长短和节奏、对话占比和对话风格、用词偏好（口语还是书面、是否爱用成语）、描写的侧重（动作/心理/环境）、情绪基调、段落习惯。8 条以内，不要空泛的形容，直接输出条目。";

const FIRST_READ: &str = "你是一个第一次看这本书的普通读者，平时在{{platform}}上看{{genre}}小说。下面是其中一章，你没有看过任何设定和大纲。请像真实读者一样说说读后感：
1. 读到哪里开始被吸引，哪里觉得拖沓、想跳过（引用原文片段）
2. 哪些地方没看懂，人物关系或设定让你困惑
3. 最喜欢的一处和最出戏的一处
4. 看完这章还想不想点下一章，为什么
说人话，不要写成编辑报告。

【{{chapter}}】
{{content}}";

const REVISION_PLAN: &str = "{{context}}

【{{chapter}}】
{{content}}

{{feedback_block}}

请把这一章的问题整理成一份修订计划：按优先级列出 3～7 条，每条写清：要改的位置（引用原文开头几个字）、问题是什么、具体怎么改（给出方向或示例句）。先解决影响追读和逻辑的问题，再处理文字细节。不要重写整章。";

const SIMULATE: &str = "{{context}}

{{characters_block}}

请站在每个主要人物自己的立场推演接下来的发展：
1. 对每个人物写：此刻最想要什么，知道什么、不知道什么，下一步最可能做什么，会和谁发生冲突。人物只能根据自己知道的信息行动，不能突然降智，也不能未卜先知。
2. 综合这些行动，给出接下来 3～5 章可能出现的 3 个剧情走向，每个写清冲突、爽点和章末钩子。{{instruction_block}}";

const GHOST: &str = "{{context}}

【本章】{{chapter}}
{{outline_block}}
{{beats_block}}

【正文（截至光标处）】
{{before}}

{{after_block}}

请从光标处接着写{{size}}。{{instruction_block}}
要求：紧接光标前的最后一个字往下写，上一句没写完就先把它写完；不重复前文，不另起开头，不做总结；和光标后已有的内容能自然衔接。只输出续写的文字本身。";

const INSERT: &str = "{{context}}

【本章】{{chapter}}
{{outline_block}}

【光标前的正文】
{{before}}

{{after_block}}

请在光标处写一段：{{goal}}，约 {{words}} 字。{{instruction_block}}
要求：和前后文自然衔接，人物、能力和设定以上面的资料为准，不重复前文。只输出要插入的正文。";

const NAMES: &str = "{{context}}

请为这部小说起名：{{goal}}。{{instruction_block}}
要求：给出 8 个候选，贴合题材和世界观，读起来顺口、好记，避免生僻字和烂大街的网文名字，每个附一句取名思路。
只输出 JSON，格式：{\"names\":[{\"name\":\"名字\",\"note\":\"取名思路\"}]}";

const LIBRARY_ANALYZE: &str = "下面是作者收藏进文风库的一段范文《{{title}}》，作者觉得它写得好。请分析它好在哪里，提炼出可以学习的写法。
{{note_block}}

{{chunks}}

只输出 JSON，格式：
{\"summary\":\"一句话说清这段最值得学的地方\",\"tags\":[\"标签\"],\"techniques\":[\"具体写法，如：动作场面用三到六字的短句连发，一个动作一句\"],\"rhythm\":\"句子长短、段落、快慢切换的特点\",\"dialogue\":\"对话的写法特点，没有对话就写“无”\",\"imitate\":\"仿写时最需要注意的一点\",\"chunk_tags\":{\"1\":[\"对话\"]}}
tags 和 chunk_tags 只能从这些标签里选：{{tag_list}}。techniques 写 3～6 条；chunk_tags 给每个编号片段选 1～3 个标签。不要“语言生动”“画面感强”这类空泛的夸奖，要说清楚是怎么做到的。";

const STYLE_DISTILL: &str = "你在帮一位网文作者整理他自己的《文风指南》。这份指南会放进每次 AI 写作的提示词里，让 AI 写出作者想要的味道。

{{current_block}}

【作者收藏的范文和分析（共 {{count}} 篇）】
{{items}}

【范文的统计数据（{{sample_size}}）】
{{stats}}

{{feedback_block}}

{{instruction_block}}

请{{action}}这份文风指南，要求：
1. 分成这几节：叙事（视角、距离、信息怎么放出来）、节奏（句长、段落、快慢切换）、对话、描写、情绪与爽点、禁忌（作者不想要的写法）。
2. 每条都是能直接执行的写作指令，尽量带一个简短的示范或反例；不要“文笔优美”“生动形象”这类空话。
3. 作者亲手做的修改（删掉了什么、把什么改成了什么）最能说明他的口味，优先写进去；但要读懂他为什么改，不要从一两处修改推出过宽的禁令。
4. 范文只有几篇时，统计数据和个别写法只能当参考：用“优先”“多用”“少用”写倾向，不要写成“严禁”“必须”“全篇不超过百分之几”这类死规定；只有作者反复改掉的写法才能放进禁忌。
5. 网文离不开对话、人物内心和爽点，不要因为范文里对话少或心理描写少，就限制或禁止对话和心理。
6. 不要照抄范文原句，不要写进范文里的具体人名和情节。
7. 总长度控制在 1500 字以内，直接输出指南正文。";

pub const TEMPLATES: &[Template] = &[
    Template {
        id: "system.writer",
        name: "写作系统提示",
        group: "系统提示",
        description: "续写、写整章、扩写、润色等写正文任务共用的规则",
        vars: &[V_GENRE_BLOCK, ("style_block", "作品设定里的文风要求"), ("sample_block", "作品设定里的样章（截取前 1500 字）")],
        text: WRITER_SYSTEM,
    },
    Template { id: "system.analyst", name: "分析系统提示", group: "系统提示", description: "摘要、提取设定、检查、审稿共用", vars: &[], text: ANALYST_SYSTEM },
    Template { id: "system.editor", name: "策划系统提示", group: "系统提示", description: "开书向导、章纲规划、问编辑共用", vars: &[V_GENRE_BLOCK], text: EDITOR_SYSTEM },
    Template {
        id: "system.chat",
        name: "角色对话",
        group: "系统提示",
        description: "角色对话时让模型扮演人物",
        vars: &[("book_title", "书名"), ("name", "人物名"), ("profile", "人物设定、不可变特征、当前状态"), ("worldview_block", "世界观"), ("related_block", "相关人物和设定")],
        text: CHAT_SYSTEM,
    },
    Template {
        id: "task.continue",
        name: "续写",
        group: "写作",
        description: "从光标处往下写",
        vars: &[V_CONTEXT, V_CHAPTER, V_OUTLINE, V_BEATS, ("before", "光标前的正文（最多 3000 字）"), ("after_block", "光标后已有的内容"), V_WORDS, V_INSTRUCTION],
        text: CONTINUE,
    },
    Template {
        id: "task.ghost",
        name: "灰字续写",
        group: "写作",
        description: "编辑器里按 Ctrl+Enter 时，在光标处给出一两句灰色的续写建议",
        vars: &[V_CONTEXT, V_CHAPTER, V_OUTLINE, V_BEATS, ("before", "光标前的正文（最多 2000 字）"), ("after_block", "光标后已有的内容"), ("size", "长度要求，如“一两句话，约 50 字”"), V_INSTRUCTION],
        text: GHOST,
    },
    Template {
        id: "task.insert",
        name: "斜杠指令",
        group: "写作",
        description: "编辑器里用 / 指令在光标处插入描写、对话、心理、转场等",
        vars: &[V_CONTEXT, V_CHAPTER, V_OUTLINE, ("before", "光标前的正文"), ("after_block", "光标后已有的内容"), ("goal", "指令要写的内容，如“写一段环境描写”"), V_WORDS, V_INSTRUCTION],
        text: INSERT,
    },
    Template {
        id: "task.names",
        name: "起名",
        group: "写作",
        description: "给人物、地点、功法、势力等起名",
        vars: &[V_CONTEXT, ("goal", "要起名的对象"), V_INSTRUCTION],
        text: NAMES,
    },
    Template {
        id: "task.beats",
        name: "节拍规划",
        group: "写作",
        description: "写整章之前，先把本章拆成场景节拍",
        vars: &[V_CONTEXT, V_CHAPTER, V_OUTLINE, V_GOLDEN, V_FINALE, V_WORDS, V_INSTRUCTION],
        text: BEATS,
    },
    Template {
        id: "task.write_chapter",
        name: "写整章",
        group: "写作",
        description: "按章纲和节拍写出整章",
        vars: &[V_CONTEXT, V_CHAPTER, V_OUTLINE, V_BEATS, V_GOLDEN, V_FINALE, V_WORDS, V_INSTRUCTION, ("beats_rule", "有节拍时要求按节拍写，否则要求按章纲推进"), ("outline_rule", "有章纲时要求严格照本章章纲写，否则按前情推进")],
        text: WRITE_CHAPTER,
    },
    Template {
        id: "task.expand",
        name: "扩写",
        group: "写作",
        description: "扩写选中的段落",
        vars: &[V_CONTEXT, ("before_block", "选区前文"), ("selection", "选中的段落"), ("after_block", "选区后文"), V_WORDS, V_INSTRUCTION],
        text: EXPAND,
    },
    Template {
        id: "task.shorten",
        name: "缩写",
        group: "写作",
        description: "精简选中的段落",
        vars: &[V_CONTEXT, ("before_block", "选区前文"), ("selection", "选中的段落"), ("after_block", "选区后文"), V_WORDS, V_INSTRUCTION],
        text: SHORTEN,
    },
    Template {
        id: "task.rewrite",
        name: "改写",
        group: "写作",
        description: "按要求改写选中的段落",
        vars: &[V_CONTEXT, ("before_block", "选区前文"), ("selection", "选中的段落"), ("after_block", "选区后文"), ("goal", "改写要求（没填时用默认要求）")],
        text: REWRITE,
    },
    Template {
        id: "task.polish",
        name: "润色",
        group: "写作",
        description: "润色选中的段落",
        vars: &[V_CONTEXT, ("before_block", "选区前文"), ("selection", "选中的段落"), ("after_block", "选区后文"), V_INSTRUCTION],
        text: POLISH,
    },
    Template {
        id: "task.deslop",
        name: "技能修订（去AI味）",
        group: "写作",
        description: "按选定的写作技能和本地检测结果修订整章或选中的段落；技能说明会附在系统提示后面",
        vars: &[
            V_CONTEXT,
            ("before_block", "选区前文（修订整章时为空）"),
            ("selection", "要修订的正文"),
            ("after_block", "选区后文（修订整章时为空）"),
            ("issues", "本地检测到的 AI 味问题和等级"),
            ("keep_block", "正文里出现的设定名、境界、关键数量等必须原样保留的词"),
            ("words", "要修订的正文字数"),
            ("min_words", "修订稿的字数下限（按 AI 味等级允许删减 15%/25%/35%）"),
            V_INSTRUCTION,
        ],
        text: DESLOP,
    },
    Template {
        id: "task.free",
        name: "问问编辑",
        group: "写作",
        description: "带着上下文讨论剧情",
        vars: &[V_CONTEXT, ("chapter_block", "当前章节"), V_OUTLINE, ("recent_block", "当前正文末尾"), ("question", "作者的问题")],
        text: FREE,
    },
    Template { id: "task.ideas", name: "开书方案", group: "开书", description: "根据创意给出 3 个开书方案", vars: &[V_PLATFORM, ("genre", "题材"), V_BRIEF, V_GENRE_BLOCK], text: IDEAS },
    Template { id: "task.world", name: "世界观", group: "开书", description: "生成世界观", vars: &[V_BRIEF, V_GENRE_BLOCK, V_PLATFORM], text: WORLD },
    Template { id: "task.characters", name: "主要人物", group: "开书", description: "设计主要人物", vars: &[V_BRIEF], text: CHARACTERS },
    Template { id: "task.outline", name: "总纲", group: "开书", description: "生成全书总纲", vars: &[V_BRIEF, V_GENRE_BLOCK, V_PLATFORM], text: OUTLINE },
    Template { id: "task.synopsis", name: "作品简介", group: "开书", description: "写平台展示用的简介", vars: &[V_BRIEF], text: SYNOPSIS },
    Template {
        id: "task.volume_outline",
        name: "卷纲",
        group: "开书",
        description: "细化某一卷的卷纲",
        vars: &[V_BRIEF, ("volume_title", "卷名"), ("existing_block", "已有的卷纲草稿")],
        text: VOLUME_OUTLINE,
    },
    Template {
        id: "task.chapter_outlines",
        name: "章纲规划",
        group: "规划",
        description: "接着最后一章规划后续章纲",
        vars: &[V_CONTEXT, ("recent_block", "最近几章的摘要或章纲"), ("start", "起始章号"), ("end", "结束章号"), ("count", "章数"), V_INSTRUCTION],
        text: CHAPTER_OUTLINES,
    },
    Template { id: "task.summarize", name: "章节摘要", group: "分析", description: "定稿时生成本章摘要", vars: &[V_CHAPTER, V_CONTENT], text: SUMMARIZE },
    Template {
        id: "task.extract",
        name: "提取设定变化",
        group: "分析",
        description: "定稿时找出人物状态变化和伏笔",
        vars: &[("entries", "已有设定列表"), ("threads", "未回收伏笔列表"), V_CHAPTER, V_CONTENT],
        text: EXTRACT,
    },
    Template { id: "task.check", name: "一致性检查", group: "分析", description: "核对本章和设定、前情是否矛盾", vars: &[V_CONTEXT, V_CHAPTER, V_CONTENT], text: CHECK },
    Template {
        id: "task.review",
        name: "编辑审稿",
        group: "分析",
        description: "从追读角度给本章打分",
        vars: &[V_PLATFORM, ("info", "作品信息"), V_OUTLINE, V_CHAPTER, V_CONTENT],
        text: REVIEW,
    },
    Template { id: "task.tension", name: "张力分析", group: "分析", description: "定稿时分析爽点、钩子和张力，画全书曲线", vars: &[V_OUTLINE, V_CHAPTER, V_CONTENT], text: TENSION },
    Template { id: "task.volume_summary", name: "卷摘要", group: "分析", description: "把一卷的章摘要压缩成卷摘要", vars: &[("volume_title", "卷名"), ("summaries", "各章摘要")], text: VOLUME_SUMMARY },
    Template { id: "task.style_profile", name: "文风提取", group: "分析", description: "从样章总结文风要求", vars: &[("stats", "本地统计的文风数据"), ("sample", "样章")], text: STYLE_PROFILE },
    Template {
        id: "task.first_read",
        name: "冷读者反馈",
        group: "分析",
        description: "假装第一次读这本书的读者，只看本章给读后感",
        vars: &[V_PLATFORM, ("genre", "题材"), V_CHAPTER, V_CONTENT],
        text: FIRST_READ,
    },
    Template {
        id: "task.revision_plan",
        name: "修订计划",
        group: "分析",
        description: "把本章问题整理成按优先级排序的修改清单",
        vars: &[V_CONTEXT, V_CHAPTER, V_CONTENT, ("feedback_block", "作者贴进来的审稿、检查结果或自己的想法")],
        text: REVISION_PLAN,
    },
    Template {
        id: "task.simulate",
        name: "人物推演",
        group: "规划",
        description: "让主要人物按各自目标和已知信息推演下一步，给章纲找素材",
        vars: &[V_CONTEXT, ("characters_block", "主要人物的设定、当前状态和关系"), V_INSTRUCTION],
        text: SIMULATE,
    },
    Template {
        id: "task.library_analyze",
        name: "范文分析",
        group: "文风库",
        description: "分析收藏的范文好在哪里，提炼写法要点和场景标签",
        vars: &[("title", "范文标题"), ("note_block", "作者的收藏理由"), ("chunks", "编号的范文片段"), ("tag_list", "可选的场景标签")],
        text: LIBRARY_ANALYZE,
    },
    Template {
        id: "task.style_distill",
        name: "提炼文风指南",
        group: "文风库",
        description: "综合范文分析、统计和作者对 AI 文字的修改，更新文风指南",
        vars: &[
            ("current_block", "现有的指南"),
            ("count", "范文篇数"),
            ("items", "范文和分析"),
            ("stats", "范文的统计数据"),
            ("sample_size", "范文的篇数和字数，样本少时会注明只能参考"),
            ("feedback_block", "作者对 AI 文字的修改和删除"),
            V_INSTRUCTION,
            ("action", "“写出”或“在现有基础上更新”"),
        ],
        text: STYLE_DISTILL,
    },
];

pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// 取模板文本：作者改过就用改过的版本。
pub fn text_of<'a>(id: &str, overrides: &'a HashMap<String, String>) -> std::borrow::Cow<'a, str> {
    match overrides.get(id).filter(|t| !t.trim().is_empty()) {
        Some(t) => std::borrow::Cow::Borrowed(t.as_str()),
        None => std::borrow::Cow::Borrowed(template(id).map(|t| t.text).unwrap_or("")),
    }
}

pub fn render(text: &str, vars: &HashMap<&str, String>) -> String {
    static VAR: OnceLock<Regex> = OnceLock::new();
    static BLANK: OnceLock<Regex> = OnceLock::new();
    let var = VAR.get_or_init(|| Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_\-]*(?::[A-Za-z0-9_\-]+)?)\s*\}\}").unwrap());
    let out = var.replace_all(text, |c: &Captures| {
        let key = &c[1];
        match key.strip_prefix("craft:") {
            Some(id) => crate::library::craft_text(id).unwrap_or_default(),
            None => vars.get(key).cloned().unwrap_or_default(),
        }
    });
    let blank = BLANK.get_or_init(|| Regex::new(r"\n[ \t]*\n(?:[ \t]*\n)+").unwrap());
    blank.replace_all(&out, "\n\n").trim().to_string()
}

pub fn render_id(id: &str, vars: &HashMap<&str, String>, overrides: &HashMap<String, String>) -> String {
    render(&text_of(id, overrides), vars)
}

pub fn platform_name(p: &str) -> &'static str {
    match p {
        "fanqie" => "番茄小说",
        "qidian" => "起点中文网",
        _ => "网文平台",
    }
}

/// 有内容时加上【标题】，没内容时返回空，模板里就不会留下空标题。
pub fn titled(label: &str, v: &str) -> String {
    if v.trim().is_empty() { String::new() } else { format!("【{label}】{}", v.trim()) }
}

pub fn titled_block(label: &str, v: &str, max: usize) -> String {
    if v.trim().is_empty() { String::new() } else { format!("【{label}】\n{}", clip(v, max)) }
}

pub fn instruction_block(instruction: &str, label: &str) -> String {
    if instruction.trim().is_empty() { String::new() } else { format!("\n{label}：{}", instruction.trim()) }
}

pub fn golden_hint(n: Option<i64>) -> &'static str {
    match n {
        Some(1) => "这是开篇第一章：前三百字内让主角登场并卷入冲突，尽早亮出核心卖点或金手指；金手指先写能干什么、要付什么代价，来历和分级以后再说。世界观只写主角此刻碰得到的部分，在冲突里带出来，不要大段交代；新名词最多三个，每个都绑在动作或后果上；不写境界大全、地图介绍、势力图谱。",
        Some(2) => "这是第二章：在第一章冲突的基础上让主角做出第一次有效行动或反击，给出一个小爽点，同时抛出更大的悬念。",
        Some(3) => "这是第三章：完成第一个小高潮，明确主角的目标和接下来的主线方向，章末留强钩子。",
        _ => "",
    }
}

#[derive(Default)]
pub struct Brief {
    pub title: String,
    pub genre: String,
    pub platform: String,
    pub idea: String,
    pub protagonist: String,
    pub logline: String,
    pub worldview: String,
    pub characters: String,
    pub outline: String,
}

impl Brief {
    pub fn render(&self) -> String {
        let platform = if self.platform.is_empty() { String::new() } else { platform_name(&self.platform).to_string() };
        [
            titled("书名", &self.title),
            titled("题材", &self.genre),
            titled("目标平台", &platform),
            titled("核心创意", &self.idea),
            titled("主角设定", &self.protagonist),
            titled("一句话梗概", &self.logline),
            titled_block("世界观", &self.worldview, 2500),
            titled_block("主要人物", &self.characters, 2000),
            titled_block("总纲", &self.outline, 3000),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_templates_and_overrides() {
        let mut vars = HashMap::new();
        vars.insert("chapter", "第1章 开端".to_string());
        vars.insert("content", "正文".to_string());
        let out = render_id("task.summarize", &vars, &HashMap::new());
        assert!(out.contains("【第1章 开端】\n正文"));
        let overrides: HashMap<String, String> = [("task.summarize".to_string(), "摘要：{{chapter}}{{missing}}".to_string())].into();
        assert_eq!(render_id("task.summarize", &vars, &overrides), "摘要：第1章 开端");
        assert!(render("{{craft:cool-points}}", &HashMap::new()).contains("爽点"));
    }

    #[test]
    fn every_template_is_unique_and_documents_its_vars() {
        let re = Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_\-]*)\s*\}\}").unwrap();
        for (i, t) in TEMPLATES.iter().enumerate() {
            assert!(TEMPLATES[i + 1..].iter().all(|o| o.id != t.id), "重复的模板 {}", t.id);
            for c in re.captures_iter(t.text) {
                assert!(t.vars.iter().any(|(k, _)| *k == &c[1]), "模板 {} 用了未声明的变量 {}", t.id, &c[1]);
            }
        }
    }

    #[test]
    fn collapses_empty_blocks() {
        let mut vars = HashMap::new();
        vars.insert("context", "上下文".to_string());
        vars.insert("chapter", "第2章".to_string());
        let out = render_id("task.continue", &vars, &HashMap::new());
        assert!(!out.contains("\n\n\n"));
    }
}
