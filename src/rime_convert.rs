// =========================================================================
// 🔄 星月 RIME 方案包 → 魔灵 RIME 方案包（只换码）
// =========================================================================
//
// 以现成的星月（starmoon-ime）方案包为底包，复制一份出来，把里面所有**单字码、
// 词码、简码**换成按优化结果（roots.tsv）重算的字母；schema / lua / 注音 / 字集
// / 码位等其余内容原样保留。
//
// 要换码的文件：
//   `<前缀>_chaifen.dict.yaml`          字\t[拆分式,全码,分组简码,注音,保留,字集,码位]
//   `yuhao/<前缀>.full.dict.yaml`       字\t码\t频次
//   `yuhao/<前缀>_sc.words.dict.yaml`   词\t码
//   `yuhao/<前缀>_sc.quick.dict.yaml`   字\t简码（保留原档位，只换字母）
//   `yuhao/<前缀>_sc.short.dict.yaml`   词\t简码（保留原档位，只换字母）
//   `yuhao/<前缀>.roots.dict.yaml`      字根提示（由 zigen-*.csv 重新生成）
//
// 不动的：`*_sc.dict.yaml` 主词典（import_tables 只做改名）、`*.schema.yaml`、
// `lua/yuhao/*`、`yuhao.*` 符号表 —— 仅做「星月 → 魔灵」文本替换。
//
// 单字编码规则（用星月 123372 条真实数据全量反推验证，0 不符）：
//   1 根 : A + S + Y
//   2 根 : A1 A2 S2 S1
//   ≥3 根: A1 A2 … An + S_末，截断到 4 码
// 其中 A=键位串首字母、S=第 2 个字母（不足两字母则取首字母）、Y=末字母。
//
// 词码规则与星月 `encoder.rules` 一致：
//   2 字 AaAbBaBb / 3 字 AaBaCaCb / 4+ 字 AaBaCaZa
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 字根 → 键位串（如 `白` → `Fk`），取自 `roots.tsv`
type 字根表 = HashMap<String, String>;
/// 字 → 拆分根序列，取自 `chaifen-all.txt`
type 拆分表 = HashMap<String, Vec<String>>;

/// `rime-convert` 的入参
pub struct 参数 {
    /// 底包目录（starmoon-ime 根目录）
    pub 底包目录: String,
    /// 算码输出目录（含 `roots.tsv`、`zigen-<标识>.csv`）
    pub 算码目录: String,
    /// 拆分表（`chaifen-all.txt`：字\t根 根\t频次）
    pub 拆分文件: String,
    /// 输出目录
    pub 输出目录: String,
    /// 新方案标识（文件名前缀 + schema_id）
    pub 标识: String,
    /// 新方案显示名
    pub 名称: String,
    /// 底包的方案前缀
    pub 源前缀: String,
    /// 底包的方案显示名（正文替换用）
    pub 源名称: String,
}

// =========================================================================
// 入口
// =========================================================================

