// =========================================================================
// 🧪 形码测评诊断
// =========================================================================
//
// 对标 ceping.py / ceping_cizu.py（科学形码测评系统 v1.6.3 的 Python 复刻），
// 口径逐条对齐，用于补充宇浩没有的三块：手感七项、字频分段、词组测评。
//
//   当量      = 相邻键对当量之和（当量表按十分位存整数，避免浮点误差）
//   手感七项  = 左右互击 / 同指大跨排 / 同指小跨排 / 小指干扰 / 错手 / 三连击 / 超标键位
//   选码      = 无重码 > 短码 > 低当量
//   词组取码  = 2字 a1a2b1b2 / 3字 a1b1c1c2 / 4字 a1b1c1d1 / 多字 a1b1c1x1（末字首码）
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const 合法键: &str = "qwertyuiopasdfghjkl;zxcvbnm,./_";
const 左键: &str = "qwertasdfgzxcvb";
const 上排: &str = "qwertyuiop";
const 中排: &str = "asdfghjkl;";
const 下排: &str = "zxcvbnm,./";
/// 二码离散图的键序：字母在前，标点在后
const 键序: &str = "qwertyuiopasdfghjklzxcvbnm;,./";
const 标点: &str = "，。！？：";

/// 字频分段（字频序区间）
const 单字段表: [(&str, usize, usize); 5] = [
    ("1-300", 1, 300),
    ("301-500", 301, 500),
    ("501-1500", 501, 1500),
    ("1501-3000", 1501, 3000),
    ("3001-6000", 3001, 6000),
];

/// 词组分段
const 词段表: [(&str, usize, usize); 6] = [
    ("1-2000", 1, 2000),
    ("2001-5000", 2001, 5000),
    ("5001-10000", 5001, 10000),
    ("10001-20000", 10001, 20000),
    ("20001-40000", 20001, 40000),
    ("40001-60000", 40001, 60000),
];

fn 指(c: char) -> u8 {
    match c {
        'q' | 'a' | 'z' => 5,
        'w' | 's' | 'x' => 4,
        'e' | 'd' | 'c' => 3,
        'i' | 'k' => 3,
        'o' | 'l' => 4,
        'p' | ';' | '.' | '/' => 5,
        'r' | 'f' | 'v' | 't' | 'g' | 'b' | 'y' | 'h' | 'n' | 'u' | 'j' | 'm' | ',' => 2,
        _ => 0,
    }
}

fn 行(c: char) -> i32 {
    if 上排.contains(c) {
        1
    } else if 中排.contains(c) {
        2
    } else if 下排.contains(c) {
        3
    } else {
        0
    }
}

fn 同侧(a: char, b: char) -> bool {
    左键.contains(a) == 左键.contains(b)
}

/// 手感指标（与 get_sta 完全一致）
///
/// n=1 左右互击　n=2 同指大跨排　n=3 同指小跨排
/// n=4 小指干扰　n=5 错手　　　n=6 三连击　n=7 超标键位
fn 手感(s: &[char], n: u8) -> i64 {
    if s.len() <= 1 {
        return 0;
    }
    if n == 7 {
        return s.iter().filter(|c| !合法键.contains(**c)).count() as i64;
    }
    if n == 6 {
        for i in 0..s.len().saturating_sub(2) {
            if s[i] == s[i + 1] && s[i + 1] == s[i + 2] {
                return 1;
            }
        }
        return 0;
    }

    let mut 计 = 0i64;
    for i in 0..s.len() - 1 {
        let (a, b) = (s[i], s[i + 1]);
        let 同边 = 同侧(a, b);
        let 排差 = (行(a) - 行(b)).abs();
        let (fa, fb) = (指(a), 指(b));
        let 同指 = fa == fb && fa != 0;

        match n {
            1 => {
                if !同边 && a != '_' && b != '_' && a != '.' && b != '.' {
                    计 += 1;
                }
            }
            2 => {
                if 同边 && 同指 && 排差 == 2 {
                    计 += 1;
                }
            }
            3 => {
                if 同边 && 同指 && 排差 <= 1 && a != b {
                    计 += 1;
                }
            }
            4 => {
                if 同边 && (fa == 5 || fb == 5) {
                    计 += 1;
                }
            }
            5 => {
                if 同边 && 排差 == 2 && !同指 {
                    计 += 1;
                }
            }
            _ => {}
        }
    }
    计
}

