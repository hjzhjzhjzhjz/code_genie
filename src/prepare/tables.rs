// =========================================================================
// 📊 prepare 步骤 0–4：字频表、字符集、拆分表、字根频率
// =========================================================================
//
// 步骤编号对应 `prepare-inputs.sh` 里的 `echo '(N) ...'`。
// 各步骤的排序键（`-k3,3nr -k1,1` 之类）已在函数注释里标出，改代码时别动。
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use crate::prepare::names;
use crate::prepare::text::{self, 数值字段};
use crate::prepare::上下文;

const 简体字频网址: &str = "https://ceping.shurufa.app/data/charAbsoluteFrequencySC.json";
const 繁体字频网址: &str = "https://ceping.shurufa.app/data/charAbsoluteFrequencyTC.json";

/// 步骤 0：生成全字集字频表 `全字频表.txt`
///
/// 混合模式（`USE_MIXED_FREQ != 0`）取「简体字频」与「繁体字频 × 权重」的较大值，
/// 这样简繁各自的高频顺序都保留；纯简体模式直接透传 `beiyu-char-freq.txt`。
pub fn 步骤0_全字频表(上: &上下文) -> Result<(), String> {
    if 上.全字频表 != names::全字频表 {
        println!("(0) 使用方案自行提供的字频表 {} ...", 上.全字频表);
        return Ok(());
    }

    if 上.方案.混合字频权重 == 0.0 {
        println!("(0) 生成简体字频表 全字频表.txt ...");
        let mut 输出 = String::new();
        for 行 in 上.读("beiyu-char-freq.txt")? {
            let 列: Vec<&str> = 行.split_whitespace().collect();
            if 列.len() < 2 {
                continue;
            }
            输出.push_str(列[0]);
            输出.push('\t');
            输出.push_str(列[1]);
            输出.push('\n');
        }
        return 上.写(names::全字频表, &输出);
    }

    println!(
        "(0) 生成简繁混合字频表 全字频表.txt ，繁体字频权重={} ...",
        上.方案.混合字频权重
    );
    let 简体 = 读字频json(上, "charAbsoluteFrequencySC.json", 简体字频网址)?;
    let 繁体 = 读字频json(上, "charAbsoluteFrequencyTC.json", 繁体字频网址)?;
    let 权重 = 上.方案.混合字频权重;
    let mut 合并 = 简体;
    for (字, 值) in 繁体 {
        let 条目 = 合并.entry(字).or_insert(0.0);
        let 繁体权重 = 权重 * 值;
        if 繁体权重 > *条目 {
            *条目 = 繁体权重;
        }
    }

    let mut 表: Vec<(String, f64)> = 合并.into_iter().collect();
    // Perl: sort { $h->{$b} <=> $h->{$a} || $a cmp $b }
    text::按值降序(&mut 表);
    let mut 输出 = String::with_capacity(表.len() * 12);
    for (字, 值) in 表 {
        // Perl: print "$_\t", ceil($h->{$_})
        输出.push_str(&format!("{字}\t{}\n", 值.ceil()));
    }
    上.写(names::全字频表, &输出)
}

fn 读字频json(上: &上下文, 名: &str, 网址: &str) -> Result<HashMap<String, f64>, String> {
    // 素材目录（`yuhao-data/`）里通常已有这两份 JSON；没有才下载，且下到素材目录
    let 路径 = crate::data_dir::找(&上.目录, 名).unwrap_or_else(|| 上.素材目录.join(名));
    if !路径.exists() {
        println!("    下载 {名} ...");
        下载(&上.素材目录, 网址, 名)?;
    }
    let 文本 = std::fs::read_to_string(&路径)
        .map_err(|e| format!("读取 {} 失败：{}", 路径.display(), e))?;
    serde_json::from_str(&文本).map_err(|e| format!("解析 {名} 失败：{e}"))
}