/// 执行转换。返回 (改写条数, 告警数)。
pub fn run(参: &参数) -> Result<(usize, usize), String> {
    let 底包 = Path::new(&参.底包目录);
    let 拆分源 = 底包.join(format!("{}_chaifen.dict.yaml", 参.源前缀));
    if !拆分源.is_file() {
        return Err(format!(
            "底包目录不对：{} 下没有 {}",
            参.底包目录,
            文件名(&拆分源)
        ));
    }

    let 算码 = Path::new(&参.算码目录);
    let 字根 = 读字根表(&算码.join("roots.tsv"))?;
    let 拆分 = 读拆分表(Path::new(&参.拆分文件))?;
    println!("  roots.tsv: {} 个字根；拆分表: {} 个字", 字根.len(), 拆分.len());

    // ---------- 1. 复制 ----------
    let 输出 = PathBuf::from(&参.输出目录);
    if 输出.exists() {
        crate::fsutil::archive_if_exists(&输出);
    }
    复制底包(底包, &输出, &参.源前缀)?;
    println!("(1) 已复制底包到 {}", 输出.display());

    // ---------- 2. 改名 + 正文替换 ----------
    let 改名 = 重命名(&输出, &参.源前缀, &参.标识)?;
    let 替换 = 替换正文(&输出, &参.源前缀, &参.标识, &参.源名称, &参.名称)?;
    println!("(2) 改名 {} 个，正文替换 {} 个文件", 改名, 替换);

    // ---------- 3. 拆分表 ----------
    let 拆分路径 = 输出.join(format!("{}_chaifen.dict.yaml", 参.标识));
    let 根清单 = 根清单(&拆分, &字根);
    let (已解析, 改, 留) = 重写拆分表(&拆分路径, &拆分, &字根, &根清单)?;
    let mut 警告 = 留;
    println!("(3) {}: 重写 {} 行，保留 {} 行", 文件名(&拆分路径), 改, 留);

    // ---------- 4. 全码表 ----------
    let 全码路径 = 输出.join("yuhao").join(format!("{}.full.dict.yaml", 参.标识));
    let (字码, 全条, 跳) = 重写全码表(&全码路径, &已解析, &字根)?;
    警告 += 跳;
    println!("(4) {}: {} 条全码", 文件名(&全码路径), 全条);

    // ---------- 5. 词表 ----------
    let 词路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.words.dict.yaml", 参.标识));
    let (词码表, 词条, 跳) = 重写词表(&词路径, &字码)?;
    警告 += 跳;
    println!("(5) {}: {} 条词", 文件名(&词路径), 词条);

    // ---------- 6. 快码（单字简码）----------
    let 快码路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.quick.dict.yaml", 参.标识));
    let (快条, 跳) = 重写简码表(&快码路径, &字码)?;
    警告 += 跳;
    println!("(6) {}: {} 条快码", 文件名(&快码路径), 快条);

    // ---------- 7. 简码（词组简码）----------
    let 简码路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.short.dict.yaml", 参.标识));
    let (简条, 跳) = 重写简码表(&简码路径, &词码表)?;
    警告 += 跳;
    println!("(7) {}: {} 条简码", 文件名(&简码路径), 简条);

    // ---------- 8. 字根提示 ----------
    let 字根路径 = 输出.join("yuhao").join(format!("{}.roots.dict.yaml", 参.标识));
    let 图 = 算码.join(format!("zigen-{}.csv", 参.标识));
    let 字根条 = 写字根表(&字根路径, &图, &参.名称)?;
    println!("(8) {}: {} 条字根提示（来源 {}）", 文件名(&字根路径), 字根条, 文件名(&图));

    println!("\n✅ 魔灵方案包已生成：{}", 输出.display());
    println!("   装法：把该目录内容拷进 Rime 用户目录，重新部署后选「{}」", 参.名称);
    if 警告 > 0 {
        println!("⚠️ 共 {} 处告警，详见上方输出", 警告);
    }
    Ok((改 + 全条 + 词条 + 快条 + 简条 + 字根条, 警告))
}

// =========================================================================
// 文件工具
// =========================================================================

fn 文件名(路径: &Path) -> String {
    路径
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| 路径.display().to_string())
}

fn 读文本(路径: &Path) -> Result<String, String> {
    std::fs::read_to_string(路径).map_err(|e| format!("读不了 {}: {}", 路径.display(), e))
}

fn 写文本(路径: &Path, 内容: &str) -> Result<(), String> {
    std::fs::write(路径, 内容).map_err(|e| format!("写不了 {}: {}", 路径.display(), e))
}

fn 读行(路径: &Path) -> Result<Vec<String>, String> {
    Ok(读文本(路径)?.lines().map(|s| s.to_string()).collect())
}

fn 拼行(行: &[String]) -> String {
    let mut 文 = String::with_capacity(行.iter().map(|s| s.len() + 1).sum());
    for 一 in 行 {
        文.push_str(一);
        文.push('\n');
    }
    文
}

/// 复制底包里的星月子集（跳过 build/、其它方案、用户状态文件）
fn 复制底包(底包: &Path, 输出: &Path, 前缀: &str) -> Result<(), String> {
    std::fs::create_dir_all(输出).map_err(|e| format!("建目录 {} 失败: {}", 输出.display(), e))?;
    for 一条 in std::fs::read_dir(底包)
        .map_err(|e| format!("读目录 {} 失败: {}", 底包.display(), e))?
        .flatten()
    {
        let 路径 = 一条.path();
        let 名 = 文件名(&路径);
        if 路径.is_file() {
            if 名.starts_with(&format!("{}_", 前缀)) || 名.starts_with("yuhao.") {
                复制文件(&路径, &输出.join(&名))?;
            }
        } else if 名 == "yuhao" {
            复制目录(&路径, &输出.join("yuhao"))?;
        } else if 名 == "lua" {
            let 子 = 路径.join("yuhao");
            if 子.is_dir() {
                复制目录(&子, &输出.join("lua/yuhao"))?;
            }
        }
    }
    let 默认 = 底包.join("default.custom.yaml");
    if 默认.is_file() {
        复制文件(&默认, &输出.join("default.custom.yaml"))?;
    }
    Ok(())
}

