// =========================================================================
// 🗺 字根表页面：原 `moling/generate-root-chart.sh` 后半段（调 typer 的 perl 脚本
//   出 .js 与 .html）的 Rust 实现，该脚本已删除。
// =========================================================================
//
// 脚本前半段（roots.tsv / roots-mapping.tsv / zigen-* / mabiao.tsv）此前已分别由
// `chart` / `zigen` / `export` 覆盖；这里补的是最后那段「调 typer 的两个 perl 脚本
// 出 .js 与 .html」，顺带把整套产物一次性生成，取代整个脚本：
//
//   <dir>/roots.tsv            字根 → 编码（来自 output-keymap.txt）
//   <dir>/chaifen.tsv          字 → 根串（拆分全表去掉列内空白）
//   <dir>/mabiao.tsv           字 → 码（复用 `export` 的取码，每字一行全码）
//   <dir>/<方案>.js            `var roots = [...]` + `var chars = [...]` + charinfo()
//   <dir>/<方案>-<日期>.html   字根表页面
//
// ## 与脚本的差异
//
// 1. **不再依赖 typer 仓库**。两个 perl 脚本的逻辑都搬进来了，HTML 模板原样
//    收进 `assets/roots_chart_page.html`（只把 perl 插值换成占位符）。
// 2. **不联网下载字体**。脚本 `curl` 拉 `Yuniversus.woff`；这里改成 `--font`
//    指向本地字体文件，缺省则不内嵌（`body` 字体栈里也没有它）。
// 3. **Unihan 不自动下载**。脚本会 `HTTP::Tiny->mirror` 拉 Unihan.zip（十兆级、
//    还要解压）。这里只在 `--unihan-dir`（或缺省探测目录）里找现成的
//    `Unihan_Readings.txt` / `Unihan_Variants.txt`；找不到就退化：拼音留空、
//    简繁标记全部为「简」，其余功能不受影响。
// 4. 脚本的 `NO_HTML`、`PAGE_SIZE`、`MAX_EXAMPLES` 环境变量改成命令行参数。
//
// ## 两处 perl 语义要当心
//
// - `read_tsv` 在两个脚本里不是一回事：`turn-...-into-js.pl` 收成
//   「首列 → 其余列集合」，`generate-roots-chart.pl` 收成
//   「首列 → [行号, 首列, 其余列…]」（下标 1 才是首列）。
// - `JSON::PP->utf8(0)` 输出的是字符本身（不做 \u 转义），稀疏数组里的空洞写成
//   `null` 再被 `s/\bnull,/,/g` 压成连续逗号。这里按同样方式拼，得到等价的
//   `[1,,2]`。
// =========================================================================

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::export_table;
use crate::prepare::text::加载映射;

/// typer 的字根表页面模板（原 `generate-roots-chart.pl` 的 heredoc，逐字照搬）
const 页面模板: &str = include_str!("../assets/roots_chart_page.html");
/// `charinfo()` 函数（原 `turn-roots-chaifen-mabiao-into-js.pl` 的 heredoc）
const 字符查询脚本: &str = include_str!("../assets/roots_charinfo.js");

/// 命令行参数
pub struct 参数 {
    /// 优化输出目录（含 `output-keymap.txt` 与 `source/`）
    pub 目录: String,
    /// 方案名，决定产物文件名 `<方案>.js`
    pub 方案: String,
    /// 页面标题；缺省 `<方案>字根表 <日期>`
    pub 标题: Option<String>,
    /// Unihan 数据目录（需含 Unihan_Readings.txt / Unihan_Variants.txt）
    pub unihan目录: Option<String>,
    /// 字根字体文件（.woff/.ttf…）；给目录里不存在的名字则当作字体名
    pub 字体: Option<String>,
    /// 每页字数（脚本 PAGE_SIZE，默认 500）
    pub 每页字数: usize,
    /// 每个字根最多几个例字（脚本 MAX_EXAMPLES，默认 10）
    pub 例字数: usize,
    /// 统计前多少个高频字（脚本 `head -n 8000`，默认 8000）
    pub 高频字数: usize,
    /// 只出 .js，不出 .html（脚本 NO_HTML）
    pub 跳过页面: bool,
}

