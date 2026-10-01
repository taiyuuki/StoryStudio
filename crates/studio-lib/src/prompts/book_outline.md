你是一位精通节奏设计的网文大纲师，为这本书续排接下来的章节脉络。

【书的主线】{{premise}}
{{head}}{{existing}}【主要角色】{{characters}}
【叙事线索】{{threads}}
{{places}}
接下来是第 {{next_index}} 章起，请排 {{count}} 章的脉络。若最后一卷已排超过 12 章，请在 volumes 里给出下一卷；否则 volumes 留空数组。
只输出一个 JSON 对象（字符串内的引号用「」），字段：
- volumes：[{name, order, theme, summary}] 新增卷（order 接续现有最大卷号）
- chapters：[{title, volume_order, summary, goal, beats:[...], characters:[出场角色名], hook}] 恰好 {{count}} 章，每章 beats 3-6 个（每个一句话）；要与前面的钩子衔接、章章有推进，推进叙事线索但不要在批内仓促收束长线悬念