fn 复制文件(源: &Path, 目标: &Path) -> Result<(), String> {
    if let Some(父) = 目标.parent() {
        std::fs::create_dir_all(父).map_err(|e| format!("建目录 {} 失败: {}", 父.display(), e))?;
    }
    std::fs::copy(源, 目标)
        .map(|_| ())
        .map_err(|e| format!("复制 {} → {} 失败: {}", 源.display(), 目标.display(), e))
}

fn 复制目录(源: &Path, 目标: &Path) -> Result<(), String> {
    std::fs::create_dir_all(目标).map_err(|e| format!("建目录 {} 失败: {}", 目标.display(), e))?;
    for 一条 in std::fs::read_dir(源)
        .map_err(|e| format!("读目录 {} 失败: {}", 源.display(), e))?
        .flatten()
    {
        let 路径 = 一条.path();
        let 目标路径 = 目标.join(一条.file_name());
        if 路径.is_dir() {
            复制目录(&路径, &目标路径)?;
        } else {
            复制文件(&路径, &目标路径)?;
        }
    }
    Ok(())
}

/// 递归收集条目，深度降序（先改深层，避免父目录改名后子路径失效）
fn 全部条目(根: &Path) -> Result<Vec<PathBuf>, String> {
    let mut 结果 = Vec::new();
    let mut 栈 = vec![根.to_path_buf()];
    while let Some(当前) = 栈.pop() {
        for 一条 in std::fs::read_dir(&当前)
            .map_err(|e| format!("读目录 {} 失败: {}", 当前.display(), e))?
            .flatten()
        {
            let 路径 = 一条.path();
            if 路径.is_dir() {
                栈.push(路径.clone());
            }
            结果.push(路径);
        }
    }
    结果.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    Ok(结果)
}

fn 重命名(输出: &Path, 旧: &str, 新: &str) -> Result<usize, String> {
    let mut 数 = 0usize;
    for 路径 in 全部条目(输出)? {
        let Some(名) = 路径.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if !名.starts_with(旧) {
            continue;
        }
        let 新名 = 名.replace(旧, 新);
        if 新名 == 名 {
            continue;
        }
        let 目标 = 路径.with_file_name(&新名);
        std::fs::rename(&路径, &目标)
            .map_err(|e| format!("改名 {} → {} 失败: {}", 路径.display(), 目标.display(), e))?;
        数 += 1;
    }
    Ok(数)
}

/// 正文替换：源前缀 → 标识，源名称 → 名称（只处理文本文件）
fn 替换正文(
    输出: &Path,
    源前缀: &str,
    标识: &str,
    源名称: &str,
    名称: &str,
) -> Result<usize, String> {
    let mut 数 = 0usize;
    for 路径 in 全部条目(输出)? {
        if !路径.is_file() {
            continue;
        }
        let 扩展 = 路径
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(扩展.as_str(), "yaml" | "yml" | "txt" | "lua") {
            continue;
        }
        let 原文 = match std::fs::read_to_string(&路径) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let 新文 = 原文.replace(源前缀, 标识).replace(源名称, 名称);
        if 新文 != 原文 {
            写文本(&路径, &新文)?;
            数 += 1;
        }
    }
    Ok(数)
}

// =========================================================================
// 编码规则
// =========================================================================

fn 首(k: &str) -> char {
    k.chars().next().unwrap_or('?').to_ascii_lowercase()
}

fn 次(k: &str) -> char {
    k.chars()
        .nth(1)
        .unwrap_or_else(|| k.chars().next().unwrap_or('?'))
        .to_ascii_lowercase()
}

fn 末(k: &str) -> char {
    k.chars().last().unwrap_or('?').to_ascii_lowercase()
}