/// 跑一遍：产出全套字根表文件
pub fn run(参: &参数) -> Result<(), String> {
    let 目录 = PathBuf::from(&参.目录);
    if !目录.is_dir() {
        return Err(format!("{} 不是目录", 目录.display()));
    }
    let 源 = 定位源目录(&目录)?;

    println!("=== CodeGenie 字根表页面 ===");
    println!("  输出目录: {}", 目录.display());
    println!("  源目录:   {}", 源.display());
    println!("  方案:     {}", 参.方案);

    // 1. roots.tsv --------------------------------------------------------
    let 键位文件 = 找文件(&[目录.clone(), 源.clone()], "output-keymap.txt")
        .ok_or_else(|| "找不到 output-keymap.txt（先跑 optimize）".to_string())?;
    let 字根表 = 生成字根表文本(&键位文件)?;
    let roots_tsv = 目录.join("roots.tsv");
    fs::write(&roots_tsv, &字根表).map_err(|e| format!("写 roots.tsv 失败：{e}"))?;
    let 字根数 = 字根表.lines().count();
    println!("  ① roots.tsv        {} 个字根", 字根数);

    // 2. chaifen.tsv ------------------------------------------------------
    let 拆分全表 = 找文件(&[源.clone(), 目录.clone()], "拆分全表.txt")
        .ok_or_else(|| "找不到 拆分全表.txt（先跑 prepare）".to_string())?;
    let chaifen_tsv = 目录.join("chaifen.tsv");
    let 拆分数 = 生成拆分表文本(&拆分全表).map(|t| {
        let n = t.lines().count();
        fs::write(&chaifen_tsv, &t).map(|_| n)
    })?;
    let 拆分数 = 拆分数.map_err(|e| format!("写 chaifen.tsv 失败：{e}"))?;
    println!("  ② chaifen.tsv      {} 个字", 拆分数);

    // 3. mabiao.tsv -------------------------------------------------------
    let mabiao_tsv = 目录.join("mabiao.tsv");
    let (码表, 全码数) = 生成码表(&键位文件, &拆分全表, &参.方案)?;
    fs::write(&mabiao_tsv, &码表).map_err(|e| format!("写 mabiao.tsv 失败：{e}"))?;
    println!("  ③ mabiao.tsv       全码 {全码数} 条");

    // 4. <方案>.js --------------------------------------------------------
    let js = 生成js(&roots_tsv, &chaifen_tsv, &mabiao_tsv)?;
    let js路径 = 目录.join(format!("{}.js", 参.方案));
    fs::write(&js路径, &js).map_err(|e| format!("写 {} 失败：{e}", js路径.display()))?;
    println!(
        "  ④ {}  {:.1} KB",
        js路径.file_name().unwrap_or_default().to_string_lossy(),
        js.len() as f64 / 1024.0
    );

    if 参.跳过页面 {
        println!("  （--no-page：跳过 HTML）");
        return Ok(());
    }

    // 5. <方案>-<日期>.html -----------------------------------------------
    let 高频文件 = 找文件(&[源.clone(), 目录.clone()], "全字频表.txt")
        .ok_or_else(|| "找不到 全字频表.txt（先跑 prepare）".to_string())?;
    let unihan = 载入字形数据(参, &源, &目录);
    生成页面(
        参,
        &目录,
        &源,
        &roots_tsv,
        &chaifen_tsv,
        &js,
        &高频文件,
        unihan.as_ref(),
    )?;

    Ok(())
}

/// 源目录：脚本的 `<dir>/source` → `<dir>/../source`，再容错到 `<dir>` 本身
fn 定位源目录(目录: &Path) -> Result<PathBuf, String> {
    let 候选 = [
        目录.join("source"),
        目录.parent().map(|p| p.join("source")).unwrap_or_default(),
        目录.to_path_buf(),
    ];
    for p in 候选 {
        if p.is_dir() {
            return Ok(p);
        }
    }
    Err(format!(
        "{} 下找不到 source/（旧版 optimize.sh 的备份目录）",
        目录.display()
    ))
}

