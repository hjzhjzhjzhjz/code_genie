// =========================================================================
// 📇 字形变体表（zigen）与字根映射表
// =========================================================================
//
// 取代 `moling/generate-root-chart.sh` 里**纯数据**的那几步。脚本的 8 个产物中，
// 已有 Rust 同口径实现的一律不搬：
//
//   roots.tsv      → `chart`（`root_chart::生成字根表`，同一套 keymap 解析与排序键）
//   chaifen.tsv    → `拆分全表.txt` 去掉频次列，`loader` 直接读原表
//   mabiao.tsv     → `export`（`export_table`，支持 `--schema` 三套取码规则，
//                                并打印全码长 / 动重）
//   *.js / *.html  → 依赖外部 typer 仓库的 perl 脚本，不搬
//
// 这里只补 Rust 侧真正没有、且有下游消费者的三样：
//
//   zigen-<方案>.csv             `字形,键位串,拼音` —— `rime-convert` 写 .roots.dict.yaml
//                                时读它；也是 typer 排版字根表的数据源
//   roots-mapping.tsv            `{花括号名}<TAB>字形` —— `chart -m` 用它把 PUA 字形
//                                贴到花括号字根上
//   zigen-trainer-<方案>.json    分「类 / 通 / 混」三型的字根卡组，给 zigen-trainer 背字根
//
// 与脚本的一处**故意差异**：脚本在字根数与 `yuhao-zigens.csv` 不一致时直接
// `die "Inconsistent number of roots: $a vs $b"`。实测魔灵方案就是 370 vs 368
// （多出 `⺮`、`失`），照搬等于搬一个必崩的分支。这里默认降级为警告并列出差集，
// 加 `--strict` 才按失败退出。
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::prepare::text::{写文本, 读行};

/// 元音字母：判「键位串是否含元音」用于字根排序
const 元音: [char; 5] = ['a', 'e', 'u', 'i', 'o'];

/// 末位字母 → 笔画名（与脚本里的 `%strokes = qw(a 折 e 撇 u 竖 i 点 o 横)` 一致）
const 笔画表: [(char, &str); 5] = [
    ('a', "折"),
    ('e', "撇"),
    ('u', "竖"),
    ('i', "点"),
    ('o', "横"),
];

/// 「简」型字根：简体部首（与脚本 `@classify` 的简表一致）
const 简根: [&str; 7] = ["乌", "{纟上}", "纟", "钅", "长", "{鸟上}", "鸟"];

/// 「繁」型字根：繁体部首
const 繁根: [&str; 24] = [
    "來", "僉", "烏", "{烏上}", "見", "貝", "頁", "車", "{專上}", "叀", "門", "風", "飛", "馬",
    "{馬上}", "鬥", "魚", "鳥", "{鳥上}", "鹵", "齒", "{龍右}", "{亞下}", "{亜下}",
];

/// 「混」型字对：简繁形近、需要放在一起记的字根组（含 PUA 字形，逐字照抄脚本）
const 混组: [&str; 25] = [
    "门門",
    "幺厶",
    "穴宀",
    "人\u{f620}",
    "口囗",
    "心\u{f765}",
    "火灬",
    "耂\u{f4c5}\u{f407}土",
    "\u{f887}糸",
    "金\u{f606}",
    "食\u{f6a4}",
    "长長",
    "鸟鳥",
    "乌烏",
    "鱼魚",
    "马馬",
    "车車",
    "页頁",
    "见見",
    "贝貝",
    "刀\u{f525}",
    "立辛",
    "\u{f745}丬",
    "\u{f780}言",
    "田甲",
];

/// 参数
pub struct 参数 {
    /// 方案名（只用于拼产物文件名，编码规则不参与）
    pub 方案: String,
    /// 字根键位表：`output-keymap.txt` 或 `字根键位表.txt`
    pub 键位文件: String,
    /// 源目录：取 `yuhao-zigens.csv` / `字根声韵.txt` / `roots-cluster.txt`
    pub 源目录: String,
    /// 输出目录
    pub 输出目录: String,
    /// 字根数与字形表不一致时按失败退出（脚本原行为）
    pub 严格: bool,
}

/// 一个键位槽位：A=大码、S=声码、Y=韵码、U=飞键
#[derive(Default, Clone, Copy)]
struct 槽位 {
    a: Option<char>,
    s: Option<char>,
    y: Option<char>,
    u: Option<char>,
}