/// 单字全码（星月规则），返回小写 3~4 码
fn 单字码(根们: &[String], 字根: &字根表) -> Result<String, String> {
    if 根们.is_empty() {
        return Ok(String::new());
    }
    let mut 键们 = Vec::with_capacity(根们.len());
    for r in 根们 {
        match 字根.get(r) {
            Some(k) if !k.is_empty() => 键们.push(k.as_str()),
            _ => return Err(format!("未知字根「{}」", r)),
        }
    }
    Ok(match 键们.len() {
        1 => format!("{}{}{}", 首(键们[0]), 次(键们[0]), 末(键们[0])),
        2 => format!(
            "{}{}{}{}",
            首(键们[0]),
            首(键们[1]),
            次(键们[1]),
            次(键们[0])
        ),
        _ => {
            let mut s = String::new();
            for k in &键们 {
                s.push(首(k));
            }
            s.push(次(键们[键们.len() - 1]));
            s.chars().take(4).collect()
        }
    })
}

/// 词码：与星月 encoder.rules 一致
fn 词码(词: &str, 字码: &HashMap<String, String>) -> Option<String> {
    let 字列: Vec<char> = 词.chars().collect();
    if 字列.len() < 2 {
        return None;
    }
    let mut 码们 = Vec::with_capacity(字列.len());
    for c in &字列 {
        码们.push(字码.get(&c.to_string())?.clone());
    }
    let 取 = |s: &str, n: usize| -> String { s.chars().take(n).collect() };
    let 码 = match 字列.len() {
        2 => format!("{}{}", 取(&码们[0], 2), 取(&码们[1], 2)),
        3 => format!("{}{}{}", 取(&码们[0], 1), 取(&码们[1], 1), 取(&码们[2], 2)),
        _ => format!(
            "{}{}{}{}",
            取(&码们[0], 1),
            取(&码们[1], 1),
            取(&码们[2], 1),
            取(&码们[码们.len() - 1], 1)
        ),
    };
    Some(码)
}

// =========================================================================
// 数据加载
// =========================================================================

fn 读字根表(路径: &Path) -> Result<字根表, String> {
    let mut 表 = 字根表::new();
    for 行 in 读行(路径)? {
        if 行.trim().is_empty() || 行.starts_with('#') {
            continue;
        }
        let mut 列 = 行.split('\t');
        let 根 = 列.next().unwrap_or("").trim();
        let 键 = 列.next().unwrap_or("").trim();
        if 根.is_empty() || 键.is_empty() {
            continue;
        }
        表.insert(根.to_string(), 键.to_string());
    }
    Ok(表)
}

fn 读拆分表(路径: &Path) -> Result<拆分表, String> {
    let mut 表 = 拆分表::new();
    for 行 in 读行(路径)? {
        if 行.trim().is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 根们: Vec<String> = 列[1].split_whitespace().map(|s| s.to_string()).collect();
        if 根们.is_empty() {
            continue;
        }
        表.insert(列[0].to_string(), 根们);
    }
    Ok(表)
}

/// 字根清单（按长度降序），供「拆分式」最长匹配用
fn 根清单(拆分: &拆分表, 字根: &字根表) -> Vec<String> {
    let mut 集: HashSet<String> = 字根.keys().cloned().collect();
    for 根们 in 拆分.values() {
        for r in 根们 {
            集.insert(r.clone());
        }
    }
    let mut 清单: Vec<String> = 集.into_iter().collect();
    清单.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
    清单
}

/// 把「拆分式」按字根清单做最长匹配；有剩余（如 `{畏下}`）返回 None
fn 拆拆分式(式: &str, 清单: &[String]) -> Option<Vec<String>> {
    let 字符: Vec<char> = 式.chars().collect();
    let mut 位 = 0usize;
    let mut 结果 = Vec::new();
    while 位 < 字符.len() {
        let 余: String = 字符[位..].iter().collect();
        let mut 命中 = None;
        for r in 清单 {
            if 余.starts_with(r.as_str()) {
                命中 = Some(r.clone());
                break;
            }
        }
        let r = 命中?;
        位 += r.chars().count();
        结果.push(r);
    }
    if 结果.is_empty() {
        None
    } else {
        Some(结果)
    }
}