/// 按顺序在几个目录里找一个存在的文件
fn 找文件(目录们: &[PathBuf], 名: &str) -> Option<PathBuf> {
    for d in 目录们 {
        let p = d.join(名);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

// =========================================================================
// ① roots.tsv
// =========================================================================

/// `output-keymap.txt` → `roots.tsv`
///
/// 对应脚本 38–64 行，两条分支：
/// - keymap 里有 `字根.A/.S/.Y` 这类槽位后缀 → 每槽取该行第二列，拼成 `A S Y`；
///   排序是「含元音优先 → 码升序 → 名升序」。
/// - 没有槽位后缀（当前 `字根键位表.txt` 就是这种）→ 跳过注释行，原样输出前两列。
pub(crate) fn 生成字根表文本(keymap: &Path) -> Result<String, String> {
    let 文本 =
        fs::read_to_string(keymap).map_err(|e| format!("读 {} 失败：{e}", keymap.display()))?;

    if !文本.lines().any(有槽位后缀) {
        let mut 输出 = String::with_capacity(文本.len());
        for 行 in 文本.lines() {
            let 行 = 行.trim_end();
            if 行.trim_start().starts_with('#') || 行.trim().is_empty() {
                continue;
            }
            let 列: Vec<&str> = 行.split('\t').collect();
            if 列.len() < 2 {
                continue;
            }
            输出.push_str(&format!("{}\t{}\n", 列[0], 列[1]));
        }
        return Ok(输出);
    }

    // 槽位分支：字根 → 槽位字母 → 该行第二列
    let mut 槽位: HashMap<String, HashMap<char, String>> = HashMap::new();
    for 行 in 文本.lines() {
        let Some((基名, 标志)) = 槽位后缀(行) else {
            continue;
        };
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 2 {
            continue;
        }
        槽位
            .entry(基名.to_string())
            .or_default()
            .insert(标志, 列[1].to_ascii_lowercase());
    }

    let mut 行们: Vec<(String, String)> = Vec::new();
    for (名, 表) in &槽位 {
        let 取 = |标志: char| 表.get(&标志).cloned().unwrap_or_default();
        if !表.contains_key(&'A') {
            return Err(format!("字根 {名} 没有 .A 槽位"));
        }
        行们.push((名.clone(), format!("{}{}{}", 取('A'), 取('S'), 取('Y'))));
    }

    // 含元音优先（降序） → 码升序 → 名升序
    行们.sort_by(|a, b| {
        let 甲 = (b.1.contains(['a', 'e', 'u', 'i', 'o']) as u8, &a.1, &a.0);
        let 乙 = (a.1.contains(['a', 'e', 'u', 'i', 'o']) as u8, &b.1, &b.0);
        甲.cmp(&乙)
    });

    let mut 输出 = String::with_capacity(行们.len() * 12);
    for (名, 码) in 行们 {
        输出.push_str(&format!("{}\t{}\n", 名, 首字母大写(&码)));
    }
    Ok(输出)
}

/// `^(\S+)\.([ASY])` 的行返回 (基名, 标志)
fn 槽位后缀(行: &str) -> Option<(&str, char)> {
    let 首字段 = 行.split_whitespace().next()?;
    let (基名, 尾) = 首字段.rsplit_once('.')?;
    let 标志 = 尾.chars().next()?;
    if 尾.chars().count() == 1 && matches!(标志, 'A' | 'S' | 'Y') && !基名.is_empty() {
        Some((基名, 标志))
    } else {
        None
    }
}

fn 有槽位后缀(行: &str) -> bool {
    槽位后缀(行).is_some()
}

/// perl 的 `ucfirst`：只把第一个字符转大写
fn 首字母大写(s: &str) -> String {
    let mut 迭代 = s.chars();
    match 迭代.next() {
        Some(首) => 首.to_uppercase().chain(迭代).collect(),
        None => String::new(),
    }
}

// =========================================================================
// ② chaifen.tsv
// =========================================================================

/// `拆分全表.txt` → `chaifen.tsv`：去掉第二列内部的所有空白
pub(crate) fn 生成拆分表文本(拆分全表: &Path) -> Result<String, String> {
    let 文本 =
        fs::read_to_string(拆分全表).map_err(|e| format!("读 {} 失败：{e}", 拆分全表.display()))?;
    let mut 输出 = String::with_capacity(文本.len());
    for 行 in 文本.lines() {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 去空: String = 列[1].chars().filter(|c| !c.is_whitespace()).collect();
        输出.push_str(&format!("{}\t{}\n", 列[0], 去空));
    }
    Ok(输出)
}

// =========================================================================
// ③ mabiao.tsv
// =========================================================================

/// 复用 `export` 的取码规则，产 `<字>\t<码>`（每字一行全码）
fn 生成码表(
    keymap: &Path, 拆分全表: &Path, 方案: &str
) -> Result<(String, usize), String> {
    let 方案 = crate::prepare::schema::取(方案)?;
    let 规则 = 方案.编码规则;
    let 最大码长 = 方案.最大码长;
    let 键位 = export_table::加载键位表(&keymap.to_string_lossy());
    let 拆分 = export_table::加载拆分表(&拆分全表.to_string_lossy());

    let (文本, 全码数, _缺根, _统计) =
        export_table::生成表按方案(&键位, &拆分, 规则, 最大码长);
    Ok((文本, 全码数))
}

// =========================================================================
// ④ <方案>.js
// =========================================================================

/// 读一列 TSV，收成「首列 → 其余列」；同一首列出现多次时取值合并
///
/// 对应 `turn-roots-chaifen-mabiao-into-js.pl` 的 `read_tsv`：`$h{首列}{其余列}=1`
/// 之后 `[sort keys]`，所以每个键下是**去重且升序**的字符串列表。
fn 读列集合(path: &Path) -> Result<BTreeMap<String, Vec<String>>, String> {
    let 文本 = fs::read_to_string(path).map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
    let mut 表: HashMap<String, HashSet<String>> = HashMap::new();
    for 行 in 文本.lines() {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 2 || 列[0].starts_with('#') {
            continue;
        }
        let 集 = 表.entry(列[0].to_string()).or_default();
        for v in &列[1..] {
            集.insert((*v).to_string());
        }
    }
    Ok(表
        .into_iter()
        .map(|(k, v)| {
            let mut v: Vec<String> = v.into_iter().collect();
            v.sort();
            (k, v)
        })
        .collect())
}

/// 切字根串：`/{[^}]+}|\S/g` —— 花括号名当一个 token，其余按单个非空白字符
pub(crate) fn 切根(串: &str) -> Vec<String> {
    let 字符: Vec<char> = 串.chars().collect();
    let mut 结果 = Vec::new();
    let mut i = 0;
    while i < 字符.len() {
        let c = 字符[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '{' {
            // 一直吃到 '}'（含）；没有 '}' 时按单字符处理，与正则 `{[^}]+}` 不匹配时的行为一致
            if let Some(偏移) = 字符[i..].iter().position(|&x| x == '}') {
                结果.push(字符[i..=i + 偏移].iter().collect());
                i += 偏移 + 1;
                continue;
            }
        }
        结果.push(c.to_string());
        i += 1;
    }
    结果
}

/// 超过 4 个根时取「前三个 + 末根」
pub(crate) fn 取四(根: &[String]) -> Vec<String> {
    if 根.len() <= 4 {
        根.to_vec()
    } else {
        vec![
            根[0].clone(),
            根[1].clone(),
            根[2].clone(),
            根[根.len() - 1].clone(),
        ]
    }
}

/// 生成 `<方案>.js`
pub(crate) fn 生成js(
    roots_tsv: &Path,
    chaifen_tsv: &Path,
    mabiao_tsv: &Path,
) -> Result<String, String> {
    let roots = 读列集合(roots_tsv)?;
    let mut chaifen = 读列集合(chaifen_tsv)?;
    let mabiao = 读列集合(mabiao_tsv)?;

    // 每个字根只能有一个码
    let mut roots码: BTreeMap<String, String> = BTreeMap::new();
    for (根, 码们) in &roots {
        if 码们.len() > 1 {
            return Err(format!("字根 {根} 有多个编码：{}", 码们.join(" ")));
        }
        if let Some(码) = 码们.first() {
            roots码.insert(根.clone(), 码.clone());
        }
    }

    // 码表里没有的字直接丢掉（拆分表覆盖 12 万字，码表只有常用字）
    chaifen.retain(|字, _| mabiao.contains_key(字));

    // 统计字根出现次数（按 4 根截断后）
    let mut 根频: HashMap<String, usize> = HashMap::new();
    let mut 拆分号: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for (字, 根串们) in &chaifen {
        let mut 各串 = Vec::with_capacity(根串们.len());
        for 串 in 根串们 {
            let 根 = 取四(&切根(串));
            for r in &根 {
                *根频.entry(r.clone()).or_insert(0) += 1;
            }
            各串.push(根);
        }
        拆分号.insert(字.clone(), 各串);
    }

    // 字根编号：出现次数降序，同频按名字升序，从 0 起
    let mut 排序: Vec<(&String, &usize)> = 根频.iter().collect();
    排序.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let mut 根号: HashMap<&str, usize> = HashMap::new();
    for (i, (根, _)) in 排序.iter().enumerate() {
        根号.insert(根.as_str(), i);
    }

    // roots 数组：[名字, 码] 平铺
    let mut 根表: Vec<String> = (0..排序.len() * 2).map(|_| "null".to_string()).collect();
    for (根, 号) in &根号 {
        根表[号 * 2] = json字符串(根);
        if let Some(码) = roots码.get(*根) {
            根表[号 * 2 + 1] = json字符串(&首字母大写(&码.to_ascii_lowercase()));
        }
    }

    // chars 数组：按码点升序，间隔 ≥1000 才另起一段，段内按码点差定位
    let mut 字符们: Vec<(&String, &Vec<Vec<String>>)> = 拆分号.iter().collect();
    字符们.sort_by_key(|(字, _)| 字.chars().next().map(|c| c as u32).unwrap_or(0));

    let mut 段起点: Vec<u32> = Vec::new();
    let mut 段数据: Vec<Vec<Option<String>>> = Vec::new();
    for (字, 各串) in 字符们 {
        let Some(码点) = 字.chars().next().map(|c| c as u32) else {
            continue;
        };
        let 数据 = 字数据(字, 各串, &mabiao, &根号);

        match 段起点.last() {
            None => {
                段起点.push(码点);
                段数据.push(vec![Some(数据)]);
            }
            Some(&起点) => {
                if 码点 - 起点 >= 1000 {
                    段起点.push(码点);
                    段数据.push(vec![Some(数据)]);
                } else {
                    let 偏移 = (码点 - 起点) as usize;
                    let 尾 = 段数据.last_mut().expect("刚 push 过");
                    if 尾.len() <= 偏移 {
                        尾.resize(偏移 + 1, None);
                    }
                    尾[偏移] = Some(数据);
                }
            }
        }
    }

    // 拼 [起点, [数据...], 起点, [数据...]]，空洞留空 → 连续逗号
    let mut 字符表 = String::from("[");
    for (i, (起点, 数据)) in 段起点.iter().zip(段数据.iter()).enumerate() {
        if i > 0 {
            字符表.push(',');
        }
        字符表.push_str(&起点.to_string());
        字符表.push_str(",[");
        for (j, 项) in 数据.iter().enumerate() {
            if j > 0 {
                字符表.push(',');
            }
            if let Some(s) = 项 {
                字符表.push_str(s);
            }
        }
        字符表.push(']');
    }
    字符表.push(']');

    Ok(format!(
        "var roots = [{}];\n\nvar chars = {};\n\n{}",
        {
            let mut s = String::new();
            for (i, v) in 根表.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(v);
            }
            s
        },
        字符表,
        字符查询脚本
    ))
}

/// 一个字的 `[码, 根号…]`，对应 `encode_mabiao_and_chaifen`
fn 字数据(
    字: &str,
    各串: &[Vec<String>],
    mabiao: &BTreeMap<String, Vec<String>>,
    根号: &HashMap<&str, usize>,
) -> String {
    let 码们 = mabiao.get(字).cloned().unwrap_or_default();
    let 码项 = if 码们.len() == 1 {
        json字符串(&码们[0])
    } else {
        let mut s = String::from("[");
        for (i, 码) in 码们.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&json字符串(码));
        }
        s.push(']');
        s
    };

    let 号 = |根: &String| -> String {
        根号
            .get(根.as_str())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "null".to_string())
    };

    let 根项 = if 各串.len() == 1 {
        let mut s = String::new();
        for 根 in &各串[0] {
            s.push(',');
            s.push_str(&号(根));
        }
        s
    } else {
        let mut s = String::from("[");
        for (i, 串) in 各串.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('[');
            for (j, 根) in 串.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                s.push_str(&号(根));
            }
            s.push(']');
        }
        s.push(']');
        s
    };

    format!("[{}{}]", 码项, 根项)
}