/// 与 bash 版一致地依赖本机 `curl`，失败时给出可操作的手动下载提示
fn 下载(目录: &Path, 网址: &str, 名: &str) -> Result<(), String> {
    std::fs::create_dir_all(目录)
        .map_err(|e| format!("创建素材目录 {} 失败：{e}", 目录.display()))?;
    let 结果 = Command::new("curl")
        .args(["-O", "--fail", "--retry", "3", "--retry-delay", "2", 网址])
        .current_dir(目录)
        .status();
    match 结果 {
        Ok(状态) if 状态.success() => Ok(()),
        Ok(状态) => Err(format!(
            "下载 {名} 失败（curl 退出码 {}）；请手动下载 {网址} 放到 {} 下",
            状态.code().unwrap_or(-1),
            目录.display()
        )),
        Err(e) => Err(format!(
            "无法执行 curl 下载 {名}（{e}）；请手动下载 {网址} 放到 {} 下",
            目录.display()
        )),
    }
}

/// 步骤 1：从 `yuhao_charsets.lua` 提取简繁常用字符集 `算码字集.txt`
///
/// Perl 侧是 `(/\[\[/ .. eof) && /^\p{Han}$/`：从首个含 `[[` 的行起，只保留
/// 「整行恰好一个汉字」的行，最后 `LC_ALL=C sort -u`。
pub fn 步骤1_字符集(上: &上下文) -> Result<(), String> {
    println!("(1) 从 yuhao_charsets.lua 生成简繁常用字符集 算码字集.txt ...");
    let mut 字表: Vec<String> = Vec::new();
    let mut 已开始 = false;
    for 行 in 上.读("yuhao_charsets.lua")? {
        if !已开始 {
            if !行.contains("[[") {
                continue;
            }
            已开始 = true;
        }
        let mut 字符 = 行.chars();
        if let (Some(字), None) = (字符.next(), 字符.next()) {
            if text::是汉字(字) {
                字表.push(行);
            }
        }
    }
    字表.sort();
    字表.dedup();
    let 输出: String = 字表.iter().map(|s| format!("{s}\n")).collect();
    上.写(names::算码字集, &输出)?;
    if 上.算码字集 != names::算码字集 {
        println!("    !!! 使用方案自行提供的算码字集 {} ...", 上.算码字集);
    }
    Ok(())
}

/// 步骤 2：按算码字符集过滤字频表，生成 `算码字频.txt`
///
/// 未出现在字频表里的字符补 `\t0` 追加在末尾（码点序），并向 stderr 告警。
pub fn 步骤2_算码字频(上: &上下文) -> Result<(), String> {
    println!(
        "(2) 从字频表 {} 生成算码字符集的字频表 算码字频.txt ...",
        上.全字频表
    );
    let mut 剩余: HashSet<String> = 上
        .读(&上.算码字集)?
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let mut 输出 = String::new();
    for 行 in 上.读(&上.全字频表)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 2 || 数值字段(列[1]) <= 0.0 {
            continue;
        }
        if !剩余.remove(列[0]) {
            continue;
        }
        输出.push_str(列[0]);
        输出.push('\t');
        输出.push_str(列[1]);
        输出.push('\n');
    }

    let mut 缺: Vec<&String> = 剩余.iter().collect();
    缺.sort();
    if !缺.is_empty() {
        eprintln!("    WARN: {} 个字符没有权重!", 缺.len());
    }
    for 字 in 缺 {
        输出.push_str(&format!("{字}\t0\n"));
    }
    上.写(names::算码字频, &输出)
}