fn 七项(s: &str) -> [i64; 7] {
    let 字: Vec<char> = s.chars().collect();
    let mut 果 = [0i64; 7];
    for n in 1..=7u8 {
        果[(n - 1) as usize] = 手感(&字, n);
    }
    果
}

// -------------------------------------------------------------------------
// 数据加载
// -------------------------------------------------------------------------

fn 有汉字(s: &str) -> bool {
    s.chars().any(|c| c as u32 > 0x7F)
}

/// 读码表，自动判方向（字在前还是码在前）
fn 读码表(路径: &Path) -> Vec<(String, String)> {
    let 文本 = std::fs::read_to_string(路径).unwrap_or_default();
    let mut 出 = Vec::new();
    for 原行 in 文本.lines() {
        let 行 = 原行.trim_end_matches('\r');
        if 行.trim().is_empty() || 行.trim_start().starts_with('#') {
            continue;
        }
        let mut 段: Vec<&str> = 行.split('\t').collect();
        if 段.len() < 2 {
            段 = 行.split_whitespace().collect();
        }
        if 段.len() < 2 {
            continue;
        }
        let (a, b) = (段[0].trim(), 段[1].trim());
        if 有汉字(a) {
            出.push((a.to_string(), b.to_string()));
        } else if 有汉字(b) {
            出.push((b.to_string(), a.to_string()));
        }
    }
    出
}

/// 十分位整数：与 int(Fraction(文本) * 10) 等价
fn 取十分位(s: &str) -> Option<i64> {
    let s = s.trim();
    let (负, 正文) = match s.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1i64, s),
    };
    let (mut 整, mut 小, mut 见点) = (0i64, 0i64, false);
    for c in 正文.chars() {
        if c == '.' {
            见点 = true;
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        let d = c as i64 - '0' as i64;
        if !见点 {
            整 = 整 * 10 + d;
        } else if 小 == 0 {
            小 = d;
        }
    }
    Some(负 * (整 * 10 + 小))
}

fn 读当量(路径: &Path) -> HashMap<(char, char), i64> {
    let 文本 = std::fs::read_to_string(路径).unwrap_or_default();
    let mut 表 = HashMap::new();
    for 原行 in 文本.lines() {
        let 行 = 原行.trim_end_matches('\r');
        if 行.trim().is_empty() || 行.trim_start().starts_with('#') {
            continue;
        }
        let 段: Vec<&str> = 行.split('\t').collect();
        if 段.len() < 2 {
            continue;
        }
        let k: Vec<char> = 段[0].trim().chars().collect();
        if k.len() != 2 {
            continue;
        }
        if let Some(v) = 取十分位(段[1]) {
            表.insert((k[0], k[1]), v);
        }
    }
    表
}

fn 读字频(路径: &Path) -> Vec<(char, f64)> {
    let 文本 = std::fs::read_to_string(路径).unwrap_or_default();
    let mut 出 = Vec::new();
    for 原行 in 文本.lines() {
        let 行 = 原行.trim_end_matches('\r');
        if 行.trim().is_empty() || 行.trim_start().starts_with('#') {
            continue;
        }
        let 段: Vec<&str> = 行.split('\t').collect();
        if 段.len() < 3 {
            continue;
        }
        let mut 字 = 段[1].trim().chars();
        let (Some(c), None) = (字.next(), 字.next()) else {
            continue;
        };
        if let Ok(f) = 段[2].trim().parse::<f64>() {
            出.push((c, f));
        }
    }
    出
}

fn 读词表(路径: &Path) -> Vec<(String, f64)> {
    let 文本 = std::fs::read_to_string(路径).unwrap_or_default();
    let mut 出 = Vec::new();
    for 原行 in 文本.lines() {
        let 行 = 原行.trim_end_matches('\r');
        if 行.trim().is_empty() || 行.trim_start().starts_with('#') {
            continue;
        }
        let mut 段: Vec<&str> = 行.split('\t').collect();
        if 段.len() < 2 {
            段 = 行.split_whitespace().collect();
        }
        if 段.len() < 2 {
            continue;
        }
        let f = 段[1].trim().parse::<f64>().unwrap_or(0.0);
        出.push((段[0].trim().to_string(), f));
    }
    出
}

