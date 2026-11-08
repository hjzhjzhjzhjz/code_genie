// =========================================================================
// 📥 prepare 步骤 8–13：生成 字根约束 / 单字拆分
// =========================================================================
//
// 第 8、9 步按 `>>` 语义逐步追加到 `字根约束.txt`，因此单独跑某一步时
// 会接在已有内容后面——与 bash 版一致。第 12 步按方案的编码规则把「字根序列」
// 展开成「码位序列」。
//
// 槽位后缀不写死：第 8、9、12 步都接一个「槽位」参数，魔灵/灵明 传入
// `.A/.S/.Y`，星陈 由 [`super::yustar`] 直接传 `.0/.1`，省掉第 13 步改名。
// =========================================================================

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::prepare::names;
use crate::prepare::schema::{方案, 编码规则, 韵码来源};
use crate::prepare::text::{self, 数值字段};
use crate::prepare::上下文;
use crate::roots_cluster;

/// 左手声母：撇、折在这些声母后面时用右手的替代映射
const 左声母: &str = "qwrtsdfgzxcvb";
/// 右手声母
const 右声母: &str = "yphjklnm";

/// 步骤 8：大码约束（字根聚类 + 字根频率分档），覆盖写 `字根约束.txt`
pub fn 步骤8_大码约束(上: &上下文) -> Result<(), String> {
    大码约束(上, ".A")
}

