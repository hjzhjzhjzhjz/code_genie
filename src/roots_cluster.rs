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

    #[test]
    fn 内嵌表簇数与字根数符合预期() {
        let 簇们 = 内嵌().unwrap();
        assert_eq!(簇们.len(), 92, "原 roots-cluster.txt 是 92 簇");
        let 根数: usize = 簇们.iter().map(|c| c.roots.len()).sum();
        assert_eq!(根数, 257, "原文件共 257 个字根；改数据时同步这条断言");
    }

    #[test]
    fn 一被钉在f键() {
        let 簇们 = 内嵌().unwrap();
        let 一 = 簇们
            .iter()
            .find(|c| c.roots.iter().any(|r| r == "一"))
            .expect("聚类表里应有「一」");
        assert_eq!(一.keys, "f");
        assert_eq!(一.roots, ["一"]);
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
