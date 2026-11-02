// =========================================================================
// 📤 测评码表导出
// =========================================================================
//
// 把「优化后的 keymap + 纯拆分表 + 简码表」直接合成一份测评用码表：
//
//   字 <TAB> 码
//
// 简码行在前、全码行在后；码里不带尾随下划线（测评器会给短码自动补 `_`）。
//
// 取代 `moling/generate-root-chart.sh` 里 `mabiao.tsv` 那一段（第 184–388 行）：
//
//   | 脚本 | 这里 |
//   |---|---|
//   | `perl -F'\t'` 读 roots.tsv（字根 → 键位串） | `加载键位表` |
//   | `$ENV{ENCODE_RULE}` 四分支取码 | `取码序列按规则` + `--schema` |
//   | 简码靠 `SHORTCODE_*` 环境变量贪心分配 | 直接读优化器产出的简码表（同源，更准） |
//   | `printf STDERR "全码长: %.2f 动重: %.2f‱"` | `统计`，同口径 |
//
// 三套编码规则的取码序列（键位串第 1 字母 = A 大码、第 2 字母 = S 小码）：
//
//   星陈 yustar：1 根 A1S1S1 / 2 根 A1A2S2S1 / 3 根 A1A2A3S3 / ≥4 根 A1A2A3A末
//   魔灵 moling：所有根 .A，再末根 .S/.Y；二根字追加回头码 S2S1Y1（截断后即 A1A2S2S1）
//   灵明 yuling：A1S1 A2A3…AzSzYz；≥4 根且首根有声母时跳过第 3 根
//
// 与脚本的一处**故意差异**：脚本自己用 `DUP_WEIGHT_FOR_SHORTCODE` 那套贪心算简码，
// 本模块只读 `简码表.txt`。优化器出的简码才是最终上屏码，两边贪心必然有差异，
// 重算一遍没有意义。
// =========================================================================

use std::collections::HashMap;
use std::fs;

use crate::prepare::schema::{self, 编码规则};

/// 码位标签：A=首码、S=次码、Y=末码（对应 keymap 里键位串的第 1/2/末个字母）
///
/// 键位串只有两个字母时 S 与 Y 取到同一个字母，与脚本的 `substr($roots{$r}, 1)` 一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum 码位 {
    A,
    S,
    Y,
}

impl 码位 {
    pub(crate) fn 序号(self) -> usize {
        match self {
            码位::A => 0,
            码位::S => 1,
            码位::Y => 2,
        }
    }
}

/// 码表统计，口径对齐脚本的 `printf STDERR "全码长: %.2f 动重: %.2f‱"`：
///
/// `全码长` = Σ(码长 × 频次) / Σ频次，同码非首选字额外 +1 码（相当于翻页一次）；
/// `动重`   = 非首选字的频次占比，乘 1 万即 ‱。
#[derive(Clone, Copy, Debug, Default)]
pub struct 统计 {
    /// 参与统计的总频次（拆分表第三列之和）
    pub 总频: f64,
    /// 加权平均全码长
    pub 全码长: f64,
    /// 动重（万分比，即 ‱）
    pub 动重: f64,
    /// 出现重码的码位组数
    pub 重码组: usize,
    /// 落在重码上的字数
    pub 重码字: usize,
}

/// 读取 keymap：`字根 <TAB> 键位串 <TAB> 频次`，只取前两列
pub(crate) fn 加载键位表(path: &str) -> HashMap<String, Vec<char>> {
    let 文本 = fs::read_to_string(path).expect("无法读取 keymap");
    let mut 表 = HashMap::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() >= 2 {
            表.insert(列[0].to_string(), 列[1].chars().collect());
        }
    }
    表
}

/// 读取纯拆分表：`字 <TAB> 根 根 … <TAB> 频次`
pub(crate) fn 加载拆分表(path: &str) -> Vec<(String, Vec<String>, u64)> {
    let 文本 = fs::read_to_string(path).expect("无法读取拆分表");
    let mut 表 = Vec::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        // 首列可能带变体选择符（如「直」+ VS），按原字符串保留，避免截成同一个字
        let 字 = 列[0].to_string();
        let 根: Vec<String> = 列[1].split_whitespace().map(|s| s.to_string()).collect();
        let 频次 = 列
            .get(2)
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        表.push((字, 根, 频次));
    }
    表
}