/// 步骤 8 的实现体
///
/// `槽位` 是约束里用的槽位后缀：魔灵/灵明 写 `.A`（再由步骤 13 改名为 `.0`），
/// 星陈 由 [`super::yustar`] 直接传 `.0`，不经过改名。
pub(crate) fn 大码约束(上: &上下文, 槽位: &str) -> Result<(), String> {
    println!("(8) 生成码灵输入文件 字根约束.txt, 大码约束 ...");
    // 聚类表已内嵌（assets/roots_cluster.ron）；只有放了一份
    // `roots-cluster.ron` / `roots-cluster-<方案>.ron` 时才走外部文件
    let 聚类路径 = 上.源路径(&上.聚类表);
    if 聚类路径.is_some() {
        println!("    !!! 使用方案自行提供的字根聚类表 {} ...", 上.聚类表);
    }

    let 方案 = 上.方案;
    let mut 根频: HashMap<String, f64> = HashMap::new();
    for 行 in 上.读(names::字根频率)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 {
            根频.insert(列[0].to_string(), 数值字段(列[1]));
        }
    }

    let mut 已用: HashSet<String> = HashSet::new();
    let mut 出 = String::new();

    // 第一阶段：聚类表，每一簇输出一条约束
    for 簇 in roots_cluster::加载(聚类路径.as_deref())? {
        if 跳过一列约束(&簇, 方案.全部字根键位) {
            continue;
        }
        let 非法 = 簇
            .roots
            .iter()
            .any(|r| r.chars().any(|c| c.is_ascii_lowercase()))
            || 簇
                .keys
                .chars()
                .any(|c| !(c.is_ascii_lowercase() || c.is_whitespace()));
        if 非法 {
            return Err(format!(
                "Invalid entry in {}: {} / {}",
                上.聚类表,
                簇.roots.join(" "),
                簇.keys
            ));
        }

        let mut 根: Vec<&str> = 簇.roots.iter().map(String::as_str).collect();
        根.sort();
        for r in &根 {
            if !根频.contains_key(*r) {
                return Err(format!("ERROR: Unknown root {} in {}", r, 上.聚类表));
            }
        }

        let 键 = if 簇.keys.trim().is_empty() {
            方案.全部字根键位
        } else {
            簇.keys.trim()
        };
        出.push_str(
            &根.iter()
                .map(|r| format!("{r}{槽位}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        出.push('\t');
        出.push_str(&text::打散(键));
        出.push('\n');

        for r in &根 {
            if !已用.insert((*r).to_string()) {
                return Err(format!(
                    "ERROR: root {} in multiple clusters, check {}!",
                    r, 上.聚类表
                ));
            }
        }
    }

    // 第二阶段：簇外字根逐个输出
    for 行 in 上.读(&上.字根表)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列[0].is_empty() || 已用.contains(列[0]) {
            continue;
        }
        出.push_str(&format!(
            "{}{槽位}\t{}\n",
            列[0],
            text::打散(方案.全部字根键位)
        ));
    }
    上.写(names::字根约束, &出)
}

/// 「一」被钉在 F 键、而本方案的字根键位全集里没有 f 时跳过该簇
///
/// 宇码传统把「一」的大码钉在 F 上；对把 F 另作他用的方案，这条约束不适用。
fn 跳过一列约束(簇: &roots_cluster::簇, 字根键位: &str) -> bool {
    let 匹配 = 簇.roots.first().map(String::as_str) == Some("一")
        && 簇.keys.trim_start().starts_with(['f', 'F']);
    匹配 && !字根键位.to_ascii_lowercase().contains('f')
}

/// 步骤 9：声码与韵码约束（追加到 `字根约束.txt`）
pub fn 步骤9_声韵约束(上: &上下文) -> Result<(), String> {
    声韵约束(上, ".S", ".Y")
}

/// 步骤 9 的实现体
///
/// `声码槽` / `韵码槽`：魔灵/灵明 写 `.S` / `.Y`，星陈 写 `.1`（无韵码，
/// `韵码槽` 用不到），见 [`super::yustar`]。
pub(crate) fn 声韵约束(
    上: &上下文, 声码槽: &str, 韵码槽: &str
) -> Result<(), String> {
    println!("(9) 添加码灵输入文件 字根约束.txt, 声码和韵码约束 ...");
    let 方案 = 上.方案;
    let 笔画 = if 方案.韵码 == 韵码来源::首笔 {
        Some(加载笔画表(上)?)
    } else {
        None
    };

    let mut 出 = String::new();
    // 首笔模式把同一笔画的字根攒到最后统一输出（对应 Perl 的 END 块）
    let mut 韵码组: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // 声码行先攒起来，等循环结束再决定按什么顺序输出（见下方「与 .0 对齐」）
    let mut 声码行: Vec<(String, String)> = Vec::new();

    for 行 in 上.读(&上.字根表)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        let 根 = 列[0];
        let 码 = 列.get(1).copied().unwrap_or("");
        let 码长 = 码.chars().count();

        // 声码：声韵码长度 > 1 才说明真有声母；星陈（`小码恒有`）不分有没有声母，
        // 每个根都给一行 `.1`，候选键集同样是 `声码键位`。
        if 码长 > 1 || 方案.小码恒有 {
            let 内容 = match 方案.声码键位 {
                // 统一候选键集：不按声母钉死，交给退火在集合内分配
                Some(键集) => text::打散(键集),
                None => {
                    let 首 = 码.chars().next().unwrap_or(' ');
                    方案.优化键(首).unwrap_or(首).to_string()
                }
            };
            声码行.push((根.to_string(), 内容));
        }

        match 方案.韵码 {
            韵码来源::无 => {}
            韵码来源::韵腹 => {
                let 尾 = 码.chars().last().map(String::from).unwrap_or_default();
                出.push_str(&format!("{根}{韵码槽}\t{尾}\n"));
            }
            韵码来源::首笔 => {
                let 表 = 笔画.as_ref().expect("首笔模式必然加载笔画表");
                let Some(原始) = 表.get(根) else {
                    return Err(format!("No stroke found for {根}!"));
                };
                let mut 笔 = 原始.clone();
                if 码长 > 1 {
                    let 有左 = 码.chars().any(|c| 左声母.contains(c));
                    let 有右 = 码.chars().any(|c| 右声母.contains(c));
                    // 手感：左声母后避开撇，右声母后避开横竖点，折在左手后推到 A 区
                    let 该加五 = (有左 && 笔.contains('3'))
                        || (有右 && 笔.chars().any(|c| matches!(c, '1' | '2' | '4')));
                    if 该加五 {
                        笔 = 加五(&笔);
                    } else if 有左 && 笔.contains('5') {
                        笔 = "A".to_string();
                    }
                }
                韵码组.entry(笔).or_default().push(根.to_string());
            }
        }
    }

    // ---- 声码输出 ----
    // `声码键位` 存在时（星陈这类「统一候选键集」方案），按已生成的 `.0` 段顺序
    // 排列，让 `.0` 的簇与 `.1` 的根上下对照；**每一行都写完整候选键集**，小码
    // 完全交给退火在集合内分配，不按簇钉死单键。
    //
    // 历史行为曾把多根簇内的根钉成互不相同的单键，理由是 `load_fixed` 把 `.0` 同行
    // 多根建为共享 `group_key` 的 `RootGroup`，组内大码必然同键；若小码也撞同键就
    // 出现「大小码完全相同」的死重码。现在取消钉死以换取小码的完整自由度，代价是
    // 这条硬保证消失——同簇根是否撞小码改由退火的重码惩罚驱散。
    // 声母钉死单键的方案（魔灵 / 灵明）不走这条分支，仍是一行一根。
    if 方案.声码键位.is_some() {
        let 大码槽 = 声码槽
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_owned()
            + "0";
        // 单独跑第 9 步时 `字根约束.txt` 可能还没生成（第 8 步没跑过），
        // 这时 `已有分组` 为空，函数退化为按字根表顺序一行一根，不报错。
        let 已有分组 = 上.读(names::字根约束).unwrap_or_default();
        出.push_str(&写小码行(&声码行, &已有分组, &大码槽, 声码槽));
    } else {
        for (根, 内容) in &声码行 {
            出.push_str(&format!("{根}{声码槽}\t{内容}\n"));
        }
    }

    // 按笔画分组输出，组内按字根字节序
    let mut 尾段 = String::new();
    for (笔, 根列表) in &韵码组 {
        let Some(映射) = 笔画映射(笔) else {
            return Err(format!("No mapping found for {笔}!"));
        };
        let mut 排序 = 根列表.clone();
        排序.sort();
        for r in 排序 {
            尾段.push_str(&format!("{r}{韵码槽}\t{映射}\n"));
        }
    }

    出.push_str(&尾段);
    上.追加(names::字根约束, &出)
}

