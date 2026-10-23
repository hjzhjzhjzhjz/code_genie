// =========================================================================
// 📮 整句准确率
// =========================================================================
//
// 流程与 hanzi-chai/sentence 一致：
//   语料句子 → 每字取全码 → 串成一条按键流 → librime 解码 → 与原文比对
//
// 解码器是外部进程（tools/rime_eval.cc 编译出来的），一条输入一行输出，
// 这样 librime 版本、方案内容都能在 CI 里换，Rust 这边不绑死。
//
// 指标：
//   整句准确率  编辑距离为 0 的句子占比
//   字准确率    1 − Σ编辑距离 / Σ字数
//   选字错      长度一致但字不同（重码选错）
//   切分错      长度不同（词组/简码把码流切歪了）
// =========================================================================

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct 参数 {
    pub 码表: PathBuf,
    pub 语料: PathBuf,
    pub 解码器: PathBuf,
    pub 共享目录: PathBuf,
    pub 用户目录: PathBuf,
    pub 方案: String,
    pub 上限: usize,
    pub 最长: usize,
    pub 导出: Option<PathBuf>,
    pub 结果: Option<PathBuf>,
}

fn 是汉字(c: char) -> bool {
    let u = c as u32;
    (0x4E00..=0x9FFF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x20000..=0x2A6DF).contains(&u)
}

fn 读码表(路径: &Path) -> HashMap<char, String> {
    let mut 表 = HashMap::new();
    let Ok(文本) = std::fs::read_to_string(路径) else {
        eprintln!("⚠️ 读不了码表 {}", 路径.display());
        return 表;
    };
    for 行 in 文本.lines() {
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        let 字 = 列[0].trim();
        let 码 = 列[1].trim();
        let mut it = 字.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            if !码.is_empty() && 码.chars().all(|x| x.is_ascii_lowercase()) {
                let e = 表.entry(c).or_insert_with(String::new);
                if 码.chars().count() > e.chars().count() {
                    *e = 码.to_string();
                }
            }
        }
    }
    表
}