/// 读取简码表：`字 <TAB> 码 <TAB> 频次`，码尾的 `_` 去掉
pub(crate) fn 加载简码表(path: &str) -> HashMap<String, String> {
    let 文本 = fs::read_to_string(path).expect("无法读取简码表");
    let mut 表 = HashMap::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 字 = 列[0].to_string();
        let 码 = 列[1].trim_end_matches('_');
        if !码.is_empty() {
            表.insert(字, 码.to_string());
        }
    }
    表
}

/// 星月取码序列（对应 `yustar.yaml` 的 encoder 状态机）：返回 (字根, 码位) 列表，已截断到最大码长。
///
/// ```text
///   1 根 : A1 S1 S1                    s1 → c0(根2不存在) → s3(S1) → s6(S1)
///   2 根 : A1 A2 S2 S1                 s1 → s2 → c1(根3不存在) → s4(S2) → s9(S1)
///   3 根 : A1 A2 A3 S3                 s1 → s2 → s5 → c2(根4不存在) → s8(S3)
///   ≥4 根: A1 A2 A3 A_last             c2(根4存在) → s7(rootIndex -1)
/// ```
///
/// A=键位串第 1 个字母（大码）、S=第 2 个字母（键位串只有 1 个字母时回落大码）。
/// 与 `rime_convert::单字码` 同一口径。
pub(crate) fn 取码序列<'a>(
    根: &'a [String],
    键位: &HashMap<String, Vec<char>>,
    最大码长: usize,
) -> Vec<(&'a str, 码位)> {
    取码序列按规则(根, 键位, 最大码长, 编码规则::星陈)
}

/// 按方案规则取码：返回 (字根, 码位) 列表，已截断到最大码长。
///
/// 三套规则与 `prepare::inputs::展开` 的槽位序列一一对应，只是那边出 `根.A` 这种标签、
/// 这边直接给出「取键位串第几个字母」。
pub(crate) fn 取码序列按规则<'a>(
    根: &'a [String],
    键位: &HashMap<String, Vec<char>>,
    最大码长: usize,
    规则: 编码规则,
) -> Vec<(&'a str, 码位)> {
    let n = 根.len();
    if n == 0 {
        return Vec::new();
    }
    // 「有声母」= 键位串不止一个字母（脚本里的 `length($roots{$r}) > 1`）
    let 有声母 = |i: usize| {
        键位
            .get(根[i].as_str())
            .map(|k| k.len() > 1)
            .unwrap_or(false)
    };
    let 末 = n - 1;

    let mut 序列: Vec<(&'a str, 码位)> = match 规则 {
        编码规则::星陈 => match n {
            1 => vec![
                (根[0].as_str(), 码位::A),
                (根[0].as_str(), 码位::S),
                (根[0].as_str(), 码位::S),
            ],
            2 => vec![
                (根[0].as_str(), 码位::A),
                (根[1].as_str(), 码位::A),
                (根[1].as_str(), 码位::S),
                (根[0].as_str(), 码位::S),
            ],
            3 => vec![
                (根[0].as_str(), 码位::A),
                (根[1].as_str(), 码位::A),
                (根[2].as_str(), 码位::A),
                (根[2].as_str(), 码位::S),
            ],
            _ => vec![
                (根[0].as_str(), 码位::A),
                (根[1].as_str(), 码位::A),
                (根[2].as_str(), 码位::A),
                (根[末].as_str(), 码位::A),
            ],
        },
        // 魔灵：所有根取大码，再按情况追加末根的小码 / 末码
        //   1 根 A1 S1 Y1；2 根 A1 A2 S2 S1 Y1（回头码）；≥3 根 A1…Az Sz Yz
        编码规则::魔灵 => {
            let mut s: Vec<(&str, 码位)> = (0..n).map(|i| (根[i].as_str(), 码位::A)).collect();
            if n == 2 {
                if 有声母(1) {
                    s.push((根[1].as_str(), 码位::S));
                }
                if 有声母(0) {
                    s.push((根[0].as_str(), 码位::S));
                }
                s.push((根[0].as_str(), 码位::Y));
            } else {
                if 有声母(末) {
                    s.push((根[末].as_str(), 码位::S));
                }
                s.push((根[末].as_str(), 码位::Y));
            }
            s
        }
        // 灵明：A1 [S1] A2 A3 … Az Sz Yz，≥4 根且首根有声母时跳过第 3 根
        编码规则::灵明 => {
            let mut s: Vec<(&str, 码位)> = vec![(根[0].as_str(), 码位::A)];
            if 有声母(0) {
                s.push((根[0].as_str(), 码位::S));
            }
            if n == 1 {
                s.push((根[0].as_str(), 码位::Y));
            } else {
                for i in 1..n {
                    if n > 3 && i == 2 && 有声母(0) {
                        continue;
                    }
                    s.push((根[i].as_str(), 码位::A));
                }
                if 有声母(末) {
                    s.push((根[末].as_str(), 码位::S));
                }
                s.push((根[末].as_str(), 码位::Y));
            }
            s
        }
    };
    序列.truncate(最大码长);
    序列
}