// -------------------------------------------------------------------------
// 当量
// -------------------------------------------------------------------------

struct 当量表 {
    对: HashMap<(char, char), i64>,
}

impl 当量表 {
    /// 相邻键对当量之和，按十分位整数算
    fn 合计(&self, 码: &[char]) -> i64 {
        if 码.len() <= 1 {
            return 10;
        }
        let mut 总 = 0;
        for i in 0..码.len() - 1 {
            总 += self.对.get(&(码[i], 码[i + 1])).copied().unwrap_or(0);
        }
        总
    }
}

fn 码长(码: &str) -> usize {
    if 码.ends_with('_') {
        码.chars().count() - 1
    } else {
        码.chars().count()
    }
}

// -------------------------------------------------------------------------
// 单字
// -------------------------------------------------------------------------

#[derive(Clone)]
struct 单字行 {
    字: char,
    频: f64,
    码: String,
    码长: usize,
    键长: usize,
    当量: f64,
    键均: f64,
    手感: [i64; 7],
    缺字: i64,
    选重: i64,
    理论二简: i64,
}

/// 选码：无重码 > 短码 > 低当量
fn 选码(
    条目: &[(String, String)],
    字序: &HashMap<char, usize>,
    最大码长: usize,
    当量: &当量表,
) -> HashMap<usize, (String, i64)> {
    let mut 见过: HashSet<String> = HashSet::new();
    let mut 选中: HashMap<usize, (String, i64)> = HashMap::new();

    for (字, 码) in 条目 {
        let mut 本码 = 码.clone();
        if 本码.chars().count() < 最大码长 {
            本码.push('_');
        }
        let 字列: Vec<char> = 字.chars().collect();
        if 字列.len() != 1 {
            continue;
        }
        let 首次 = !见过.contains(&本码);
        if 首次 {
            见过.insert(本码.clone());
        }
        let Some(&序) = 字序.get(&字列[0]) else {
            continue;
        };

        match 选中.get_mut(&序) {
            None => {
                选中.insert(序, (本码, if 首次 { 0 } else { 1 }));
            }
            Some((旧码, 重)) => {
                let 旧长 = 码长(旧码);
                let 新长 = 码长(&本码);
                if 首次 {
                    if *重 == 1 {
                        *旧码 = 本码;
                        *重 = 0;
                    } else if 旧长 > 新长
                        || (旧长 == 新长
                            && 当量.合计(&旧码.chars().collect::<Vec<_>>())
                                > 当量.合计(&本码.chars().collect::<Vec<_>>()))
                    {
                        *旧码 = 本码;
                    }
                } else if *重 == 1
                    && (旧长 > 新长
                        || (旧长 == 新长
                            && 当量.合计(&旧码.chars().collect::<Vec<_>>())
                                > 当量.合计(&本码.chars().collect::<Vec<_>>())))
                {
                    *旧码 = 本码;
                }
            }
        }
    }
    选中
}

fn 建单字行(
    字频: &[(char, f64)],
    选中: &HashMap<usize, (String, i64)>,
    当量: &当量表,
) -> Vec<单字行> {
    let mut 已用二码: HashSet<String> = HashSet::new();
    let mut 出 = Vec::with_capacity(字频.len());

    for (i, (字, 频)) in 字频.iter().enumerate() {
        let 序 = i + 1;
        let (码, 重) = match 选中.get(&序) {
            Some((m, h)) => (m.clone(), *h),
            None => (String::new(), 0),
        };
        let 列: Vec<char> = 码.chars().collect();
        let 实长 = 码长(&码);
        let 键长 = 列.len();
        let 合计 = 当量.合计(&列);
        let 键均 = if 码.is_empty() {
            0.0
        } else if 键长 == 1 {
            1.0
        } else {
            合计 as f64 / 10.0 / (键长 - 1) as f64
        };

        let 二简 = if 实长 >= 2 {
            let 前二: String = 列.iter().take(2).collect();
            if 已用二码.contains(&前二) {
                ""
            } else {
                已用二码.insert(前二.clone());
                if 前二.len() == 2 {
                    "是"
                } else {
                    ""
                }
            }
        } else {
            ""
        };

        出.push(单字行 {
            字: *字,
            频: *频,
            码: 码.clone(),
            码长: 实长,
            键长,
            当量: 合计 as f64 / 10.0,
            键均,
            手感: 七项(&码),
            缺字: if 码.is_empty() { 1 } else { 0 },
            选重: 重,
            理论二简: if 二简.is_empty() { 0 } else { 1 },
        });
    }
    出
}