fn 切句(文本: &str, 码表: &HashMap<char, String>, 最长: usize) -> Vec<String> {
    let mut 句们 = Vec::new();
    let mut 当前 = String::new();
    let mut 收 = |buf: &mut String, 出: &mut Vec<String>| {
        let n = buf.chars().count();
        if (2..=最长).contains(&n) && buf.chars().all(|c| 码表.contains_key(&c)) {
            出.push(buf.clone());
        }
        buf.clear();
    };
    for c in 文本.chars() {
        if 是汉字(c) {
            当前.push(c);
        } else {
            收(&mut 当前, &mut 句们);
        }
    }
    收(&mut 当前, &mut 句们);
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

pub fn run(参: &参数) {
    let 码表 = 读码表(&参.码表);
    if 码表.is_empty() {
        eprintln!("❌ 码表为空，退出");
        return;
    }

    let 语料 = std::fs::read_to_string(&参.语料).unwrap_or_default();
    let mut 句们 = 切句(&语料, &码表, 参.最长);
    if 参.上限 > 0 && 句们.len() > 参.上限 {
        句们.truncate(参.上限);
    }
    if 句们.is_empty() {
        eprintln!("❌ 语料里没有可用句子");
        return;
    }

    // 编码
    let mut 按键流们: Vec<String> = Vec::with_capacity(句们.len());
    let mut 击键数 = 0usize;
    for 句 in &句们 {
        let mut 行 = String::new();
        for c in 句.chars() {
            let 码 = &码表[&c];
            行.push_str(码);
            击键数 += 码.chars().count();
        }
        行.push(' ');
        按键流们.push(行);
    }
    let 总字数: usize = 句们.iter().map(|s| s.chars().count()).sum();

    println!(
        "语料 {} 句，{} 字，平均击键 {:.3}/字",
        句们.len(),
        总字数,
        击键数 as f64 / 总字数 as f64
    );

    // 跑解码器
    let 开始 = std::time::Instant::now();
    let 解码 = match 起解码器(&参.解码器, &参.共享目录, &参.用户目录, &参.方案, &按键流们) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("❌ 解码失败: {}", e);
            return;
        }
    };
    let 耗时 = 开始.elapsed();

    if 解码.len() < 句们.len() {
        eprintln!(
            "⚠️ 只拿到 {} 条解码结果（应有 {} 条），后面按空处理",
            解码.len(),
            句们.len()
        );
    }

    if let Some(目录) = &参.导出 {
        std::fs::create_dir_all(目录).ok();
        let _ = std::fs::write(
            目录.join("按键流.txt"),
            按键流们.join("\n") + "\n",
        );
        let _ = std::fs::write(目录.join("原文.txt"), 句们.join("\n") + "\n");
        let _ = std::fs::write(目录.join("解码.txt"), 解码.join("\n") + "\n");
        println!("排查用中间文件已写入 {}", 目录.display());
    }

    // 打分
    let mut 全对 = 0usize;
    let mut 距离和 = 0usize;
    let mut 选字错 = 0usize;
    let mut 切分错 = 0usize;
    let mut 桶: Vec<(usize, usize, usize, usize)> = vec![(0, 0, 0, 0); 参.最长 + 1];
    // (句数, 字数, 全对句数, 距离和)

    let mut 例子: Vec<(String, String)> = Vec::new();
    for (i, 期望) in 句们.iter().enumerate() {
        let 实 = 解码.get(i).map(|s| s.as_str()).unwrap_or("");
        let a: Vec<char> = 期望.chars().collect();
        let b: Vec<char> = 实.chars().collect();
        let d = 编辑距离(&a, &b);
        let n = a.len();

        let r = &mut 桶[n.min(参.最长)];
        r.0 += 1;
        r.1 += n;
        r.3 += d;

        if d == 0 {
            全对 += 1;
            桶[n.min(参.最长)].2 += 1;
        } else if a.len() == b.len() {
            选字错 += 1;
        } else {
            切分错 += 1;
            例子.push((期望.clone(), 实.to_string()));
        }
        距离和 += d;
    }
    例子.truncate(6);

    let 整句率 = 全对 as f64 / 句们.len() as f64 * 100.0;
    let 字准确率 = (1.0 - 距离和 as f64 / 总字数 as f64) * 100.0;

    let mut md = String::from("## 整句准确率\n\n");
    md.push_str(&format!(
        "语料 {} 句 / {} 字　解码方案 `{}`　耗时 {:.1}s\n\n",
        句们.len(),
        总字数,
        参.方案,
        耗时.as_secs_f64()
    ));
    md.push_str("| 指标 | 值 |\n|---|---:|\n");
    md.push_str(&format!("| 整句全对 | {} / {} = **{:.2}%** |\n", 全对, 句们.len(), 整句率));
    md.push_str(&format!("| 字准确率 | **{:.2}%**（编辑距离 {}） |\n", 字准确率, 距离和));
    md.push_str(&format!("| 选字错（长度一致） | {} 句 |\n", 选字错));
    md.push_str(&format!("| 切分错（长度不同） | {} 句 |\n", 切分错));
    md.push_str(&format!(
        "| 平均击键 | {:.3}/字 |\n",
        击键数 as f64 / 总字数 as f64
    ));

    md.push_str("\n### 按句长\n\n| 句长 | 句数 | 字准确率 | 整句全对 |\n|---:|---:|---:|---:|\n");
    for (n, (句数, 字数, 全, 距)) in 桶.iter().enumerate() {
        if *句数 == 0 || n == 0 {
            continue;
        }
        let 率 = (1.0 - *距 as f64 / *字数 as f64) * 100.0;
        md.push_str(&format!(
            "| {} | {} | {:.2}% | {:.2}% |\n",
            n,
            *句数,
            率,
            *全 as f64 / *句数 as f64 * 100.0
        ));
    }

    if !例子.is_empty() {
        md.push_str("\n### 切分错例\n\n| 原文 | 解出 |\n|---|---|\n");
        for (原, 出) in 例子.iter().take(6) {
            md.push_str(&format!(
                "| {} | {} |\n",
                原,
                if 出.is_empty() { "（空）" } else { 出 }
            ));
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

fn 起解码器(
    程序: &Path,
    共享: &Path,
    用户: &Path,
    方案: &str,
    输入: &[String],
) -> std::io::Result<Vec<String>> {
    let mut 子 = Command::new(程序)
        .arg("--shared")
        .arg(共享)
        .arg("--user")
        .arg(用户)
        .arg("--schema")
        .arg(方案)
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
