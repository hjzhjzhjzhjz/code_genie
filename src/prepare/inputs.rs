// =========================================================================
// 📥 prepare 步骤 8–13：生成 字根约束 / 动态字根 / 单字拆分
// =========================================================================
//
// 前四步（8–11）按 `>>` 语义逐步追加到 `字根约束.txt`，因此单独跑某一步时
// 会接在已有内容后面——与 bash 版一致。第 12 步按方案的编码规则把「字根序列」
// 展开成「码位序列」，第 13 步在开启简码时把 `.A/.S/.Y` 改写成 `.0/.1/.2`。
// =========================================================================

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::prepare::names;
use crate::prepare::schema::{方案, 编码规则, 韵码来源};
use crate::prepare::text::{self, 数值字段};
use crate::prepare::上下文;

/// 左手声母：撇、折在这些声母后面时用右手的替代映射
const 左声母: &str = "qwrtsdfgzxcvb";
/// 右手声母
const 右声母: &str = "yphjklnm";

/// 步骤 8：大码约束（字根聚类 + 字根频率分档），覆盖写 `字根约束.txt`
pub fn 步骤8_大码约束(上: &上下文) -> Result<(), String> {
    println!("(8) 生成码灵输入文件 字根约束.txt, 大码约束 ...");
    if 上.聚类表 != "roots-cluster.txt" {
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

    // 第一阶段：聚类表，每一簇输出一条 `# freq=` + 一条约束
    for 行 in 上.读(&上.聚类表)? {
        if 行.trim().is_empty() || 行.trim_start().starts_with('#') {
            continue;
        }
        if 跳过一列约束(&行, 方案.低频字根键位) {
            continue;
        }
        let (根串, 键串) = 行.split_once('\t').unwrap_or((行.as_str(), ""));
        let 非法 = 根串.chars().any(|c| c.is_ascii_lowercase())
            || 键串
                .chars()
                .any(|c| !(c.is_ascii_lowercase() || c.is_whitespace()));
        if 非法 {
            return Err(format!("Invalid line in {}: {}", 上.聚类表, 行));
        }

        let mut 根: Vec<&str> = 根串.split_whitespace().collect();
        根.sort();
        let mut 合计 = 0.0;
        for r in &根 {
            let Some(值) = 根频.get(*r) else {
                return Err(format!("ERROR: Unknown root {} in {}", r, 上.聚类表));
            };
            合计 += *值;
        }

        let 键 = if 键串.trim().is_empty() {
            方案.分档键位(合计)
        } else {
            键串.trim()
        };
        出.push_str(&format!("# freq={合计:.8}\n"));
        出.push_str(
            &根.iter()
                .map(|r| format!("{r}.A"))
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
        let 频 = *根频.get(列[0]).unwrap_or(&0.0);
        出.push_str(&format!("# freq={频:.8}\n"));
        出.push_str(&format!("{}.A\t{}\n", 列[0], text::打散(方案.分档键位(频))));
    }
    上.写(names::字根约束, &出)
}

/// `^\s*一\s+f` 且 `ALL_ROOT_KEYS` 里没有 f 时跳过该行
///
/// 宇码传统把「一」的大码钉在 F 上；对把 F 用作 B 区的方案，这条约束不适用。
fn 跳过一列约束(行: &str, 低频键位: &str) -> bool {
    let 匹配 = 行
        .trim_start()
        .strip_prefix('一')
        .and_then(|余| {
            let 去空白 = 余.trim_start();
            (去空白.len() != 余.len()).then_some(去空白)
        })
        .map(|余| 余.starts_with(['f', 'F']))
        .unwrap_or(false);
    匹配 && !低频键位.to_ascii_lowercase().contains('f')
}

/// 步骤 9：声码与韵码约束（追加到 `字根约束.txt`）
pub fn 步骤9_声韵约束(上: &上下文) -> Result<(), String> {
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

    for 行 in 上.读(&上.字根表)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        let 根 = 列[0];
        let 码 = 列.get(1).copied().unwrap_or("");
        let 码长 = 码.chars().count();

        // 声码：声韵码长度 > 1 才说明真有声母
        if 码长 > 1 {
            let 首 = 码.chars().next().unwrap_or(' ');
            let 声 = 方案.优化键(首).unwrap_or(首);
            出.push_str(&format!("{根}.S\t{声}\n"));
        }

        match 方案.韵码 {
            韵码来源::无 => {}
            韵码来源::韵腹 => {
                let 尾 = 码.chars().last().map(String::from).unwrap_or_default();
                出.push_str(&format!("{根}.Y\t{尾}\n"));
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

    // 按笔画分组输出，组内按字根字节序
    let mut 尾段 = String::new();
    for (笔, 根列表) in &韵码组 {
        let Some(映射) = 笔画映射(笔) else {
            return Err(format!("No mapping found for {笔}!"));
        };
        let mut 排序 = 根列表.clone();
        排序.sort();
        for r in 排序 {
            尾段.push_str(&format!("{r}.Y\t{映射}\n"));
        }
    }

    出.push_str(&尾段);
    上.追加(names::字根约束, &出)
}

/// 步骤 10：飞键约束（`roots-fly.txt` 里每个字根占满 AEUIO）
pub fn 步骤10_飞键约束(上: &上下文) -> Result<(), String> {
    println!("(10) 添加码灵输入文件 字根约束.txt, 飞键约束 ...");
    let mut 出 = String::new();
    for 行 in 上.读("roots-fly.txt")? {
        if 行.trim_start().starts_with('#') {
            continue;
        }
        if let Some(根) = 行.split_whitespace().next() {
            出.push_str(&format!("{根}.U\ta e u i o\n"));
        }
    }
    上.追加(names::字根约束, &出)
}

/// 步骤 11：`动态字根.txt` 是空文件（字根全部定义在 `字根约束.txt` 里）
pub fn 步骤11_空字根表(上: &上下文) -> Result<(), String> {
    println!("(11) 生成空的 动态字根.txt, 所有字根已定义于 字根约束.txt ...");
    上.写(names::动态字根, "")
}

/// 步骤 12：按编码规则把拆分展开成码位序列 `单字拆分.txt`
pub fn 步骤12_编码展开(上: &上下文) -> Result<(), String> {
    println!("(12) 生成码灵输入文件 单字拆分.txt ...");
    let 方案 = 上.方案;

    let mut 码表: HashMap<String, String> = HashMap::new();
    for 行 in 上.读(&上.字根表)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 {
            码表.insert(列[0].to_string(), 列[1].to_string());
        }
    }
    let mut 飞键: HashSet<String> = HashSet::new();
    for 行 in 上.读("roots-fly.txt")? {
        if 行.trim_start().starts_with('#') {
            continue;
        }
        let 根: String = 行.chars().filter(|c| !c.is_whitespace()).collect();
        if !根.is_empty() {
            飞键.insert(根);
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
            if !码表.contains_key(*r) && !飞键.contains(*r) {
                return Err(format!("Bad root {r}"));
            }
        }
        let 序列 = 展开(&根, &码表, &飞键, 方案);
        出.push_str(&format!("{}\t{}\t{}\n", 列[0], 序列.join(" "), 列[2]));
    }
    上.写(names::单字拆分, &出)
}

/// 按方案展开码位序列（截断到 `MAX_CODE_LEN` 在展开**之后**做）
fn 展开(
    根: &[&str],
    码表: &HashMap<String, String>,
    飞键: &HashSet<String>,
    方案: &方案,
) -> Vec<String> {
    let 有声母 = |r: &str| 码表.get(r).map(|c| c.chars().count() > 1).unwrap_or(false);
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
        // A1A2A3AzSzYz；二根字回头码 A1A2S2S1Y1；首根是飞键字根时用 .U
        编码规则::魔灵 => {
            for r in 根 {
                序列.push(format!("{r}.A"));
            }
            if 飞键.contains(根[0]) {
                序列[0] = format!("{}.U", 根[0]);
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

/// 步骤 13：开启简码时把 `.A/.S/.Y` 改写成 `.0/.1/.2`
pub fn 步骤13_简码码位(上: &上下文, 配置: Option<&Path>) -> Result<(), String> {
    let Some(路径) = 配置 else {
        return Ok(());
    };
    if !简码已开启(路径)? {
        return Ok(());
    }
    println!(
        "(13) 替换 字根约束.txt 和 单字拆分.txt 中的 .A, .S, .Y  为 .0, .1, .2 以让码灵能计算简码 ..."
    );
    for 名 in [names::字根约束, names::单字拆分] {
        let 路径 = 上.路径(名);
        // 整段读入整段替换，保留原有行尾（Perl 的 `-i -lpE` 不改变 \r\n）
        let 文本 = std::fs::read_to_string(&路径)
            .map_err(|e| format!("读取 {} 失败：{}", 路径.display(), e))?;
        上.写(名, &替换码位标签(&文本))?;
    }
    Ok(())
}

/// `sed -ne '/^\s*\[weights.simple_code\]/,/^\s*\[/p' | grep -qi '^\s*enabled\s*=\s*true'`
fn 简码已开启(路径: &Path) -> Result<bool, String> {
    let mut 在内 = false;
    for 行 in text::读行(路径)? {
        let 归一: String = 行
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if 归一.starts_with('[') {
            在内 = 归一.starts_with("[weights.simple_code]");
            continue;
        }
        if 在内 && 归一.starts_with("enabled=true") {
            return Ok(true);
        }
    }
    Ok(false)
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
        assert_eq!(
            展开(&根, &表, &HashSet::new(), &魔灵),
            vec!["白.A", "勹.A", "丶.A", "丶.Y"]
        );
    }

    #[test]
    fn 魔灵二根走回头码并被码长截断() {
        let 表 = 码表(&[("木", "mu"), ("木2", "mu")]);
        let 根 = ["木", "木"];
        assert_eq!(
            展开(&根, &表, &HashSet::new(), &魔灵),
            vec!["木.A", "木.A", "木.S", "木.S"]
        );
    }

    #[test]
    fn 灵明四根且首根有声母时跳第三根() {
        let 表 = 码表(&[("弓", "gu"), ("口", "ko"), ("一", "a"), ("丨", "su")]);
        let 根 = ["弓", "口", "一", "丨"];
        assert_eq!(
            展开(&根, &表, &HashSet::new(), &灵明),
            vec!["弓.A", "弓.S", "口.A", "丨.A"]
        );
    }

    #[test]
    fn 星陈单根重复小码() {
        let 表 = 码表(&[("一", "yi")]);
        assert_eq!(
            展开(&["一"], &表, &HashSet::new(), &星陈),
            vec!["一.A", "一.S", "一.S"]
        );
    }

    #[test]
    fn 码位改写只动前面非空白的点标签() {
        assert_eq!(替换码位标签("白.A 勹.A\tkeys\n"), "白.0 勹.0\tkeys\n");
        assert_eq!(替换码位标签("白.S\tb\n"), "白.1\tb\n");
        assert_eq!(替换码位标签("白.Y\tu\n"), "白.2\tu\n");
        // 行首的 `.A` 前面没有非空白字符，不替换
        assert_eq!(替换码位标签(".A\n"), ".A\n");
        // `.U` 不在改写范围内
        assert_eq!(替换码位标签("白.U\ta e\n"), "白.U\ta e\n");
    }

    #[test]
    fn 一列约束按低频键位决定是否跳过() {
        assert!(!跳过一列约束("一 f", "qwrtyp sdfghjkl xcvbnm")); // 含 f → 不跳过
        assert!(跳过一列约束("一 f", "qwrtyp sdghjkl xcvbnm")); // 不含 f → 跳过
        assert!(!跳过一列约束("二 冫", "qwrtyp sdghjkl")); // 不是「一」
    }
}
