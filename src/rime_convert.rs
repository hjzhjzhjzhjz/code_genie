// =========================================================================
// 🔄 星月 RIME 方案包：只换码
// =========================================================================
//
// 底包（starmoon-ime）**一个字都不改**：文件名、schema_id、方案名、lua、
// 主词典的裸词表、符号表全部原样沿用。只把下列词典里的**码**换成按优化结果
// （算码输出的字根键位表）重算的字母。
//
// 换码的文件：
//   `<前缀>_chaifen.dict.yaml`          字\t[拆分式,全码,分组简码,注音,保留,字集,码位]
//   `yuhao/<前缀>.full.dict.yaml`       字\t码\t频次
//   `yuhao/<前缀>_sc.words.dict.yaml`   词\t码
//   `yuhao/<前缀>_sc.quick.dict.yaml`   字\t简码（保留原档位，只换字母）
//   `yuhao/<前缀>_sc.short.dict.yaml`   词\t简码（保留原档位，只换字母）
//   `yuhao/<前缀>.roots.dict.yaml`      字根提示（标题行沿用原文，只重生成分组）
//
// 其余一律不动：`<前缀>_sc.dict.yaml` 主词典、`*.schema.yaml`、`lua/yuhao/*`、
// `yuhao.*` 符号表、`default.custom.yaml`。
//
// 额外产出（不影响 Rime 加载，仅供导入别家输入法或测评）：
//   输出目录根下合并一张 `完整码表.txt`：单字全码 → 单字简码 → 词组 → 词组简码
//   → 主词典裸词（按 encoder 现算），按 (文本, 码) 去重，格式 `文本<TAB>码`。
//
// 单字编码规则：严格按星陈（yustar）方案 `encoder` 状态机
// （https://github.com/hanzi-chai/hanzi-chai.github.io/blob/main/examples/yustar.yaml）
//   s1(A1) → c0: 根2存在? ┬ 否 → s3(S1) → s6(S1)               ⇒ 1 根: A1 S1 S1
//                        └ 是 → s2(A2) → c1: 根3存在? ┬ 否 → s4(S2) → s9(S1)  ⇒ 2 根: A1 A2 S2 S1
//                                                    └ 是 → s5(A3) → c2: 根4存在? ┬ 否 → s8(S3)          ⇒ 3 根: A1 A2 A3 S3
//                                                                                └ 是 → s7(A_last)     ⇒ ≥4 根: A1 A2 A3 A_last
// 其中 A=键位串第 0 个字母（大码）、S=第 1 个字母（小码，缺失时回退大码），
// max_length=4。底包 123372 条真实数据闭环验证：123367 条一致，
// 余 3 条是底包自身存了大写码（AJHx/KKBU/YAJH），属底包数据瑕疵。
//
// 词码规则与星月 `encoder.rules` 一致：
//   2 字 AaAbBaBb / 3 字 AaBaCaCb / 4+ 字 AaBaCaZa
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 字根 → 键位串（如 `白` → `Fk`），取自算码输出的字根键位表
pub(crate) type 字根表 = HashMap<String, String>;
/// 字 → 拆分根序列，取自 `chaifen-all.txt`
type 拆分表 = HashMap<String, Vec<String>>;

/// `重写拆分表` 的产出：(字→根们, 改写行数, 原样保留行数)
type 拆分重写结果 = (HashMap<String, Vec<String>>, usize, usize);

/// `rime-convert` 的入参
///
/// 底包的一切（文件名、schema_id、方案名、lua、词表裸词）**原样沿用**，
/// 只把词典里的码换成按字根键位表重算的字母。
pub struct 参数 {
    /// 底包目录（starmoon-ime 根目录）
    pub 底包目录: String,
    /// 算码输出目录（含字根键位表、`zigen-<前缀>.csv`）
    pub 算码目录: String,
    /// 字根键位表；`None` 时按 `output-keymap.txt` → `roots.tsv` 顺序自动找
    pub 键位文件: Option<String>,
    /// 拆分表（`chaifen-all.txt`：字\t根 根\t频次）
    pub 拆分文件: String,
    /// 输出目录
    pub 输出目录: String,
    /// 底包的方案前缀（同时用于 `zigen-<前缀>.csv`）
    pub 前缀: String,
}

// =========================================================================
// 入口
// =========================================================================