/// 生成小码段：按 `.0` 共键组的出现顺序逐根一行写出，每行都保留 `声码行` 里
/// 的候选键集（星陈即 `声码键位` 全集），不把任何根钉成单键。
///
/// `已有分组` 只用于排序与去重；未出现在 `.0` 段的根按 `声码行` 原顺序兜底，
/// 保证 `.1` 条目数恒等于应有根数，不会因为约束文件缺行而丢根。
fn 写小码行(
    声码行: &[(String, String)],
    已有分组: &[String],
    大码槽: &str,
    声码槽: &str,
) -> String {
    let 内容表: HashMap<&str, &str> = 声码行
        .iter()
        .map(|(根, 内容)| (根.as_str(), 内容.as_str()))
        .collect();
    let mut 已出: HashSet<String> = HashSet::new();
    let mut 出 = String::new();
    for 行 in 已有分组 {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        // 只认本方案的大码槽；上一次跑留下的 `.1` 行等其它槽位自然被过滤掉
        let 根们 = 列[0]
            .split_whitespace()
            .filter_map(|t| t.strip_suffix(大码槽));
        for 根 in 根们 {
            let Some(内容) = 内容表.get(根).copied() else {
                continue;
            };
            if 已出.insert(根.to_string()) {
                出.push_str(&format!("{根}{声码槽}\t{内容}\n"));
            }
        }
    }
    for (根, 内容) in 声码行 {
        if 已出.insert(根.clone()) {
            出.push_str(&format!("{根}{声码槽}\t{内容}\n"));
        }
    }
    出
}

/// 步骤 12：按编码规则把拆分展开成码位序列 `单字拆分.txt`
pub fn 步骤12_编码展开(上: &上下文) -> Result<(), String> {
    编码展开(上, false)
}