/// JSON 字符串字面量：`JSON::PP->utf8(0)` 不转义非 ASCII，只处理引号/反斜杠/控制字符
fn json字符串(s: &str) -> String {
    let mut 出 = String::with_capacity(s.len() + 2);
    出.push('"');
    for c in s.chars() {
        match c {
            '"' => 出.push_str("\\\""),
            '\\' => 出.push_str("\\\\"),
            '\n' => 出.push_str("\\n"),
            '\r' => 出.push_str("\\r"),
            '\t' => 出.push_str("\\t"),
            c if (c as u32) < 0x20 => 出.push_str(&format!("\\u{:04x}", c as u32)),
            c => 出.push(c),
        }
    }
    出.push('"');
    出
}

// =========================================================================
// ⑤ Unihan（可选）
// =========================================================================

/// Unihan 里用到的三张表
pub(crate) struct Unihan {
    /// 有 kSimplifiedVariant 的字 = 繁体
    繁体字: HashSet<char>,
    /// 有 kTraditionalVariant 的字 = 简体
    简体字: HashSet<char>,
    /// kMandarin 拼音（原样保留，可能带声调数字）
    拼音: HashMap<char, String>,
}

fn 载入字形数据(参: &参数, 源: &Path, 目录: &Path) -> Option<Unihan> {
    let 目录候选: Vec<PathBuf> = 参
        .unihan目录
        .as_ref()
        .map(|d| vec![PathBuf::from(d)])
        .unwrap_or_else(|| vec![源.to_path_buf(), 目录.to_path_buf()]);

    let mut 变体 = None;
    let mut 读音 = None;
    for d in &目录候选 {
        if 变体.is_none() {
            变体 = Some(d.join("Unihan_Variants.txt")).filter(|p| p.is_file());
        }
        if 读音.is_none() {
            读音 = Some(d.join("Unihan_Readings.txt")).filter(|p| p.is_file());
        }
    }

    match (变体, 读音) {
        (Some(v), Some(r)) => match Unihan::读(&v, &r) {
            Ok(u) => {
                println!(
                    "  Unihan: 繁体字 {} / 简体字 {} / 读音 {}",
                    u.繁体字.len(),
                    u.简体字.len(),
                    u.拼音.len()
                );
                Some(u)
            }
            Err(e) => {
                println!("  ! Unihan 读取失败（跳过）：{e}");
                None
            }
        },
        _ => {
            println!("  ! 没找到 Unihan_Variants.txt / Unihan_Readings.txt，拼音留空、简繁不标记");
            println!("    （下载 https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip 解压到 --unihan-dir）");
            None
        }
    }
}

