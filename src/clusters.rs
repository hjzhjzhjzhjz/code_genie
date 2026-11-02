// =========================================================================
// 🧩 聚类重码分析
// =========================================================================
//
// 回答两个问题：
//
//   1. 哪一码区分力不够？—— 去掉第 i 码后重码暴涨，说明那一码在该位置几乎不提供信息；
//   2. 聚类表里哪一簇代价最高？—— 把一簇根压成同一个大码，换来键位复用，
//      付出重码，这一簇值不值。
//
// 聚类数据来自内嵌的 `assets/roots_cluster.ron`（见 `roots_cluster` 模块），
// 也可用 `--cluster` 指定一份 RON 文件，或临时写 `"根1 根2 ; 根3 根4"` 内联。
//
// 两种模式：
//
//   code_genie clusters                          逐簇代入聚类表，与「全不聚类」基线做差
//   code_genie clusters --cluster "卯 {卯左}"     只看指定这一簇
//
// 编码规则直接复用 `prepare::inputs::展开`（含回头码、声韵位），与优化器实际
// 出码同口径。已删的 `moling/analyze-duplicates-by-cluster.pl` 那套是简化版（只展开大码 +
// 声韵），两者数值不可比，本模块不追求与它逐字节一致。

use std::collections::HashMap;
use std::path::Path;

use crate::prepare::schema::{self, 方案};
use crate::prepare::{inputs, text};
use crate::roots_cluster::{self, 簇};

/// 五种取码法：前四种各去掉一码看剩下三码能否区分，最后一种用全码。
const 取码法: [&str; 5] = ["去0", "去1", "去2", "去3", "全码"];

pub struct 参数 {
    /// 方案名（moling / yuling / yustar），决定编码规则
    pub 方案: String,
    /// 纯根名拆分表（拆分表.txt：字<TAB>根 根 …<TAB>频次）
    pub 拆分表: String,
    /// 字根声韵表（字根声韵.txt：根<TAB>声韵<TAB>拼音）
    pub 字根声韵: String,
    /// 聚类表：留空用内嵌表；给路径则读该 RON；也可直接写 "根1 根2 ; 根3 根4"
    pub 聚类: String,
    /// 明细只列超过这么多重的组（全码那一种不受限）
    pub 最多重数: usize,
    /// 逐行代入聚类表，与「全不聚类」基线做差
    pub 逐行: bool,
}

pub fn run(参: &参数) -> Result<(), String> {
    let 方案 = schema::取(&参.方案)?;
    let 语境 = 加载(参, 方案)?;
    println!(
        "方案 {}（{}）／ {} 字／ 总频 {}",
        方案.名,
        方案.名称,
        语境.字表.len(),
        语境.总频 as u64
    );

    if 参.逐行 {
        逐行分析(&语境, &参.聚类)
    } else {
        单次分析(&语境, &参.聚类, 参.最多重数)
    }
}

// ───────────────────────────────── 数据 ─────────────────────────────────

struct 语境 {
    方案: &'static 方案,
    /// 根 → 声韵码（"ba"、"a"）
    声韵: HashMap<String, String>,
    /// (字, 根序列, 频次)
    字表: Vec<(String, Vec<String>, f64)>,
    总频: f64,
}

/// 一簇聚类的分析结果
struct 统计 {
    动重: [f64; 5],
    静重: [usize; 5],
    组数: [usize; 5],
    /// [取码法][重码组][字表下标]，组内频降序、组间首字频降序
    明细: [Vec<Vec<usize>>; 5],
}

fn 加载(参: &参数, 方案: &'static 方案) -> Result<语境, String> {
    let 声韵路径 = Path::new(&参.字根声韵);
    let mut 声韵 = HashMap::new();
    for 行 in text::读行(声韵路径)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.is_empty() || 列[0].is_empty() {
            continue;
        }
        声韵.insert(
            列[0].to_string(),
            列.get(1).copied().unwrap_or("").to_string(),
        );
    }
    if 声韵.is_empty() {
        return Err(format!("{} 里没有任何字根声韵", 声韵路径.display()));
    }

    let 拆分路径 = Path::new(&参.拆分表);
    let mut 字表 = Vec::new();
    for 行 in text::读行(拆分路径)? {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 3 {
            continue;
        }
        let 根: Vec<String> = 列[1].split_whitespace().map(str::to_string).collect();
        if 根.is_empty() {
            continue;
        }
        字表.push((列[0].to_string(), 根, text::数值字段(列[2])));
    }
    if 字表.is_empty() {
        return Err(format!("{} 里没有可用的拆分行", 拆分路径.display()));
    }
    let 总频: f64 = 字表.iter().map(|(_, _, f)| *f).sum();

    Ok(语境 {
        方案,
        声韵,
        字表,
        总频,
    })
}