/// 步骤 12 的实现体
///
/// `数字槽位`：为真时把展开出的 `.A/.S/.Y` 直接写成 `.0/.1/.2`（星陈 用），
/// 这样 `字根约束.txt` 与 `单字拆分.txt` 的槽位号一致，不再需要步骤 13 改名。
pub(crate) fn 编码展开(上: &上下文, 数字槽位: bool) -> Result<(), String> {
    println!("(12) 生成码灵输入文件 单字拆分.txt ...");
    let 方案 = 上.方案;

    let mut 码表: HashMap<String, String> = HashMap::new();
    for 行 in 上.读(&上.字根表)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 {
            码表.insert(列[0].to_string(), 列[1].to_string());
        }
    }
    let mut 出 = String::new();
    for 行 in 上.读(names::拆分表)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 3 {
            continue;
        }
        let 根: Vec<&str> = 列[1].split_whitespace().collect();
        if 根.is_empty() {
            continue;
        }
        for r in &根 {
            if !码表.contains_key(*r) {
                return Err(format!("Bad root {r}"));
            }
        }
        let 序列 = 展开(&根, &码表, 方案);
        let 序列: Vec<String> = if 数字槽位 {
            序列.iter().map(|s| 替换码位标签(s)).collect()
        } else {
            序列
        };
        出.push_str(&format!("{}\t{}\t{}\n", 列[0], 序列.join(" "), 列[2]));
    }
    上.写(names::单字拆分, &出)
}

/// 按方案展开码位序列（截断到 `MAX_CODE_LEN` 在展开**之后**做）
///
/// 返回槽位标签（`根.A` / `根.S` / `根.Y`），聚类重码分析直接复用这一份，
/// 保证与优化器实际出码同口径。
pub(crate) fn 展开(
    根: &[&str], 码表: &HashMap<String, String>, 方案: &方案
) -> Vec<String> {
    // 星陈（`小码恒有`）不看声母：候选键集对每个根一视同仁，查不到读音的部件根
    // 也照样分到一个小码；魔灵/灵明 仍按「声码长度 > 1」判定。
    let 有声母 =
        |r: &str| 方案.小码恒有 || 码表.get(r).map(|c| c.chars().count() > 1).unwrap_or(false);
    let 末 = 根[根.len() - 1];
    let mut 序列: Vec<String> = Vec::with_capacity(6);

    match 方案.编码规则 {
        // 单根 ASY；多根 A1S1A2A3AzSzYz，≥4 根且首根有声母时跳第三根
        编码规则::灵明 => {
            序列.push(format!("{}.A", 根[0]));
            if 有声母(根[0]) {
                序列.push(format!("{}.S", 根[0]));
            }
            if 根.len() == 1 {
                序列.push(format!("{}.Y", 根[0]));
            } else {
                for (i, r) in 根.iter().enumerate().skip(1) {
                    if 根.len() > 3 && i == 2 && 有声母(根[0]) {
                        continue;
                    }
                    序列.push(format!("{r}.A"));
                }
                if 有声母(末) {
                    序列.push(format!("{末}.S"));
                }
                序列.push(format!("{末}.Y"));
            }
        }
        // 1 根 A1S1S1；2 根 A1A2S2S1；3 根 A1A2A3S3；≥4 根 A1A2A3A末
        编码规则::星陈 => {
            let n = 根.len();
            序列.push(format!("{}.A", 根[0]));
            if n == 1 {
                if 有声母(根[0]) {
                    序列.push(format!("{}.S", 根[0]));
                    序列.push(format!("{}.S", 根[0]));
                }
            } else if n == 2 {
                序列.push(format!("{}.A", 根[1]));
                if 有声母(根[1]) {
                    序列.push(format!("{}.S", 根[1]));
                }
                if 有声母(根[0]) {
                    序列.push(format!("{}.S", 根[0]));
                }
            } else {
                序列.push(format!("{}.A", 根[1]));
                序列.push(format!("{}.A", 根[2]));
                if n == 3 {
                    if 有声母(根[2]) {
                        序列.push(format!("{}.S", 根[2]));
                    }
                } else {
                    序列.push(format!("{末}.A"));
                }
            }
        }
        // A1A2A3AzSzYz；二根字回头码 A1A2S2S1Y1
        编码规则::魔灵 => {
            for r in 根 {
                序列.push(format!("{r}.A"));
            }
            if 根.len() == 2 {
                if 有声母(根[1]) {
                    序列.push(format!("{}.S", 根[1]));
                }
                if 有声母(根[0]) {
                    序列.push(format!("{}.S", 根[0]));
                }
                序列.push(format!("{}.Y", 根[0]));
            } else {
                if 有声母(末) {
                    序列.push(format!("{末}.S"));
                }
                序列.push(format!("{末}.Y"));
            }
        }
    }

    序列.truncate(方案.最大码长);
    序列
}