/// 17 项统计：1码..4码计数、选重、理论二简、加权键长/字均/键均、七项手感、缺字
fn 单字分段(行: &[单字行], 起: usize, 止: usize) -> [f64; 17] {
    let 段 = &行[(起 - 1).min(行.len())..止.min(行.len())];
    let 频和: f64 = 段.iter().map(|r| r.频).sum();
    let mut 果 = [0.0f64; 17];

    for r in 段 {
        match r.码长 {
            1 => 果[0] += 1.0,
            2 => 果[1] += 1.0,
            3 => 果[2] += 1.0,
            4 => 果[3] += 1.0,
            _ => {}
        }
        果[4] += r.选重 as f64;
        果[5] += r.理论二简 as f64;
        果[6] += r.频 * r.键长 as f64;
        果[7] += r.频 * r.当量;
        果[8] += r.频 * r.键均;
        for k in 0..7 {
            果[9 + k] += r.手感[k] as f64;
        }
        果[16] += r.缺字 as f64;
    }

    if 频和 > 0.0 {
        for k in 6..9 {
            果[k] /= 频和;
        }
    } else {
        for k in 6..9 {
            果[k] = 0.0;
        }
    }
    果
}

/// 加权比重：前 3 项码长占比、选重占比、理论二简占比、六项手感占比
fn 单字比重(行: &[单字行], 止: usize) -> [f64; 17] {
    let 段 = &行[..止.min(行.len())];
    let 频和: f64 = 段.iter().map(|r| r.频).sum();
    let 分母: f64 = 段.iter().map(|r| (r.键长 as f64 - 1.0) * r.频).sum();
    let mut 果 = [f64::NAN; 17];

    if 频和 > 0.0 {
        for m in 1..=4 {
            果[m - 1] = 段
                .iter()
                .filter(|r| r.码长 == m)
                .map(|r| r.频)
                .sum::<f64>()
                / 频和;
        }
        果[4] = 段.iter().filter(|r| r.选重 == 1).map(|r| r.频).sum::<f64>() / 频和;
        果[5] = 段
            .iter()
            .filter(|r| r.理论二简 == 1)
            .map(|r| r.频)
            .sum::<f64>()
            / 频和;
    }
    if 分母 > 0.0 {
        for k in 0..6 {
            果[9 + k] = 段.iter().map(|r| r.手感[k] as f64 * r.频).sum::<f64>() / 分母;
        }
    }
    果
}

// -------------------------------------------------------------------------
// 词组
// -------------------------------------------------------------------------

#[derive(Clone)]
struct 词组行 {
    词: String,
    频: Option<f64>,
    码: String,
    当量: f64,
    键长: usize,
    手感: [i64; 7],
    缺词: i64,
    选重: Option<i64>,
}

fn 去标点(s: &str) -> String {
    s.chars().filter(|c| !标点.contains(*c)).collect()
}

/// 取词规则：两字符一组，`x` 表末字，字母表第 N 字，数字表该字第 N 码
pub(crate) fn 取词码(词: &[char], 规则: &str, 字码: &HashMap<char, String>) -> String {
    let 单码 = |c: char| -> String {
        let u = c as u32;
        if (65..=122).contains(&u) {
            c.to_ascii_lowercase().to_string()
        } else {
            字码.get(&c).cloned().unwrap_or_else(|| "%".to_string())
        }
    };

    if 词.len() == 1 {
        return 单码(词[0]);
    }

    let 记: Vec<char> = 规则.chars().collect();
    let mut 码 = String::new();
    let mut i = 0;
    while i + 1 < 记.len() {
        let 位 = 记[i];
        let 序 = 记[i + 1].to_digit(10).unwrap_or(0) as usize;
        i += 2;

        let 取字 = if 位.eq_ignore_ascii_case(&'x') {
            词.last().copied()
        } else {
            let ci = (位.to_ascii_lowercase() as u32).wrapping_sub('a' as u32) as usize;
            词.get(ci).copied()
        };
        if let Some(c) = 取字 {
            let 码串 = 单码(c);
            if 序 >= 1 {
                if let Some(ch) = 码串.chars().nth(序 - 1) {
                    码.push(ch);
                }
            }
        }
    }
    码
}