/// 按序列把字根翻成键位字母
pub(crate) fn 编码(
    序列: &[(&str, 码位)], 键位: &HashMap<String, Vec<char>>
) -> Option<String> {
    let mut 码 = String::with_capacity(序列.len());
    for (根, 位) in 序列 {
        let 键 = 键位.get(*根)?;
        if 键.is_empty() {
            return None;
        }
        let 下标 = 位.序号().min(键.len() - 1);
        // keymap 里同一字根的两个键位用大小写区分（.0 大写、.1 小写），输出统一小写
        码.push(键[下标].to_ascii_lowercase());
    }
    Some(码)
}

/// 导出测评码表的参数
pub struct 参数<'a> {
    /// 键位映射文件（字根键位表.txt / output-keymap.txt：字根<TAB>键位串[<TAB>频次]）
    pub 键位: &'a str,
    /// 拆分表（拆分表.txt / 拆分全表.txt：字<TAB>根 根 …<TAB>频次）
    pub 拆分: &'a str,
    /// 简码表（简码表.txt，可选）
    pub 简码: Option<&'a str>,
    /// 输出文件
    pub 输出: &'a str,
    /// 方案名（moling / yuling / yustar），决定取码规则
    pub 方案: &'a str,
    /// 覆盖方案自带的最大码长；`None` 用方案默认
    pub 最大码长: Option<usize>,
}

/// 导出测评码表
pub fn run(参: &参数) -> Result<(), String> {
    let 方案 = schema::取(参.方案)?;
    let 最大码长 = 参.最大码长.unwrap_or(方案.最大码长);

    println!("=== CodeGenie 测评码表导出 ===");
    println!("  方案:     {}（{}）", 方案.名, 方案.名称);
    println!("  键位映射: {}", 参.键位);
    println!("  拆分表:   {}", 参.拆分);
    if let Some(p) = 参.简码 {
        println!("  简码表:   {}", p);
    }
    println!("  最大码长: {}", 最大码长);
    println!("  输出文件: {}", 参.输出);

    let 键位 = 加载键位表(参.键位);
    let 拆分 = 加载拆分表(参.拆分);
    let 简码 = 参.简码.map(加载简码表).unwrap_or_default();

    println!(
        "  字根 {} 个，汉字 {} 个，简码 {} 条",
        键位.len(),
        拆分.len(),
        简码.len()
    );

    let (输出, 全码条数, 简码条数, 缺根, 统计) =
        生成表按方案(&键位, &拆分, &简码, 方案.编码规则, 最大码长);
    fs::write(参.输出, 输出).map_err(|e| format!("无法写入 {}：{e}", 参.输出))?;
    println!(
        "  完成：全码 {} 条，简码 {} 条，跳过（缺字根）{} 条",
        全码条数, 简码条数, 缺根
    );
    if 统计.总频 > 0.0 {
        println!(
            "  全码长: {:.2} 动重: {:.2}‱（重码 {} 组 / {} 字）",
            统计.全码长, 统计.动重, 统计.重码组, 统计.重码字
        );
    } else {
        println!("  拆分表没有频次列，跳过全码长/动重统计");
    }
    println!("  已写入 {}", 参.输出);
    Ok(())
}