/// 步骤 3：从宇浩星陈拆分表生成 `拆分表.txt`（算码字集）与
/// `拆分全表.txt` / `拆分全表-台.txt`（全字集）
pub fn 步骤3_拆分表(上: &上下文) -> Result<(), String> {
    println!(
        "(3) 从宇浩星陈方案的大陆字形拆分表 yustar_chaifen.dict.yaml 生成算码字符集的拆分表 拆分表.txt 和 全字集拆分表 拆分全表.txt ..."
    );

    // 算码版：只保留 算码字频.txt 里的字，且要求表里一个都不缺
    let mut 频: HashMap<String, String> = HashMap::new();
    for 行 in 上.读(names::算码字频)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 {
            频.insert(列[0].to_string(), 列[1].to_string());
        }
    }
    // 注意 `remove`：Perl 版是 `delete $h{$a}`，跑完剩下的就是「拆分表里缺失的字」
    let mut 行表 = 解析拆分表(上, "yustar_chaifen.dict.yaml", false, false, |字| {
        频.remove(字)
    })?;
    if !频.is_empty() {
        let mut 缺: Vec<&String> = 频.keys().collect();
        缺.sort();
        let 名单: Vec<&str> = 缺.iter().map(|s| s.as_str()).collect();
        return Err(format!("No chaifen found for {}", 名单.join(" ")));
    }
    写拆分表(&上.路径(names::拆分表), &mut 行表)?;

    // 全字集版：字频缺失按 0，首列允许多字符（可能是变体选择符）
    let mut 全频: HashMap<String, String> = HashMap::new();
    for 行 in 上.读(&上.全字频表)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 {
            全频.insert(列[0].to_string(), 列[1].to_string());
        }
    }
    // 大陆字形 → 拆分全表.txt，台湾字形 → 拆分全表-台.txt
    for (源文件, 产物) in [
        ("yustar_chaifen.dict.yaml", names::拆分全表),
        ("yustar_chaifen_tw.dict.yaml", names::拆分全表台),
    ] {
        if !上.路径(源文件).exists() {
            continue;
        }
        let mut 表 = 解析拆分表(上, 源文件, true, true, |字| 全频.get(字).cloned())?;
        写拆分表(&上.路径(产物), &mut 表)?;
    }
    Ok(())
}

/// 解析拆分表的 `...` 之后的数据区
///
/// `取频` 返回 `None` 表示「字频表里没有这个字」。此时有两种处理：
/// - `保留缺失 = false`（算码版）：跳过该行。闭包里用 `remove` 顺手记录「已覆盖」，
///   调用方跑完再看还剩哪些字没拆分到（对应 Perl 的 `delete $h{$a}` + `die`）。
/// - `保留缺失 = true`（全字集版）：仍然保留该行，字频记 0
///   （对应 Perl 的 `print ... , $h{$a} // 0`）。
fn 解析拆分表<F>(
    上: &上下文,
    文件: &str,
    首列可多字: bool,
    保留缺失: bool,
    mut 取频: F,
) -> Result<Vec<(String, String, String)>, String>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut 表 = Vec::new();
    let mut 已完成 = false;
    for 行 in 上.读(文件)? {
        if !已完成 {
            if !行.starts_with("...") {
                continue;
            }
            已完成 = true;
            continue;
        }
        let Some((字, 根串)) = 拆出字与根串(&行, 首列可多字) else {
            continue;
        };
        let 值 = match 取频(&字) {
            Some(值) => 值,
            None if 保留缺失 => "0".to_string(),
            None => continue,
        };
        let 根 = 提取字根(&根串);
        表.push((字, 根.join(" "), 值));
    }
    Ok(表)
}

/// 从拆分表一行里取出「字」与根串
///
/// 对应 Perl 的 `^(\S)\t\[([^,]+)/`（单字版）与 `^([^\t]+)\t\[([^,]+)/`（全字集版）。
fn 拆出字与根串(行: &str, 首列可多字: bool) -> Option<(String, String)> {
    let 制表 = 行.find('\t')?;
    let 首列 = &行[..制表];
    if 首列.is_empty() {
        return None;
    }
    if !首列可多字 {
        let mut 字符 = 首列.chars();
        let 字 = 字符.next()?;
        if 字符.next().is_some() || 字.is_whitespace() {
            return None;
        }
    }
    let 余 = 行[制表 + 1..].strip_prefix('[')?;
    let 根串 = 余.split(',').next()?;
    if 根串.is_empty() {
        return None;
    }
    Some((首列.to_string(), 根串.to_string()))
}

/// 提取字根：`/\{[^\}]+\}|\S/g`
///
/// `{京上}` 这类花括号整体算一个根；其余按单字符切。最后把 `⺮` 归并到 `竹`
/// （拆分表本身不区分 ⺮ / 竹，归并后这类字根才有声母）。
fn 提取字根(s: &str) -> Vec<String> {
    let 字: Vec<char> = s.chars().collect();
    let mut 结果: Vec<String> = Vec::new();
    let mut i = 0;
    while i < 字.len() {
        if 字[i].is_whitespace() {
            i += 1;
            continue;
        }
        if 字[i] == '{' {
            if let Some(偏移) = 字[i + 1..].iter().position(|c| *c == '}') {
                if 偏移 > 0 {
                    结果.push(字[i..=i + 1 + 偏移].iter().collect());
                    i += 偏移 + 2;
                    continue;
                }
            }
        }
        结果.push(字[i].to_string());
        i += 1;
    }
    结果
        .into_iter()
        .map(|根| if 根 == "⺮" { "竹".to_string() } else { 根 })
        .collect()
}

