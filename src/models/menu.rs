//! 后台菜单（对齐 Go 版 `config.DefaultMenuGroups`）。
//!
//! 菜单是静态配置，这里把 Go 版的数组原样导出成 `menu.json` 编译期嵌入
//! （`include_str!`，没有运行时 IO），两边菜单结构因此不会各写一份而漂移。
//! Go 侧改了菜单，重新导出一次即可。

use serde_json::Value;

/// Go 版 `config.DefaultMenuGroups` 的导出结果（字段：key / name / menus[]，
/// 每个菜单项是 group_key / name / path；`Backend` 在 Go 里就是 `json:"-"`）。
const MENU_JSON: &str = include_str!("menu.json");

/// 后台菜单分组，顺序即左侧导航顺序。
pub fn default_menu_groups() -> Value {
    serde_json::from_str(MENU_JSON).unwrap_or_else(|_| Value::Array(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exported_menu_groups() {
        let groups = default_menu_groups();
        let list = groups.as_array().expect("菜单应是数组");
        assert!(list.len() >= 8, "菜单分组只有 {} 个", list.len());

        for group in list {
            let key = group["key"].as_str().expect("分组缺 key");
            assert!(group["name"].as_str().is_some_and(|name| !name.is_empty()));
            let menus = group["menus"].as_array().expect("menus 应是数组");
            assert!(!menus.is_empty(), "分组 {key} 没有菜单项");

            for menu in menus {
                // 注意：Go 版「多站点」分组里菜单项的 group_key 就是 `tool`
                // （数据本身如此），所以这里只校验字段存在，不校验两者相等。
                assert!(
                    menu["group_key"].as_str().is_some_and(|value| !value.is_empty()),
                    "{menu}"
                );
                assert!(menu["path"].as_str().is_some_and(|path| !path.is_empty()));
                assert!(menu["name"].as_str().is_some_and(|name| !name.is_empty()));
                // Go 的 Backend 字段是 json:"-"，不该出现在导出结果里
                assert!(menu.get("backend").is_none(), "{menu}");
            }
        }
    }
}