// ───────────────────────────────── 取码 ─────────────────────────────────

/// 把一簇根展开成 4 个码字符
///
/// 槽位来自 `prepare::inputs::展开`，形如 `至.A` / `至.S` / `至.Y`：
///   - `.A` 大码位——在聚类里就换成簇标记 `#簇号`（一簇共用一个大码），否则留根名；
///   - `.S` / `.Y` 声母位 / 韵母位——取真实声韵字符，不同根同声韵会自然撞码。
fn 取码(根: &[&str], 语境: &语境, 聚类: &HashMap<String, usize>) -> [String; 4] {
    let 槽 = inputs::展开(根, &语境.声韵, 语境.方案);
    let mut 码: Vec<String> = Vec::with_capacity(4);
    for s in 槽 {
        let 值 = match s.rsplit_once('.') {
            Some((根名, "A")) => match 聚类.get(根名) {
                Some(组) => format!("#{组}"),
                None => format!("{根名}.A"),
            },
            Some((根名, "S")) => 声母(语境, 根名),
            Some((根名, "Y")) => 韵母(语境, 根名),
            _ => s.clone(),
        };
        码.push(值);
    }
    while 码.len() < 4 {
        码.push(String::new());
    }
    码.truncate(4);
    [码.remove(0), 码.remove(0), 码.remove(0), 码.remove(0)]
}

fn 声母(语境: &语境, 根: &str) -> String {
    match 语境.声韵.get(根).and_then(|v| v.chars().next()) {
        Some(c) => c.to_string(),
        // 声韵表里没有这个根：退回唯一标识，避免误判成重码
        None => format!("{根}.S"),
    }
}

/// 韵母 = 声韵码去掉首字符；单字符的纯韵母根（`a`、`o`）整个就是韵
fn 韵母(语境: &语境, 根: &str) -> String {
    let Some(值) = 语境.声韵.get(根) else {
        return format!("{根}.Y");
    };
    let 余: String = 值.chars().skip(1).collect();
    if 余.is_empty() {
        值.clone()
    } else {
        余
    }
}

// ───────────────────────────────── 统计 ─────────────────────────────────

fn 统计(语境: &语境, 聚类: &HashMap<String, usize>) -> 统计 {
    // 先把所有字的码算出来，五种取码法共用
    let 码表: Vec<[String; 4]> = 语境
        .字表
        .iter()
        .map(|(_, 根, _)| {
            let 根引用: Vec<&str> = 根.iter().map(String::as_str).collect();
            取码(&根引用, 语境, 聚类)
        })
        .collect();

    let mut 动重 = [0.0f64; 5];
    let mut 静重 = [0usize; 5];
    let mut 组数 = [0usize; 5];
    let mut 明细: [Vec<Vec<usize>>; 5] = std::array::from_fn(|_| Vec::new());

    for i in 0..5 {
        let mut 桶: HashMap<String, Vec<usize>> = HashMap::new();
        for (下标, 码) in 码表.iter().enumerate() {
            let 键 = if i < 4 {
                码.iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, s)| s.as_str())
                    .collect::<Vec<_>>()
                    .join("-")
            } else {
                码.join("-")
            };
            桶.entry(键).or_default().push(下标);
        }

        for mut 成员 in 桶.into_values() {
            if 成员.len() < 2 {
                continue;
            }
            静重[i] += 成员.len();
            组数[i] += 1;
            for &j in &成员 {
                动重[i] += 语境.字表[j].2;
            }
            // 组内：频降序，同频按字字节序升序
            成员.sort_by(|&a, &b| {
                语境.字表[b]
                    .2
                    .total_cmp(&语境.字表[a].2)
                    .then_with(|| 语境.字表[a].0.cmp(&语境.字表[b].0))
            });
            明细[i].push(成员);
        }

        // 组间：首字（组内最高频）频降序，同频按字升序
        明细[i].sort_by(|a, b| {
            语境.字表[b[0]]
                .2
                .total_cmp(&语境.字表[a[0]].2)
                .then_with(|| 语境.字表[a[0]].0.cmp(&语境.字表[b[0]].0))
        });
    }

    for v in 动重.iter_mut() {
        *v = if 语境.总频 > 0.0 {
            100.0 * *v / 语境.总频
        } else {
            0.0
        };
    }

    统计 {
        动重,
        静重,
        组数,
        明细,
    }
}