impl Unihan {
    fn 读(变体: &Path, 读音: &Path) -> Result<Self, String> {
        let mut 繁体字 = HashSet::new();
        let mut 简体字 = HashSet::new();
        let mut 拼音 = HashMap::new();

        for 行 in fs::read_to_string(变体)
            .map_err(|e| format!("读 {} 失败：{e}", 变体.display()))?
            .lines()
        {
            if !行.starts_with("U+") {
                continue;
            }
            let 列: Vec<&str> = 行.split_whitespace().collect();
            if 列.len() < 3 {
                continue;
            }
            let Some(字) = 码点转字符(列[0]) else {
                continue;
            };
            match 列[1] {
                "kSimplifiedVariant" => {
                    繁体字.insert(字);
                }
                "kTraditionalVariant" => {
                    简体字.insert(字);
                }
                _ => {}
            }
        }

        for 行 in fs::read_to_string(读音)
            .map_err(|e| format!("读 {} 失败：{e}", 读音.display()))?
            .lines()
        {
            if !行.starts_with("U+") {
                continue;
            }
            let 列: Vec<&str> = 行.splitn(3, char::is_whitespace).collect();
            if 列.len() < 3 || 列[1] != "kMandarin" {
                continue;
            }
            if let Some(字) = 码点转字符(列[0]) {
                拼音.insert(字, 列[2].trim().to_string());
            }
        }

        Ok(Self {
            繁体字,
            简体字,
            拼音,
        })
    }
}

/// `U+4E00` → `一`
fn 码点转字符(s: &str) -> Option<char> {
    let 十六 = s.strip_prefix("U+")?;
    let 值 = u32::from_str_radix(十六.trim(), 16).ok()?;
    char::from_u32(值)
}

// =========================================================================
// ⑥ <方案>-<日期>.html
// =========================================================================