/// `s/(\S)\.A/\1.0/g; s/(\S)\.S/\1.1/g; s/(\S)\.Y/\1.2/g`
///
/// 三次替换可以合并成一趟：替换出来的是 `0/1/2`，不会被后续规则再命中。
fn 替换码位标签(文本: &str) -> String {
    let 字: Vec<char> = 文本.chars().collect();
    let mut 出 = String::with_capacity(文本.len());
    let mut i = 0;
    while i < 字.len() {
        if i + 1 < 字.len() && 字[i] == '.' && i > 0 && !字[i - 1].is_whitespace() {
            let 替 = match 字[i + 1] {
                'A' => Some('0'),
                'S' => Some('1'),
                'Y' => Some('2'),
                _ => None,
            };
            if let Some(c) = 替 {
                出.push('.');
                出.push(c);
                i += 2;
                continue;
            }
        }
        出.push(字[i]);
        i += 1;
    }
    出
}

fn 笔画映射(笔: &str) -> Option<&'static str> {
    Some(match 笔 {
        "1" => "o",
        "2" => "u",
        "3" => "e",
        "4" => "i",
        "5" => "a",
        "6" => "e",
        "7" => "e",
        "8" => "i",
        "9" => "e",
        "A" => "u",
        _ => return None,
    })
}

fn 加五(笔: &str) -> String {
    (笔.parse::<u32>().unwrap_or(0) + 5).to_string()
}

/// 笔画修正表（脚本里的 `%stroke_overrides`，修 CSV 里的数据错误）
fn 笔画修正表() -> [(&'static str, &'static str); 11] {
    [
        ("艹", "1"),
        ("山", "2"),
        ("水", "2"),
        ("{冂丶}", "2"),
        ("{曾中}", "2"),
        ("卯", "3"),
        ("冖", "4"),
        ("忄", "4"),
        ("乃", "5"),
        ("乙", "5"),
        ("{纟上}", "6"),
    ]
}