// ───────────────────────────────── 聚类表 ─────────────────────────────────
//
// 数据本身（哪几个根一簇、簇的顺序）统一由 `roots_cluster` 解析，
// 本模块只负责「把簇映射成组号」和报表。原先这里另有一份文本解析，与
// `prepare` 第 8 步、`zigen` 各写一遍，格式细节早已漂移；改 RON 后合成一份。

/// 解析 `--cluster`：留空 → 内嵌表；文件路径 → 读 RON；否则按
/// `"根1 根2 ; 根3 根4"` 内联解析
fn 解析聚类(源: &str) -> Result<Vec<簇>, String> {
    let 源 = 源.trim();
    if 源.is_empty() {
        return roots_cluster::内嵌();
    }
    let 路径 = std::path::Path::new(源);
    if 路径.is_file() {
        return roots_cluster::加载(Some(路径));
    }
    let mut 行表 = Vec::new();
    for 段 in 源.split([';', '；', ',', '，']) {
        let 根: Vec<String> = 段.split_whitespace().map(str::to_string).collect();
        if !根.is_empty() {
            行表.push(簇 {
                roots: 根,
                keys: String::new(),
            });
        }
    }
    if 行表.is_empty() {
        return Err("--cluster 既不是 RON 文件路径，也没有解析出任何字根".to_string());
    }
    Ok(行表)
}

fn 建映射(行表: &[簇]) -> HashMap<String, usize> {
    let mut 映射 = HashMap::new();
    for (组号, 簇) in 行表.iter().enumerate() {
        for 根 in &簇.roots {
            映射.insert(根.clone(), 组号);
        }
    }
    映射
}

// ───────────────────────────────── 输出 ─────────────────────────────────

fn 单次分析(语境: &语境, 源: &str, 最多: usize) -> Result<(), String> {
    let 行表 = 解析聚类(源)?;
    let 映射 = 建映射(&行表);
    let 全部: Vec<&str> = 行表
        .iter()
        .flat_map(|r| r.roots.iter().map(String::as_str))
        .collect();
    println!("聚类字根： {} {}", 全部.len(), 全部.join(" "));
    println!();
    let 统 = 统计(语境, &映射);
    打印统计(语境, &统, 最多);
    println!("口径说明：不同簇之间视为互不相同，只建模「簇内共用一个大码」。");
    println!("绝对数值不与 output-keymap.txt 的重码率可比，仅用于横向比较。");
    Ok(())
}

fn 打印统计(语境: &语境, 统: &统计, 最多: usize) {
    for i in 0..5 {
        println!("[{i}] 动重: {:.4}%", 统.动重[i]);
        println!("[{i}] 静重: {}", 统.静重[i]);
        println!("[{i}] 组数: {}", 统.组数[i]);
        for 组 in &统.明细[i] {
            // 一、二、三、四码位只剩 20/25 种取值，只看高重才有意义；
            // 全码（i=4）是真实重码，全列
            if i < 4 && 组.len() <= 最多 {
                continue;
            }
            let 字: Vec<&str> = 组.iter().map(|&j| 语境.字表[j].0.as_str()).collect();
            println!("[{i}]  {:3} {}", 组.len(), 字.join(" "));
        }
        println!();
    }
}

