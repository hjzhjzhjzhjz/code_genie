// =========================================================================
// 📮 整句准确率
// =========================================================================
//
// 四码定长方案的「整句」有两种口径，两个都算：
//
//   A 逐单元（真实操作）
//     每个字打满自己的码 → 空格上屏第一个候选。所以逐字看「该码的第一候选
//     是不是这个字」，一句里全对才算整句正确。这是用户真实感受到的准确率。
//
//   B 连续解码（把整句的码首尾相连，让解码器自己切）
//     用词频做 Viterbi，衡量词组能不能帮忙、以及三码字会不会把对齐带歪。
//
// 两者都不依赖 librime：词典就是 Rime 的 dict.yaml，权重就是字频/词频，
// 结果可复现，也不受外部库版本影响。
// =========================================================================

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct 参数 {
    pub 全码词典: PathBuf,
    pub 词组词典: Option<PathBuf>,
    pub 语料: PathBuf,
    pub 解码器: Option<PathBuf>,
    pub 共享目录: PathBuf,
    pub 用户目录: PathBuf,
    pub 方案: String,
    pub 上限: usize,
    pub 最长: usize,
    pub 导出: Option<PathBuf>,
    pub 结果: Option<PathBuf>,
    pub 详细: bool,
}

/// 一条词典条目
#[derive(Clone)]
struct 条目 {
    文本: String,
    权重: u64,
}

fn 是汉字(c: char) -> bool {
    let u = c as u32;
    (0x4E00..=0x9FFF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x20000..=0x2A6DF).contains(&u)
}