// =========================================================================
// 输入解析
// =========================================================================

/// 读字根键位表，返回 `(字根, 键位串)`，顺序同脚本。
///
/// 两种格式都吃：
/// - 带槽位后缀（`干.A` / `干.S` / `干.Y` / `干.U`）→ 拼成 `A S Y`，飞键根另出一行 `U S Y`
/// - 朴素格式（`干<TAB>od<TAB>频次`）→ 原样照抄
pub(crate) fn 读字根码(路径: &Path) -> Result<Vec<(String, String)>, String> {
    解析字根码(&读行(路径)?)
}

/// [`读字根码`] 的纯解析部分
pub(crate) fn 解析字根码(行们: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut 槽表: HashMap<String, 槽位> = HashMap::new();
    let mut 朴素: Vec<(String, String)> = Vec::new();

    for 行 in 行们 {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 2 || 列[0].starts_with('#') {
            continue;
        }
        let (名, 码) = (列[0], 列[1]);
        let 码字 = 码.chars().next().map(|c| c.to_ascii_lowercase());

        if let Some((基, 尾)) = 名.rsplit_once('.') {
            if let (true, Some(码字)) = (尾.chars().count() == 1, 码字) {
                let 槽 = 槽表.entry(基.to_string()).or_default();
                match 尾 {
                    "A" => 槽.a = Some(码字),
                    "S" => 槽.s = Some(码字),
                    "Y" => 槽.y = Some(码字),
                    "U" => 槽.u = Some(码字),
                    _ => {}
                }
                if matches!(尾, "A" | "S" | "Y" | "U") {
                    continue;
                }
                // 后缀不是槽位（如 `.0`）→ 当朴素行，不能动已收集的槽位
            }
        }
        朴素.push((名.to_string(), 码.to_string()));
    }

    // 两种格式混用时以带后缀的为准——脚本是全局 grep 决定分支的，没有逐行兼容
    if 槽表.is_empty() {
        return Ok(朴素);
    }
    if !朴素.is_empty() {
        eprintln!("⚠️ 键位表里混了 {} 行朴素格式，按脚本口径忽略", 朴素.len());
    }

    let mut 出: Vec<(String, String)> = Vec::with_capacity(槽表.len() * 2);
    for (根, 槽) in &槽表 {
        let 拼 = |首: Option<char>| {
            let mut s = String::new();
            if let Some(c) = 首 {
                s.push(c);
            }
            if let Some(c) = 槽.s {
                s.push(c);
            }
            if let Some(c) = 槽.y {
                s.push(c);
            }
            s
        };
        if 槽.u.is_some() {
            出.push((根.clone(), 拼(槽.u)));
        }
        if 槽.a.is_none() {
            return Err(format!("字根「{}」只有小码没有 `.A` 大码", 根));
        }
        出.push((根.clone(), 拼(槽.a)));
    }

    // 脚本的排序键：含元音的在前 → 键位串升序 → 字根升序
    出.sort_by(|a, b| {
        let 甲 = a.1.contains(元音) as u8;
        let 乙 = b.1.contains(元音) as u8;
        乙.cmp(&甲)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    // ucfirst：首字母大写，其余保持小写
    for (_, 码) in &mut 出 {
        let mut 字 = 码.chars();
        if let Some(首) = 字.next() {
            *码 = format!("{}{}", 首.to_uppercase(), 字.as_str());
        }
    }
    Ok(出)
}

/// 读 `yuhao-zigens.csv`，返回 `(repr, font, 行号)`。
///
/// 跳过表头与 `⺮`（脚本注明「用『竹』代替了」），行号用于排序时保留原表顺序。
pub(crate) fn 读字形表(路径: &Path) -> Result<Vec<(String, String, usize)>, String> {
    解析字形表(&读行(路径)?)
}

/// [`读字形表`] 的纯解析部分
pub(crate) fn 解析字形表(行们: &[String]) -> Result<Vec<(String, String, usize)>, String> {
    let mut 出 = Vec::new();
    for (序号, 行) in 行们.iter().enumerate() {
        if 序号 == 0 || 行.trim().is_empty() {
            continue;
        }
        let 列: Vec<&str> = 行.split(',').collect();
        if 列.len() < 3 {
            continue;
        }
        let (repr, font) = (列[1].trim(), 列[2].trim());
        if repr == "⺮" {
            continue;
        }
        出.push((repr.to_string(), font.to_string(), 序号 + 1));
    }
    Ok(出)
}

/// `roots-mapping.tsv`：花括号名 → 字形，只留「字形与名字不同」的首现行
///
/// 对应脚本的 `print "$F[1]\t$F[2]" if $F[1] ne $F[2] && $F[1] =~ /\{/` +
/// `LC_ALL=C sort -u`（按 UTF-8 字节序去重排序）。
pub(crate) fn 字根映射(表: &[(String, String, usize)]) -> Vec<(String, String)> {
    let mut 见过: HashSet<&str> = HashSet::new();
    let mut 集: HashSet<(&str, &str)> = HashSet::new();
    for (repr, font, _) in 表 {
        if !见过.insert(repr.as_str()) {
            continue;
        }
        if repr != font && repr.contains('{') {
            集.insert((repr.as_str(), font.as_str()));
        }
    }
    let mut 出: Vec<(String, String)> = 集
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    出.sort();
    出
}

/// 读 `字根声韵.txt`：字根 → 拼音（第 3 列，缺失记空串）
pub(crate) fn 读拼音(路径: &Path) -> Result<HashMap<String, String>, String> {
    let mut 表 = HashMap::new();
    for 行 in 读行(路径)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.is_empty() {
            continue;
        }
        表.insert(
            列[0].to_string(),
            列.get(2).copied().unwrap_or("").to_string(),
        );
    }
    Ok(表)
}

/// 读 `roots-cluster.txt`：每行第一个 TAB 段是「首领 + 成员」，按空白切开
pub(crate) fn 读聚类(路径: &Path) -> Result<Vec<Vec<String>>, String> {
    let mut 出 = Vec::new();
    for 行 in 读行(路径)? {
        let 去空 = 行.trim();
        if 去空.is_empty() || 去空.starts_with('#') {
            continue;
        }
        let 首段 = 行.split('\t').next().unwrap_or("");
        let 组: Vec<String> = 首段.split_whitespace().map(str::to_string).collect();
        if !组.is_empty() {
            出.push(组);
        }
    }
    Ok(出)
}

// =========================================================================
// 排序序号
// =========================================================================

/// 字根排序序号：按（键位串, 字根）排序后每个 ×1000；同簇成员取「首领 + 位置」
///
/// `% 1000 == 0` 即「这一组的头一个字根」，决定 JSON 里何时另起一个「类」。
pub(crate) fn 计算序号(
    字根表: &[(String, String)],
    聚类: &[Vec<String>],
) -> HashMap<String, usize> {
    let mut 排序: Vec<&(String, String)> = 字根表.iter().collect();
    排序.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    let mut 序 = HashMap::with_capacity(字根表.len());
    for (i, (根, _)) in 排序.iter().enumerate() {
        序.insert(根.clone(), i * 1000);
    }
    for 组 in 聚类 {
        let Some(首) = 组.first() else { continue };
        let Some(&基) = 序.get(首) else { continue };
        for (i, 成) in 组.iter().enumerate().skip(1) {
            序.insert(成.clone(), 基 + i);
        }
    }
    序
}

// =========================================================================
// 输出
// =========================================================================

/// 一个字根下的字形排序：字形等于字根本身时排最前，其余按 CSV 原表行号
fn 字形排序(根: &str, 甲: &(String, usize), 乙: &(String, usize)) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (甲.0 == 根, 乙.0 == 根) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => 甲.1.cmp(&乙.1),
    }
}