fn 逐行分析(语境: &语境, 源: &str) -> Result<(), String> {
    let 行表 = 解析聚类(源)?;
    let 空: HashMap<String, usize> = HashMap::new();
    let 基线 = 统计(语境, &空);

    println!();
    println!("=== 基线（全不聚类）===");
    for i in 0..5 {
        println!(
            "  {:<4} 动重 {:.4}%  静重 {}  组数 {}",
            取码法[i], 基线.动重[i], 基线.静重[i], 基线.组数[i]
        );
    }
    println!();

    struct 增量 {
        根: Vec<String>,
        动重: [f64; 5],
        /// 全码档的静重增量（重码字数）
        静重: isize,
    }

    let mut 全部 = Vec::with_capacity(行表.len());
    for 行 in &行表 {
        let mut 映射 = HashMap::new();
        for 根 in &行.roots {
            映射.insert(根.clone(), 0usize);
        }
        let 统 = 统计(语境, &映射);
        let 动重: [f64; 5] = std::array::from_fn(|i| 统.动重[i] - 基线.动重[i]);
        全部.push(增量 {
            根: 行.roots.clone(),
            动重,
            静重: 统.静重[4] as isize - 基线.静重[4] as isize,
        });
    }

    // 主键全码动重增量（最贴近真实重码），次键静重增量，最后按根串保证输出稳定
    全部.sort_by(|a, b| {
        b.动重[4]
            .total_cmp(&a.动重[4])
            .then_with(|| b.静重.cmp(&a.静重))
            .then_with(|| a.根.join(" ").cmp(&b.根.join(" ")))
    });

    println!(
        "{:<34} {:>10} {:>10} {:>10} {:>10} {:>10} {:>8}",
        "聚类根", "Δ动重0", "Δ动重1", "Δ动重2", "Δ动重3", "Δ动重全码", "Δ静重"
    );
    for 项 in &全部 {
        let 根串 = 项.根.join(" ");
        let 显: String = if 根串.chars().count() > 32 {
            let 截: String = 根串.chars().take(31).collect();
            format!("{截}…")
        } else {
            根串
        };
        println!(
            "{:<34} {:>+10.4} {:>+10.4} {:>+10.4} {:>+10.4} {:>+10.4} {:>+8}",
            显, 项.动重[0], 项.动重[1], 项.动重[2], 项.动重[3], 项.动重[4], 项.静重
        );
    }
    println!();
    println!(
        "共 {} 行聚类。Δ 为「只把这一行压成一个大码」相对全不聚类的变化，单位百分点。",
        全部.len()
    );
    println!("Δ动重全码 越大，说明这一行聚类换来的键位复用越贵。");
    println!();
    println!("口径说明：这里只建模「同一簇内的根共用一个大码」，不同簇之间仍视为互不相同。");
    println!("因此绝对数值不能与 output-keymap.txt 的「重码率」直接相比——后者把 975 个");
    println!("大码组压到 20 个键位上，还要扣掉出简字，绝对重码率天然低得多。");
    println!("本表只用于横向比较：在同一套口径下，哪一行聚类更贵。");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::schema::魔灵;

    fn 造语境(声韵: &[(&str, &str)]) -> 语境 {
        语境 {
            方案: &魔灵,
            声韵: 声韵
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            字表: Vec::new(),
            总频: 0.0,
        }
    }

    /// 魔灵二根字码位：A1 A2 S2 S1（`.Y` 被 MAX_CODE_LEN=4 截掉）
    #[test]
    fn 二根字的槽位与聚类替换() {
        let 语境 = 造语境(&[("白", "ba"), ("勺", "shao")]);
        let mut 聚类 = HashMap::new();
        聚类.insert("白".to_string(), 0usize);

        let 码 = 取码(&["白", "勺"], &语境, &聚类);
        // 白在簇 0 → #0；勺不在簇里 → 留根名；勺.S → s；白.S → b
        assert_eq!(码, ["#0", "勺.A", "s", "b"]);
    }

    #[test]
    fn 无聚类时大码位保留根名() {
        let 语境 = 造语境(&[("白", "ba"), ("勺", "shao")]);
        let 码 = 取码(&["白", "勺"], &语境, &HashMap::new());
        assert_eq!(码, ["白.A", "勺.A", "s", "b"]);
    }

    /// 单字符声韵（纯韵母根）没有声母位，`.Y` 取整个字符
    #[test]
    fn 纯韵母根只有韵码() {
        let 语境 = 造语境(&[("丶", "a")]);
        let 码 = 取码(&["丶"], &语境, &HashMap::new());
        // 一根本该是 A S S Y，但无声母时只 push A + Y
        assert_eq!(码[0], "丶.A");
        assert_eq!(码[1], "a");
    }

    #[test]
    fn 韵母多字符时去掉首字符() {
        let 语境 = 造语境(&[("白", "bai"), ("丄", "a")]);
        assert_eq!(韵母(&语境, "白"), "ai");
        assert_eq!(韵母(&语境, "丄"), "a", "单字符根整个就是韵");
        assert_eq!(声母(&语境, "白"), "b");
    }

    /// 声韵表里查不到根的槽位退回唯一标识，避免把两个未知根误判成重码
    #[test]
    fn 未知根退回唯一标识() {
        let 语境 = 造语境(&[]);
        assert_eq!(声母(&语境, "某根"), "某根.S");
        assert_eq!(韵母(&语境, "某根"), "某根.Y");
    }

    #[test]
    fn 字符串形式的聚类参数按分隔符切分() {
        let 行表 = 解析聚类("卯 {卯左} ; 片 爿").unwrap();
        assert_eq!(行表.len(), 2);
        assert_eq!(行表[0].roots, ["卯", "{卯左}"]);
        assert_eq!(行表[1].roots, ["片", "爿"]);
    }

    /// 留空 → 内嵌聚类表；顺序必须稳定（`prepare` 按此顺序写约束文件）
    #[test]
    fn 空参数回落内嵌聚类表() {
        let 行表 = 解析聚类("  ").unwrap();
        assert_eq!(行表.len(), 92);
        assert_eq!(行表[0].roots, ["又", "ス", "マ", "癶"]);
    }

    #[test]
    fn 同簇两字同码时计入重码() {
        // 「纟」故意给一个以 b 开头的声韵，让两字的 S 位也相同——
        // 否则声母不同，聚类也只影响首码，全码仍能区分
        let mut 语境 = 造语境(&[("白", "ba"), ("勺", "shao"), ("纟", "ba")]);
        语境.字表 = vec![
            (
                "的".to_string(),
                vec!["白".to_string(), "勺".to_string()],
                10.0,
            ),
            (
                "约".to_string(),
                vec!["纟".to_string(), "勺".to_string()],
                5.0,
            ),
        ];
        语境.总频 = 15.0;

        // 不聚类：白.A vs 纟.A 不同，两者不撞
        let 空 = 统计(&语境, &HashMap::new());
        assert_eq!(空.静重[4], 0);

        // 把白和纟聚成一簇：A1 位相同，后面 勺.A / s 也相同 → 全码撞
        let mut 聚类 = HashMap::new();
        聚类.insert("白".to_string(), 0usize);
        聚类.insert("纟".to_string(), 0usize);
        let 聚 = 统计(&语境, &聚类);
        assert_eq!(聚.静重[4], 2);
        assert_eq!(聚.组数[4], 1);
        assert!((聚.动重[4] - 100.0).abs() < 1e-9, "两字频率覆盖全部总量");
    }

    #[test]
    fn 各取码法按去掉第i码分桶() {
        // 三根字有 4 个槽，去掉任一码都可能改变分组
        let mut 语境 = 造语境(&[("白", "ba"), ("勺", "shao"), ("丶", "a"), ("纟", "si")]);
        语境.字表 = vec![
            (
                "甲".to_string(),
                vec!["白".to_string(), "勺".to_string(), "丶".to_string()],
                1.0,
            ),
            (
                "乙".to_string(),
                vec!["纟".to_string(), "勺".to_string(), "丶".to_string()],
                1.0,
            ),
        ];
        语境.总频 = 2.0;
        let 统 = 统计(&语境, &HashMap::new());
        // 全码首码不同 → 不撞；去掉第 0 码后剩 [勺.A, 丶.A, a] → 撞
        assert_eq!(统.静重[4], 0);
        assert_eq!(统.静重[0], 2, "去掉首码后两字无法区分");
    }
}
