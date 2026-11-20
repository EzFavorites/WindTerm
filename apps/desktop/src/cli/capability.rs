//! `term-desk capability`：能力面三元组（命令总数 / 已接线 / 置灰）。
//!
//! 数字不在这里定义——`term_app::capability` 是唯一判定点（`Action::parse`
//! 能解析就算接线），这里只负责把它排版成人和脚本都吃得下的两种形态。
//!
//! **JSON 的字段名与 `docs/facts.txt` 完全同名**（`menu_commands` /
//! `commands_wired` / `commands_planned`）。上一版 CLI 的注释也这么写，但实际
//! 输出的是 `commands` / `wired` / `planned`——承诺与实现不一致，脚本要读就得
//! 再翻译一次。这一版把名字对上了，代价是短名字更啰嗦，收益是 CI 可以直接
//! `jq .menu_commands` 与 `check-docs.sh` 对账。

use term_app::action::FeatureId;
use term_app::capability;

pub fn run(json: bool, all: bool) -> i32 {
    if json {
        print!("{}", render_json());
    } else {
        print!("{}", render_text(all));
    }
    0
}

pub fn render_text(all: bool) -> String {
    let rows = capability::inventory();
    let (wired, planned) = capability::counts();
    let mut out = format!(
        "能力面：{} 条命令 / 已接线 {} / 置灰 {}\n",
        rows.len(),
        wired,
        planned
    );
    for feature in FeatureId::ALL {
        let ids: Vec<&str> = rows
            .iter()
            .filter(|c| c.feature == Some(feature))
            .map(|c| c.id)
            .collect();
        out.push_str(&format!("  {}（{}）\n", feature.name(), ids.len()));
        for id in ids {
            out.push_str(&format!("    {id}\n"));
        }
    }
    if all {
        out.push_str(&format!("置灰 {planned} 条：\n"));
        for c in rows.iter().filter(|c| !c.is_implemented()) {
            out.push_str(&format!("    {}\n", c.id));
        }
    } else {
        out.push_str(&format!("置灰 {planned} 条（加 --all 列出）\n"));
    }
    out
}

/// 给脚本用的形态。字段名就是 `docs/facts.txt` 里的键名，省得再翻译一次。
///
/// 命令 ID 与分区名都是本仓库的 ASCII 标识符，不含引号 / 反斜杠 / 控制字符，
/// 所以这里手写 JSON 是安全的；为此拉一个序列化库进来不划算，也会让
/// `apps/*` 多一条依赖（ADR-0020 对「薄」的要求）。
pub fn render_json() -> String {
    let rows = capability::inventory();
    let (wired, planned) = capability::counts();
    let items: Vec<String> = rows
        .iter()
        .map(|c| {
            let feature = match c.feature {
                Some(f) => format!("\"{}\"", f.name()),
                None => "null".to_string(),
            };
            format!(
                "{{\"id\":\"{}\",\"state\":\"{}\",\"feature\":{}}}",
                c.id,
                if c.is_implemented() {
                    "implemented"
                } else {
                    "planned"
                },
                feature
            )
        })
        .collect();
    format!(
        "{{\"menu_commands\":{},\"commands_wired\":{},\"commands_planned\":{},\"items\":[{}]}}\n",
        rows.len(),
        wired,
        planned,
        items.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_and_planned_add_up_to_the_inventory() {
        let (wired, planned) = capability::counts();
        assert_eq!(wired + planned, capability::inventory().len());
    }

    #[test]
    fn text_names_every_feature_and_its_count() {
        let text = render_text(false);
        for feature in FeatureId::ALL {
            assert!(text.contains(&format!("{}（", feature.name())), "缺少分区");
        }
        let (wired, planned) = capability::counts();
        assert!(text.contains(&format!("已接线 {wired} / 置灰 {planned}")));
        // 默认不铺几十行占位，否则这个命令就没法读了。
        assert!(text.contains("--all 列出"));
    }

    #[test]
    fn all_lists_every_planned_command() {
        let text = render_text(true);
        let (_, planned) = capability::counts();
        let listed = text
            .lines()
            .skip_while(|l| !l.starts_with("置灰"))
            .skip(1)
            .count();
        assert_eq!(listed, planned);
    }

    #[test]
    fn json_carries_one_item_per_command() {
        let json = render_json();
        let items = json.matches("{\"id\":\"").count();
        assert_eq!(items, capability::inventory().len());
        let (wired, planned) = capability::counts();
        assert!(json.contains(&format!("\"commands_wired\":{wired}")));
        assert!(json.contains(&format!("\"commands_planned\":{planned}")));
        assert_eq!(json.matches("\"state\":\"implemented\"").count(), wired);
        assert_eq!(json.matches("\"state\":\"planned\"").count(), planned);
    }

    /// 这条钉的是「承诺兑现」：字段名必须与 facts.txt 同名，而不是另一套短名字。
    #[test]
    fn json_keys_match_the_facts_baseline() {
        let json = render_json();
        for key in ["menu_commands", "commands_wired", "commands_planned"] {
            assert!(
                json.contains(&format!("\"{key}\":")),
                "缺少 facts 同名键 {key}"
            );
        }
        // 旧的短名字不许回来：脚本按 facts 的名字读，两套名字就是两个口径。
        assert!(!json.contains("\"wired\":"));
        assert!(!json.contains("\"planned\":"));
        assert!(!json.contains("\"commands\":"));
    }
}