/// 执行转换。返回 (改写条数, 告警数)。
pub fn run(参: &参数) -> Result<(usize, usize), String> {
    let 底包 = Path::new(&参.底包目录);
    let 前缀 = &参.前缀;
    let 拆分源 = 底包.join(format!("{}_chaifen.dict.yaml", 前缀));
    if !拆分源.is_file() {
        return Err(format!(
            "底包目录不对：{} 下没有 {}",
            参.底包目录,
            文件名(&拆分源)
        ));
    }

    let 算码 = Path::new(&参.算码目录);
    let 键位路径 = 定位键位表(算码, 参.键位文件.as_deref())?;
    let 字根 = 读字根键位(&键位路径)?;
    let 拆分 = 读拆分表(Path::new(&参.拆分文件))?;
    println!(
        "  字根键位: {} （{} 个字根）；拆分表: {} 个字",
        键位路径.display(),
        字根.len(),
        拆分.len()
    );

    // ---------- 1. 原样复制底包 ----------
    let 输出 = PathBuf::from(&参.输出目录);
    if 输出.exists() {
        crate::fsutil::archive_if_exists(&输出);
    }
    复制底包(底包, &输出, 前缀)?;
    println!("(1) 已原样复制底包到 {}", 输出.display());

    // ---------- 2. 拆分表 ----------
    let 拆分路径 = 输出.join(format!("{}_chaifen.dict.yaml", 前缀));
    let 根清单 = 根清单(&拆分, &字根);
    let (已解析, 改, 留) = 重写拆分表(&拆分路径, &拆分, &字根, &根清单)?;
    let mut 警告 = 留;
    println!("(2) {}: 重写 {} 行，保留 {} 行", 文件名(&拆分路径), 改, 留);

    // ---------- 3. 全码表 ----------
    let 全码路径 = 输出.join("yuhao").join(format!("{}.full.dict.yaml", 前缀));
    let (字码, 全条, 跳) = 重写全码表(&全码路径, &已解析, &字根)?;
    警告 += 跳;
    println!("(3) {}: {} 条全码", 文件名(&全码路径), 全条);

    // ---------- 4. 词表 ----------
    let 词路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.words.dict.yaml", 前缀));
    let (词码表, 词条, 跳) = 重写词表(&词路径, &字码)?;
    警告 += 跳;
    println!("(4) {}: {} 条词", 文件名(&词路径), 词条);

    // ---------- 5. 快码（单字简码）----------
    let 快码路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.quick.dict.yaml", 前缀));
    let (快条, 跳) = 重写简码表(&快码路径, &字码)?;
    警告 += 跳;
    println!("(5) {}: {} 条快码", 文件名(&快码路径), 快条);

    // ---------- 6. 简码（词组简码）----------
    let 简码路径 = 输出
        .join("yuhao")
        .join(format!("{}_sc.short.dict.yaml", 前缀));
    let (简条, 跳) = 重写简码表(&简码路径, &词码表)?;
    警告 += 跳;
    println!("(6) {}: {} 条简码", 文件名(&简码路径), 简条);

    // ---------- 7. 完整码表 ----------
    let 完整路径 = 输出.join("完整码表.txt");
    let 主词典 = 输出.join(format!("{}_sc.dict.yaml", 前缀));
    let 完整 = 写完整码表(
        &完整路径,
        &[
            ("单字全码", 全码路径.as_path()),
            ("单字简码", 快码路径.as_path()),
            ("词组", 词路径.as_path()),
            ("词组简码", 简码路径.as_path()),
        ],
        &主词典,
        &字码,
    )?;
    println!(
        "(7) {}: {} 条（{}）",
        文件名(&完整路径),
        完整.0,
        完整
            .1
            .iter()
            .map(|(n, c)| format!("{} {}", n, c))
            .collect::<Vec<_>>()
            .join(" / ")
    );

    // ---------- 8. 字根提示 ----------
    let 字根路径 = 输出.join("yuhao").join(format!("{}.roots.dict.yaml", 前缀));
    let 图 = 算码.join(format!("zigen-{}.csv", 前缀));
    let 字根条 = 写字根表(&字根路径, &图, &字根)?;
    println!(
        "(8) {}: {} 条字根提示（来源 {}）",
        文件名(&字根路径),
        字根条,
        文件名(&图)
    );

    println!("\n✅ 已生成：{}", 输出.display());
    println!("   除码以外与底包完全一致；拷进 Rime 用户目录重新部署即可");
    if 警告 > 0 {
        println!("⚠️ 共 {} 处告警，详见上方输出", 警告);
    }
    Ok((改 + 全条 + 词条 + 快条 + 简条 + 字根条 + 完整.0, 警告))
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

/// 往完整码表里收一条（按 文本+码 去重）。返回是否新增。
fn 收一条(文: &mut String, 已见: &mut HashSet<String>, 文本: &str, 码: &str) -> bool {
    let 码 = 码.trim();
    if 文本.is_empty() || 码.is_empty() {
        return false;
    }
    let 键 = format!("{}\t{}", 文本, 码);
    if !已见.insert(键.clone()) {
        return false;
    }
    文.push_str(&键);
    文.push('\n');
    true
}

/// 合并出一张完整码表：单字全码 → 单字简码 → 词组 → 词组简码 → 主词典裸词（现算）。
///
/// 输出 `文本<TAB>码`，按 (文本, 码) 去重，保持各来源原有顺序。
/// 返回 (总条数, 各来源条数)。
fn 写完整码表(
    路径: &Path,
    来源: &[(&str, &Path)],
    主词典: &Path,
    字码: &HashMap<String, String>,
) -> Result<(usize, Vec<(String, usize)>), String> {
    let mut 已见: HashSet<String> = HashSet::new();
    let mut 文 = String::new();
    let mut 明细: Vec<(String, usize)> = Vec::new();

    for (名, 源) in 来源 {
        let mut 数 = 0usize;
        if 源.is_file() {
            let 行们 = 读行(源)?;
            let 起 = 行们
                .iter()
                .position(|l| l.starts_with("..."))
                .map(|i| i + 1)
                .unwrap_or(0);
            for 行 in &行们[起..] {
                let 行 = 行.trim_end();
                if 行.is_empty() || !行.contains('\t') {
                    continue;
                }
                let mut 列 = 行.split('\t');
                let 文本 = 列.next().unwrap_or("");
                let 码 = 列.next().unwrap_or("");
                if 收一条(&mut 文, &mut 已见, 文本, 码) {
                    数 += 1;
                }
            }
        }
        明细.push((名.to_string(), 数));
    }

    // 主词典里的裸词（只有文本、没有码），按 encoder 规则现算
    let mut 裸数 = 0usize;
    if 主词典.is_file() {
        let 行们 = 读行(主词典)?;
        let 起 = 行们
            .iter()
            .position(|l| l.starts_with("..."))
            .map(|i| i + 1)
            .unwrap_or(0);
        for 行 in &行们[起..] {
            let 词 = 行.trim();
            if 词.is_empty() || 词.contains('\t') || 词.starts_with('#') {
                continue;
            }
            if let Some(码) = 词码(词, 字码) {
                if 收一条(&mut 文, &mut 已见, 词, &码) {
                    裸数 += 1;
                }
            }
        }
    }
    明细.push(("主词典裸词".to_string(), 裸数));

    写文本(路径, &文)?;
    Ok((已见.len(), 明细))
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

/// 单字全码（严格按 yustar encoder 状态机），返回小写 3~4 码
pub(crate) fn 单字码(根们: &[String], 字根: &字根表) -> Result<String, String> {
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
    // 严格按 yustar.yaml 的 encoder 状态机走（s0→s1→c0→…），四条路径：
    //   1 根 : A1 S1 S1            （s1 → c0(负) → s3 → s6）
    //   2 根 : A1 A2 S2 S1         （s1 → c0(正) → s2 → c1(负) → s4 → s9）
    //   3 根 : A1 A2 A3 S3         （s1 → s2 → c1(正) → s5 → c2(负) → s8）
    //   ≥4 根: A1 A2 A3 A_last     （s1 → s2 → s5 → c2(正) → s7，取末根大码）
    let n = 键们.len();
    Ok(match n {
        1 => {
            let k = 键们[0];
            format!("{}{}{}", 首(k), 次(k), 次(k))
        }
        2 => format!(
            "{}{}{}{}",
            首(键们[0]),
            首(键们[1]),
            次(键们[1]),
            次(键们[0])
        ),
        3 => format!(
            "{}{}{}{}",
            首(键们[0]),
            首(键们[1]),
            首(键们[2]),
            次(键们[2])
        ),
        _ => format!(
            "{}{}{}{}",
            首(键们[0]),
            首(键们[1]),
            首(键们[2]),
            首(键们[n - 1])
        ),
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

/// 定位字根键位表：显式指定 > `output-keymap.txt` > `roots.tsv`
fn 定位键位表(算码: &Path, 指定: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = 指定 {
        let p = PathBuf::from(p);
        if !p.is_file() {
            return Err(format!("字根键位表不存在: {}", p.display()));
        }
        return Ok(p);
    }
    for 名 in ["output-keymap.txt", "roots.tsv"] {
        let p = 算码.join(名);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(format!(
        "{} 下找不到字根键位表（找过 output-keymap.txt、roots.tsv），请用 --keymap 指定",
        算码.display()
    ))
}

fn ucfirst(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => format!(
            "{}{}",
            c.to_ascii_uppercase(),
            cs.as_str().to_ascii_lowercase()
        ),
        None => String::new(),
    }
}

/// 读算码输出的字根键位表：`字根<TAB>键位串[<TAB>频次]`，取前两列。
///
/// 兼容 `generate-root-chart.sh` 支持的飞键写法——同一字根拆成
/// `白.A` / `白.S` / `白.Y`（或 `白.U` 表示飞键）多行时，按 A+S+Y（飞键取 U+S+Y）拼接，
/// 与 `roots.tsv` 的口径一致。首字母统一大写，与底包「分组简码」风格对齐。
fn 读字根键位(路径: &Path) -> Result<字根表, String> {
    let mut 平表: 字根表 = HashMap::new();
    // 根 → { A/S/Y/U: 键位串 }
    let mut 位表: HashMap<String, HashMap<char, String>> = HashMap::new();

    for 行 in 读行(路径)? {
        if 行.trim().is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 根 = 列[0].trim();
        let 键 = 列[1].trim();
        if 根.is_empty() || 键.is_empty() {
            continue;
        }
        // 飞键写法：<根>.A / .S / .Y / .U
        if let Some((名, 位)) = 根.rsplit_once('.') {
            if !名.is_empty() && 位.chars().count() == 1 {
                let c = 位.chars().next().unwrap().to_ascii_uppercase();
                if matches!(c, 'A' | 'S' | 'Y' | 'U') {
                    位表
                        .entry(名.to_string())
                        .or_default()
                        .insert(c, 键.to_string());
                    continue;
                }
            }
        }
        平表.insert(根.to_string(), 键.to_string());
    }

    for (根, 位) in 位表 {
        let 首 = 位.get(&'U').or_else(|| 位.get(&'A'));
        let Some(首) = 首 else { continue };
        let 串 = format!(
            "{}{}{}",
            首,
            位.get(&'S').map(|s| s.as_str()).unwrap_or(""),
            位.get(&'Y').map(|s| s.as_str()).unwrap_or("")
        );
        平表.insert(根, 串);
    }

    Ok(平表.into_iter().map(|(k, v)| (k, ucfirst(&v))).collect())
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
) -> Result<拆分重写结果, String> {
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
        let 余 = if 段.len() > 3 {
            段[3..].join(",")
        } else {
            String::new()
        };
        let 尾 = if 余.is_empty() {
            String::new()
        } else {
            format!(",{}", 余)
        };
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
///
/// 标题两行（如「星月字根編碼提示」「輸入對應大碼字母」）**沿用原文件**，只重生成分组数据。
fn 写字根表(路径: &Path, 字根图: &Path, 字根: &字根表) -> Result<usize, String> {
    let 表头 = 词典表头(路径)?;
    let 全部 = 读行(路径)?;
    let 数据起 = 全部
        .iter()
        .position(|l| l.starts_with("..."))
        .map(|i| i + 1)
        .unwrap_or(0);
    // 数据区开头两个非空行就是原标题行
    let 标题: Vec<String> = 全部[数据起..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .take(2)
        .cloned()
        .collect();
    let 标记 = 标题
        .first()
        .and_then(|l| 取标记(l))
        .unwrap_or_else(|| "dl".to_string());

    let mut 按大: HashMap<String, Vec<String>> = HashMap::new();
    let mut 按小: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    {
        let mut 加入 = |字形: &str, 键: &str| {
            if 字形.is_empty() || 键.is_empty() {
                return;
            }
            let mut 字 = 键.chars();
            let Some(大) = 字.next() else { return };
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
        };

        if 字根图.is_file() {
            // 字形变体表：字形,键位串,拼音（首行表头）
            for (序号, 行) in 读行(字根图)?.into_iter().enumerate() {
                if 序号 == 0 {
                    continue;
                }
                let 段: Vec<&str> = 行.split(',').collect();
                if 段.len() < 2 {
                    continue;
                }
                加入(段[0].trim(), 段[1].trim());
            }
        } else {
            // 没有字形表就退回用字根本身，保证只依赖算码输出的键位表
            eprintln!(
                "⚠️ 没有字根图 {}，改用字根键位表本身生成提示",
                字根图.display()
            );
            let mut 根们: Vec<(&String, &String)> = 字根.iter().collect();
            根们.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            for (根, 键) in 根们 {
                加入(根, 键);
            }
        }
    }

    let mut 文 = 表头;
    let mut 条 = 0usize;
    for 行 in &标题 {
        文.push_str(行);
        文.push('\n');
        条 += 1;
    }

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
            ("白", "Fk"),
            ("勹", "Yr"),
            ("丶", "Ek"),
            ("一", "Od"),
            ("女", "Lk"),
            ("子", "Ud"),
            ("讠", "Yd"),
            ("丷", "Fs"),
            ("口", "Ie"),
            ("儿", "Hr"),
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
    fn 五根字取首三码加末根大码() {
        // yustar 状态机 c2 走 s7（rootIndex -1），取**末根**大码，不是第 4 根
        let mut m = 字根表::new();
        for (k, v) in [
            ("甲", "Ab"),
            ("乙", "Cd"),
            ("丙", "Ef"),
            ("丁", "Gh"),
            ("戊", "Ij"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        assert_eq!(
            单字码(
                &[
                    "甲".into(),
                    "乙".into(),
                    "丙".into(),
                    "丁".into(),
                    "戊".into()
                ],
                &m
            )
            .unwrap(),
            "acei",
            "应为 A1A2A3+A_last = a c e i"
        );
    }

    #[test]
    fn 单根字取两次小码() {
        // 状态机 s3 → s6 都取 index 1，即 A+S+S，不是 A+S+Y
        let mut m = 字根表::new();
        m.insert("三".to_string(), "Axy".to_string()); // 三字母键位串
        assert_eq!(单字码(&["三".into()], &m).unwrap(), "axx");
    }

    #[test]
    fn 未知字根报错() {
        let m = 造字根();
        assert!(单字码(&["不存在".into()], &m).is_err());
    }

    #[test]
    fn 词码三种长度() {
        let mut 字码 = HashMap::new();
        for (k, v) in [
            ("打", "fsll"),
            ("字", "llab"),
            ("的", "doie"),
            ("确", "qwer"),
        ] {
            字码.insert(k.to_string(), v.to_string());
        }
        assert_eq!(词码("打字", &字码).unwrap(), "fsll");
        assert_eq!(词码("的确", &字码).unwrap(), "doqw");
    }

    #[test]
    fn 词码四字以上取首三码加末字首码() {
        let mut 字码 = HashMap::new();
        for (k, v) in [
            ("甲", "abcd"),
            ("乙", "efgh"),
            ("丙", "ijkl"),
            ("丁", "mnop"),
            ("戊", "qrst"),
            ("己", "uvwx"),
        ] {
            字码.insert(k.to_string(), v.to_string());
        }
        // AaBaCaZa：首字首码 + 2字首码 + 3字首码 + 末字首码
        assert_eq!(词码("甲乙丙丁", &字码).unwrap(), "aeim");
        assert_eq!(词码("甲乙丙丁戊", &字码).unwrap(), "aeiq", "末字取戊");
        assert_eq!(词码("甲乙丙丁戊己", &字码).unwrap(), "aeiu", "末字取己");
        // 单字不成词
        assert!(词码("甲", &字码).is_none());
        // 有字不在字码表里则整词放弃
        assert!(词码("甲X", &字码).is_none());
    }

    #[test]
    fn 拆分式最长匹配() {
        let 清单 = vec![
            "白".to_string(),
            "勹".to_string(),
            "丶".to_string(),
            "乞上".to_string(),
        ];
        assert_eq!(拆拆分式("白勹丶", &清单).unwrap(), vec!["白", "勹", "丶"]);
        // 含无名部件 {…} 无法匹配
        assert!(拆拆分式("十{畏下}", &清单).is_none());
    }

    #[test]
    fn 标记提取() {
        assert_eq!(取标记("星月字根編碼提示\t/dl\n").as_deref(), Some("dl"));
        assert_eq!(取标记("no marker").as_deref(), None);
    }

    #[test]
    fn 键位表首字母大写并忽略频次列() {
        let 目录 = std::env::temp_dir().join(format!("cg-keymap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&目录);
        std::fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join("output-keymap.txt");
        std::fs::write(&路径, "# 注释\n女\tlk\t100\n子\tUd\n").unwrap();
        let 表 = 读字根键位(&路径).unwrap();
        assert_eq!(表.get("女").map(|s| s.as_str()), Some("Lk"), "应首字母大写");
        assert_eq!(表.get("子").map(|s| s.as_str()), Some("Ud"));
        let _ = std::fs::remove_dir_all(&目录);
    }

    #[test]
    fn 键位表飞键写法合并() {
        let 目录 = std::env::temp_dir().join(format!("cg-keymap-fly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&目录);
        std::fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join("output-keymap.txt");
        // 白.A/.S/.Y 拼成 ASY；艹.U 表示飞键，取 U+S+Y
        std::fs::write(
            &路径,
            "白.A\tF\n白.S\tk\n白.Y\te\n艹.U\tx\n艹.S\tq\n艹.Y\tw\n",
        )
        .unwrap();
        let 表 = 读字根键位(&路径).unwrap();
        assert_eq!(表.get("白").map(|s| s.as_str()), Some("Fke"));
        assert_eq!(
            表.get("艹").map(|s| s.as_str()),
            Some("Xqw"),
            "飞键应取 U+S+Y"
        );
        let _ = std::fs::remove_dir_all(&目录);
    }

    #[test]
    fn 端到端换码() {
        let 目录 = std::env::temp_dir().join(format!("cg-rimeconv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&目录);
        let 底包 = 目录.join("base");
        std::fs::create_dir_all(底包.join("yuhao")).unwrap();
        std::fs::create_dir_all(底包.join("lua/yuhao")).unwrap();

        let 写 = |名: &str, 内容: &str| {
            let 路径 = 底包.join(名);
            if let Some(父) = 路径.parent() {
                std::fs::create_dir_all(父).unwrap();
            }
            std::fs::write(&路径, 内容).unwrap();
        };

        let 头 = |名: &str| format!("---\nname: \"{}\"\n...\n\n", 名);

        // 拆分表：字<TAB>[拆分式,全码,分组简码,注音,保留,字集,码位]
        let mut 拆分 = 头("starmoon_chaifen");
        拆分.push_str("好\t[女子,LkUd,Lk-Ud,㊌hao,,CJK,597D]\n一\t[一,Odd,Od,㊌it,,CJK,4E00]\n打\t[打,Fss,Fs,㊌da,,CJK,6253]\n字\t[字,Lls,Ll,㊌zi,,CJK,5B57]\n");
        写("starmoon_chaifen.dict.yaml", &拆分);

        let mut 全码 = 头("starmoon.full");
        全码.push_str("好\tludk\t100\n一\todd\t50\n打\tfss\t80\n字\tlll\t70\n");
        写("yuhao/starmoon.full.dict.yaml", &全码);

        let mut 词 = 头("starmoon_sc.words");
        词.push_str("打字\tfsll\n");
        写("yuhao/starmoon_sc.words.dict.yaml", &词);

        let mut 快 = 头("starmoon_sc.quick");
        快.push_str("好\tl\n一\to\n");
        写("yuhao/starmoon_sc.quick.dict.yaml", &快);

        let mut 简 = 头("starmoon_sc.short");
        简.push_str("打字\tf\n");
        写("yuhao/starmoon_sc.short.dict.yaml", &简);

        let mut 根 = 头("starmoon.roots");
        根.push_str("星月字根編碼提示\t/dl\n輸入對應大碼字母\t/dl\n");
        写("yuhao/starmoon.roots.dict.yaml", &根);

        let mut 主词典 = 头("starmoon_sc");
        主词典.push_str("打字\n一字\n"); // 裸词：只有文本，靠 encoder 现算
        写("starmoon_sc.dict.yaml", &主词典);
        写(
            "default.custom.yaml",
            "patch:\n  schema_list:\n    - {schema: starmoon_sc}\n",
        );
        写("yuhao.essay.txt", "星月文本");

        // 算码输出：只有字根键位表（带注释行与频次列），不给 zigen 图
        let 算码 = 目录.join("calc");
        std::fs::create_dir_all(&算码).unwrap();
        std::fs::write(
            算码.join("output-keymap.txt"),
            "# 综合得分: 0.88\n女\tLk\t100\n子\tUd\t90\n一\tOd\t80\n打\tFs\t70\n字\tLl\t60\n",
        )
        .unwrap();

        let 拆分文件 = 目录.join("chaifen-all.txt");
        std::fs::write(
            &拆分文件,
            "好\t女 子\t100\n一\t一\t50\n打\t打\t80\n字\t字\t70\n",
        )
        .unwrap();

        let 参 = 参数 {
            底包目录: 底包.to_string_lossy().to_string(),
            算码目录: 算码.to_string_lossy().to_string(),
            键位文件: None,
            拆分文件: 拆分文件.to_string_lossy().to_string(),
            输出目录: 目录.join("out").to_string_lossy().to_string(),
            前缀: "starmoon".into(),
        };
        run(&参).unwrap();

        let 输出 = 目录.join("out");
        // 文件名一律沿用底包
        assert!(
            输出.join("starmoon_chaifen.dict.yaml").is_file(),
            "文件名不该改"
        );

        let 拆分文 = std::fs::read_to_string(输出.join("starmoon_chaifen.dict.yaml")).unwrap();
        assert!(
            拆分文.contains("好\t[女子,ludk,Lk-Ud,㊌hao,,CJK,597D]"),
            "拆分表应换码并保留注音等字段: {}",
            拆分文
        );

        let 全码文 = std::fs::read_to_string(输出.join("yuhao/starmoon.full.dict.yaml")).unwrap();
        assert!(全码文.contains("好\tludk\t100"), "{}", 全码文);
        assert!(全码文.contains("一\todd\t50"), "{}", 全码文);

        // 底包其余内容原样：schema 名与文本都不该被动
        let 默认 = std::fs::read_to_string(输出.join("default.custom.yaml")).unwrap();
        assert!(默认.contains("starmoon_sc"), "{}", 默认);
        let 文本 = std::fs::read_to_string(输出.join("yuhao.essay.txt")).unwrap();
        assert_eq!(文本, "星月文本", "底包文本不该被替换");

        // 字根表：标题沿用原文，数据按新码重生成（没有 zigen 图时用键位表兜底）
        let 字根文 = std::fs::read_to_string(输出.join("yuhao/starmoon.roots.dict.yaml")).unwrap();
        assert!(字根文.contains("星月字根編碼提示\t/dl"), "{}", 字根文);
        assert!(字根文.contains("輸入對應大碼字母\t/dl"), "{}", 字根文);
        assert!(字根文.contains("\t/dlL"), "应按新码重生成分组: {}", 字根文);

        let 词文 = std::fs::read_to_string(输出.join("yuhao/starmoon_sc.words.dict.yaml")).unwrap();
        assert!(词文.contains("打字\tfsll"), "词码应重算: {}", 词文);

        let 快文 = std::fs::read_to_string(输出.join("yuhao/starmoon_sc.quick.dict.yaml")).unwrap();
        assert!(快文.contains("好\tl"), "快码应保留档位: {}", 快文);

        let 简文 = std::fs::read_to_string(输出.join("yuhao/starmoon_sc.short.dict.yaml")).unwrap();
        assert!(简文.contains("打字\tf"), "简码应保留档位: {}", 简文);

        // 完整码表：四类来源 + 主词典裸词（现算）合并去重
        let 码表 = std::fs::read_to_string(输出.join("完整码表.txt")).unwrap();
        assert!(码表.contains("好\tludk"), "单字全码应在: {}", 码表);
        assert!(码表.contains("好\tl"), "单字简码应在: {}", 码表);
        assert!(码表.contains("打字\tfsll"), "词组应在: {}", 码表);
        assert!(码表.contains("打字\tf"), "词组简码应在: {}", 码表);
        assert!(
            码表.contains("一字\todll"),
            "主词典裸词应按 encoder 现算并入: {}",
            码表
        );

        let _ = std::fs::remove_dir_all(&目录);
    }
}
