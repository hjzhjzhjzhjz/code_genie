// =========================================================================
// 🀄 Rime 方案生成
// =========================================================================
//
// 从「键位表 + 拆分表 + 简码表（+ 词表）」直接产出一个可加载的 Rime 方案包：
//
//   <id>.schema.yaml        方案定义
//   <id>.dict.yaml          主词典（import_tables 汇聚下面三张）
//   <id>.full.dict.yaml     全码单字
//   <id>.short.dict.yaml    简码单字
//   <id>.words.dict.yaml    词组（给了词表才有）
//
// 排序一律 by_weight，权重取拆分表／词表里的频次。
// =========================================================================

use std::collections::HashMap;
use std::path::Path;

use crate::diagnose::取词码;
use crate::export_table::{加载拆分表, 加载简码表, 加载键位表, 生成表};

fn 版本号() -> String {
    chrono::Local::now().format("%Y%m%d").to_string()
}

/// 把「字\t码」表解析成 (全码表, 简码表)，各自带频次
fn 拆成两表(
    表文本: &str,
    频次: &HashMap<String, u64>,
) -> (Vec<(String, String, u64)>, Vec<(String, String, u64)>) {
    // 同一个字可能有两行：短的在前是简码，长的是全码
    let mut 分组: HashMap<String, Vec<String>> = HashMap::new();
    for 行 in 表文本.lines() {
        let Some((字, 码)) = 行.split_once('\t') else {
            continue;
        };
        let 码 = 码.trim();
        if 码.is_empty() {
            continue;
        }
        分组.entry(字.to_string()).or_default().push(码.to_string());
    }

    let mut 全 = Vec::new();
    let mut 简 = Vec::new();
    for (字, mut 码们) in 分组 {
        码们.sort_by_key(|s| s.chars().count());
        码们.dedup();
        let 全码 = 码们.last().unwrap().clone();
        let 频 = 频次.get(&字).copied().unwrap_or(0);
        if 码们.len() > 1 {
            let 短 = 码们.first().unwrap().clone();
            if 短 != 全码 {
                简.push((字.clone(), 短, 频));
            }
        }
        全.push((字, 全码, 频));
    }
    全.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    简.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    (全, 简)
}

fn 写词典(
    路径: &Path,
    名: &str,
    版本: &str,
    说明: &str,
    条目: &[(String, String, u64)],
) -> std::io::Result<usize> {
    let mut 文本 = String::with_capacity(条目.len() * 24 + 256);
    文本.push_str("# Rime dictionary\n# encoding: utf-8\n");
    文本.push_str(&format!("### {}\n\n", 说明));
    文本.push_str("---\n");
    文本.push_str(&format!("name: {}\n", 名));
    文本.push_str(&format!("version: \"{}\"\n", 版本));
    文本.push_str("sort: by_weight\n");
    文本.push_str("use_preset_vocabulary: false\n");
    文本.push_str("...\n\n");
    for (字, 码, 频) in 条目 {
        文本.push_str(&format!("{}\t{}\t{}\n", 字, 码, 频));
    }
    std::fs::write(路径, 文本)?;
    Ok(条目.len())
}