fn 建词组行(
    词表: &[(String, f64)],
    字码: &HashMap<char, String>,
    规则: &HashMap<usize, String>,
    最大码长: usize,
    当量: &当量表,
) -> Vec<词组行> {
    let mut 见过: HashSet<String> = HashSet::new();
    let mut 出 = Vec::with_capacity(词表.len());

    for (词, 频) in 词表 {
        let 净 = 去标点(词);
        let 字列: Vec<char> = 净.chars().collect();

        let 码 = if 字列.is_empty() {
            "%".to_string()
        } else if 字列.len() == 1 {
            let u = 字列[0] as u32;
            if (65..=122).contains(&u) {
                字列[0].to_ascii_lowercase().to_string()
            } else {
                字码
                    .get(&字列[0])
                    .cloned()
                    .unwrap_or_else(|| "%".to_string())
            }
        } else {
            let k = if 字列.len() > 4 { 5 } else { 字列.len() };
            取词码(&字列, 规则.get(&k).map(|s| s.as_str()).unwrap_or(""), 字码)
        };

        if 码.contains('%') {
            出.push(词组行 {
                词: 词.clone(),
                频: None,
                码: String::new(),
                当量: 0.0,
                键长: 0,
                手感: [0; 7],
                缺词: 1,
                选重: None,
            });
            continue;
        }

        let mut 本码 = 码;
        if 本码.chars().count() < 最大码长 {
            本码.push('_');
        }
        let 重 = if 见过.contains(&本码) { 1 } else { 0 };
        见过.insert(本码.clone());
        let 列: Vec<char> = 本码.chars().collect();

        出.push(词组行 {
            词: 词.clone(),
            频: Some(*频),
            码: 本码.clone(),
            当量: 当量.合计(&列) as f64 / 10.0,
            键长: 列.len(),
            手感: 七项(&本码),
            缺词: 0,
            选重: Some(重),
        });
    }
    出
}

/// 词组分段：选重计数、加权词均当量、八项手感计数
fn 词分段(行: &[词组行], 起: usize, 止: usize) -> [f64; 10] {
    let 段 = &行[(起 - 1).min(行.len())..止.min(行.len())];
    let 有效: Vec<&词组行> = 段.iter().filter(|r| r.频.is_some()).collect();
    let 频和: f64 = 有效.iter().map(|r| r.频.unwrap()).sum();
    let mut 果 = [0.0f64; 10];

    果[0] = 有效.iter().filter(|r| r.选重 == Some(1)).count() as f64;
    果[1] = if 频和 > 0.0 {
        有效.iter().map(|r| r.频.unwrap() * r.当量).sum::<f64>() / 频和
    } else {
        0.0
    };
    for k in 0..7 {
        果[2 + k] = 有效.iter().map(|r| r.手感[k] as f64).sum();
    }
    果[9] = 有效.iter().map(|r| r.缺词 as f64).sum();
    果
}

fn 词比重(行: &[词组行], 起: usize, 止: usize) -> [f64; 8] {
    let 段 = &行[(起 - 1).min(行.len())..止.min(行.len())];
    let 有效: Vec<&词组行> = 段.iter().filter(|r| r.频.is_some()).collect();
    let 分母: f64 = 有效
        .iter()
        .map(|r| (r.键长 as f64 - 1.0) * r.频.unwrap())
        .sum();
    let mut 果 = [f64::NAN; 8];
    if 分母 > 0.0 {
        for k in 0..7 {
            果[k] = 有效
                .iter()
                .map(|r| r.手感[k] as f64 * r.频.unwrap())
                .sum::<f64>()
                / 分母;
        }
        // 第 8 项是缺词，有效行恒为 0
        果[7] = 0.0;
    }
    果
}

