你是一位世界观与人物设定的行家，为下面这本新书搭建设定。

【书名】《{{title}}》
{{genre_line}}{{extras}}【主线】{{premise}}
【主题】{{theme}}

只输出一个 JSON 对象（字符串内的引号用「」），字段：
- concepts：[{name, category, summary}] 3-6 项基础概念（世界规则/纪元/种族等）
- systems：[{name, kind, levels, rules, summary}] 0-3 项力量体系（kind 取 cultivation/magic/technology/power；levels 写等级链；rules 写规则与代价）
- locations：[{name, kind, parent, features, summary}] 3-8 个地点，覆盖世界地理与剧情发生地（kind 取 continent/nation/city/realm/landform；大区域在前、具体地点在后；parent 填上级地点名）
- organizations：[{name, kind, leader, territory, ideology, summary}] 0-4 个组织势力（kind 取 sect/nation/clan/guild/organization）
- eras：[纪元名] 0-3 个（故事内历法）
- characters：[{name, role, gender, occupation, personality, background, aliases:[别名]}] 恰好 {{n_chars}} 个主要角色（role 取 protagonist/love_interest/antagonist/mentor/support；必须含 1 名主角与 1 名主要反派）
- relations：[{from, to, relation}] 角色间关系（from/to 用角色名；relation 如 师徒/宿敌/兄妹）
- threads：[{title, description, priority}] 2-5 条叙事线索（伏笔/悬念；priority 取 main/minor）

命名一致性（硬性要求）：每个条目的 name 是唯一权威名称——任何字段（summary、features、rules、levels、threads 的 description、relations 等）提到地点、势力、角色、概念时，必须与对应清单的 name 逐字相同，严禁出现清单外的同义变体（例如 locations 是「奥朗王都」，别处却写「鲁恩」）。主线文本里已有的专名可沿用；若你另起更贴切的新名，则所有字段统一用新名，绝不混用。