/// 把「键位 + 拆分 + 简码」合成码表文本（星陈规则、最大码长 4，供流水线复用）
///
/// 返回 (文本, 全码条数, 简码条数, 缺根条数)
pub(crate) fn 生成表(
    键位: &HashMap<String, Vec<char>>,
    拆分: &[(String, Vec<String>, u64)],
    简码: &HashMap<String, String>,
) -> (String, usize, usize, usize) {
    let (文本, 全, 简, 缺, _) = 生成表按方案(键位, 拆分, 简码, 编码规则::星陈, 4);
    (文本, 全, 简, 缺)
}

/// 把「键位 + 拆分 + 简码」按指定规则合成码表文本，并统计全码长 / 动重
///
/// 返回 (文本, 全码条数, 简码条数, 缺根条数, 统计)
pub(crate) fn 生成表按方案(
    键位: &HashMap<String, Vec<char>>,
    拆分: &[(String, Vec<String>, u64)],
    简码: &HashMap<String, String>,
    规则: 编码规则,
    最大码长: usize,
) -> (String, usize, usize, usize, 统计) {
    let mut 输出 = String::with_capacity(拆分.len() * 12);
    let mut 缺根 = 0usize;
    let mut 简码条数 = 0usize;
    let mut 全码条数 = 0usize;
    let mut 统计 = 统计::default();
    // 码位 → 已出现的字数（按拆分表顺序，与脚本的遍历顺序一致）
    let mut 已见: HashMap<String, usize> = HashMap::new();

    for (字, 根, 频次) in 拆分 {
        let 序列 = 取码序列按规则(根, 键位, 最大码长, 规则);
        let Some(全码) = 编码(&序列, 键位) else {
            缺根 += 1;
            continue;
        };

        // 重码统计：同码第二个及以后的字都算「非首选」，码长 +1（相当于翻页一次）
        let 次数 = {
            let e = 已见.entry(全码.clone()).or_insert(0);
            *e += 1;
            *e
        };
        let 频 = *频次 as f64;
        let 码长 = 全码.chars().count() + usize::from(次数 > 1);
        if 次数 > 1 {
            统计.重码字 += 1;
            统计.动重 += 频;
        }
        统计.总频 += 频;
        统计.全码长 += 码长 as f64 * 频;

        if let Some(简) = 简码.get(字.as_str()) {
            // 简码 = 码 + 上屏键（如 `到` 的 `bi` = 码 `b` + 上屏键 `i`），
            // 所以先按整串比，再退一格比「去掉上屏键」的码；都对不上才视为脏数据跳过。
            let 前缀匹配 = 全码.starts_with(简.as_str())
                || (简.len() > 1 && 全码.starts_with(&简[..简.len() - 1]));
            if 前缀匹配 {
                输出.push_str(&format!("{}\t{}\n", 字, 简));
                简码条数 += 1;
            }
        }
        输出.push_str(&format!("{}\t{}\n", 字, 全码));
        全码条数 += 1;
    }

    if 统计.总频 > 0.0 {
        统计.重码组 = 已见.values().filter(|c| **c > 1).count();
        统计.全码长 /= 统计.总频;
        统计.动重 = 统计.动重 / 统计.总频 * 10000.0;
    }

    (输出, 全码条数, 简码条数, 缺根, 统计)
}