// -------------------------------------------------------------------------
// 报告
// -------------------------------------------------------------------------

struct 方案结果 {
    名: String,
    单字段: Vec<[f64; 17]>,
    单字小计: [f64; 17],
    单字总计: [f64; 17],
    单字比重1500: [f64; 17],
    单字比重6000: [f64; 17],
    有码字数: usize,
    词段: Vec<[f64; 10]>,
    词小计: [f64; 10],
    词总计: [f64; 10],
    词比重: [f64; 8],
    词选重占比: f64,
    词条数: usize,
    缺词数: usize,
    二码: HashMap<(char, char), i64>,
}

fn 算方案(名: &str, 码表: &Path, 数据: &Path) -> Option<方案结果> {
    let 当量 = 当量表 {
        对: 读当量(&数据.join("当量表.txt")),
    };
    let 字频 = 读字频(&数据.join("字频表.txt"));
    let 词表 = 读词表(&数据.join("词表.txt"));
    if 字频.is_empty() {
        return None;
    }

    let 条目 = 读码表(码表);
    let 字序: HashMap<char, usize> = 字频
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (*c, i + 1))
        .collect();
    let 选中 = 选码(&条目, &字序, 4, &当量);
    let 单字 = 建单字行(&字频, &选中, &当量);

    let mut 单字段 = Vec::new();
    for (_, 起, 止) in 单字段表 {
        单字段.push(单字分段(&单字, 起, 止));
    }
    let 单字小计 = 单字分段(&单字, 1, 1500);
    let 单字总计 = 单字分段(&单字, 1, 6000);

    let mut 二码: HashMap<(char, char), i64> = HashMap::new();
    for r in &单字 {
        let 列: Vec<char> = r.码.chars().collect();
        if 列.len() >= 2 {
            *二码.entry((列[0], 列[1])).or_insert(0) += 1;
        }
    }

    // 词组：单字取最长码
    let mut 字码: HashMap<char, String> = HashMap::new();
    for (字, 码) in &条目 {
        let mut 字列 = 字.chars();
        if let (Some(c), None) = (字列.next(), 字列.next()) {
            let 旧 = 字码.get(&c).map(|s| s.chars().count()).unwrap_or(0);
            if 码.chars().count() > 旧 {
                字码.insert(c, 码.clone());
            }
        }
    }
    let mut 规则: HashMap<usize, String> = HashMap::new();
    规则.insert(2, "A1a2B1b2".to_string());
    规则.insert(3, "A1B1C1C2".to_string());
    规则.insert(4, "A1B1C1D1".to_string());
    // 多字词取末字首码
    规则.insert(5, "A1B1C1X1".to_string());

    let 词行 = 建词组行(&词表, &字码, &规则, 4, &当量);
    let mut 词段 = Vec::new();
    for (_, 起, 止) in 词段表 {
        词段.push(词分段(&词行, 起, 止));
    }
    let 缺词数 = 词行.iter().filter(|r| r.缺词 == 1).count();

    Some(方案结果 {
        名: 名.to_string(),
        单字段,
        单字小计,
        单字总计,
        单字比重1500: 单字比重(&单字, 1500),
        单字比重6000: 单字比重(&单字, 6000),
        有码字数: 选中.len(),
        词段,
        词小计: 词分段(&词行, 1, 10000),
        词总计: 词分段(&词行, 1, 60000),
        词选重占比: {
            let 有效: Vec<&词组行> = 词行.iter().filter(|r| r.频.is_some()).collect();
            let 频和: f64 = 有效.iter().map(|r| r.频.unwrap()).sum();
            if 频和 > 0.0 {
                有效
                    .iter()
                    .filter(|r| r.选重 == Some(1))
                    .map(|r| r.频.unwrap())
                    .sum::<f64>()
                    / 频和
            } else {
                0.0
            }
        },
        词比重: 词比重(&词行, 1, 60000),
        词条数: 词表.len(),
        缺词数,
        二码,
    })
}

fn 百分比(x: f64) -> String {
    if x.is_nan() {
        "/".to_string()
    } else {
        format!("{:.2}%", x * 100.0)
    }
}

fn 三位(x: f64) -> String {
    if x.is_nan() {
        "/".to_string()
    } else {
        format!("{:.3}", x)
    }
}