/// 生成整套 Rime 方案
#[allow(clippy::too_many_arguments)]
pub fn run(
    keymap: &str,
    chaifen: &str,
    simple: Option<&str>,
    words: Option<&str>,
    目录: &str,
    标识: &str,
    名称: &str,
    作者: &str,
) {
    let 键位 = 加载键位表(keymap);
    let 拆分 = 加载拆分表(chaifen);
    let 简码源 = simple.map(加载简码表).unwrap_or_default();
    let 版本 = 版本号();

    // 频次：字 → 拆分表里的频次
    let mut 频次: HashMap<String, u64> = HashMap::new();
    let mut 字码: HashMap<char, String> = HashMap::new();
    for (字, _, 频) in &拆分 {
        频次.insert(字.clone(), *频);
    }

    let (表文本, 全条, 简条, 缺根) = 生成表(&键位, &拆分, &简码源);
    let (全码们, 简码们) = 拆成两表(&表文本, &频次);

    // 单字最长码，供词组取码
    for (字, 码, _) in &全码们 {
        let mut it = 字.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            字码.insert(c, 码.clone());
        }
    }

    let 根 = Path::new(目录);
    std::fs::create_dir_all(根).ok();

    let 全名 = format!("{}.full", 标识);
    let 简名 = format!("{}.short", 标识);
    let 词名 = format!("{}.words", 标识);

    let 全数 = 写词典(
        &根.join(format!("{}.full.dict.yaml", 标识)),
        &全名,
        &版本,
        "全码单字",
        &全码们,
    )
    .map(|n| n)
    .unwrap_or(0);
    let 简数 = 写词典(
        &根.join(format!("{}.short.dict.yaml", 标识)),
        &简名,
        &版本,
        "简码单字（全码前缀）",
        &简码们,
    )
    .unwrap_or(0);

    // 词组
    let mut 词条数 = 0usize;
    let mut 有词 = false;
    if let Some(词表路径) = words {
        if let Ok(词文本) = std::fs::read_to_string(词表路径) {
            let mut 规: HashMap<usize, String> = HashMap::new();
            规.insert(2, "A1a2B1b2".to_string());
            规.insert(3, "A1B1C1C2".to_string());
            规.insert(4, "A1B1C1D1".to_string());
            规.insert(5, "A1B1C1X1".to_string());

            let mut 词们: Vec<(String, String, u64)> = Vec::new();
            for 行 in 词文本.lines() {
                let 行 = 行.trim();
                if 行.is_empty() || 行.starts_with('#') {
                    continue;
                }
                let 段: Vec<&str> = 行.split('\t').collect();
                if 段.len() < 2 {
                    continue;
                }
                let 字列: Vec<char> = 段[0].trim().chars().collect();
                if 字列.len() < 2 {
                    continue;
                }
                let 频: u64 = 段[1].trim().parse().unwrap_or(0);
                let k = if 字列.len() > 4 { 5 } else { 字列.len() };
                let 码 = 取词码(&字列, 规.get(&k).map(|s| s.as_str()).unwrap_or(""), &字码);
                if 码.is_empty() || 码.contains('%') {
                    continue;
                }
                词们.push((段[0].trim().to_string(), 码, 频));
            }
            词们.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
            词条数 = 写词典(
                &根.join(format!("{}.words.dict.yaml", 标识)),
                &词名,
                &版本,
                "词组（2字 a1a2b1b2／3字 a1b1c1c2／4字 a1b1c1d1／多字 a1b1c1x1）",
                &词们,
            )
            .unwrap_or(0);
            有词 = true;
        }
    }

    // 主词典
    let mut 主 = String::from("# Rime dictionary\n# encoding: utf-8\n\n---\n");
    主.push_str(&format!("name: {}\n", 标识));
    主.push_str(&format!("version: \"{}\"\n", 版本));
    主.push_str("import_tables:\n");
    主.push_str(&format!("  - {}\n", 全名));
    主.push_str(&format!("  - {}\n", 简名));
    if 有词 {
        主.push_str(&format!("  - {}\n", 词名));
    }
    主.push_str("sort: by_weight\nuse_preset_vocabulary: false\n...\n");
    std::fs::write(根.join(format!("{}.dict.yaml", 标识)), 主).ok();

    // 方案定义
    let 方案 = format!(
        r#"# Rime schema
# encoding: utf-8

schema:
  schema_id: {标识}
  name: {名称}
  version: "{版本}"
  author:
    - {作者}
  description: |
    四码定长，一／二／三级简码取全码前缀，空格上屏。

switches:
  - name: ascii_mode
    reset: 0
    states: [中文, 西文]
  - name: full_shape
    reset: 0
    states: [半角, 全角]
  - name: ascii_punct
    reset: 0
    states: [。，, ．，]

engine:
  processors:
    - ascii_composer
    - recognizer
    - key_binder
    - speller
    - punctuator
    - selector
    - navigator
    - express_editor
  segmentors:
    - ascii_segmentor
    - matcher
    - abc_segmentor
    - punct_segmentor
    - fallback_segmentor
  translators:
    - punct_translator
    - table_translator
  filters:
    - uniquifier

speller:
  alphabet: zyxwvutsrqponmlkjihgfedcba
  delimiter: "`"
  max_code_length: 4

translator:
  dictionary: {标识}
  prism: {标识}
  enable_user_dict: false
  enable_sentence: false
  enable_completion: false
  spelling_hints: 5

key_binder:
  import_preset: default

punctuator:
  import_preset: default

menu:
  page_size: 9
"#,
        标识 = 标识,
        名称 = 名称,
        版本 = 版本,
        作者 = 作者
    );
    std::fs::write(根.join(format!("{}.schema.yaml", 标识)), 方案).ok();

    // 安装说明
    let 说明 = format!(
        "{}（{}）Rime 方案\n\
         ================================\n\n\
         文件\n\
           {}.schema.yaml        方案定义\n\
           {}.dict.yaml          主词典（汇聚下面三张）\n\
           {}.full.dict.yaml     全码单字 {} 条\n\
           {}.short.dict.yaml    简码单字 {} 条\n\
           {}.words.dict.yaml    词组 {} 条\n\n\
         安装（小狼毫 / 鼠须管 / fcitx-rime）\n\
           1. 把本目录全部文件拷进用户资料夹的 rime/ 下\n\
           2. 重新部署（小狼毫：右键托盘 → 重新部署）\n\
           3. 方案选单里选「{}」\n\n\
         用法\n\
           全码 4 键，一／二／三级简码取全码前缀，打简码后按空格上屏。\n\
           4 键打满后空格仍取首选，与五笔一致。\n\n\
         备注\n\
           enable_sentence 与 enable_completion 均为 false：\n\
           不开整句联想、不做前缀补全，避免一按首键就刷出上千候选。\n",
        名称,
        标识,
        标识,
        标识,
        标识,
        全数.max(全条),
        标识,
        简数.max(简条),
        标识,
        词条数,
        名称
    );
    std::fs::write(根.join("安装说明.txt"), 说明).ok();

    println!("=== Rime 方案生成 ===");
    println!("  目录: {}", 目录);
    println!("  方案名: {} ({})", 名称, 标识);
    println!(
        "  全码 {} 条，简码 {} 条，词组 {} 条，缺根 {} 条",
        全数.max(全条),
        简数.max(简条),
        词条数,
        缺根
    );
    for f in [
        format!("{}.schema.yaml", 标识),
        format!("{}.dict.yaml", 标识),
        format!("{}.full.dict.yaml", 标识),
        format!("{}.short.dict.yaml", 标识),
    ] {
        let p = 根.join(&f);
        println!("    {:<24} {:>8} 字节", f, std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0));
    }
    if 有词 {
        let p = 根.join(format!("{}.words.dict.yaml", 标识));
        println!("    {:<24} {:>8} 字节", p.file_name().unwrap().to_string_lossy(), std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0));
    }
}
