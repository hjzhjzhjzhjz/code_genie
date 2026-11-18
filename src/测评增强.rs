// =========================================================================
// 📊 测评增强：四维度 × tiers 分档统计 + 边际/阶重码分析
// =========================================================================
//
// 借鉴 hanzi-chai（冰雪四拼工具链）的两项测评能力，口径按该项目文档复刻：
//
// 1. 四维度 × tiers：把测评拆成「字/词 × 全码/简码」四维度，每个维度按频率
//    降序取若干档（top 300/1000/5000/全部），各档统计重码组数、重码对象数、
//    平均码长、频率覆盖。原测评的六项指标不分档，头部高频与长尾混在一个均值里，
//    这里补上分档视角。**纯后处理**，不碰宇浩对拍内核与搜索主循环。
//
// 2. 阶重码 / 边际重码：
//    - 零阶：全码投影组，重码数按 Σ(n−1)（非首项数）；
//    - 一阶/二阶：省略末 1/2 码的投影组，按 n²/(2×键数^阶数) 均匀分配估计。
//      两种口径**不可相加**，也不能解释为"实际新增冲突"——前者是非首项计数，
//      后者是概率估计（hanzi-chai README「元素离散性与重码分析」明确此边界）。
//    - 边际：把若干键归并到同键后，重码对象集合的净增（已重码的不重复列）。
//
// 数据来源：码表（字→码）+ 字频 JSON + 词表 TSV + 词频 JSON，全部来自测评数据目录。
// 词编码按「各字编码按序拼接，取前 maxlen 码」。

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 默认分档：按频率降序取前 N 项；0 表示全部。
const 默认档位: [usize; 4] = [300, 1000, 5000, 0];
/// 默认阶重码投影的阶数（对齐 hanzi-chai 的零/一/二阶）。
const 投影阶数: [usize; 3] = [0, 1, 2];

/// 一条带权条目：文本 + 编码 + 词频/字频。
#[derive(Clone, Debug)]
struct 条目 {
    文本: String,
    码: String,
    频: u64,
}

/// 读取码表 → (全码条目, 简码条目)。
/// 码长 4（或 >3）视为全码，2/3 视为简码；`_` 是选重键，计入按键数但投影时剔除。
fn 读码表(路径: &Path) -> Result<(Vec<条目>, Vec<条目>), String> {
    let 文本 = std::fs::read_to_string(路径)
        .map_err(|e| format!("读不了码表 {}: {}", 路径.display(), e))?;
    let mut 全 = Vec::new();
    let mut 简 = Vec::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim_end_matches('\r');
        if 行.is_empty() {
            continue;
        }
        let mut 列 = 行.split('\t');
        let Some(字) = 列.next() else { continue };
        let Some(码) = 列.next() else { continue };
        if 字.chars().count() != 1 || 码.is_empty() {
            continue; // 只要单字行；多字词行留给词维度
        }
        let 条目 = 条目 {
            文本: 字.to_string(),
            码: 码.trim().to_string(),
            频: 0,
        };
        if 码.chars().count() >= 4 {
            全.push(条目);
        } else {
            简.push(条目);
        }
    }
    if 全.is_empty() && 简.is_empty() {
        return Err(format!("码表 {} 无单字条目", 路径.display()));
    }
    Ok((全, 简))
}

/// 读频率 JSON：`{"字或词": 频次, ...}`。文件不存在时返回空表（该维度跳过）。
fn 读频率(路径: &Path) -> HashMap<String, u64> {
    let Ok(文本) = std::fs::read_to_string(路径) else {
        return HashMap::new();
    };
    let Ok(Value::Object(表)) = serde_json::from_str::<Value>(&文本) else {
        return HashMap::new();
    };
    表.into_iter()
        .filter_map(|(k, v)| {
            v.as_u64()
                .or_else(|| v.as_f64().map(|x| x as u64))
                .map(|n| (k, n))
        })
        .collect()
}

/// 读词表 TSV：`词<TAB>频次`。文件不存在时返回空。
fn 读词表(路径: &Path) -> Vec<(String, u64)> {
    let Ok(文本) = std::fs::read_to_string(路径) else {
        return Vec::new();
    };
    文本
        .lines()
        .filter_map(|行| {
            let 行 = 行.trim_end_matches('\r');
            let mut 列 = 行.split('\t');
            let 词 = 列.next()?.to_string();
            let 频 = 列.next()?.trim().parse::<u64>().ok()?;
            (!词.is_empty()).then_some((词, 频))
        })
        .collect()
}