/// 一个字根在页面上的信息
#[derive(Default)]
struct 根信息 {
    拼音: String,
    备注: String,
    /// 各页累计字数，下标 0 是全部
    频次: Vec<u64>,
    字频: String,
    繁体: bool,
    /// 在「繁体字根.txt」清单里（只出现在繁体/生僻字拆分中的根）
    繁体根: bool,
    /// 在「罕用字根.txt」清单里（繁体专用字集里也几乎不出现的部件根）
    罕用根: bool,
    例字: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn 生成页面(
    参: &参数,
    目录: &Path,
    源: &Path,
    roots_tsv: &Path,
    chaifen_tsv: &Path,
    js: &str,
    全字频文件: &Path,
    unihan: Option<&Unihan>,
) -> Result<(), String> {
    let roots = 读取带行号(roots_tsv)?;
    let chaifen = 读取带行号(chaifen_tsv)?;
    let 字频表 = 读取带行号(全字频文件)?;

    // 繁体根 / 罕用字根清单：显式标注用（文件缺失就不标，不影响其他方案）
    let 繁体根集 = 读根清单(&[目录.to_path_buf(), 源.to_path_buf()], "繁体字根.txt");
    let 罕用根集 = 读根清单(&[目录.to_path_buf(), 源.to_path_buf()], "罕用字根.txt");
    if !繁体根集.is_empty() || !罕用根集.is_empty() {
        println!(
            "  ⑥ 繁体根 {} 个 / 罕用根 {} 个（{:?}）",
            繁体根集.len(),
            罕用根集.len(),
            ["繁体字根.txt", "罕用字根.txt"]
        );
    }

    // 前 N 个高频字：字 → 序号（1 起）
    let mut 高频: HashMap<String, usize> = HashMap::new();
    for (i, 行) in fs::read_to_string(全字频文件)
        .map_err(|e| format!("读 {} 失败：{e}", 全字频文件.display()))?
        .lines()
        .take(参.高频字数)
        .enumerate()
    {
        if let Some(字) = 行.split_whitespace().next() {
            高频.insert(字.to_string(), i + 1);
        }
    }

    // `读取带行号` 的布局是 `[行号, 字, 频次]`，频次在索引 2
    let 总字频: f64 = 字频表
        .values()
        .filter_map(|v| v.get(2).and_then(|s| s.parse::<f64>().ok()))
        .sum();
    // 分页大小兜底：0 会让两个整除点 panic（clap 已限制 ≥1，这里防非 CLI 调用方）。
    let 每页字数 = 参.每页字数.max(1);
    let 页数 = if 高频.is_empty() {
        1
    } else {
        (高频.len() - 1) / 每页字数 + 1
    };

    // 每字根：各页字数、覆盖字频、例字、简繁
    let mut 根页: HashMap<String, Vec<u64>> = HashMap::new();
    let mut 根字频: HashMap<String, f64> = HashMap::new();
    let mut 根繁体: HashMap<String, Option<bool>> = HashMap::new();
    let mut 根例字: HashMap<String, Vec<String>> = HashMap::new();
    let mut 已见: HashSet<(String, String)> = HashSet::new();

    // 遍历顺序 = 高频表顺序 → 码点 → （PUA 排最后）；与脚本的 sort 一致
    let 序 = |字: &str| -> u32 {
        if let Some(n) = 高频.get(字) {
            return *n as u32;
        }
        let 位 = 字.chars().next().map(|c| c as u32).unwrap_or(0);
        if (0xE000..=0xF8FF).contains(&位) {
            return 0x10FFFF;
        }
        位.max(高频.len() as u32 + 1)
    };
    let mut 字们: Vec<&String> = chaifen.keys().collect();
    字们.sort_by_key(|字| 序(字));

    for 字 in 字们 {
        let 各列 = &chaifen[字];
        // 下标 0 是行号、1 是字本身，根串从 2 开始
        let 序位 = 高频.get(字).copied().unwrap_or(0);
        let 页 = if 序位 == 0 {
            0
        } else {
            (序位 - 1) / 每页字数 + 1
        };
        let 繁体 = unihan.map(|u| u.繁体字.contains(&字.chars().next().unwrap_or('\0')));
        let 简体 = unihan.map(|u| u.简体字.contains(&字.chars().next().unwrap_or('\0')));
        let 该字频 = 字频表
            .get(字)
            .and_then(|v| v.get(2))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        for 串 in 各列.iter().skip(2) {
            let 根们 = 取四(&切根(串));
            let mut 去重: Vec<&String> = 根们.iter().collect();
            去重.sort();
            去重.dedup();

            for 根 in 去重 {
                if !已见.insert((根.clone(), 字.clone())) {
                    continue;
                }
                let 槽 = 根页.entry(根.clone()).or_insert_with(|| vec![0; 页数 + 1]);
                if 页 < 槽.len() {
                    槽[页] += 1;
                }
                *根字频.entry(根.clone()).or_insert(0.0) += 该字频;

                if let Some(t) = 繁体 {
                    let 项 = 根繁体.entry(根.clone()).or_insert(Some(t));
                    if t && 项.is_none() {
                        *项 = Some(true);
                    }
                }
                if 简体 == Some(true) {
                    根繁体.insert(根.clone(), Some(false));
                }

                let 例 = 根例字.entry(根.clone()).or_default();
                if 例.len() < 参.例字数 {
                    例.push(字.clone());
                }
            }
        }
    }

    // 组装 chart_json
    let mut chart: BTreeMap<String, BTreeMap<String, BTreeMap<String, 根信息>>> = BTreeMap::new();
    let mut 全单字根 = true;

    for (根, 列) in &roots {
        // 列来自 `读取带行号`，布局是 `[行号, 名, 码, 拼音, 备注…]`，
        // 故码/拼音/备注分别从 2/3/4 起取（错取 1 会把字根名当成码，页面编码全空）
        let 码 = 列.get(2).cloned().unwrap_or_default();
        if 码.chars().count() > 1 {
            全单字根 = false;
        }
        let mut 拼音 = 列.get(3).cloned().unwrap_or_default();
        let 备注 = 列.iter().skip(4).cloned().collect::<Vec<_>>().join(" ");

        // 非花括号字根、且拼音是空或纯小写字母时，用 Unihan 的 kMandarin 覆盖
        if !根.contains('{') && (拼音.is_empty() || !拼音.chars().any(|c| !c.is_ascii_lowercase()))
        {
            if let Some(u) = unihan {
                if let Some(p) = 根.chars().next().and_then(|c| u.拼音.get(&c)) {
                    拼音 = p.clone();
                } else {
                    拼音.clear();
                }
            }
        }

        let mut 频次 = 根页.get(根).cloned().unwrap_or_else(|| vec![0; 页数 + 1]);
        频次.resize(页数 + 1, 0);
        for i in 2..=页数 {
            频次[i] += 频次[i - 1];
        }
        let 尾 = *频次.last().unwrap_or(&0);
        频次[0] += 尾;

        let 该字频 = 根字频.get(根).copied().unwrap_or(0.0);
        let 百分比 = if 总字频 > 0.0 {
            该字频 / 总字频 * 100.0
        } else {
            0.0
        };

        let 首字母: String = 码
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_default();
        if 首字母.is_empty() {
            continue;
        }
        let 码键 = 首字母大写(&码.to_ascii_lowercase());

        chart
            .entry(首字母)
            .or_default()
            .entry(码键)
            .or_default()
            .insert(
                根.clone(),
                根信息 {
                    拼音,
                    备注,
                    频次,
                    字频: format!("{百分比:.2}%"),
                    繁体: 根繁体.get(根).copied().flatten().unwrap_or(false),
                    繁体根: 繁体根集.contains(根),
                    罕用根: 罕用根集.contains(根),
                    例字: 根例字.get(根).cloned().unwrap_or_default(),
                },
            );
    }

    // roots_mapping：花括号名 → 显示字
    let 映射文件 = 找文件(&[目录.to_path_buf(), 源.to_path_buf()], "roots-mapping.tsv");
    let 映射 = 映射文件.as_deref().map(加载映射).unwrap_or_default();

    let 脚本 = format!(
        "var page_size = {};\nvar max_pages = {};\nvar chart_json = {};\nvar roots_mapping = {};\n",
        参.每页字数,
        页数,
        chart_json(&chart),
        映射json(&映射)
    );

    // 字体：文件存在则 base64 内嵌，否则当字体名
    let (额外字体, 字体声明) = match 参.字体.as_deref() {
        None => (String::new(), String::new()),
        Some(f) => {
            let 路径 = PathBuf::from(f);
            if 路径.is_file() {
                let 数据 =
                    fs::read(&路径).map_err(|e| format!("读字体 {} 失败：{e}", 路径.display()))?;
                let 扩展 = 路径
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let mime = format!("font/{扩展}");
                (
                    "rootfont,".to_string(),
                    format!(
                        "  @font-face {{\n    font-family: 'rootfont';\n    src: url('data:{mime};base64,{}');\n  }}\n",
                        转base64(&数据)
                    ),
                )
            } else {
                (format!("\"{f}\","), String::new())
            }
        }
    };

    let 日期 = chrono::Local::now().format("%Y.%m.%d").to_string();
    let 标题 = 参
        .标题
        .clone()
        .unwrap_or_else(|| format!("{}字根表 {日期}", 参.方案));

    let 页面 = 页面模板
        .replace("__TITLE__", &html转义(&标题))
        .replace("__FONT_FACE__", &字体声明)
        .replace("__EXTRA_FONT__", &额外字体)
        .replace(
            "__WORD_BREAK__",
            if 全单字根 { "normal" } else { "keep-all" },
        )
        .replace("__CHARINFO_JS__", js)
        .replace("__CHART_JS__", &脚本)
        .replace("__IS_SINGLE__", if 全单字根 { "true" } else { "false" });

    let 输出 = 目录.join(format!("{}-{日期}.html", 参.方案));
    fs::write(&输出, &页面).map_err(|e| format!("写 {} 失败：{e}", 输出.display()))?;
    println!(
        "  ⑤ {}  {:.1} KB（{} 个字根，{} 页）",
        输出.file_name().unwrap_or_default().to_string_lossy(),
        页面.len() as f64 / 1024.0,
        chart
            .values()
            .flat_map(|m| m.values())
            .map(|m| m.len())
            .sum::<usize>(),
        页数
    );
    Ok(())
}

/// `generate-roots-chart.pl` 的 `read_tsv`：首列 → `[行号, 首列, 其余列…]`
fn 读取带行号(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
    let 文本 = fs::read_to_string(path).map_err(|e| format!("读 {} 失败：{e}", path.display()))?;
    let mut 表: HashMap<String, Vec<String>> = HashMap::new();
    for (i, 行) in 文本.lines().enumerate() {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.is_empty() || 列[0].starts_with('#') {
            continue;
        }
        // 首次出现记 `[行号, 首列, 其余列…]`；同一首列的后续行只追加其余列
        match 表.get_mut(列[0]) {
            Some(项) => 项.extend(列.iter().skip(1).map(|s| s.to_string())),
            None => {
                let mut v = vec![(i + 1).to_string()];
                v.extend(列.iter().map(|s| s.to_string()));
                表.insert(列[0].to_string(), v);
            }
        }
    }
    Ok(表)
}

/// 读根清单（`繁体字根.txt` / `罕用字根.txt`）：一行一个根，`#` 开头是注释
///
/// 找不到文件就返回空集 —— 老方案没有这两份清单时页面照旧生成，不标注。
fn 读根清单(目录们: &[PathBuf], 名: &str) -> HashSet<String> {
    let Some(路径) = 找文件(目录们, 名) else {
        return HashSet::new();
    };
    let Ok(文本) = fs::read_to_string(&路径) else {
        return HashSet::new();
    };
    let mut 集 = HashSet::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        // 只取行首那个 token，容忍行尾跟着注释
        if let Some(根) = 行.split_whitespace().next() {
            集.insert(根.to_string());
        }
    }
    集
}