fn 归类(根: &str) -> &'static str {
    if 简根.contains(&根) {
        "简"
    } else if 繁根.contains(&根) {
        "繁"
    } else {
        "通"
    }
}

fn 末笔(code: &str) -> (String, &'static str) {
    let 尾 = code.chars().last().unwrap_or(' ');
    let 名 = 笔画表
        .iter()
        .find(|(c, _)| *c == 尾)
        .map(|(_, n)| *n)
        .unwrap_or("");
    (尾.to_string(), 名)
}

/// 一条「类」条目
struct 类条目 {
    groups: Vec<Value>,
}

/// 汇总结果：CSV 文本 + 字根映射 + trainer JSON
struct 产物 {
    csv: String,
    json: Vec<Value>,
    /// 没有字形的字根（脚本会因此 die）
    缺字形: Vec<String>,
}

fn 生成(
    字根表: &[(String, String)],
    映射: &HashMap<String, HashMap<String, usize>>,
    拼音: &HashMap<String, String>,
    序号: &HashMap<String, usize>,
) -> 产物 {
    // 按序号排；同序号时按字根名兜底，保证可复现（脚本此处顺序不保证）
    let mut 有序: Vec<&(String, String)> = 字根表.iter().collect();
    有序.sort_by(|a, b| {
        序号
            .get(&a.0)
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(&序号.get(&b.0).copied().unwrap_or(usize::MAX))
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut csv = String::from("font,ma,pinyin\n");
    let mut 已用: HashSet<String> = HashSet::new();
    let mut 类们: Vec<类条目> = Vec::new();
    let mut 缺字形 = Vec::new();

    for (根, 码) in 有序 {
        let Some(字体们) = 映射.get(根) else {
            缺字形.push(根.clone());
            continue;
        };
        let mut 字体们: Vec<(String, usize)> =
            字体们.iter().map(|(k, v)| (k.clone(), *v)).collect();
        字体们.sort_by(|a, b| 字形排序(根, a, b));

        let 首个 = 字体们.first().map(|(f, _)| f.clone());
        let 拼音串 = 拼音.get(根).map(String::as_str).unwrap_or("");
        let (尾, 笔名) = 末笔(码);

        for (字, _) in &字体们 {
            if !已用.insert(字.clone()) {
                continue; // 艹 与 卄 共用字形，只留先出现的
            }
            csv.push_str(&format!("{},{},{}\n", 字, 码, 拼音串));

            let 描述 = format!(
                "{}  拼音：{}  首笔：{}({})",
                字,
                if 拼音串.is_empty() {
                    "无"
                } else {
                    拼音串
                },
                笔名,
                尾
            );
            let 是首领 = 序号.get(根).copied().unwrap_or(0) % 1000 == 0;
            let 是第一字形 = 首个.as_deref() == Some(字.as_str());

            if 是首领 && 是第一字形 {
                类们.push(类条目 {
                    groups: vec![json!({
                        "zigens": [字],
                        "code": 码,
                        "classify": 归类(根),
                        "description": 描述,
                    })],
                });
                continue;
            }

            // 接在上一个「类」里：码相同就并进同一子组，否则新开子组
            let 接上 = 类们
                .last_mut()
                .and_then(|类| 类.groups.last_mut())
                .filter(|组| 组.get("code").and_then(Value::as_str) == Some(码.as_str()));
            match 接上 {
                Some(组) => {
                    if let Some(Value::Array(们)) = 组.get_mut("zigens") {
                        们.push(Value::String(字.clone()));
                    }
                }
                None => {
                    let 新组 = json!({
                        "zigens": [字],
                        "code": 码,
                        "classify": "通",
                        "description": 描述,
                    });
                    match 类们.last_mut() {
                        Some(类) => 类.groups.push(新组),
                        // 首个字根就没有字形时脚本会崩，这里补一个类兜底
                        None => 类们.push(类条目 {
                            groups: vec![新组]
                        }),
                    }
                }
            }
        }
    }

    let mut 卡组: Vec<Value> = 类们
        .into_iter()
        .map(|类| json!({ "type": "类", "groups": 类.groups, "description": "" }))
        .collect();
    for 组 in 混组 {
        let 字们: Vec<Value> = 组.chars().map(|c| Value::String(c.to_string())).collect();
        卡组.push(json!({ "type": "混", "zigens": 字们, "description": 组 }));
    }

    产物 {
        csv,
        json: 卡组,
        缺字形,
    }
}

// =========================================================================
// 入口
// =========================================================================

/// 在候选路径里挑第一个存在的
fn 找(目录: &Path, 候选: &[&str]) -> Option<PathBuf> {
    for 名 in 候选 {
        let p = 目录.join(名);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn run(参: &参数) -> Result<(), String> {
    let 键位 = Path::new(&参.键位文件);
    let 源 = Path::new(&参.源目录);
    let 出目录 = Path::new(&参.输出目录);
    if !出目录.is_dir() {
        std::fs::create_dir_all(出目录)
            .map_err(|e| format!("创建输出目录 {} 失败：{}", 出目录.display(), e))?;
    }

    println!("=== CodeGenie 字形变体表 ===");
    println!("  方案:     {}", 参.方案);
    println!("  键位表:   {}", 键位.display());

    // 1. 字根 → 键位串
    let 字根表 = 读字根码(键位).map_err(|e| format!("读键位表失败：{e}"))?;
    if 字根表.is_empty() {
        return Err(format!("{} 里没有解析到任何字根", 键位.display()));
    }
    println!("  字根:     {} 条", 字根表.len());

    // 2. 字形变体表
    let 字形表路径 = 找(源, &["yuhao-zigens.csv"])
        .ok_or_else(|| format!("{} 下找不到 yuhao-zigens.csv", 源.display()))?;
    let 字形表 = 读字形表(&字形表路径)?;

    // 3. 拼音
    let 拼音名 = [
        format!("字根声韵-{}.txt", 参.方案),
        format!("roots-{}.txt", 参.方案),
        "字根声韵.txt".to_string(),
    ];
    let 拼音名: Vec<&str> = 拼音名.iter().map(String::as_str).collect();
    let 拼音 = match 找(源, &拼音名) {
        Some(p) => 读拼音(&p)?,
        None => {
            eprintln!("⚠️ {} 下没有字根声韵表，拼音列留空", 源.display());
            HashMap::new()
        }
    };

    // 4. 聚类（只影响排序与「类」的分组，缺了不算错）
    let 聚类名 = [
        format!("roots-cluster-{}.txt", 参.方案),
        "roots-cluster.txt".to_string(),
    ];
    let 聚类名: Vec<&str> = 聚类名.iter().map(String::as_str).collect();
    let 聚类 = match 找(源, &聚类名) {
        Some(p) => 读聚类(&p)?,
        None => {
            eprintln!("⚠️ {} 下没有聚类表，按字根各自独立排", 源.display());
            Vec::new()
        }
    };

    // 5. 数量对账（脚本在此 die）
    let 映射根: HashSet<&str> = 字形表.iter().map(|(r, _, _)| r.as_str()).collect();
    if 字根表.len() != 映射根.len() {
        let 多: Vec<&str> = 字根表
            .iter()
            .map(|(r, _)| r.as_str())
            .filter(|r| !映射根.contains(r))
            .collect();
        let 少: Vec<&str> = 映射根
            .iter()
            .copied()
            .filter(|r| !字根表.iter().any(|(k, _)| k == r))
            .collect();
        let 说明 = format!(
            "字根数不一致：键位表 {} 个，yuhao-zigens.csv {} 个（多：{}；少：{}）",
            字根表.len(),
            映射根.len(),
            若空(&多),
            若空(&少)
        );
        if 参.严格 {
            return Err(说明);
        }
        eprintln!("⚠️ {说明}，缺字形的字根不进 CSV");
    }

    // 6. 序号
    let 序号 = 计算序号(&字根表, &聚类);

    // 7. 组装
    let mut 按根: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (repr, font, 行号) in &字形表 {
        按根
            .entry(repr.clone())
            .or_default()
            .entry(font.clone())
            .or_insert(*行号); // 同一字根同一字形取首次出现
    }
    let 结 = 生成(&字根表, &按根, &拼音, &序号);

    // 8. 落盘
    let csv路径 = 出目录.join(format!("zigen-{}.csv", 参.方案));
    写文本(&csv路径, &结.csv)?;
    println!(
        "  已写入 {}（{} 行）",
        csv路径.display(),
        csv路径.lines_count()
    );

    let 映 = 字根映射(&字形表);
    let mut 映文 = String::new();
    for (a, b) in &映 {
        映文.push_str(&format!("{a}\t{b}\n"));
    }
    let 映路径 = 出目录.join("roots-mapping.tsv");
    写文本(&映路径, &映文)?;
    println!("  已写入 {}（{} 条）", 映路径.display(), 映.len());

    let 卡路径 = 出目录.join(format!("zigen-trainer-{}.json", 参.方案));
    let 文本 = serde_json::to_string_pretty(&Value::Array(结.json))
        .map_err(|e| format!("序列化字根卡组失败：{e}"))?;
    写文本(&卡路径, &(文本 + "\n"))?;
    println!("  已写入 {}", 卡路径.display());

    if !结.缺字形.is_empty() {
        println!(
            "  未进 CSV 的字根 {} 个：{}",
            结.缺字形.len(),
            结.缺字形.join(" ")
        );
    }
    Ok(())
}

/// 差集为空时给一句人话，别让日志出现空的括号
fn 若空(们: &[&str]) -> String {
    if 们.is_empty() {
        "无".to_string()
    } else {
        们.join("、")
    }
}

/// 小工具：数行数（CSV 已含表头）
trait 行数 {
    fn lines_count(&self) -> usize;
}

impl 行数 for PathBuf {
    fn lines_count(&self) -> usize {
        std::fs::read_to_string(self)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 行(们: &[&str]) -> Vec<String> {
        们.iter().map(|s| s.to_string()).collect()
    }

    /// 朴素格式（`根<TAB>两码<TAB>频次`）：原样照抄、保持文件顺序
    #[test]
    fn 朴素键位表原样输出() {
        let 出 = 解析字根码(&行(&[
            "# 综合得分: 0.4274",
            "一\tOd\t111511498",
            "口\tIe\t110611207",
        ]))
        .unwrap();
        assert_eq!(
            出,
            vec![
                ("一".to_string(), "Od".to_string()),
                ("口".to_string(), "Ie".to_string())
            ]
        );
    }

    /// 槽位格式：拼 A/S/Y，首字母大写，按「含元音 → 键位串 → 字根」排序
    #[test]
    fn 槽位键位表拼成三码并排序() {
        let 出 = 解析字根码(&行(&[
            "白\tf", "白.A\tf", "白.S\th", "白.Y\tk", "丶.A\te", "丶.S\tk", "丶.Y\tk",
        ]))
        .unwrap();
        // 丶 只有 ek 两码（无元音），白 有 fhk…… 都不含 aeuio，按码升序：ek < fhk
        assert_eq!(
            出,
            vec![
                ("丶".to_string(), "Ekk".to_string()),
                ("白".to_string(), "Fhk".to_string())
            ]
        );
    }

    /// 含元音的键位串排前面（脚本排序键的第一位）
    #[test]
    fn 含元音的字根排在前面() {
        let 出 = 解析字根码(&行(&[
            "甲.A\tb", "甲.S\tk", "甲.Y\tk", "乙.A\ta", "乙.S\tk", "乙.Y\tk",
        ]))
        .unwrap();
        assert_eq!(出[0].0, "乙", "乙 的码 a k k 含元音 a，排在前");
        assert_eq!(出[1].0, "甲");
    }

    /// 飞键根多出一行 `U S Y`
    #[test]
    fn 飞键根出两行() {
        let 出 = 解析字根码(&行(&["乙.A\ta", "乙.S\tk", "乙.Y\tk", "乙.U\tv"])).unwrap();
        assert_eq!(出.len(), 2);
        let 码集: Vec<&str> = 出.iter().map(|(_, c)| c.as_str()).collect();
        assert!(码集.contains(&"Vkk"), "飞键行用 U 槽：{码集:?}");
        assert!(码集.contains(&"Akk"), "正常行用 A 槽：{码集:?}");
    }

    /// 缺 `.A` 大码要报错，不能悄悄放过（脚本是 `die "No ASY root found"`）
    #[test]
    fn 缺大码报错() {
        let e = 解析字根码(&行(&["白.S\th", "白.Y\tk"])).unwrap_err();
        assert!(e.contains("没有 `.A` 大码"), "{e}");
    }

    /// 字形表跳过表头与「⺮」，行号从 1 起算
    #[test]
    fn 字形表跳过表头与竹() {
        let 出 = 解析字形表(&行(&[
            "root,repr,font,pua,strokes,unbroken",
            "𰀁,{奉下},\u{f409},,112,",
            "⺮,⺮,\u{f000},,1,",
            "一,一,一,,1,",
        ]))
        .unwrap();
        assert_eq!(出.len(), 2);
        assert_eq!(出[0], ("{奉下}".to_string(), "\u{f409}".to_string(), 2));
        assert_eq!(出[1], ("一".to_string(), "一".to_string(), 4));
    }

    /// 字根映射只留「带花括号且字形与名字不同」的首现行，按字节序去重
    #[test]
    fn 字根映射只留花括号首现行() {
        let 表 = vec![
            ("{甲上}".to_string(), "\u{f001}".to_string(), 2),
            ("{甲上}".to_string(), "\u{f002}".to_string(), 3), // 重复，忽略
            ("{乙下}".to_string(), "{乙下}".to_string(), 4),   // 字形同名字，跳过
            ("好".to_string(), "好".to_string(), 5),           // 无花括号，跳过
        ];
        assert_eq!(
            字根映射(&表),
            vec![("{甲上}".to_string(), "\u{f001}".to_string())]
        );
    }

    /// 聚类成员取「首领序号 + 位置」，其余按（码, 根）排序后 ×1000
    #[test]
    fn 序号按码排序且聚类覆盖() {
        let 根表 = vec![
            ("乙".to_string(), "Bk".to_string()),
            ("甲".to_string(), "Ak".to_string()),
            ("丙".to_string(), "Ck".to_string()),
        ];
        let 序 = 计算序号(&根表, &[vec!["乙".to_string(), "丙".to_string()]]);
        assert_eq!(序["甲"], 0, "Ak 码最小");
        assert_eq!(序["乙"], 1000);
        assert_eq!(序["丙"], 1001, "丙 被聚类收编，跟着乙走");
    }

    /// 一个字根下，字形等于字根本身的排最前，其余按原表行号
    #[test]
    fn 字形排序本体优先() {
        let 甲 = ("白".to_string(), 9);
        let 乙 = ("\u{f100}".to_string(), 3);
        assert_eq!(字形排序("白", &甲, &乙), std::cmp::Ordering::Less);
        assert_eq!(字形排序("白", &乙, &甲), std::cmp::Ordering::Greater);
        // 都不等于字根时按行号
        let 丙 = ("\u{f200}".to_string(), 1);
        assert_eq!(字形排序("白", &丙, &乙), std::cmp::Ordering::Less);
    }

    /// CSV 首行是表头，同一字形只出现一次，JSON 里「类」覆盖全部字形
    #[test]
    fn 产物结构自洽() {
        let 根表 = vec![
            ("甲".to_string(), "Ak".to_string()),
            ("乙".to_string(), "Bk".to_string()),
        ];
        let mut 按根: HashMap<String, HashMap<String, usize>> = HashMap::new();
        // 甲 有两个字形，第一个是它自己
        按根.insert(
            "甲".to_string(),
            HashMap::from([("甲".to_string(), 2usize), ("\u{f001}".to_string(), 3)]),
        );
        // 乙 的字形与甲共用 \u{f001}，应被去重掉，乙 因此没有字形进 CSV
        按根.insert(
            "乙".to_string(),
            HashMap::from([("\u{f001}".to_string(), 4usize)]),
        );
        let 拼音 = HashMap::from([("甲".to_string(), "jia".to_string())]);
        let 序 = 计算序号(&根表, &[]);
        let 结 = 生成(&根表, &按根, &拼音, &序);

        let 行们: Vec<&str> = 结.csv.lines().collect();
        assert_eq!(行们[0], "font,ma,pinyin");
        assert_eq!(行们.len(), 3, "表头 + 甲 + \u{f001}");
        assert!(行们[1].starts_with("甲,Ak,jia"), "{}", 行们[1]);

        let 类: Vec<&Value> = 结.json.iter().filter(|v| v["type"] == "类").collect();
        assert!(!类.is_empty());
        let 混 = 结.json.iter().filter(|v| v["type"] == "混").count();
        assert_eq!(混, 混组.len());
        assert!(结.缺字形.is_empty() || 结.缺字形 == ["乙"]);
    }

    /// 末笔取键位串最后一个字母，映射不到笔画时留空（脚本同样只有空串）
    #[test]
    fn 末笔映射() {
        assert_eq!(末笔("Akk"), ("k".to_string(), ""));
        assert_eq!(末笔("Aoa"), ("a".to_string(), "折"));
        assert_eq!(末笔("Aio"), ("o".to_string(), "横"));
    }

    /// 归类：简表优先于繁表，未列入的都是「通」
    #[test]
    fn 归类三型() {
        assert_eq!(归类("鸟"), "简");
        assert_eq!(归类("鳥"), "繁");
        assert_eq!(归类("白"), "通");
    }
}