/// 投影键：把编码按阶数省略末 n 码，`_` 选重键剔除；短序列补 `ε` 表示空位。
fn 投影(码: &str, 阶: usize, 全键数: usize) -> String {
    let 有效: Vec<char> = 码.chars().filter(|&c| c != '_').collect();
    let 保留 = 有效.len().saturating_sub(阶);
    let mut 位: Vec<String> = (0..全键数)
        .map(|i| {
            if i < 保留 {
                有效[i].to_string()
            } else if i < 有效.len() {
                "*".to_string() // 被省略的码位 = 通配符
            } else {
                "ε".to_string() // 短序列空位
            }
        })
        .collect();
    // 阶>0 时末 n 位通配；阶=0 时全码位有效，其余补 ε
    if 阶 == 0 {
        位 = (0..全键数)
            .map(|i| {
                有效
                    .get(i)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "ε".to_string())
            })
            .collect();
    }
    位.join("")
}

/// 统计一档的零/一/二阶重码。
/// 零阶 = Σ(组大小−1)；高阶 = Σ(n²/(2×键数^阶))，两者不可相加。
fn 阶重码(条目: &[条目], 键数: usize, 全键数: usize) -> Value {
    let mut 结果 = Map::new();
    for &阶 in &投影阶数 {
        let mut 组: HashMap<String, Vec<&str>> = HashMap::new();
        for 项 in 条目 {
            组.entry(投影(&项.码, 阶, 全键数))
                .or_default()
                .push(&项.文本);
        }
        let 组数 = 组.len();
        let 重码组 = 组.values().filter(|v| v.len() > 1).count();
        let 重码对象: HashSet<&str> = 组
            .values()
            .filter(|v| v.len() > 1)
            .flat_map(|v| v.iter().copied())
            .collect();
        let 估计: f64 = 组
            .values()
            .map(|v| {
                let n = v.len() as f64;
                if 阶 == 0 {
                    n - 1.0
                } else {
                    n * n / (2.0 * (键数 as f64).powi(阶 as i32))
                }
            })
            .sum();
        结果.insert(
            format!("{}阶", 阶),
            json!({
                "投影组数": 组数,
                "重码组数": 重码组,
                "重码对象数": 重码对象.len(),
                "重码估计": (估计 * 1000.0).round() / 1000.0,
            }),
        );
    }
    Value::Object(结果)
}

/// 边际重码：把 `合并组`（如上标键集合）视为同键后，重码对象集合的净增。
/// 口径：已经重码的对象不重复列；只返回「合并后新进入重码组」的对象名。
fn 边际重码(条目: &[条目], 合并组: &[&str], 键数: usize, 全键数: usize) -> Value {
    fn 替换键(码: &str, 合并组: &[&str]) -> String {
        码.chars()
            .filter(|&c| c != '_')
            .map(|c| {
                if 合并组.iter().any(|s| s.starts_with(c)) {
                    '§'
                } else {
                    c
                }
            })
            .collect()
    }
    fn 分组<'a>(
        条目: &'a [条目],
        合并组: &[&str],
        归并: bool,
        全键数: usize,
    ) -> HashMap<String, Vec<&'a str>> {
        let mut 组: HashMap<String, Vec<&'a str>> = HashMap::new();
        for 项 in 条目 {
            let 源 = if 归并 {
                替换键(&项.码, 合并组)
            } else {
                项.码.clone()
            };
            let 键 = 投影(&源, 0, 全键数);
            组.entry(键).or_default().push(项.文本.as_str());
        }
        组
    }
    fn 重码对象<'a>(组: &'a HashMap<String, Vec<&'a str>>) -> HashSet<&'a str> {
        组.values()
            .filter(|v| v.len() > 1)
            .flat_map(|v| v.iter().copied())
            .collect()
    }
    let 原 = 分组(条目, 合并组, false, 全键数);
    let 合 = 分组(条目, 合并组, true, 全键数);
    let 原对象 = 重码对象(&原);
    let 合对象 = 重码对象(&合);
    let mut 新增: Vec<String> = 合对象
        .difference(&原对象)
        .map(|s| (*s).to_string())
        .collect();
    新增.sort();
    json!({
        "合并键": 合并组,
        "原重码对象数": 原对象.len(),
        "合并后重码对象数": 合对象.len(),
        "新增对象数": 新增.len(),
        "新增对象示例": 新增.iter().take(20).collect::<Vec<_>>(),
        "键数": 键数,
    })
}

