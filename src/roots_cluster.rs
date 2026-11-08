// =========================================================================
// 🌿 字根聚类表（内嵌 RON）
// =========================================================================
//
// 数据文件：`assets/roots_cluster.ron`，用 `include_str!` 在编译期打进二进制，
// 运行时不需要这个文件。若同目录下存在 `roots-cluster.ron` /
// `roots-cluster-<方案>.ron`，则以文件为准——换方案时临时替换不用重新编译。
//
// 三个消费点都走这里，只有一份解析：
//   - `prepare` 第 8 步：每簇出一条大码约束，写进 `字根约束.txt`
//   - `clusters`：把某一簇压成同一个大码，算重码代价（`--per-row` 报表）
//   - `zigen`：同簇字根在 CSV 里排在一起，决定「类」的划分
//
// 表格式是一行一簇：`"根 根 根"` 或 `("根 根", "键序列")`，簇的顺序即
// `字根约束.txt` 的出约束顺序。数据文件本身只有裸数据、不带注释。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// 编译期内嵌的字根聚类表
const 内嵌聚类表: &str = include_str!("../assets/roots_cluster.ron");

/// 一簇字根：簇内所有字根共用同一个大码，退火时只能整体迁到同一个键
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct 簇 {
    /// 簇内字根，名字与拆分表 / 字根声韵表一致
    pub roots: Vec<String>,
    /// 写死的键序列（如 `f`）；空串表示按字根频率分档自动取键
    pub keys: String,
}

// ── RON 结构：与 assets/roots_cluster.ron 一一对应 ──

#[derive(Deserialize)]
struct 文件 {
    clusters: Vec<条目>,
}

/// 一簇的两种写法，由 RON 的 untagged 反序列化天然区分
#[derive(Deserialize)]
#[serde(untagged)]
enum 条目 {
    /// `"又 ス マ 癶"` —— 只有字根，键位自动分档
    仅根(String),
    /// `("一", "f")` —— 字根串 + 钉死的键位
    带键(String, String),
}

/// 解析一份 RON 文本，摊平成「有顺序的簇列表」
///
/// 保留文件里的簇顺序：`prepare` 按此顺序写 `字根约束.txt`，
/// `zigen` 按此顺序排「类」，顺序变了产物就会跟着变。
fn 解析(文本: &str, 来源: &str) -> Result<Vec<簇>, String> {
    let 文档: 文件 = ron::from_str(文本).map_err(|e| format!("解析字根聚类表 {来源} 失败：{e}"))?;
    let mut 出 = Vec::with_capacity(文档.clusters.len());
    for (序号, 条) in 文档.clusters.into_iter().enumerate() {
        let (根串, keys) = match 条 {
            条目::仅根(s) => (s, String::new()),
            条目::带键(s, k) => (s, k),
        };
        let roots: Vec<String> = 根串.split_whitespace().map(str::to_string).collect();
        if roots.is_empty() {
            return Err(format!(
                "字根聚类表 {来源} 的第 {} 簇没有任何字根",
                序号 + 1
            ));
        }
        出.push(簇 { roots, keys });
    }
    if 出.is_empty() {
        return Err(format!("字根聚类表 {来源} 里没有任何簇"));
    }
    Ok(出)
}

/// 内嵌聚类表（无条件可用）
pub fn 内嵌() -> Result<Vec<簇>, String> {
    let 出 = 解析(内嵌聚类表, "内嵌 assets/roots_cluster.ron")?;
    // 同一字根出现在多簇里会让 `prepare` 第 8 步报「root in multiple clusters」，
    // 但那时已经跑了一半；在这里早报，指到具体字根。
    let mut 见过: HashSet<&str> = HashSet::new();
    for 簇 in &出 {
        for 根 in &簇.roots {
            if !见过.insert(根.as_str()) {
                return Err(format!("字根聚类表里字根 {根} 出现在多个簇中"));
            }
        }
    }
    Ok(出)
}

