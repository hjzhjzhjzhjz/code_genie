// =========================================================================
// 🗺 字根图生成
// =========================================================================
//
// 从 字根键位表.txt 直接生成「星月字根图」HTML：
//   1. 把 keymap 的 `字根.后缀` 合并成「大码+小码」（.0/.A=大码，.1/.S/.Y/.U=小码），
//      朴素字根取编码前两位；
//   2. 套用可选的 `字根映射表.tsv`（花括号名 → PUA 显示字）；
//   3. 渲染内嵌模板，写成自包含的 HTML。
//
// 规则与 code_genie.py 的 build_roots_tsv_from_keymap / _generate_html 一致。
// =========================================================================

use std::collections::HashMap;
use std::fs;

const 模板: &str = include_str!("../assets/root_chart.html");

/// 读取 keymap，生成 (字根名, 编码) 列表，顺序按「是否含元音、编码、字根名」排列
fn 生成字根表(keymap_path: &str) -> Vec<(String, String)> {
    let 文本 = fs::read_to_string(keymap_path).expect("无法读取 keymap");
    let mut 大小码: HashMap<String, (String, String)> = HashMap::new();
    let mut 朴素: HashMap<String, String> = HashMap::new();

    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 名 = 列[0];
        let 码 = 列[1];
        let 首字母 = 码
            .chars()
            .next()
            .map(|c| c.to_ascii_lowercase())
            .unwrap_or(' ');

        // 形如 干.0 / 干.A / 干.1 / 干.S / 干.Y / 干.U
        if let Some((基名, 标志)) = 名.rsplit_once('.') {
            let 是后缀 = 标志.len() == 1
                && matches!(
                    标志.chars().next().unwrap(),
                    '0' | '1' | 'U' | 'A' | 'S' | 'Y'
                );
            if 是后缀 {
                let 项 = 大小码
                    .entry(基名.to_string())
                    .or_insert_with(|| (String::new(), String::new()));
                if 标志 == "0" || 标志 == "A" {
                    if 项.0.is_empty() && 首字母 != ' ' {
                        项.0 = 首字母.to_string();
                    }
                } else if 项.1.is_empty() && 首字母 != ' ' {
                    项.1 = 首字母.to_string();
                }
                continue;
            }
        }
        朴素.insert(名.to_string(), 码.to_lowercase());
    }

    let mut 结果: Vec<(String, String)> = Vec::new();
    for (名, (大, 小)) in &大小码 {
        if !大.is_empty() && !小.is_empty() {
            结果.push((名.clone(), format!("{}{}", 大, 小)));
        } else if !大.is_empty() {
            结果.push((名.clone(), 大.clone()));
        }
    }
    for (名, 码) in &朴素 {
        if 大小码.contains_key(名) {
            continue;
        }
        let 截取: String = 码.chars().take(2).collect();
        if !截取.is_empty() {
            结果.push((名.clone(), 截取));
        }
    }

    结果.sort_by(|a, b| {
        let 甲 = (a.1.contains(['a', 'e', 'u', 'i', 'o']) as u8, &a.1, &a.0);
        let 乙 = (b.1.contains(['a', 'e', 'u', 'i', 'o']) as u8, &b.1, &b.0);
        甲.cmp(&乙)
    });
    结果
}

/// 读取 `字根映射表.tsv`：花括号名 → 显示字
fn 加载映射(path: &str) -> HashMap<String, String> {
    let mut 表 = HashMap::new();
    if let Ok(文本) = fs::read_to_string(path) {
        for 行 in 文本.lines() {
            let 列: Vec<&str> = 行.split('\t').collect();
            if 列.len() >= 2 && !列[0].trim().is_empty() {
                表.insert(列[0].trim().to_string(), 列[1].to_string());
            }
        }
    }
    表
}

fn 转义(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 生成字根图 HTML
pub fn run(keymap_path: &str, mapping_path: Option<&str>, output_path: &str) {
    println!("=== CodeGenie 字根图 ===");
    println!("  键位映射: {}", keymap_path);
    println!("  输出文件: {}", output_path);

    let 字根 = 生成字根表(keymap_path);
    if 字根.is_empty() {
        eprintln!("✗ 没有解析到任何字根");
        return;
    }
    let 映射 = mapping_path.map(加载映射).unwrap_or_default();
    println!("  字根 {} 个，映射表 {} 条", 字根.len(), 映射.len());

    let mut 行: Vec<(String, String, String, String)> = Vec::new();
    for (名, 码) in &字根 {
        let 首: String = 码
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        if 首.is_empty() {
            continue;
        }
        let 全码 = 码.to_uppercase();
        let (显示, 原名) = match 映射.get(名) {
            Some(符) => (符.clone(), 名.clone()),
            None => (名.clone(), String::new()),
        };
        行.push((首, 全码, 显示, 原名));
    }

    // 先按按键、再按编码、最后按显示字排序
    行.sort_by(|a, b| {
        (a.0.clone(), a.1.clone(), a.2.clone()).cmp(&(b.0.clone(), b.1.clone(), b.2.clone()))
    });

    let mut 编码集 = std::collections::HashSet::new();
    let mut 片段: Vec<String> = Vec::with_capacity(行.len());
    for (键, 码, 字, 名) in &行 {
        编码集.insert(码.clone());
        片段.push(format!(
            "    [\"{}\",\"{}\", \"{}\", \"{}\"]",
            键,
            码,
            转义(字),
            转义(名)
        ));
    }

    let 输出 = 模板
        .replace("__JS_DATA__", &片段.join(",\n"))
        .replace("__CODE_COUNT__", &编码集.len().to_string())
        .replace("__ROOT_COUNT__", &字根.len().to_string());

    fs::write(output_path, 输出).expect("无法写入字根图");
    println!("  完成：{} 个字根，{} 种编码", 字根.len(), 编码集.len());
    println!("  已写入 {}", output_path);
}