/// 一个维度的分档统计结果。
fn 统计维度(
    名: &str, 全: &[条目], 简: &[条目], 键数: usize, 全键数: usize
) -> Value {
    let mut 维度 = Map::new();
    for (维度名, 集) in [("全码", 全), ("简码", 简)] {
        if 集.is_empty() {
            continue;
        }
        let mut 排序: Vec<&条目> = 集.iter().collect();
        排序.sort_by(|a, b| b.频.cmp(&a.频).then_with(|| a.文本.cmp(&b.文本)));
        let 总频: u64 = 排序.iter().map(|x| x.频).sum();
        let mut 档们 = Vec::new();
        for &上限 in &默认档位 {
            let 取 = if 上限 == 0 {
                排序.len()
            } else {
                排序.len().min(上限)
            };
            let 样本: Vec<条目> = 排序[..取].iter().map(|x| (*x).clone()).collect();
            let 频和: u64 = 样本.iter().map(|x| x.频).sum();
            let 平均分码长 = if 样本.is_empty() {
                0.0
            } else {
                样本
                    .iter()
                    .map(|x| x.码.chars().filter(|&c| c != '_').count())
                    .sum::<usize>() as f64
                    / 样本.len() as f64
            };
            档们.push(json!({
                "档": if 上限 == 0 { "全部".to_string() } else { format!("top{}", 上限) },
                "条目数": 样本.len(),
                "频率覆盖": if 总频 > 0 { (频和 as f64 / 总频 as f64 * 10000.0).round() / 100.0 } else { 0.0 },
                "平均码长": (平均分码长 * 1000.0).round() / 1000.0,
                "阶重码": 阶重码(&样本, 键数, 全键数),
            }));
        }
        维度.insert(
            维度名.to_string(),
            json!({
                "条目总数": 集.len(),
                "分档": 档们,
            }),
        );
    }
    let _ = 名;
    Value::Object(维度)
}

/// 主入口：给一张码表生成增强测评块。
///
/// - `码表`：测评用的 `全码表.txt`（含简码段，按码长自动分全/简）
/// - `数据目录`：yuhao-data 平铺目录（字频/词频/词表）
/// - `allowed_keys`：物理键数（默认 qwerty 26 键）
/// - `全键数`：投影维度（键位序列长度，通常等于全码最大长度 4）
pub fn 增强测评(码表: &Path, 数据目录: &Path) -> Result<Value, String> {
    let (全表, 简表) = 读码表(码表)?;
    // 键数从码表实际用到的字符推断（`_` 选重键不算），避免依赖方案文件的键位格式。
    let 键集: HashSet<char> = 全表
        .iter()
        .chain(简表.iter())
        .flat_map(|项| 项.码.chars())
        .filter(|&c| c != '_')
        .collect();
    let 键数 = 键集.len().max(1);
    // 投影维度 = 全码实际最大键长（通常 4）。
    let 全键数 = 全表
        .iter()
        .map(|项| 项.码.chars().filter(|&c| c != '_').count())
        .max()
        .unwrap_or(4)
        .max(1);

    // 字维度：按字频填权
    let 字频 = 读频率(&数据目录.join("charAbsoluteFrequencySC.json"));
    let 填字频 = |集: &mut Vec<条目>| {
        for 项 in 集.iter_mut() {
            项.频 = *字频.get(&项.文本).unwrap_or(&0);
        }
        集.retain(|x| x.频 > 0); // 无频率字不参与分档（覆盖率口径干净）
    };
    let mut 字全 = 全表.clone();
    let mut 字简 = 简表.clone();
    填字频(&mut 字全);
    填字频(&mut 字简);

    // 词维度：词表 + 词频；词编码 = 各字编码按序拼接，取前 全键数 码
    let 词频表 = 读词表(&数据目录.join("词表.txt"));
    let 词频json = 读频率(&数据目录.join("wordAbsoluteFrequencySC.json"));
    let 全图: HashMap<&str, &str> = 全表
        .iter()
        .map(|x| (x.文本.as_str(), x.码.as_str()))
        .collect();
    let 简图: HashMap<&str, &str> = 简表
        .iter()
        .map(|x| (x.文本.as_str(), x.码.as_str()))
        .collect();
    let 拼词码 = |词: &str, 图: &HashMap<&str, &str>| -> Option<String> {
        let 码: String = 词
            .chars()
            .map(|c| 图.get(c.to_string().as_str()).copied().unwrap_or(""))
            .collect();
        if 码.is_empty() {
            return None;
        }
        Some(码.chars().take(全键数).collect())
    };
    let mut 词全 = Vec::new();
    let mut 词简 = Vec::new();
    for (词, 频) in 词频表 {
        let 频 = if 频 > 0 {
            频
        } else {
            *词频json.get(&词).unwrap_or(&0)
        };
        if 频 == 0 {
            continue;
        }
        if let Some(码) = 拼词码(&词, &全图) {
            词全.push(条目 {
                文本: 词.clone(),
                码,
                频,
            });
        }
        if let Some(码) = 拼词码(&词, &简图) {
            词简.push(条目 {
                文本: 词, 码, 频
            });
        }
    }

    let mut 结果 = Map::new();
    结果.insert("字".to_string(), 统计维度("字", &字全, &字简, 键数, 全键数));
    结果.insert("词".to_string(), 统计维度("词", &词全, &词简, 键数, 全键数));

    // 边际重码：以「把 q 与 p 归并到同键」为示例场景（可作为对照基线）
    if !字全.is_empty() {
        结果.insert(
            "边际重码示例".to_string(),
            边际重码(&字全, &["q", "p"], 键数, 全键数),
        );
    }

    结果.insert(
        "口径".to_string(),
        json!({
            "零阶": "Σ(组大小−1)，全码位投影",
            "高阶": "Σ(n²/(2×键数^阶))，末 n 码通配的投影估计",
            "不可相加": "零阶是计数、高阶是概率估计，二者不可相加或解释为实际新增冲突",
            "边际": "指定键归并后新进入重码组的对象，已重码者不重复列",
            "词编码": "各字编码顺序拼接，取前 N 码；N=全键数",
            "频率": "字用北语简体字频、词用词表频次；无频率者不参与分档",
        }),
    );

    Ok(Value::Object(结果))
}

