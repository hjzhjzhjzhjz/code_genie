// =========================================================================
// 📤 测评码表导出
// =========================================================================
//
// 把「优化后的 keymap + 纯拆分表 + 简码表」直接合成一份测评用码表：
//
//   字 <TAB> 码
//
// 简码行在前、全码行在后；码里不带尾随下划线（测评器会给短码自动补 `_`）。
// 取码规则为星月（yustar）encoder 状态机：
//   1 根 A+S+S / 2 根 A1A2S2S1 / 3 根 A1A2A3S3 / ≥4 根 A1A2A3A_last，截断到最大码长；
// 键位串不足两位时回落大码。与 `rime_convert::单字码` 同口径。
// =========================================================================

use std::collections::HashMap;
use std::fs;

/// 码位标签：A=首码、S=次码（对应 keymap 里键位串的第 1/2 个字母）
///
/// 星月（yustar）规则不取末码，故没有 Y。
#[derive(Clone, Copy)]
pub(crate) enum 码位 {
    A,
    S,
}

impl 码位 {
    pub(crate) fn 序号(self) -> usize {
        match self {
            码位::A => 0,
            码位::S => 1,
        }
    }
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
        let 频次 = 列.get(2).and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
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
    _键位: &HashMap<String, Vec<char>>,
    最大码长: usize,
) -> Vec<(&'a str, 码位)> {
    let n = 根.len();
    if n == 0 {
        return Vec::new();
    }
    let mut 序列: Vec<(&'a str, 码位)> = match n {
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
            (根[n - 1].as_str(), 码位::A),
        ],
    };
    序列.truncate(最大码长);
    序列
}

/// 按序列把字根翻成键位字母
pub(crate) fn 编码(序列: &[(&str, 码位)], 键位: &HashMap<String, Vec<char>>) -> Option<String> {
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

/// 导出测评码表
pub fn run(keymap_path: &str, chaifen_path: &str, simple_path: Option<&str>, output_path: &str) {
    println!("=== CodeGenie 测评码表导出 ===");
    println!("  键位映射: {}", keymap_path);
    println!("  纯拆分表: {}", chaifen_path);
    if let Some(p) = simple_path {
        println!("  简码表:   {}", p);
    }
    println!("  输出文件: {}", output_path);

    let 键位 = 加载键位表(keymap_path);
    let 拆分 = 加载拆分表(chaifen_path);
    let 简码 = simple_path.map(加载简码表).unwrap_or_default();

    println!("  字根 {} 个，汉字 {} 个，简码 {} 条", 键位.len(), 拆分.len(), 简码.len());

    let (输出, 全码条数, 简码条数, 缺根) = 生成表(&键位, &拆分, &简码);
    fs::write(output_path, 输出).expect("无法写入输出文件");
    println!(
        "  完成：全码 {} 条，简码 {} 条，跳过（缺字根）{} 条",
        全码条数, 简码条数, 缺根
    );
    println!("  已写入 {}", output_path);
}

/// 把「键位 + 拆分 + 简码」合成码表文本
///
/// 返回 (文本, 全码条数, 简码条数, 缺根条数)
pub(crate) fn 生成表(
    键位: &HashMap<String, Vec<char>>,
    拆分: &[(String, Vec<String>, u64)],
    简码: &HashMap<String, String>,
) -> (String, usize, usize, usize) {
    let 最大码长 = 4usize;
    let mut 输出 = String::with_capacity(拆分.len() * 12);
    let mut 缺根 = 0usize;
    let mut 简码条数 = 0usize;
    let mut 全码条数 = 0usize;

    for (字, 根, _频次) in 拆分 {
        let 序列 = 取码序列(根, 键位, 最大码长);
        let Some(全码) = 编码(&序列, 键位) else {
            缺根 += 1;
            continue;
        };

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

    (输出, 全码条数, 简码条数, 缺根)
}