/// 万分比：选重占比用这个口径显示，负零统一成正零
fn 万分比(x: f64) -> String {
    if x.is_nan() {
        "/".to_string()
    } else if x.abs() < 1e-12 {
        "0.00".to_string()
    } else {
        format!("{:.2}", x * 10000.0)
    }
}

fn 整数(x: f64) -> String {
    if x.is_nan() {
        "/".to_string()
    } else {
        format!("{}", x.round() as i64)
    }
}

fn 写单字表(s: &mut String, r: &方案结果) {
    s.push_str(&format!(
        "\n#### 单字（{} / {} 字有码）\n\n",
        r.有码字数,
        6000
    ));
    s.push_str("| 范围 | 1码 | 2码 | 3码 | 4码 | 选重 | 理论二简 | 加权键长 | 字均当量 | 键均当量 | 左右互击 | 同指大跨 | 同指小跨 | 小指干扰 | 错手 | 三连击 | 超标键位 | 缺字 |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");

    let 段宽 = [300.0, 200.0, 1000.0, 1500.0, 3000.0];
    for (i, (名, _, _)) in 单字段表.into_iter().enumerate() {
        let v = r.单字段[i];
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            名,
            百分比(v[0] / 段宽[i]),
            百分比(v[1] / 段宽[i]),
            百分比(v[2] / 段宽[i]),
            百分比(v[3] / 段宽[i]),
            整数(v[4]),
            整数(v[5]),
            三位(v[6]),
            三位(v[7]),
            三位(v[8]),
            整数(v[9]),
            整数(v[10]),
            整数(v[11]),
            整数(v[12]),
            整数(v[13]),
            整数(v[14]),
            整数(v[15]),
            整数(v[16]),
        ));
    }

    let 行们: [(&str, &[f64; 17], f64); 2] = [
        ("小计（1-1500）", &r.单字小计, 1500.0),
        ("总计（1-6000）", &r.单字总计, 6000.0),
    ];
    for (名, v, 宽) in 行们 {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            名,
            百分比(v[0] / 宽),
            百分比(v[1] / 宽),
            百分比(v[2] / 宽),
            百分比(v[3] / 宽),
            整数(v[4]),
            整数(v[5]),
            三位(v[6]),
            三位(v[7]),
            三位(v[8]),
            整数(v[9]),
            整数(v[10]),
            整数(v[11]),
            整数(v[12]),
            整数(v[13]),
            整数(v[14]),
            整数(v[15]),
            整数(v[16]),
        ));
    }

    for (名, v) in [
        ("加权比重（1-1500）", &r.单字比重1500),
        ("加权比重（1-6000）", &r.单字比重6000),
    ] {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            名,
            百分比(v[0]),
            百分比(v[1]),
            百分比(v[2]),
            百分比(v[3]),
            万分比(v[4]),
            百分比(v[5]),
            "/",
            "/",
            "/",
            百分比(v[9]),
            百分比(v[10]),
            百分比(v[11]),
            百分比(v[12]),
            百分比(v[13]),
            百分比(v[14]),
            "/",
            "/",
        ));
    }
}

fn 写词组表(s: &mut String, r: &方案结果) {
    s.push_str(&format!(
        "\n#### 词组（{} 条，缺词 {}）\n\n",
        r.词条数, r.缺词数
    ));
    s.push_str("| 范围 | 选重 | 加权词均当量 | 左右互击 | 同指大跨 | 同指小跨 | 小指干扰 | 错手 | 三连击 | 超标键位 | 缺词 |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");

    for (i, (名, _, _)) in 词段表.into_iter().enumerate() {
        let v = r.词段[i];
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            名,
            整数(v[0]),
            三位(v[1]),
            整数(v[2]),
            整数(v[3]),
            整数(v[4]),
            整数(v[5]),
            整数(v[6]),
            整数(v[7]),
            整数(v[8]),
            整数(v[9]),
        ));
    }
    let v = &r.词总计;
    s.push_str(&format!(
        "| 总计 | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        整数(v[0]),
        三位(v[1]),
        整数(v[2]),
        整数(v[3]),
        整数(v[4]),
        整数(v[5]),
        整数(v[6]),
        整数(v[7]),
        整数(v[8]),
        整数(v[9]),
    ));
    let b = &r.词比重;
    s.push_str(&format!(
        "| 加权比重 | {} | / | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        百分比(r.词选重占比),
        百分比(b[0]),
        百分比(b[1]),
        百分比(b[2]),
        百分比(b[3]),
        百分比(b[4]),
        百分比(b[5]),
        百分比(b[6]),
        百分比(b[7]),
    ));
}