// =========================================================================
// 🧪 单元测试
// =========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn 造(文本: &str, 码: &str, 频: u64) -> 条目 {
        条目 {
            文本: 文本.to_string(),
            码: 码.to_string(),
            频,
        }
    }

    #[test]
    fn 投影阶数正确() {
        assert_eq!(投影("abcd", 0, 4), "abcd");
        assert_eq!(投影("abcd", 1, 4), "abc*");
        assert_eq!(投影("abcd", 2, 4), "ab**");
        assert_eq!(投影("abc_", 0, 4), "abcε");
        assert_eq!(投影("ab", 0, 4), "abεε");
    }

    #[test]
    fn 零阶重码为组内非首项数() {
        // abcd 两字同码 → 零阶 Σ(n−1) = 1
        let 集 = vec![造("甲", "abcd", 1), 造("乙", "abcd", 1)];
        let v = 阶重码(&集, 26, 4);
        assert_eq!(v["0阶"]["重码估计"], 1.0);
    }

    #[test]
    fn 高阶重码按均匀分配估计() {
        // 三组各 2 字、一阶投影（键数 26）：3 × (4/(2×26)) ≈ 0.231
        let 集 = vec![
            造("甲", "abcd", 1),
            造("乙", "abcd", 1),
            造("丙", "efgh", 1),
            造("丁", "efgh", 1),
            造("戊", "ijkl", 1),
            造("己", "ijkl", 1),
        ];
        let v = 阶重码(&集, 26, 4);
        let 期望: f64 = (3.0_f64 * 4.0 / (2.0 * 26.0) * 1000.0).round() / 1000.0;
        assert_eq!(v["1阶"]["重码估计"], 期望);
    }

    #[test]
    fn 边际重码只列新增对象() {
        // ab / ac 原不重码；把 b、c 归并后都变 a§ → 新增两个对象
        let 集 = vec![造("甲", "ab", 1), 造("乙", "ac", 1)];
        let v = 边际重码(&集, &["b", "c"], 26, 4);
        assert_eq!(v["新增对象数"], 2);
    }

    #[test]
    fn 边际重码不重复列已重码对象() {
        // ab 已与 ab 重码；归并 b/c 后新增的只有 ac
        let 集 = vec![造("甲", "ab", 1), 造("乙", "ab", 1), 造("丙", "ac", 1)];
        let v = 边际重码(&集, &["b", "c"], 26, 4);
        assert_eq!(v["新增对象数"], 1);
    }

    #[test]
    fn 空集不崩且档位齐全() {
        let v = 阶重码(&[], 26, 4);
        assert_eq!(v["0阶"]["重码估计"], 0.0);
        assert_eq!(v["2阶"]["重码估计"], 0.0);
    }
}
