// =========================================================================
// 📤 测评码表导出
// =========================================================================
//
// 把「优化后的 keymap + 纯拆分表 + 简码表」直接合成一份测评用码表：
//
//   字 <TAB> 码
//
// 简码行在前、全码行在后；码里不带尾随下划线（测评器会给短码自动补 `_`）。
// 取码规则与 prepare-inputs.sh 的魔灵分支一致：全根取 .A，二根补末根 .S，
// 末根补 .S/.Y，截断到最大码长；索引越界回落到该字根的末位键。
// =========================================================================

use std::collections::HashMap;
use std::fs;

/// 码位标签：A=首码、S=次码、Y=末码（对应 keymap 里键位串的第 1/2/3 个字母）
#[derive(Clone, Copy)]
enum 码位 {
    A,
    S,
    Y,
}

impl 码位 {
    fn 序号(self) -> usize {
        match self {
            码位::A => 0,
            码位::S => 1,
            码位::Y => 2,
        }
    }
}

/// 读取 keymap：`字根 <TAB> 键位串 <TAB> 频次`，只取前两列
fn 加载键位表(path: &str) -> HashMap<String, Vec<char>> {
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
fn 加载拆分表(path: &str) -> Vec<(char, Vec<String>, u64)> {
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
        let Some(字) = 列[0].chars().next() else { continue };
        let 根: Vec<String> = 列[1].split_whitespace().map(|s| s.to_string()).collect();
        let 频次 = 列.get(2).and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        表.push((字, 根, 频次));
    }
    表
}

/// 读取简码表：`字 <TAB> 码 <TAB> 频次`，码尾的 `_` 去掉
fn 加载简码表(path: &str) -> HashMap<char, String> {
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
        let Some(字) = 列[0].chars().next() else { continue };
        let 码 = 列[1].trim_end_matches('_');
        if !码.is_empty() {
            表.insert(字, 码.to_string());
        }
    }
    表
}

/// 魔灵取码序列：返回 (字根, 码位) 列表，已截断到最大码长
fn 取码序列<'a>(
    根: &'a [String],
    键位: &HashMap<String, Vec<char>>,
    最大码长: usize,
) -> Vec<(&'a str, 码位)> {
    if 根.is_empty() {
        return Vec::new();
    }
    let mut 序列: Vec<(&'a str, 码位)> = 根.iter().map(|r| (r.as_str(), 码位::A)).collect();

    let 有多个键 = |r: &str| 键位.get(r).map(|k| k.len() > 1).unwrap_or(false);

    let 末尾根 = if 根.len() == 2 {
        let 末 = 根[1].as_str();
        if 有多个键(末) {
            序列.push((末, 码位::S));
        }
        根[0].as_str()
    } else {
        根[根.len() - 1].as_str()
    };

    if 有多个键(末尾根) {
        序列.push((末尾根, 码位::S));
    }
    序列.push((末尾根, 码位::Y));

    序列.truncate(最大码长);
    序列
}

/// 按序列把字根翻成键位字母
fn 编码(序列: &[(&str, 码位)], 键位: &HashMap<String, Vec<char>>) -> Option<String> {
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

    let 最大码长 = 4usize;
    let mut 输出 = String::with_capacity(拆分.len() * 12);
    let mut 缺根 = 0usize;
    let mut 简码条数 = 0usize;
    let mut 全码条数 = 0usize;

    for (字, 根, _频次) in &拆分 {
        let 序列 = 取码序列(根, &键位, 最大码长);
        let Some(全码) = 编码(&序列, &键位) else {
            缺根 += 1;
            continue;
        };

        if let Some(简) = 简码.get(字) {
            // 简码必须是全码前缀，否则视为脏数据跳过
            if 全码.starts_with(简.as_str()) {
                输出.push_str(&format!("{}\t{}\n", 字, 简));
                简码条数 += 1;
            }
        }
        输出.push_str(&format!("{}\t{}\n", 字, 全码));
        全码条数 += 1;
    }

    fs::write(output_path, 输出).expect("无法写入输出文件");
    println!(
        "  完成：全码 {} 条，简码 {} 条，跳过（缺字根）{} 条",
        全码条数, 简码条数, 缺根
    );
    println!("  已写入 {}", output_path);
}