// =========================================================================
// 🧪 测试
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张键位表：每个字根一个两字母键位串（大码小码），即「有声母」
    fn 键位表(根们: &[(&str, [char; 2])]) -> HashMap<String, Vec<char>> {
        根们
            .iter()
            .map(|(根, k)| (根.to_string(), k.to_vec()))
            .collect()
    }

    fn 码(序列: &[(&str, 码位)], 键位: &HashMap<String, Vec<char>>) -> String {
        编码(序列, 键位).unwrap()
    }

    /// 五个根：星陈取末根大码，魔灵取第 4 根大码 —— 这是两套规则唯一有分歧的地方
    #[test]
    fn 五根时星陈取末根魔灵取第四根() {
        let 键位 = 键位表(&[
            ("甲", ['A', 'x']),
            ("乙", ['B', 'y']),
            ("丙", ['C', 'z']),
            ("丁", ['D', 'w']),
            ("戊", ['E', 'v']),
        ]);
        let 根: Vec<String> = ["甲", "乙", "丙", "丁", "戊"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let 星陈 = 取码序列按规则(&根, &键位, 4, 编码规则::星陈);
        assert_eq!(码(&星陈, &键位), "abcE".to_lowercase());

        let 魔灵 = 取码序列按规则(&根, &键位, 4, 编码规则::魔灵);
        assert_eq!(码(&魔灵, &键位), "abcd");
    }

    /// 灵明：A1S1 A2 A3…，≥4 根且首根有声母时跳过第 3 根
    #[test]
    fn 灵明四根跳过第三根() {
        let 键位 = 键位表(&[
            ("甲", ['A', 'x']),
            ("乙", ['B', 'y']),
            ("丙", ['C', 'z']),
            ("丁", ['D', 'w']),
        ]);
        let 根: Vec<String> = ["甲", "乙", "丙", "丁"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        // A1=甲大码、S1=甲小码、A2=乙大码、（丙跳过）、A4=丁大码
        assert_eq!(
            码(&取码序列按规则(&根, &键位, 4, 编码规则::灵明), &键位),
            "axbd"
        );

        // 三根不跳：A1 S1 A2 A3
        let 三根: Vec<String> = ["甲", "乙", "丙"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            码(&取码序列按规则(&三根, &键位, 4, 编码规则::灵明), &键位),
            "axbc"
        );
    }

    /// 单根：三套规则都落到「大码 + 小码 + 小码」（键位串只有两个字母时 Y 回落 S）
    #[test]
    fn 单根三套规则同码() {
        let 键位 = 键位表(&[("一", ['O', 'd'])]);
        let 根 = vec!["一".to_string()];
        for 规则 in [编码规则::星陈, 编码规则::魔灵, 编码规则::灵明] {
            assert_eq!(码(&取码序列按规则(&根, &键位, 4, 规则), &键位), "odd");
        }
    }

    /// 全码长 / 动重：三字同码 → 后两个算非首选，各 +1 码并计入动重
    #[test]
    fn 统计重码与动重() {
        let 键位 = 键位表(&[("一", ['A', 'a'])]);
        let 拆分 = vec![
            ("甲".to_string(), vec!["一".to_string()], 70u64),
            ("乙".to_string(), vec!["一".to_string()], 20u64),
            ("丙".to_string(), vec!["一".to_string()], 10u64),
        ];
        let (文本, 全码条数, 简码条数, 缺根, 统) =
            生成表按方案(&键位, &拆分, &HashMap::new(), 编码规则::魔灵, 4);

        assert_eq!((全码条数, 简码条数, 缺根), (3, 0, 0));
        assert_eq!(文本.lines().count(), 3);
        // 码长分别是 3、4、4，按频次加权 = (3×70 + 4×20 + 4×10) / 100 = 3.3
        assert!((统.全码长 - 3.3).abs() < 1e-9, "全码长 {}", 统.全码长);
        // 动重 = (20 + 10) / 100 × 10000 = 3000‱
        assert!((统.动重 - 3000.0).abs() < 1e-9, "动重 {}", 统.动重);
        assert_eq!(统.重码组, 1);
        assert_eq!(统.重码字, 2);
    }

    /// 拆分表没有频次列时不统计，避免除零
    #[test]
    fn 无频次时统计为零() {
        let 键位 = 键位表(&[("一", ['A', 'a'])]);
        let 拆分 = vec![("甲".to_string(), vec!["一".to_string()], 0u64)];
        let (_, _, _, _, 统) = 生成表按方案(&键位, &拆分, &HashMap::new(), 编码规则::魔灵, 4);
        assert_eq!(统.总频, 0.0);
        assert_eq!(统.全码长, 0.0);
        assert_eq!(统.动重, 0.0);
    }

    /// 码位序号：A=0、S=1、Y=2，越界时 `编码` 回落到最后一个字母
    #[test]
    fn 码位越界回落到末字母() {
        assert_eq!(码位::A.序号(), 0);
        assert_eq!(码位::S.序号(), 1);
        assert_eq!(码位::Y.序号(), 2);
        let 键位 = 键位表(&[("一", ['A', 'a'])]);
        let 序列 = vec![("一", 码位::Y)];
        assert_eq!(码(&序列, &键位), "a");
    }
}