fn 写离散图(s: &mut String, r: &方案结果) {
    s.push_str("\n#### 二码离散图\n\n");
    s.push_str("纵坐标先打。每格是该二码开头的编码数，越亮越密。\n\n```\n    ");
    for k in 键序.chars() {
        s.push(k);
    }
    s.push('\n');

    let 最大 = r.二码.values().copied().max().unwrap_or(1).max(1) as f64;
    for a in 键序.chars() {
        let mut 行 = format!("{} | ", a);
        for b in 键序.chars() {
            let n = r.二码.get(&(a, b)).copied().unwrap_or(0);
            行.push(if n == 0 {
                '·'
            } else {
                let 档 = (((n as f64 / 最大) * 7.0).ceil() as usize).clamp(1, 7);
                ['▁', '▂', '▃', '▄', '▅', '▆', '█'][档 - 1]
            });
        }
        行.push('\n');
        s.push_str(&行);
    }
    s.push_str("```\n");
}

/// 入口
pub fn run(目录: &str, 数据目录: &str, 文件名: &str, 汇总: Option<&str>) {
    let 根 = PathBuf::from(目录);
    let 数据 = PathBuf::from(数据目录);

    let mut 清单: Vec<PathBuf> = Vec::new();
    let mut 栈 = vec![根.clone()];
    while let Some(当前) = 栈.pop() {
        let Ok(条目) = std::fs::read_dir(&当前) else {
            continue;
        };
        for 项 in 条目.flatten() {
            let 路径 = 项.path();
            if 路径.is_dir() {
                栈.push(路径);
            } else if 路径.file_name().map(|n| n == 文件名).unwrap_or(false) {
                清单.push(路径);
            }
        }
    }
    清单.sort();
    if 清单.is_empty() {
        eprintln!("✗ 没有找到码表");
        return;
    }

    println!("=== 形码测评诊断（{} 份）===", 清单.len());
    let mut 全部 = Vec::new();
    for 路径 in &清单 {
        let 名 = 路径
            .strip_prefix(&根)
            .ok()
            .and_then(|p| p.parent())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "码表".to_string());
        match 算方案(&名, 路径, &数据) {
            Some(r) => {
                println!(
                    "  {} · 单字键长 {:.4} · 词组选重 {}",
                    r.名, r.单字总计[6], r.词总计[0]
                );
                全部.push(r);
            }
            None => eprintln!("⚠️ {} 跳过（缺数据）", 名),
        }
    }
    if 全部.is_empty() {
        return;
    }

    全部.sort_by(|a, b| {
        a.单字总计[6]
            .partial_cmp(&b.单字总计[6])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut md = String::from("## 形码测评诊断\n\n");
    md.push_str("口径对齐科学形码测评系统 v1.6.3。多字词按末字首码取码。\n\n### 总览\n\n");
    md.push_str("| 方案 | 加权键长 | 字均当量 | 键均当量 | 单字选重 | 理论二简 | 词组选重 | 词组当量 |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
    for r in &全部 {
        md.push_str(&format!(
            "| {} | {:.4} | {:.4} | {:.4} | {} | {} | {} | {:.4} |\n",
            r.名,
            r.单字总计[6],
            r.单字总计[7],
            r.单字总计[8],
            整数(r.单字总计[4]),
            整数(r.单字总计[5]),
            整数(r.词总计[0]),
            r.词总计[1]
        ));
    }

    写单字表(&mut md, &全部[0]);
    写词组表(&mut md, &全部[0]);
    写离散图(&mut md, &全部[0]);

    println!("\n{}", md);
    if let Some(路径) = 汇总 {
        match std::fs::write(路径, &md) {
            Ok(_) => println!("诊断报告已写入 {}", 路径),
            Err(e) => eprintln!("⚠️ 写不了诊断报告: {}", e),
        }
    }
}
