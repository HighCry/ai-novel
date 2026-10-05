# 第三方内容声明

本项目以 **GNU Affero General Public License v3.0 或更新版本（AGPL-3.0-or-later）** 发布，全文见 [LICENSE](LICENSE)。项目中包含或改编了以下开源项目的内容，原项目的许可证文本放在 `third_party/licenses/` 目录下。

## 直接收录的内容

| 文件 | 来源 | 许可证 | 说明 |
| --- | --- | --- | --- |
| `assets/genres.json` | [qianmengnet/StoryForge](https://github.com/qianmengnet/StoryForge) `templates/genres.json` | MIT（见下方说明） | 37 个网文题材模板，原样收录 |
| `assets/craft/cool-points.md` | [lingfengQAQ/webnovel-writer](https://github.com/lingfengQAQ/webnovel-writer) `references/shared/cool-points-guide.md` | GPL-3.0 | 爽点设计指南，原样收录 |
| `assets/craft/chapter-planning.md` | webnovel-writer `skills/webnovel-plan/references/outlining/chapter-planning.md` | GPL-3.0 | 章节规划，原样收录 |
| `assets/craft/conflict-design.md` | webnovel-writer `skills/webnovel-plan/references/outlining/conflict-design.md` | GPL-3.0 | 冲突设计，原样收录 |
| `assets/craft/strand-weave.md` | webnovel-writer `references/shared/strand-weave-pattern.md` | GPL-3.0 | 多线交织，原样收录 |
| `assets/craft/polish-guide.md` | webnovel-writer `skills/webnovel-write/references/polish-guide.md` | GPL-3.0 | 润色指南，原样收录 |
| `assets/craft/hooks-chapter.md` | [Xiaoyangy/novel-studio](https://github.com/Xiaoyangy/novel-studio) `skills/story-long-write/references/hooks-chapter.md` | Apache-2.0 | 章首章尾钩子，原样收录 |
| `assets/craft/hooks-suspense.md` | novel-studio `.../hooks-suspense.md` | Apache-2.0 | 悬念设计，原样收录 |
| `assets/craft/natural-prose.md` | novel-studio `.../anti-ai-writing.md` | Apache-2.0 | 自然文笔与 AI 腔自查指南，原样收录 |
| `assets/craft/style-craft.md` | novel-studio `.../style-craft.md` | Apache-2.0 | 文风技法，原样收录 |
| `assets/craft/emotion-system.md` | novel-studio `.../plot-emotion-system.md` | Apache-2.0 | 情绪节奏体系，原样收录 |
| `assets/craft/combat-face.md` | novel-studio `.../style-combat-face.md` | Apache-2.0 | 战斗与打脸写法，原样收录 |
| `assets/craft/genre-formulas.md` | novel-studio `.../genre-writing-formulas.md` | Apache-2.0 | 题材写作公式，原样收录 |

## 改编的设计和规则

| 本项目位置 | 参考来源 | 许可证 | 改编内容 |
| --- | --- | --- | --- |
| `src/continuity.rs` | [mrigankad/Novel-OS](https://github.com/mrigankad/Novel-OS) `core/continuity_engine.py` | MIT | 确定性连续性检查的检查项和阈值（伏笔沉寂、人物缺席、已死亡人物出场、人物信息单薄），用 Rust 重新实现 |
| `src/lint.rs` | novel-studio `anti-ai-writing.md` | Apache-2.0 | 高频套话、弱化副词密度、意义膨胀、万能结论、论文体、书面连词、解释腔/上帝视角等检查规则 |
| `src/lint.rs` | [worldwonderer/oh-story-claudecode](https://github.com/worldwonderer/oh-story-claudecode) `skills/story-deslop/references/banned-words.md`、`SKILL.md` | MIT | 一级禁用词、“不是A而是B”等高频模板句（只查引号外叙述）、章末预告检测，以及按每千字命中数分轻度/中度/重度的阈值和对应的删减上限 |
| `src/skills/deslop.md`（内置技能「网文去AI味」） | oh-story-claudecode `skills/story-deslop/`（`SKILL.md`、`references/anti-ai-writing.md`、`references/banned-words.md`）；novel-studio `anti-ai-writing.md` | MIT / Apache-2.0 | 写作规则（深度限知视角、逗号长句的句长基准、禁用句式和词表）、去 AI 三遍法、防止矫枉过正的条目和改写范例，按本项目的用法重写和精简 |
| `src/prompts.rs` | webnovel-writer `agents/reviewer.md`、`references/shared/cool-points-guide.md`；novel-studio `hooks-chapter.md` | GPL-3.0 / Apache-2.0 | 一致性检查的五个类别与“只报有证据的问题”原则；节拍规划中的爽点三段式、章末钩子类型；章节张力分析的爽点与钩子分类 |
| 人物结构化状态 | StoryForge 角色动态状态 6 字段 | MIT | 字段设计（位置、实力、身体、心理、关键物品、近期经历），另加“知道的秘密 / 还不知道的事” |
| `src/prompts.rs`（写作系统提示第 10 条「信息投放」、第 1 章开篇要求） | [miserylee/webnovel-handbook](https://github.com/miserylee/webnovel-handbook) `docs/storycraft/28-worldbuilding-setting-exposition.md`、`docs/core-writing/69-pov-narrative-distance-information-focus.md` | MIT | 冰山原则（作者知道十成、正文先露一成）、设定随行动和后果带出、第一章新名词不超过三个、不写等级大全和金手指说明书、旁白不替视角人物说破，按本项目的提示词风格重写 |
| `src/logic.rs`（新名词过多、大段纯设定） | webnovel-handbook `docs/storycraft/28-worldbuilding-setting-exposition.md` | MIT | 每章新名词上限和“设定扎堆、没有动作和对话”的判断思路，用 Rust 重新实现，阈值按本项目调整 |

## 关于 StoryForge 的许可证

StoryForge 的 README 声明 “MIT License”，但仓库（2026-05-17 的提交 `f521876`）根目录没有 LICENSE 文件，根目录 `package.json` 标注的是 ISC。MIT 和 ISC 都是宽松许可证，与 AGPL-3.0 兼容。按 MIT 的要求，我们在 `third_party/licenses/StoryForge-LICENSE.txt` 里保留了版权与许可声明。如果原作者对许可证有其他说明，以原作者为准。

## 只借鉴思路、没有使用其代码或文本的项目

[blader/humanizer](https://github.com/blader/humanizer)（MIT，借鉴“改完后再自问哪里还像 AI”的复查思路）、[sam-paech/slop-score](https://github.com/sam-paech/slop-score)（按密度而不是分类器衡量 AI 腔的思路）、[MaoXiaoYuZ/Long-Novel-GPT](https://github.com/MaoXiaoYuZ/Long-Novel-GPT)（未声明许可证）、[aiyinluya/wenmai](https://github.com/aiyinluya/wenmai)（未附许可证）、[QishanHe/PlotPilot](https://github.com/QishanHe/PlotPilot)（Apache-2.0 + Commons Clause，与 AGPL 不兼容），以及 Novelcrafter、Sudowrite 等商业产品。

`src/visibility.rs` 的可见性设计借鉴了 Novelcrafter 的 Progressions（设定按稿件位置生效）和马良写作的「设定可见性」（对 AI 隐藏、按卷渐进开放，可见性先于上下文装配），`src/logic.rs` 的境界、物品检查借鉴了 FactTrack（NAACL 2025）按时间入账再比对的思路，都只参考设计，没有使用其代码或文字。