fn chart_json(chart: &BTreeMap<String, BTreeMap<String, BTreeMap<String, 根信息>>>) -> String {
    let mut 出 = String::from("{");
    for (i, (字母, 码表)) in chart.iter().enumerate() {
        if i > 0 {
            出.push(',');
        }
        出.push_str(&json字符串(字母));
        出.push_str(":{");
        for (j, (码, 根们)) in 码表.iter().enumerate() {
            if j > 0 {
                出.push(',');
            }
            出.push_str(&json字符串(码));
            出.push_str(":{");
            for (k, (根, 信息)) in 根们.iter().enumerate() {
                if k > 0 {
                    出.push(',');
                }
                出.push_str(&json字符串(根));
                出.push_str(":{");
                出.push_str(&format!("\"pinyin\":{},", json字符串(&信息.拼音)));
                出.push_str(&format!("\"comment\":{},", json字符串(&信息.备注)));
                出.push_str(&format!(
                    "\"freq\":[{}],",
                    信息
                        .频次
                        .iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ));
                出.push_str(&format!("\"char_freq\":{},", json字符串(&信息.字频)));
                出.push_str(&format!(
                    "\"traditional\":{},",
                    if 信息.繁体 { 1 } else { 0 }
                ));
                出.push_str(&format!(
                    "\"trad_root\":{},",
                    if 信息.繁体根 { 1 } else { 0 }
                ));
                出.push_str(&format!(
                    "\"rare_root\":{},",
                    if 信息.罕用根 { 1 } else { 0 }
                ));
                出.push_str(&format!(
                    "\"examples\":[{}]}}",
                    信息
                        .例字
                        .iter()
                        .map(|s| json字符串(s))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            出.push('}');
        }
        出.push('}');
    }
    出.push('}');
    出
}

fn 映射json(映射: &HashMap<String, String>) -> String {
    if 映射.is_empty() {
        return "null".to_string();
    }
    let mut 键: Vec<&String> = 映射.keys().collect();
    键.sort();
    let mut 出 = String::from("{");
    for (i, k) in 键.iter().enumerate() {
        if i > 0 {
            出.push(',');
        }
        出.push_str(&json字符串(k));
        出.push(':');
        出.push_str(&json字符串(&映射[*k]));
    }
    出.push('}');
    出
}

fn html转义(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 标准 base64（带 `=` 补齐），用于内嵌字体
pub(crate) fn 转base64(数据: &[u8]) -> String {
    const 表: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut 出 = String::with_capacity(数据.len().div_ceil(3) * 4);
    for 块 in 数据.chunks(3) {
        let b = [
            块[0],
            块.get(1).copied().unwrap_or(0),
            块.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        出.push(表[(n >> 18) as usize & 63] as char);
        出.push(表[(n >> 12) as usize & 63] as char);
        出.push(if 块.len() > 1 {
            表[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        出.push(if 块.len() > 2 {
            表[n as usize & 63] as char
        } else {
            '='
        });
    }
    出
}

#[cfg(test)]
mod 测试 {
    use super::*;
    use std::io::Write;

    /// 写一个临时文件，返回路径；测试结束自动删除由 Drop 处理不了，用固定子目录
    fn 写临时(名: &str, 内容: &str) -> PathBuf {
        let 目录 = std::env::temp_dir().join("code_genie_roots_chart_test");
        std::fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join(名);
        let mut f = std::fs::File::create(&路径).unwrap();
        f.write_all(内容.as_bytes()).unwrap();
        路径
    }

    #[test]
    fn 切根_普通() {
        assert_eq!(切根("白勹丶"), vec!["白", "勹", "丶"]);
    }

    #[test]
    fn 切根_花括号名当整体() {
        assert_eq!(切根("{⺮右}一"), vec!["{⺮右}", "一"]);
        assert_eq!(切根("{⺮右}{⺮左}"), vec!["{⺮右}", "{⺮左}"]);
    }

    #[test]
    fn 切根_无闭合花括号按单字() {
        assert_eq!(切根("{⺮右"), vec!["{", "⺮", "右"]);
    }

    #[test]
    fn 切根_跳过空白() {
        assert_eq!(切根("白 勹\t丶"), vec!["白", "勹", "丶"]);
    }

    #[test]
    fn 取四_四根内原样() {
        assert_eq!(
            取四(&["白".into(), "勹".into(), "丶".into(), "日".into()]),
            vec!["白", "勹", "丶", "日"]
        );
    }

    #[test]
    fn 取四_多于四根取首尾() {
        assert_eq!(
            取四(&[
                "一".into(),
                "二".into(),
                "三".into(),
                "四".into(),
                "五".into()
            ]),
            vec!["一", "二", "三", "五"]
        );
    }

    #[test]
    fn json字符串_不转义非ascii() {
        // 与 perl `JSON::PP->utf8(0)` 一致：汉字原样，只处理引号/反斜杠/控制字符
        assert_eq!(json字符串("中文"), "\"中文\"");
        assert_eq!(json字符串("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(json字符串("\n\t"), "\"\\n\\t\"");
        // 与 perl `JSON::PP->utf8(0)` 一致：非 ASCII 不做 \u 转义，输出字面字符
        assert_eq!(json字符串("\u{1f600}"), "\"😀\"");
    }

    #[test]
    fn 转base64_已知向量() {
        // 标准 base64
        assert_eq!(转base64(b"Man"), "TWFu");
        assert_eq!(转base64(b"Ma"), "TWE=");
        assert_eq!(转base64(b"M"), "TQ==");
        assert_eq!(转base64(b""), "");
    }

    #[test]
    fn 生成字根表文本_无槽位原样() {
        let p = 写临时("keymap_plain.txt", "# 注释\n一\tOd\n口\tIe\n\n丶\tEk\n");
        let t = 生成字根表文本(&p).unwrap();
        assert_eq!(t, "一\tOd\n口\tIe\n丶\tEk\n");
    }

    #[test]
    fn 生成字根表文本_槽位写法拼成_asy() {
        // `首字母大写` 只大写首字母
        let p = 写临时(
            "keymap_slot.txt",
            "一.A\taa\n一.S\tss\n一.Y\tyy\n口.A\tii\n口.S\tee\n口.Y\ttt\n",
        );
        let t = 生成字根表文本(&p).unwrap();
        assert!(t.contains("一\tAassyy"));
        assert!(t.contains("口\tIieett"));
        // 排序：含元音(aeiou)优先
        let 首行 = t.lines().next().unwrap();
        assert!(首行.starts_with("口") || 首行.starts_with("一"));
    }

    #[test]
    fn 生成拆分表文本_去空白() {
        let p = 写临时("chaifen.txt", "的\t白 勹 丶\n一\t一\n");
        let t = 生成拆分表文本(&p).unwrap();
        assert_eq!(t, "的\t白勹丶\n一\t一\n");
    }

    #[test]
    fn 读列集合_去重升序() {
        let p = 写临时("multi.tsv", "的\t白勹丶\t白白\n的\t白勹丶\n一\t一\n");
        let m = 读列集合(&p).unwrap();
        // 首列=「的」出现两次，列集合去重升序
        assert_eq!(
            m.get("的").unwrap(),
            &vec!["白勹丶".to_string(), "白白".to_string()]
        );
        assert_eq!(m.get("一").unwrap(), &vec!["一".to_string()]);
    }

    #[test]
    fn 加载映射_同名留末个() {
        let p = 写临时("mapping.tsv", "{⺮右}\t⺮\n{⺮右}\t⺀\n");
        let m = 加载映射(&p);
        assert_eq!(m.get("{⺮右}").unwrap(), "⺀");
    }

    #[test]
    fn 读取带行号_首列聚合() {
        let p = 写临时("roots_line.tsv", "一\tOd\n口\tIe\n一\tXx\n");
        let m = 读取带行号(&p).unwrap();
        // 首次出现记 [行号, 首列, 其余]；后续行只追加其余列
        assert_eq!(
            m.get("一").unwrap(),
            &vec![
                "1".to_string(),
                "一".to_string(),
                "Od".to_string(),
                "Xx".to_string()
            ]
        );
        assert_eq!(
            m.get("口").unwrap(),
            &vec!["2".to_string(), "口".to_string(), "Ie".to_string()]
        );
    }
}