/// 一份 Rime 词典的表头：到第一行 `...` 为止（含），再补一个空行
fn 词典表头(路径: &Path) -> Result<String, String> {
    let mut 文 = String::new();
    for 行 in 读行(路径)? {
        文.push_str(&行);
        文.push('\n');
        if 行.starts_with("...") {
            文.push('\n');
            return Ok(文);
        }
    }
    Ok(文)
}

// =========================================================================
// 换码
// =========================================================================

/// 拆分表：`字\t[拆分式,全码,分组简码,注音,保留,字集,码位]`
///
/// 返回 (字→根们, 改写行数, 保留行数)。字→根们 供全码表复用，保证两张表口径一致。
fn 重写拆分表(
    路径: &Path,
    拆分: &拆分表,
    字根: &字根表,
    清单: &[String],
) -> Result<(HashMap<String, Vec<String>>, usize, usize), String> {
    let mut 输出 = Vec::new();
    let mut 已解析: HashMap<String, Vec<String>> = HashMap::new();
    let (mut 改, mut 留) = (0usize, 0usize);

    for 行 in 读行(路径)? {
        let Some((字, 列)) = 行.split_once('\t') else {
            输出.push(行);
            continue;
        };
        let 列 = 列.trim();
        let Some(内) = 列.strip_prefix('[').and_then(|x| x.strip_suffix(']')) else {
            输出.push(行);
            continue;
        };
        let 段: Vec<&str> = 内.split(',').collect();
        if 段.len() < 3 {
            输出.push(行);
            continue;
        }
        // 根序列：优先用拆分表，其次按拆分式最长匹配
        let 根们 = match 拆分.get(字) {
            Some(x) => Some(x.clone()),
            None => 拆拆分式(段[0], 清单),
        };
        let Some(根们) = 根们 else {
            输出.push(行);
            留 += 1;
            continue;
        };
        let 码 = match 单字码(&根们, 字根) {
            Ok(c) => c,
            Err(_) => {
                输出.push(行);
                留 += 1;
                continue;
            }
        };
        let 分组 = 根们
            .iter()
            .map(|r| 字根.get(r).cloned().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("-");
        let 式 = 根们.concat();
        let 余 = if 段.len() > 3 { 段[3..].join(",") } else { String::new() };
        let 尾 = if 余.is_empty() { String::new() } else { format!(",{}", 余) };
        输出.push(format!("{}\t[{},{},{}{}]", 字, 式, 码, 分组, 尾));
        已解析.insert(字.to_string(), 根们);
        改 += 1;
    }

    写文本(路径, &拼行(&输出))?;
    Ok((已解析, 改, 留))
}

/// 全码表：`字\t码\t频次`。返回 (字→新码, 条数, 跳过数)
fn 重写全码表(
    路径: &Path,
    已解析: &HashMap<String, Vec<String>>,
    字根: &字根表,
) -> Result<(HashMap<String, String>, usize, usize), String> {
    let 表头 = 词典表头(路径)?;

    // 旧频次
    let mut 频次: HashMap<String, u64> = HashMap::new();
    for 行 in 读行(路径)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() >= 3 {
            if let Ok(频) = 列[2].trim().parse::<u64>() {
                频次.insert(列[0].to_string(), 频);
            }
        }
    }

    let mut 条目: Vec<(String, String, u64)> = Vec::with_capacity(已解析.len());
    let mut 跳 = 0usize;
    for (字, 根们) in 已解析 {
        match 单字码(根们, 字根) {
            Ok(码) => {
                let 频 = 频次.get(字).copied().unwrap_or(0);
                条目.push((字.clone(), 码, 频));
            }
            Err(_) => 跳 += 1,
        }
    }
    条目.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));

    let mut 字码: HashMap<String, String> = HashMap::with_capacity(条目.len());
    let mut 文 = 表头;
    for (字, 码, 频) in &条目 {
        字码.insert(字.clone(), 码.clone());
        文.push_str(&format!("{}\t{}\t{}\n", 字, 码, 频));
    }
    写文本(路径, &文)?;
    Ok((字码, 条目.len(), 跳))
}