/// 加载聚类表：路径给了且文件存在 → 读该 RON；否则 → 内嵌表
pub fn 加载(路径: Option<&Path>) -> Result<Vec<簇>, String> {
    match 路径 {
        Some(p) if p.is_file() => {
            let 文本 = fs::read_to_string(p)
                .map_err(|e| format!("无法读取字根聚类表 {}：{e}", p.display()))?;
            解析(&文本, &p.display().to_string())
        }
        _ => 内嵌(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数据快照：改 `assets/roots_cluster.ron` 后这里会失败，属预期提醒——
    /// 确认新数据无误，就把下面两个数字改成实测值即可。
    #[test]
    fn 内嵌表簇数与字根数符合预期() {
        let 簇们 = 内嵌().unwrap();
        assert_eq!(簇们.len(), 90, "当前 roots_cluster.ron 是 90 簇");
        let 根数: usize = 簇们.iter().map(|c| c.roots.len()).sum();
        assert_eq!(根数, 253, "当前文件共 253 个字根；改数据时同步这条断言");
        // 现表全部按字根频率分档取键，没有写死的键位；
        // 若将来特意把某簇钉到某个键上（`("一", "f")` 那种写法），把这条放宽即可。
        assert!(
            簇们.iter().all(|c| c.keys.is_empty()),
            "内嵌表已全部分档取键，不应出现写死的键位：{:?}",
            簇们
                .iter()
                .filter(|c| !c.keys.is_empty())
                .map(|c| (&c.roots, &c.keys))
                .collect::<Vec<_>>()
        );
    }

    /// 钉键写法 `("一", "f")` 仍要能解析——`prepare` 第 8 步遇到非空 keys 会直接用，
    /// 所以即使当前数据表不钉键，这条代码路径也得有覆盖。
    #[test]
    fn 钉键条目解析为固定键位() {
        let 簇们 = 解析(r#"(clusters: [("一", "f"), "ス マ"])"#, "测试").unwrap();
        assert_eq!(簇们.len(), 2);
        assert_eq!(簇们[0].roots, ["一"]);
        assert_eq!(簇们[0].keys, "f");
        // 只给字根的那种写法，keys 落空 → 由 prepare 按频率分档
        assert_eq!(簇们[1].roots, ["ス", "マ"]);
        assert!(簇们[1].keys.is_empty());
    }

    #[test]
    fn 注释行不吞后续数据() {
        let 簇们 = 内嵌().unwrap();
        let 全部: Vec<&str> = 簇们
            .iter()
            .flat_map(|c| c.roots.iter().map(String::as_str))
            .collect();
        // 原文 `#讠` 那行是注释，紧跟其后的「之 辶 ⻍」必须照常成为一簇
        assert!(全部.contains(&"之"), "「#讠」注释吞掉了后面的簇");
        // 键位分组标题（`// Q`）不进数据
        for 根 in &全部 {
            assert!(!根.starts_with('#'), "字根里混进了注释：{根}");
            assert!(!根.contains('#'), "字根里混进了注释：{根}");
        }
    }

    #[test]
    fn 读文件与内嵌一致() {
        let p = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/roots_cluster.ron"
        ));
        assert_eq!(加载(Some(p)).unwrap(), 内嵌().unwrap());
    }

    #[test]
    fn 路径缺失回落内嵌() {
        let p = Path::new("不存在的路径/roots-cluster.ron");
        assert_eq!(加载(Some(p)).unwrap(), 内嵌().unwrap());
        assert_eq!(加载(None).unwrap(), 内嵌().unwrap());
    }

    #[test]
    fn 坏格式报错而不是静默空表() {
        let e = 解析("(clusters: [)", "测试").unwrap_err();
        assert!(e.contains("解析字根聚类表"), "错误信息应指明来源：{e}");
        let e = 解析("(clusters: [])", "测试").unwrap_err();
        assert!(e.contains("没有任何簇"), "{e}");
        let e = 解析("(clusters: [\"   \"])", "测试").unwrap_err();
        assert!(e.contains("没有任何字根"), "{e}");
    }
}