/// 从 `yuhao-zigens.csv` 取每个字根的首笔笔画
fn 加载笔画表(上: &上下文) -> Result<HashMap<String, String>, String> {
    let 修正 = 笔画修正表();
    let mut 表: HashMap<String, String> = HashMap::new();
    for (序号, 行) in 上.读("yuhao-zigens.csv")?.into_iter().enumerate() {
        if 序号 == 0 {
            continue;
        }
        let 列: Vec<&str> = 行.split(',').collect();
        if 列.len() < 2 {
            continue;
        }
        let 根 = 列[1];
        let mut 笔 = 列
            .get(4)
            .and_then(|s| s.chars().next())
            .map(String::from)
            .unwrap_or_default();
        if 笔 == "6" && 上.方案.笔画6并入5 {
            笔 = "5".to_string();
        }
        if let Some(旧) = 表.get(根) {
            if 旧 != &笔 {
                return Err(format!("Conflict strokes: {行} vs. previously {旧}"));
            }
        }
        if !表.contains_key(根) && !修正.iter().any(|(k, _)| k == &根) {
            表.insert(根.to_string(), 笔);
        }
    }
    for (根, 笔) in 修正 {
        let 笔 = if 笔 == "6" && 上.方案.笔画6并入5 {
            "5"
        } else {
            笔
        };
        表.insert(根.to_string(), 笔.to_string());
    }
    Ok(表)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::schema::{星陈, 灵明, 魔灵};

    fn 码表(条目: &[(&str, &str)]) -> HashMap<String, String> {
        条目
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn 魔灵三根取大码再加末根声韵() {
        let 表 = 码表(&[("白", "ba"), ("勹", "bo"), ("丶", "a")]);
        let 根 = ["白", "勹", "丶"];
        assert_eq!(展开(&根, &表, &魔灵), vec!["白.A", "勹.A", "丶.A", "丶.Y"]);
    }

    #[test]
    fn 魔灵二根走回头码并被码长截断() {
        let 表 = 码表(&[("木", "mu"), ("木2", "mu")]);
        let 根 = ["木", "木"];
        assert_eq!(展开(&根, &表, &魔灵), vec!["木.A", "木.A", "木.S", "木.S"]);
    }

    #[test]
    fn 灵明四根且首根有声母时跳第三根() {
        let 表 = 码表(&[("弓", "gu"), ("口", "ko"), ("一", "a"), ("丨", "su")]);
        let 根 = ["弓", "口", "一", "丨"];
        assert_eq!(展开(&根, &表, &灵明), vec!["弓.A", "弓.S", "口.A", "丨.A"]);
    }

    #[test]
    fn 星陈单根重复小码() {
        let 表 = 码表(&[("一", "yi")]);
        assert_eq!(展开(&["一"], &表, &星陈), vec!["一.A", "一.S", "一.S"]);
    }

    #[test]
    fn 小码全部写候选集且按大码簇排序() {
        let 全集 = text::打散(星陈.声码键位.unwrap());
        let 声码行: Vec<(String, String)> = ["乙", "己", "已", "巳", "巴"]
            .iter()
            .map(|r| (r.to_string(), 全集.clone()))
            .collect();
        let 分组 = vec![
            "己.0 已.0 巳.0 巴.0\tq w e\n".to_string(),
            "乙.0\tq w e\n".to_string(),
            "己.1\tw\n".to_string(), // 上一次跑残留的 .1 行必须被忽略
        ];
        let 文本 = 写小码行(&声码行, &分组, ".0", ".1");
        let 行: Vec<&str> = 文本.lines().collect();
        assert_eq!(行.len(), 5, "每根一行，不能因分组去重丢根");
        for (i, 根) in ["己", "已", "巳", "巴", "乙"].iter().enumerate() {
            assert_eq!(行[i], format!("{根}.1\t{全集}").as_str());
        }
        assert!(
            行.iter()
                .all(|l| l.split('\t').nth(1) == Some(全集.as_str())),
            "所有 .1 行必须是完整候选键集，不能有钉死单键"
        );
        assert_eq!(全集.split_whitespace().count(), 16);
    }

    #[test]
    fn 码位改写只动前面非空白的点标签() {
        assert_eq!(替换码位标签("白.A 勹.A\tkeys\n"), "白.0 勹.0\tkeys\n");
        assert_eq!(替换码位标签("白.S\tb\n"), "白.1\tb\n");
        assert_eq!(替换码位标签("白.Y\tu\n"), "白.2\tu\n");
        // 行首的 `.A` 前面没有非空白字符，不替换
        assert_eq!(替换码位标签(".A\n"), ".A\n");
        // 不认识的码位标签一律不动
        assert_eq!(替换码位标签("白.Q\ta e\n"), "白.Q\ta e\n");
    }

    #[test]
    fn 一列约束按字根键位决定是否跳过() {
        let 一 = roots_cluster::簇 {
            roots: vec!["一".into()],
            keys: "f".into(),
        };
        let 二 = roots_cluster::簇 {
            roots: vec!["二".into(), "冫".into()],
            keys: String::new(),
        };
        assert!(!跳过一列约束(&一, "qwrtyp sdfghjkl xcvbnm")); // 含 f → 不跳过
        assert!(跳过一列约束(&一, "qwrtyp sdghjkl xcvbnm")); // 不含 f → 跳过
        assert!(!跳过一列约束(&二, "qwrtyp sdghjkl")); // 不是「一」
    }

    #[test]
    fn 全部字根键位等于可用键全集且含f() {
        // 取消按根频分档后，所有字根的候选键集都来自这一个全集；
        // 「一」被钉在 f 上，因此全集必须含 f，否则该簇会被整条跳过。
        let 键集 = crate::prepare::schema::魔灵.全部字根键位;
        // 与 config.ron 的 keys.allowed 同一套键：25 个；z 只在 display_order 里，
        // 不做大码键位，所以候选键不能出现 z。
        assert_eq!(键集, "qwertyuiopasdfghjklxcvbnm");
        assert_eq!(键集.chars().count(), 25);
        assert!(键集.contains('f'));
        assert!(!键集.contains('z'));
    }
}