/// 词表：`词\t码`
fn 重写词表(
    路径: &Path,
    字码: &HashMap<String, String>,
) -> Result<(HashMap<String, String>, usize, usize), String> {
    let 表头 = 词典表头(路径)?;
    let mut 词码表: HashMap<String, String> = HashMap::new();
    let mut 文 = 表头;
    let (mut 条, mut 跳) = (0usize, 0usize);
    for 行 in 读行(路径)? {
        let Some((词, _)) = 行.split_once('\t') else {
            continue;
        };
        match 词码(词, 字码) {
            Some(码) => {
                词码表.insert(词.to_string(), 码.clone());
                文.push_str(&format!("{}\t{}\n", 词, 码));
                条 += 1;
            }
            None => 跳 += 1,
        }
    }
    写文本(路径, &文)?;
    Ok((词码表, 条, 跳))
}

/// 简码表（quick / short）：**保留原有档位**（每条用几码），只换字母。
/// 冲突时自动加长一码，仍冲突则丢弃。
fn 重写简码表(
    路径: &Path,
    新码表: &HashMap<String, String>,
) -> Result<(usize, usize), String> {
    let 表头 = 词典表头(路径)?;
    let mut 旧档: Vec<(String, usize)> = Vec::new();
    for 行 in 读行(路径)? {
        let Some((词, 码)) = 行.split_once('\t') else {
            continue;
        };
        let 码 = 码.trim();
        if 码.is_empty() {
            continue;
        }
        旧档.push((词.to_string(), 码.chars().count()));
    }

    let mut 已占: HashMap<String, ()> = HashMap::new();
    let mut 文 = 表头;
    let (mut 条, mut 丢) = (0usize, 0usize);
    for (词, 档) in 旧档 {
        let Some(码) = 新码表.get(&词) else {
            丢 += 1;
            continue;
        };
        let mut 选: Option<String> = None;
        for l in 档..=4 {
            let cand: String = 码.chars().take(l).collect();
            if cand.chars().count() < l {
                break;
            }
            if !已占.contains_key(&cand) {
                选 = Some(cand);
                break;
            }
        }
        match 选 {
            Some(c) => {
                已占.insert(c.clone(), ());
                文.push_str(&format!("{}\t{}\n", 词, c));
                条 += 1;
            }
            None => 丢 += 1,
        }
    }
    写文本(路径, &文)?;
    Ok((条, 丢))
}