/// 拆分表排序写盘：`LC_ALL=C sort -t <TAB> -s -k3,3nr -k1,1`
///
/// 频率降序、同频按汉字字节序升序；`-s` 稳定排序，等价于 Rust 的 `sort_by`（稳定）。
fn 写拆分表(路径: &Path, 行表: &mut [(String, String, String)]) -> Result<(), String> {
    行表.sort_by(|a, b| {
        数值字段(&b.2)
            .partial_cmp(&数值字段(&a.2))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut 出 = String::with_capacity(行表.len() * 16);
    for (字, 根, 值) in 行表.iter() {
        出.push_str(字);
        出.push('\t');
        出.push_str(根);
        出.push('\t');
        出.push_str(值);
        出.push('\n');
    }
    text::写文本(路径, &出)
}

/// 步骤 4：从 `拆分全表.txt` 统计字根频率，生成 `字根频率.txt`
///
/// 分母是「字频 × 根数」的累加；同一个字根在一次拆分里出现两次就算两次。
pub fn 步骤4_字根频率(上: &上下文) -> Result<(), String> {
    println!("(4) 从 拆分全表.txt 生成字根频率表 字根频率.txt ...");
    let mut 根频: HashMap<String, i64> = HashMap::new();
    let mut 总: i64 = 0;
    for 行 in 上.读(names::拆分全表)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        let 频: i64 = 列
            .get(2)
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let 根: Vec<&str> = 列
            .get(1)
            .map(|s| s.split_whitespace().collect())
            .unwrap_or_default();
        总 += 频 * 根.len() as i64;
        for r in 根 {
            *根频.entry(r.to_string()).or_insert(0) += 频;
        }
    }

    let 总 = 总 as f64;
    let mut 表: Vec<(String, f64)> = 根频.into_iter().map(|(k, v)| (k, v as f64)).collect();
    text::按值降序(&mut 表);
    let mut 出 = String::with_capacity(表.len() * 24);
    for (根, 频) in 表 {
        // Perl: printf "%s\t%.8f\n", $_, 100 * $h{$_} / $n
        出.push_str(&format!("{根}\t{:.8}\n", 100.0 * 频 / 总));
    }
    上.写(names::字根频率, &出)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 字根提取支持花括号与归并() {
        assert_eq!(提取字根("白 勹 丶"), vec!["白", "勹", "丶"]);
        assert_eq!(提取字根("{京上}口"), vec!["{京上}", "口"]);
        assert_eq!(提取字根("⺮"), vec!["竹"]);
        assert_eq!(提取字根("一勹"), vec!["一", "勹"]);
        assert_eq!(提取字根(""), Vec::<String>::new());
    }

    #[test]
    fn 拆行要求制表符与方括号() {
        assert_eq!(
            拆出字与根串("的\t[白 勹 丶,BSyⓈ,de,,CJK]", false),
            Some(("的".to_string(), "白 勹 丶".to_string()))
        );
        assert_eq!(拆出字与根串("无制表符", false), None);
        assert_eq!(拆出字与根串("的\t白 勹", false), None);
        // 单字版要求首列恰好一个字
        assert_eq!(拆出字与根串("直\u{fe00}\t[十 目]", false), None);
        // 全字集版允许首列带变体选择符。注意这里**故意**保留 Perl `([^,]+)` 的
        // 原样行为：没有逗号时闭方括号会被一起吞进根串（数据里不会出现这种行，
        // 但既然要求逐字节等价，就不在这里"顺手修好"）。
        assert_eq!(
            拆出字与根串("直\u{fe00}\t[十 目]", true),
            Some(("直\u{fe00}".to_string(), "十 目]".to_string()))
        );
        assert_eq!(
            拆出字与根串("直\u{fe00}\t[十 目,BSyⓈ]", true),
            Some(("直\u{fe00}".to_string(), "十 目".to_string()))
        );
    }
}