/// 读 Rime dict.yaml：跳过头部，读「文本 码 权重」
fn 读词典(路径: &Path) -> Vec<(String, String, u64)> {
    let Ok(文本) = std::fs::read_to_string(路径) else {
        eprintln!("⚠️ 读不了词典 {}", 路径.display());
        return Vec::new();
    };
    let mut 出 = Vec::new();
    let mut 开始 = false;
    for 行 in 文本.lines() {
        if !开始 {
            if 行.trim() == "..." {
                开始 = true;
            }
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 文本 = 列[0].trim();
        let 码 = 列[1].trim();
        if 文本.is_empty() || 码.is_empty() {
            continue;
        }
        let 权重 = 列.get(2).and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(1);
        出.push((文本.to_string(), 码.to_string(), 权重));
    }
    出
}

fn 切句(文本: &str, 字码: &HashMap<char, String>, 最长: usize) -> Vec<String> {
    let mut 句们 = Vec::new();
    let mut 当前 = String::new();
    for c in 文本.chars() {
        if 是汉字(c) {
            当前.push(c);
            continue;
        }
        let n = 当前.chars().count();
        if (2..=最长).contains(&n) && 当前.chars().all(|x| 字码.contains_key(&x)) {
            句们.push(当前.clone());
        }
        当前.clear();
    }
    let n = 当前.chars().count();
    if (2..=最长).contains(&n) && 当前.chars().all(|x| 字码.contains_key(&x)) {
        句们.push(当前);
    }
    句们
}

fn 编辑距离(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut 上行: Vec<usize> = (0..=m).collect();
    let mut 本行 = vec![0usize; m + 1];
    for i in 1..=n {
        本行[0] = i;
        for j in 1..=m {
            let 代价 = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            本行[j] = (上行[j] + 1).min(本行[j - 1] + 1).min(上行[j - 1] + 代价);
        }
        std::mem::swap(&mut 上行, &mut 本行);
    }
    上行[m]
}

/// Viterbi：在码流上找一条总权重最大的切分
fn 连续解码(
    码流: &str,
    码表: &HashMap<String, Vec<条目>>,
) -> (String, usize) {
    let 字节 = 码流.as_bytes();
    let n = 字节.len();
    let 无穷 = f64::NEG_INFINITY;
    let mut 分数 = vec![无穷; n + 1];
    let mut 前驱 = vec![usize::MAX; n + 1];
    let mut 用的码长 = vec![0usize; n + 1];
    分数[0] = 0.0;

    for i in 0..n {
        if 分数[i] == 无穷 {
            continue;
        }
        // 码长 1~4（单根字可能只有 3 码）
        for 长 in 1..=4usize {
            if i + 长 > n {
                break;
            }
            let 片 = &码流[i..i + 长];
            let Some(候选们) = 码表.get(片) else {
                continue;
            };
            let 最重 = 候选们
                .iter()
                .map(|e| e.权重 as f64)
                .fold(0.0_f64, f64::max);
            if 最重 <= 0.0 {
                continue;
            }
            let 新分 = 分数[i] + 最重.ln();
            let 更优 = 新分 > 分数[i + 长] + 1e-9
                || ((新分 - 分数[i + 长]).abs() <= 1e-9 && 长 > 用的码长[i + 长]);
            if 更优 {
                分数[i + 长] = 新分;
                前驱[i + 长] = i;
                用的码长[i + 长] = 长;
            }
        }
    }

    if 分数[n] == 无穷 {
        return (String::new(), 0);
    }

    let mut 片段 = Vec::new();
    let mut 位置 = n;
    while 位置 > 0 {
        let 起 = 前驱[位置];
        if 起 == usize::MAX {
            return (String::new(), 0);
        }
        片段.push(&码流[起..位置]);
        位置 = 起;
    }
    片段.reverse();

    let mut 文本 = String::new();
    let mut 单元 = 0usize;
    for 片 in 片段 {
        单元 += 1;
        if let Some(候选们) = 码表.get(片) {
            let 最重 = 候选们
                .iter()
                .max_by_key(|e| e.权重)
                .map(|e| e.文本.clone())
                .unwrap_or_default();
            文本.push_str(&最重);
        }
    }
    (文本, 单元)
}

#[allow(dead_code)]
fn 起解码器(
    程序: &Path,
    共享: &Path,
    用户: &Path,
    方案: &str,
    输入: &[String],
    详细: bool,
) -> std::io::Result<Vec<String>> {
    let mut 子 = Command::new(程序)
        .arg("--shared")
        .arg(共享)
        .arg("--user")
        .arg(用户)
        .arg("--schema")
        .arg(方案)
        .args(if 详细 { vec!["--verbose"] } else { Vec::new() })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut 进 = 子.stdin.take().expect("stdin");
    let 全部 = 输入.join("\n") + "\n";
    let 写线程 = std::thread::spawn(move || {
        let _ = 进.write_all(全部.as_bytes());
        let _ = 进.flush();
    });

    let 出 = 子.stdout.take().expect("stdout");
    let 期望条数 = 输入.len();
    let mut 结果 = Vec::with_capacity(期望条数);
    for 行 in BufReader::new(出).lines() {
        match 行 {
            Ok(l) => 结果.push(l.trim_end_matches('\r').to_string()),
            Err(_) => break,
        }
        if 结果.len() >= 期望条数 {
            break;
        }
    }
    let _ = 写线程.join();
    let _ = 子.wait();
    Ok(结果)
}

pub fn run(参: &参数) {
    // ---- 词典 ----
    let mut 字码: HashMap<char, String> = HashMap::new();
    let mut 码表: HashMap<String, Vec<条目>> = HashMap::new();

    for (文本, 码, 权重) in 读词典(&参.全码词典) {
        let mut it = 文本.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            字码.insert(c, 码.clone());
        }
        码表.entry(码).or_default().push(条目 { 文本, 权重 });
    }
    if let Some(词径) = &参.词组词典 {
        for (文本, 码, 权重) in 读词典(词径) {
            码表.entry(码).or_default().push(条目 { 文本, 权重 });
        }
    }
    if 字码.is_empty() {
        eprintln!("❌ 全码词典为空，退出");
        return;
    }
    for v in 码表.values_mut() {
        v.sort_by(|a, b| b.权重.cmp(&a.权重));
    }

    // ---- 语料 ----
    let 语料 = std::fs::read_to_string(&参.语料).unwrap_or_default();
    let mut 句们 = 切句(&语料, &字码, 参.最长);
    if 参.上限 > 0 && 句们.len() > 参.上限 {
        句们.truncate(参.上限);
    }
    if 句们.is_empty() {
        eprintln!("❌ 语料里没有可用句子");
        return;
    }

    // ---- A 逐单元 ----
    let mut 单元总数 = 0usize;
    let mut 首候选对 = 0usize;
    let mut 句全对 = 0usize;
    let mut 桶单元: Vec<(usize, usize, usize)> = vec![(0, 0, 0); 参.最长 + 1];
    let mut 错例: Vec<(String, String, String)> = Vec::new(); // 原字 / 码 / 首候选

    // ---- B 连续解码 ----
    let mut 连对 = 0usize;
    let mut 连距离 = 0usize;
    let mut 连切分错 = 0usize;
    let mut 击键 = 0usize;

    for 句 in &句们 {
        let mut 本句全对 = true;
        let mut 码流 = String::new();
        for c in 句.chars() {
            let 码 = &字码[&c];
            码流.push_str(码);
            击键 += 码.chars().count();
            单元总数 += 1;

            let 首 = 码表
                .get(码.as_str())
                .and_then(|v| v.first())
                .map(|e| e.文本.as_str())
                .unwrap_or("");
            let 对 = 首 == c.to_string().as_str();
            if 对 {
                首候选对 += 1;
            } else {
                本句全对 = false;
                if 错例.len() < 8 {
                    错例.push((c.to_string(), 码.clone(), 首.to_string()));
                }
            }
        }
        if 本句全对 {
            句全对 += 1;
        }

        let n = 句.chars().count();
        let r = &mut 桶单元[n.min(参.最长)];
        r.0 += 1;
        r.1 += n;
        if 本句全对 {
            r.2 += 1;
        }

        // B
        let (解出, _) = match &参.解码器 {
            Some(_) => (String::new(), 0),
            None => 连续解码(&码流, &码表),
        };
        let a: Vec<char> = 句.chars().collect();
        let b: Vec<char> = 解出.chars().collect();
        let d = 编辑距离(&a, &b);
        连距离 += d;
        if d == 0 {
            连对 += 1;
        } else if a.len() != b.len() {
            连切分错 += 1;
        }
    }

    let 字准确 = 首候选对 as f64 / 单元总数 as f64 * 100.0;
    let 句准确 = 句全对 as f64 / 句们.len() as f64 * 100.0;
    let 连准确 = 连对 as f64 / 句们.len() as f64 * 100.0;

    if let Some(目录) = &参.导出 {
        std::fs::create_dir_all(目录).ok();
        let _ = std::fs::write(目录.join("原文.txt"), 句们.join("\n") + "\n");
        println!("排查用原文已写入 {}", 目录.display());
    }

    let mut md = String::from("## 整句准确率\n\n");
    md.push_str(&format!(
        "语料 {} 句 / {} 字　词典 {} 条　平均击键 {:.3}/字\n\n",
        句们.len(),
        单元总数,
        码表.values().map(|v| v.len()).sum::<usize>(),
        击键 as f64 / 单元总数 as f64
    ));
    md.push_str("**A 逐单元**（每字打满自己的码、空格取第一个候选，即真实操作）\n\n");
    md.push_str("| 指标 | 值 |\n|---|---:|\n");
    md.push_str(&format!(
        "| 首候选准确率（字级） | **{:.2}%**（{} / {}） |\n",
        字准确, 首候选对, 单元总数
    ));
    md.push_str(&format!(
        "| 整句准确率（一句全对） | **{:.2}%**（{} / {}） |\n",
        句准确, 句全对, 句们.len()
    ));

    md.push_str("\n**B 连续解码**（整句码流首尾相连，让解码器按词频自己切）\n\n");
    md.push_str("| 指标 | 值 |\n|---|---:|\n");
    md.push_str(&format!(
        "| 整句准确率 | **{:.2}%**（{} / {}） |\n",
        连准确, 连对, 句们.len()
    ));
    md.push_str(&format!(
        "| 字准确率 | **{:.2}%**（编辑距离 {} / {} 字） |\n",
        (1.0 - 连距离 as f64 / 单元总数 as f64) * 100.0,
        连距离,
        单元总数
    ));
    md.push_str(&format!("| 切分错（长度不同） | {} 句 |\n", 连切分错));

    md.push_str("\n### 按句长（A 口径）\n\n| 句长 | 句数 | 整句准确率 |\n|---:|---:|---:|\n");
    for (n, (句数, _, 全)) in 桶单元.iter().enumerate() {
        if *句数 == 0 || n == 0 {
            continue;
        }
        md.push_str(&format!(
            "| {} | {} | {:.2}% |\n",
            n,
            *句数,
            *全 as f64 / *句数 as f64 * 100.0
        ));
    }

    if !错例.is_empty() {
        md.push_str("\n### 首候选错的字（前 8）\n\n| 应是 | 码 | 首候选 |\n|---|---|---|\n");
        for (原, 码, 首) in &错例 {
            md.push_str(&format!("| {} | `{}` | {} |\n", 原, 码, 首));
        }
    }

    println!("{}", md);
    if let Some(路径) = &参.结果 {
        match std::fs::write(路径, &md) {
            Ok(_) => println!("结果已写入 {}", 路径.display()),
            Err(e) => eprintln!("⚠️ 写不了结果: {}", e),
        }
    }
}