/// 从原表里取提示标记（如 `/dl` 里的 `dl`）
fn 取标记(文本: &str) -> Option<String> {
    for 行 in 文本.lines() {
        if let Some(斜) = 行.rfind('/') {
            let 后: String = 行[斜 + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !后.is_empty() {
                return Some(后);
            }
        }
    }
    None
}

/// 字根提示表：`字根们\t/<标记><大码>` 与 `+ <小码> = 字根们\t/<标记><大码>`
fn 写字根表(路径: &Path, 字根图: &Path, 名称: &str) -> Result<usize, String> {
    let 表头 = 词典表头(路径)?;
    let 标记 = 取标记(&表头).unwrap_or_else(|| "dl".to_string());

    let mut 按大: HashMap<String, Vec<String>> = HashMap::new();
    let mut 按小: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    if 字根图.is_file() {
        for (序号, 行) in 读行(字根图)?.into_iter().enumerate() {
            if 序号 == 0 {
                continue;
            }
            let 段: Vec<&str> = 行.split(',').collect();
            if 段.len() < 2 {
                continue;
            }
            let (字形, 键) = (段[0].trim(), 段[1].trim());
            if 字形.is_empty() || 键.is_empty() {
                continue;
            }
            let mut 字 = 键.chars();
            let Some(大) = 字.next() else { continue };
            let 小: String = 字.collect();
            let 大 = 大.to_string();
            按大.entry(大.clone()).or_default().push(字形.to_string());
            if !小.is_empty() {
                按小
                    .entry(大)
                    .or_default()
                    .entry(小)
                    .or_default()
                    .push(字形.to_string());
            }
        }
    } else {
        eprintln!("⚠️ 没有字根图 {}，字根提示表只写两行表头", 字根图.display());
    }

    let mut 文 = 表头;
    文.push_str(&format!("{}字根編碼提示\t/{}\n", 名称, 标记));
    文.push_str(&format!("輸入對應大碼字母\t/{}\n", 标记));
    let mut 条 = 2usize;

    let mut 大们: Vec<String> = 按大.keys().cloned().collect();
    大们.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for 大 in &大们 {
        let 字们 = 按大.get(大).map(|v| v.concat()).unwrap_or_default();
        文.push_str(&format!("{}\t/{}{}\n", 字们, 标记, 大));
        条 += 1;
    }
    let mut 大们2: Vec<String> = 按小.keys().cloned().collect();
    大们2.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for 大 in &大们2 {
        let 内 = &按小[大];
        let mut 小们: Vec<String> = 内.keys().cloned().collect();
        小们.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        for 小 in 小们 {
            let 字们 = 内.get(&小).map(|v| v.concat()).unwrap_or_default();
            文.push_str(&format!("+ {} = {}\t/{}{}\n", 小, 字们, 标记, 大));
            条 += 1;
        }
    }
    写文本(路径, &文)?;
    Ok(条)
}

// =========================================================================
// 测试
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn 造字根() -> 字根表 {
        let mut m = 字根表::new();
        for (k, v) in [
            ("白", "Fk"), ("勹", "Yr"), ("丶", "Ek"), ("一", "Od"),
            ("女", "Lk"), ("子", "Ud"), ("讠", "Yd"), ("丷", "Fs"),
            ("口", "Ie"), ("儿", "Hr"), ("打", "Fs"), ("字", "Ll"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        m
    }

    #[test]
    fn 单根字取_asy() {
        let m = 造字根();
        assert_eq!(单字码(&["一".into()], &m).unwrap(), "odd");
    }

    #[test]
    fn 二根字取_a1a2s2s1() {
        let m = 造字根();
        assert_eq!(单字码(&["女".into(), "子".into()], &m).unwrap(), "ludk");
    }

    #[test]
    fn 三根字取三大码加末小码() {
        let m = 造字根();
        assert_eq!(
            单字码(&["白".into(), "勹".into(), "丶".into()], &m).unwrap(),
            "fyek"
        );
    }

    #[test]
    fn 四根字取四大码() {
        let m = 造字根();
        assert_eq!(
            单字码(&["讠".into(), "丷".into(), "口".into(), "儿".into()], &m).unwrap(),
            "yfih"
        );
    }

    #[test]
    fn 未知字根报错() {
        let m = 造字根();
        assert!(单字码(&["不存在".into()], &m).is_err());
    }

    #[test]
    fn 词码三种长度() {
        let mut 字码 = HashMap::new();
        for (k, v) in [("打", "fsll"), ("字", "llab"), ("的", "doie"), ("确", "qwer")] {
            字码.insert(k.to_string(), v.to_string());
        }
        assert_eq!(词码("打字", &字码).unwrap(), "fsll");
        assert_eq!(词码("的确", &字码).unwrap(), "doqw");
    }

    #[test]
    fn 拆分式最长匹配() {
        let 清单 = vec!["白".to_string(), "勹".to_string(), "丶".to_string(), "乞上".to_string()];
        assert_eq!(
            拆拆分式("白勹丶", &清单).unwrap(),
            vec!["白", "勹", "丶"]
        );
        // 含无名部件 {…} 无法匹配
        assert!(拆拆分式("十{畏下}", &清单).is_none());
    }

    #[test]
    fn 标记提取() {
        assert_eq!(取标记("星月字根編碼提示\t/dl\n").as_deref(), Some("dl"));
        assert_eq!(取标记("no marker").as_deref(), None);
    }

    #[test]
    fn 端到端换码() {
        let 目录 = std::env::temp_dir().join(format!("cg-rimeconv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&目录);
        let 底包 = 目录.join("base");
        std::fs::create_dir_all(底包.join("yuhao")).unwrap();
        std::fs::create_dir_all(底包.join("lua/yuhao")).unwrap();

        std::fs::write(
            底包.join("starmoon_chaifen.dict.yaml"),
            "---\nname: \"starmoon_chaifen\"\n...\n\n好\t[女子,LkUd,Lk-Ud,㊌hao,,CJK,597D]\n一\t[一,Odd,Od,㊌it,,CJK,4E00]\n打\t[打,Fss,Fs,㊌da,,CJK,6253]\n字\t[字,Lls,Ll,㊌zi,,CJK,5B57]\n",
        ).unwrap();
        std::fs::write(
            底包.join("yuhao/starmoon.full.dict.yaml"),
            "---\nname: \"starmoon.full\"\n...\n\n好\tludk\t100\n一\todd\t50\n打\tfss\t80\n字\tlll\t70\n",
        ).unwrap();
        std::fs::write(
            底包.join("yuhao/starmoon_sc.words.dict.yaml"),
            "---\nname: \"starmoon_sc.words\"\n...\n\n打字\tfsll\n",
        ).unwrap();
        std::fs::write(
            底包.join("yuhao/starmoon_sc.quick.dict.yaml"),
            "---\nname: \"starmoon_sc.quick\"\n...\n\n好\tl\n一\to\n",
        ).unwrap();
        std::fs::write(
            底包.join("yuhao/starmoon_sc.short.dict.yaml"),
            "---\nname: \"starmoon_sc.short\"\n...\n\n打字\tf\n",
        ).unwrap();
        std::fs::write(
            底包.join("yuhao/starmoon.roots.dict.yaml"),
            "---\nname: \"starmoon.roots\"\n...\n\n星月字根編碼提示\t/dl\n",
        ).unwrap();
        std::fs::write(底包.join("starmoon_sc.dict.yaml"), "---\nname: \"starmoon_sc\"\n...\n").unwrap();
        std::fs::write(
            底包.join("default.custom.yaml"),
            "patch:\n  schema_list:\n    - {schema: starmoon_sc}\n",
        ).unwrap();
        std::fs::write(底包.join("yuhao.essay.txt"), "星月文本").unwrap();

        let 算码 = 目录.join("calc");
        std::fs::create_dir_all(&算码).unwrap();
        std::fs::write(算码.join("roots.tsv"), "女\tLk\n子\tUd\n一\tOd\n打\tFs\n字\tLl\n").unwrap();
        std::fs::write(算码.join("zigen-moling.csv"), "字形,键位,拼音\n女,Lk,nv\n").unwrap();
        let 拆分 = 目录.join("chaifen-all.txt");
        std::fs::write(&拆分, "好\t女 子\t100\n一\t一\t50\n打\t打\t80\n字\t字\t70\n").unwrap();

        let 参 = 参数 {
            底包目录: 底包.to_string_lossy().to_string(),
            算码目录: 算码.to_string_lossy().to_string(),
            拆分文件: 拆分.to_string_lossy().to_string(),
            输出目录: 目录.join("out").to_string_lossy().to_string(),
            标识: "moling".into(),
            名称: "魔靈".into(),
            源前缀: "starmoon".into(),
            源名称: "星月".into(),
        };
        run(&参).unwrap();

        let 输出 = 目录.join("out");
        assert!(输出.join("moling_chaifen.dict.yaml").is_file());
        assert!(!输出.join("starmoon_chaifen.dict.yaml").exists());

        let 拆分文 = std::fs::read_to_string(输出.join("moling_chaifen.dict.yaml")).unwrap();
        assert!(
            拆分文.contains("好\t[女子,ludk,Lk-Ud,㊌hao,,CJK,597D]"),
            "拆分表应换码并保留注音等字段: {}",
            拆分文
        );

        let 全码 = std::fs::read_to_string(输出.join("yuhao/moling.full.dict.yaml")).unwrap();
        assert!(全码.contains("好\tludk\t100"), "{}", 全码);
        assert!(全码.contains("一\todd\t50"), "{}", 全码);

        let 默认 = std::fs::read_to_string(输出.join("default.custom.yaml")).unwrap();
        assert!(默认.contains("moling_sc"), "{}", 默认);

        let 字根 = std::fs::read_to_string(输出.join("yuhao/moling.roots.dict.yaml")).unwrap();
        assert!(字根.contains("魔靈字根編碼提示\t/dl"), "{}", 字根);

        let 词 = std::fs::read_to_string(输出.join("yuhao/moling_sc.words.dict.yaml")).unwrap();
        assert!(词.contains("打字\tfsll"), "词码应重算: {}", 词);

        let 快 = std::fs::read_to_string(输出.join("yuhao/moling_sc.quick.dict.yaml")).unwrap();
        assert!(快.contains("好\tl"), "快码应保留档位: {}", 快);

        let 简 = std::fs::read_to_string(输出.join("yuhao/moling_sc.short.dict.yaml")).unwrap();
        assert!(简.contains("打字\tf"), "简码应保留档位: {}", 简);

        let _ = std::fs::remove_dir_all(&目录);
    }
}
