// =========================================================================
// 📊 宇浩测评封装
// =========================================================================
//
// 调 yuhao-assess 的 Node CLI（`pnpm cli`），把 JSON 结果解析成对比表。
// 方案之间的强弱只看这里输出的宇浩指标，不看 CodeGenie 内部得分。
//
// 除宇浩自带的速度当量、重码、选重、码长之外，另补三项它没折算进排序的：
//   · 三级简码增益（一級/二級/全部 速度当量差）
//   · 指位负荷与行分布（读「鍵位熱力」）
//   · 同指连击率与换手率（码表 + 词频现算）
// =========================================================================

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::handfeel;

/// 一份码表的测评指标
struct 指标 {
    名: String,
    速度全码: f64,
    速度简码: f64,
    速度一级: Option<f64>,
    速度二级: Option<f64>,
    选重全码: f64,
    选重简码: f64,
    码长500: f64,
    通用全码: (u64, u64),
    通用简码: (u64, u64),
    常用全码: (u64, u64),
    常用简码: (u64, u64),
    指位: Option<handfeel::指位分布>,
    连击: Option<handfeel::连击>,
    选重: Option<handfeel::选重统计>,
}

impl 指标 {
    /// 三级简码相对二级的提升（%）
    fn 三级增益(&self) -> Option<f64> {
        let 二 = self.速度二级?;
        if 二 <= 0.0 {
            return None;
        }
        Some((self.速度简码 - 二) / 二 * 100.0)
    }

    /// 一级简码相对全码的提升（%）
    fn 一级增益(&self) -> Option<f64> {
        let 一 = self.速度一级?;
        if self.速度全码 <= 0.0 {
            return None;
        }
        Some((一 - self.速度全码) / self.速度全码 * 100.0)
    }

    /// 二级简码相对一级的提升（%）
    fn 二级增益(&self) -> Option<f64> {
        let (一, 二) = (self.速度一级?, self.速度二级?);
        if 一 <= 0.0 {
            return None;
        }
        Some((二 - 一) / 一 * 100.0)
    }
}

fn 取数(v: &Value, 路径: &[&str]) -> Option<f64> {
    let mut cur = v;
    for 段 in 路径 {
        cur = cur.get(*段)?;
    }
    cur.as_f64()
}

fn 取整(v: &Value, 路径: &[&str]) -> u64 {
    取数(v, 路径).unwrap_or(0.0) as u64
}

/// 从 yuhao 的完整 JSON 里抽出指标
fn 解析(
    名: &str,
    结果: &Value,
    指位: Option<handfeel::指位分布>,
    连击: Option<handfeel::连击>,
    选重: Option<handfeel::选重统计>,
) -> Option<指标> {
    let r = 结果.get("測評結果")?;
    let 码长 = r
        .get("簡碼效率分析")?
        .get("北語簡體字頻下之簡碼效率")?
        .get("N值結果")?
        .as_array()?
        .iter()
        .find(|x| x.get("最有效率的簡碼個數").and_then(|n| n.as_u64()) == Some(500))
        .and_then(|x| x.get("字頻加權碼長"))
        .and_then(|n| n.as_f64())?;

    let 速度 = |k: &str| 取数(r, &["速度當量分析", k]);

    Some(指标 {
        名: 名.to_string(),
        速度全码: 速度("北語簡體字頻全碼速度當量")?,
        速度简码: 速度("北語簡體字頻全部簡碼速度當量")?,
        速度一级: 速度("北語簡體字頻一級簡碼速度當量"),
        速度二级: 速度("北語簡體字頻二級簡碼速度當量"),
        选重全码: 取数(r, &["動態選重分析", "北語簡體動態選重率", "全碼"])?,
        选重简码: 取数(r, &["動態選重分析", "北語簡體動態選重率", "簡碼"])?,
        码长500: 码长,
        通用全码: (
            取整(r, &["靜態重碼分析", "通用規範", "全碼重碼字數"]),
            取整(r, &["靜態重碼分析", "通用規範", "全碼重碼組數"]),
        ),
        通用简码: (
            取整(r, &["靜態重碼分析", "通用規範", "簡碼重碼字數"]),
            取整(r, &["靜態重碼分析", "通用規範", "簡碼重碼組數"]),
        ),
        常用全码: (
            取整(r, &["靜態重碼分析", "常用國字", "全碼重碼字數"]),
            取整(r, &["靜態重碼分析", "常用國字", "全碼重碼組數"]),
        ),
        常用简码: (
            取整(r, &["靜態重碼分析", "常用國字", "簡碼重碼字數"]),
            取整(r, &["靜態重碼分析", "常用國字", "簡碼重碼組數"]),
        ),
        指位,
        连击,
        选重,
    })
}

/// 找可用的 pnpm 命令（Windows 上是 pnpm.cmd）
fn pnpm() -> String {
    for 候选 in ["pnpm", "pnpm.cmd"] {
        if Command::new(候选)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return 候选.to_string();
        }
    }
    String::from("pnpm")
}

/// 对一张码表跑测评，返回 (结果 JSON, 输出文件路径)
fn 测评一张(
    码表: &Path,
    名: &str,
    方案: &Path,
    工作目录: &Path,
) -> Result<(Value, PathBuf), String> {
    // CLI 在 yuhao 目录里跑，必须给绝对路径
    let 码表 = &std::fs::canonicalize(码表).unwrap_or_else(|_| 码表.to_path_buf());

    let 子目录 = 工作目录.join("assess-tmp");
    std::fs::create_dir_all(&子目录).map_err(|e| format!("建不了临时目录: {}", e))?;
    let 安全名 = 名.replace(['/', '\\'], "_");
    let 相对 = format!("assess-tmp/{}.json", 安全名);
    let 绝对 = 工作目录.join(&相对);
    let _ = std::fs::remove_file(&绝对);

    let 输出 = Command::new(pnpm())
        .current_dir(工作目录)
        .arg("cli")
        .arg(码表)
        .arg("--scheme")
        .arg(方案)
        .arg("--format")
        .arg("json")
        .arg("--output")
        .arg(&相对)
        .output()
        .map_err(|e| format!("无法执行 pnpm: {}", e))?;

    let 错误尾 = || -> String {
        String::from_utf8_lossy(&输出.stderr)
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .join(" / ")
    };

    if !输出.status.success() {
        return Err(format!("测评失败: {} ({})", 码表.display(), 错误尾()));
    }

    let 文本 = std::fs::read_to_string(&绝对)
        .map_err(|e| format!("读不到 {}: {} / {}", 绝对.display(), e, 错误尾()))?;
    let 值 = serde_json::from_str(&文本).map_err(|e| format!("解析测评结果失败: {}", e))?;
    Ok((值, 绝对))
}

fn 百分比(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:.2}%", v),
        None => "—".to_string(),
    }
}

fn 数量(x: Option<u64>) -> String {
    match x {
        Some(v) => v.to_string(),
        None => "—".to_string(),
    }
}

fn 带符号(x: Option<f64>) -> String {
    match x {
        Some(v) => format!("{:+.2}%", v),
        None => "—".to_string(),
    }
}

fn 打印(表: &[指标]) {
    println!(
        "{:<22}{:>9}{:>9}{:>11}{:>11}{:>9}{:>10}{:>10}{:>9}{:>9}{:>10}{:>10}",
        "方案", "速度全码", "速度简码", "动态选重全", "动态选重简", "码长N500", "通用全码", "通用简码", "常用全码", "常用简码", "全码选重数", "简码选重数"
    );
    println!("{}", "-".repeat(129));
    for m in 表 {
        println!(
            "{:<22}{:>9.4}{:>9.4}{:>11.6}{:>11.6}{:>9.4}{:>5}/{:<4}{:>5}/{:<4}{:>4}/{:<4}{:>4}/{:<4}{:>10}{:>10}",
            m.名,
            m.速度全码,
            m.速度简码,
            m.选重全码,
            m.选重简码,
            m.码长500,
            m.通用全码.0,
            m.通用全码.1,
            m.通用简码.0,
            m.通用简码.1,
            m.常用全码.0,
            m.常用全码.1,
            m.常用简码.0,
            m.常用简码.1,
            数量(m.选重.as_ref().map(|x| x.全码)),
            数量(m.选重.as_ref().map(|x| x.简码))
        );
    }

    let 有心跳 = 表
        .iter()
        .any(|m| m.指位.is_some() || m.连击.is_some());
    if !有心跳 {
        return;
    }

    println!("\n── 指位与连击（字母键，不含空格）──");
    println!(
        "{:<22}{:>9}{:>9}{:>9}{:>9}{:>9}{:>9}{:>9}",
        "方案", "同指连击", "换手率", "字内同指", "食指负荷", "小指负荷", "中排", "三级增益"
    );
    println!("{}", "-".repeat(94));
    for m in 表 {
        let (同指, 换手, 字内) = match &m.连击 {
            Some(c) => (Some(c.同指), Some(c.换手), Some(c.字内)),
            None => (None, None, None),
        };
        let (食指, 小指, 中排) = match &m.指位 {
            Some(f) => (Some(f.食指()), Some(f.小指()), Some(f.中排())),
            None => (None, None, None),
        };
        println!(
            "{:<22}{:>9}{:>9}{:>9}{:>9}{:>9}{:>9}{:>9}",
            m.名,
            百分比(同指),
            百分比(换手),
            百分比(字内),
            百分比(食指),
            百分比(小指),
            百分比(中排),
            带符号(m.三级增益())
        );
    }
}

/// 递归收集目录下的码表文件
fn 收集(目录: &Path, 文件名: &str) -> Vec<PathBuf> {
    let mut 结果 = Vec::new();
    let mut 栈 = vec![目录.to_path_buf()];
    while let Some(当前) = 栈.pop() {
        let Ok(条目) = std::fs::read_dir(&当前) else {
            continue;
        };
        for 项 in 条目.flatten() {
            let 路径 = 项.path();
            if 路径.is_dir() {
                栈.push(路径);
            } else if 路径.file_name().map(|n| n == 文件名).unwrap_or(false) {
                结果.push(路径);
            }
        }
    }
    结果.sort();
    结果
}

/// 去掉名字里任意位置的 `-run<数字>`（artifact 名带运行号）
fn 去运行号(s: &str) -> String {
    let 字节: Vec<char> = s.chars().collect();
    let mut 输出 = String::new();
    let mut i = 0;
    while i < 字节.len() {
        if 字节[i] == '-'
            && 字节.get(i + 1) == Some(&'r')
            && 字节.get(i + 2) == Some(&'u')
            && 字节.get(i + 3) == Some(&'n')
        {
            let mut j = i + 4;
            while j < 字节.len() && 字节[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 4 {
                i = j;
                continue;
            }
        }
        输出.push(字节[i]);
        i += 1;
    }
    输出
}

/// 把 artifact 目录名整理成好读的方案名
///
/// `codegenie-out-auto-run17-s1/out-auto/thread-03`
///   → 去掉 `codegenie-` 前缀、`-run17` 尾巴，再丢掉被别的段包含的冗余段
///   → `out-auto-s1/thread-03`
fn 整理名字(段: &[String]) -> String {
    if 段.is_empty() {
        return "码表".to_string();
    }

    let mut 清理: Vec<String> = 段.to_vec();

    // 第一段：去掉 codegenie- 前缀与任意位置的 -run<数字>
    if let Some(首) = 清理.first_mut() {
        if let Some(rest) = 首.strip_prefix("codegenie-") {
            *首 = rest.to_string();
        }
        *首 = 去运行号(首);
    }

    // 丢掉被其他段包含的冗余段（out-auto 出现在 out-auto-s1 里）
    let 保留: Vec<String> = 清理
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            !清理
                .iter()
                .enumerate()
                .any(|(j, other)| j != *i && other.len() > s.len() && other.contains(s.as_str()))
        })
        .map(|(_, s)| s.clone())
        .filter(|s| s != ".")
        .collect();

    if 保留.is_empty() {
        "码表".to_string()
    } else {
        保留.join("/")
    }
}

/// 入口
pub fn run(
    表: Option<&str>,
    目录: Option<&str>,
    文件名: &str,
    方案: &str,
    yuhao目录: &str,
    保留: Option<&str>,
    汇总: Option<&str>,
    档案: Option<&str>,
    作者: &str,
    版本: &str,
) {
    // 版本留空就用当天日期
    let 版本默认;
    let 版本 = if 版本.trim().is_empty() {
        版本默认 = chrono::Local::now().format("%Y%m%d").to_string();
        &版本默认[..]
    } else {
        版本
    };

    let 工作目录 =
        std::fs::canonicalize(yuhao目录).unwrap_or_else(|_| PathBuf::from(yuhao目录));
    let 方案路径 = std::fs::canonicalize(方案).unwrap_or_else(|_| PathBuf::from(方案));

    let mut 清单: Vec<(PathBuf, String)> = Vec::new();
    if let Some(t) = 表 {
        let 路径 = std::fs::canonicalize(t).unwrap_or_else(|_| PathBuf::from(t));
        let 名 = 路径
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "码表".to_string());
        清单.push((路径, 名));
    }
    if let Some(d) = 目录 {
        let 根 = PathBuf::from(d);
        for 路径 in 收集(&根, 文件名) {
            let 段: Vec<String> = 路径
                .strip_prefix(&根)
                .ok()
                .and_then(|p| p.parent())
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let 名 = 整理名字(&段);
            清单.push((路径, 名));
        }
    }
    if 清单.is_empty() {
        eprintln!("✗ 没有找到码表：请给 --table 或 --dir");
        return;
    }

    if let Some(目录) = 保留 {
        std::fs::create_dir_all(目录).ok();
    }

    // 词表用于算连击
    let 词表 = handfeel::找词表(&工作目录);
    match &词表 {
        Some(p) => println!("=== 宇浩测评（{} 份）· 连击按 {} 加权 ===", 清单.len(), p.display()),
        None => {
            println!("=== 宇浩测评（{} 份）===", 清单.len());
            eprintln!("⚠️ 没找到词频表，跳过同指连击/换手率");
        }
    }

    let mut 指标表: Vec<指标> = Vec::new();
    let mut 输出文件: Vec<PathBuf> = Vec::new();

    for (路径, 名) in &清单 {
        match 测评一张(路径, 名, &方案路径, &工作目录) {
            Ok((值, 文件)) => {
                let 指位 = 值
                    .get("測評結果")
                    .and_then(|r| handfeel::从热力(r, "簡碼"))
                    .filter(|f| f.中排() > 0.0);
                let 连 = 词表.as_ref().and_then(|w| handfeel::算连击(路径, w).ok());
                let 选 = handfeel::算选重(路径).ok();
                if let Some(m) = 解析(名, &值, 指位, 连, 选) {
                    指标表.push(m);
                }
                if let Some(目录) = 档案 {
                    match 生成方案档案(&值, 名, 路径, Path::new(目录), 作者, 版本) {
                        Ok(p) => println!("  方案档案: {}", p.display()),
                        Err(e) => eprintln!("⚠️ 方案档案「{}」生成失败: {}", 名, e),
                    }
                }
                输出文件.push(文件);
            }
            Err(e) => eprintln!("⚠️ {}", e),
        }
    }

    // 保留结果 JSON
    if let Some(目录) = 保留 {
        for 文件 in &输出文件 {
            if let Some(名) = 文件.file_name() {
                std::fs::copy(文件, Path::new(目录).join(名)).ok();
            }
        }
    }
    for 文件 in &输出文件 {
        let _ = std::fs::remove_file(文件);
    }

    指标表.sort_by(|a, b| {
        a.码长500
            .partial_cmp(&b.码长500)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.速度全码
                    .partial_cmp(&b.速度全码)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    println!();
    打印(&指标表);

    if let Some(路径) = 汇总 {
        match 生成汇总(&指标表) {
            Ok(文本) => match std::fs::write(路径, 文本) {
                Ok(_) => println!("\n汇总已写入 {}", 路径),
                Err(e) => eprintln!("⚠️ 写不了汇总: {}", e),
            },
            Err(e) => eprintln!("⚠️ {}", e),
        }
    }
}

/// 生成 Markdown 对比表（供 Job Summary 用）
fn 生成汇总(表: &[指标]) -> Result<String, String> {
    let mut s = String::new();
    s.push_str("## 宇浩测评对比\n\n");

    // ---- 表一：速度与重码 ----
    s.push_str("### 速度与重码\n\n");
    s.push_str("按加权码长 N=500 升序，越小越快。重码列写作「字数/组数」。\n\n");
    s.push_str("| 方案 | 速度当量(全码) | 速度当量(简码) | 动态选重(全码) | 码长 N=500 | 通用规范重码 | 常用国字重码 | 全码选重数 | 简码选重数 |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for m in 表 {
        s.push_str(&format!(
            "| {} | {:.4} | {:.4} | {:.6} | **{:.4}** | {}/{} | {}/{} | {} | {} |\n",
            m.名,
            m.速度全码,
            m.速度简码,
            m.选重全码,
            m.码长500,
            m.通用全码.0,
            m.通用全码.1,
            m.常用全码.0,
            m.常用全码.1,
            数量(m.选重.as_ref().map(|x| x.全码)),
            数量(m.选重.as_ref().map(|x| x.简码))
        ));
    }
    s.push_str("\n> 选重数按全表统计：选重个数 = 重码字数 − 重码组数。全码取每字最长码，简码取每字最短码（没有简码的字保留全码）。通用规范／常用国字两列只覆盖各自字集，选重数是全表范围。\n");

    // ---- 表二：指位与连击 ----
    if 表.iter().any(|m| m.指位.is_some() || m.连击.is_some()) {
        s.push_str("\n### 指位与连击\n\n");
        s.push_str(
            "连击按词频加权、只算字母键（不含空格）。\
             同指连击越低越顺，换手率越高越有节奏。\
             食指负荷反映指位均衡（理想约 40%），小指负荷过低说明 QAZ/P;/ 一圈键闲置。\
             三级增益是三级简码相对二级简码的额外提速。\n\n",
        );
        s.push_str("| 方案 | 同指连击 | 换手率 | 字内同指 | 食指负荷 | 小指负荷 | 中排 | 三级增益 |\n");
        s.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
        for m in 表 {
            let (同指, 换手, 字内) = match &m.连击 {
                Some(c) => (百分比(Some(c.同指)), 百分比(Some(c.换手)), 百分比(Some(c.字内))),
                None => ("—".to_string(), "—".to_string(), "—".to_string()),
            };
            let (食指, 小指, 中排) = match &m.指位 {
                Some(f) => (
                    百分比(Some(f.食指())),
                    百分比(Some(f.小指())),
                    百分比(Some(f.中排())),
                ),
                None => ("—".to_string(), "—".to_string(), "—".to_string()),
            };
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                m.名,
                同指,
                换手,
                字内,
                食指,
                小指,
                中排,
                带符号(m.三级增益())
            ));
        }
    }

    // ---- 最优方案详情 ----
    if let Some(最优) = 表.first() {
        s.push_str(&format!(
            "\n### 最优：{}\n\n码长 {:.4}，速度当量 {:.4}（全码）/{:.4}（简码）\n\n",
            最优.名, 最优.码长500, 最优.速度全码, 最优.速度简码
        ));

        let 有级 = 最优.速度一级.is_some() && 最优.速度二级.is_some();
        if 有级 {
            s.push_str("简码分层收益：\n\n");
            s.push_str("| 层级 | 速度当量 | 本级增益 |\n|---|---:|---:|\n");
            s.push_str(&format!("| 全码 | {:.4} | — |\n", 最优.速度全码));
            if let Some(v) = 最优.速度一级 {
                s.push_str(&format!(
                    "| 加一级 | {:.4} | {} |\n",
                    v,
                    带符号(最优.一级增益())
                ));
            }
            if let Some(v) = 最优.速度二级 {
                s.push_str(&format!(
                    "| 加二级 | {:.4} | {} |\n",
                    v,
                    带符号(最优.二级增益())
                ));
            }
            s.push_str(&format!(
                "| 加三级 | {:.4} | {} |\n",
                最优.速度简码,
                带符号(最优.三级增益())
            ));
            s.push_str("\n");
        }

        if let Some(f) = &最优.指位 {
            s.push_str("指位负荷：\n\n```\n");
            let 最重 = f.最重();
            for i in 0..8 {
                let 占比 = f.指(i) / (0..8).map(|j| f.指(j)).sum::<f64>() * 100.0;
                let 条 = "█".repeat((占比 / 2.0) as usize);
                s.push_str(&format!(
                    "{:<8}{:>6.2}%  {:<26}{}\n",
                    handfeel::指名[i],
                    占比,
                    条,
                    if i == 最重 { "← 最重" } else { "" }
                ));
            }
            s.push_str("```\n\n行分布：\n\n```\n");
            for i in 0..3 {
                let 占比 = f.行(i) / (0..8).map(|j| f.指(j)).sum::<f64>() * 100.0;
                s.push_str(&format!("{:<8}{:>6.2}%\n", handfeel::行名[i], 占比));
            }
            s.push_str("```\n");
        }
    }

    Ok(s)
}

// -------------------------------------------------------------------------
// 宇浩测评「方案档案」——可直接导入网页版的 JSON
// -------------------------------------------------------------------------

/// 把方案名转成文件名用的标识符
fn 标识符(名: &str) -> String {
    let mut 原 = String::from("codegenie-");
    for c in 名.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            原.push(c.to_ascii_lowercase());
        } else {
            原.push('-');
        }
    }
    let mut 结果 = String::new();
    let mut 上横 = false;
    for c in 原.chars() {
        if c == '-' {
            if !上横 {
                结果.push(c);
            }
            上横 = true;
        } else {
            结果.push(c);
            上横 = false;
        }
    }
    结果.trim_matches('-').to_string()
}

fn 生成方案档案(
    值: &Value,
    名: &str,
    码表: &Path,
    目录: &Path,
    作者: &str,
    版本: &str,
) -> Result<PathBuf, String> {
    let 时间 = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let 标识 = 标识符(名);

    let 字数 = 值
        .get("測評結果")
        .and_then(|r| r.get("候選個數分析"))
        .and_then(|c| c.get("字符數"))
        .and_then(|n| n.as_u64());

    let 字节 = std::fs::read(码表).map_err(|e| format!("读不了码表: {}", e))?;
    let 摘要 = format!("{:x}", Sha256::digest(&字节));

    let mut 档 = 值.clone();
    {
        let obj = 档
            .as_object_mut()
            .ok_or_else(|| "测评结果不是 JSON 对象".to_string())?;

        {
            let 元 = obj.entry("元數據".to_string()).or_insert_with(|| json!({}));
            if let Some(m) = 元.as_object_mut() {
                m.insert("方案名".into(), json!(名));
                m.insert("標識符".into(), json!(标识));
                m.insert("作者".into(), json!(作者));
                m.insert("版本".into(), json!(版本));
                m.insert(
                    "描述".into(),
                    json!("魔靈四碼定長，一／二／三級簡碼取全碼前綴，空格上屏"),
                );
                m.insert("標籤".into(), json!(["形碼", "四碼定長", "簡碼"]));
                m.entry("創建時間".to_string())
                    .or_insert_with(|| json!(时间));
                m.insert("更新時間".into(), json!(时间));
            }
        }

        {
            let 参 = obj
                .entry("方案參數".to_string())
                .or_insert_with(|| json!({}));
            if let Some(m) = 参.as_object_mut() {
                m.entry("選重編碼化".to_string())
                    .or_insert_with(|| json!(false));
                m.entry("出簡不出全".to_string())
                    .or_insert_with(|| json!(false));
                m.entry("最大碼長".to_string()).or_insert_with(|| json!(4));
            }
        }

        {
            let 码元 = obj
                .entry("碼表元數據".to_string())
                .or_insert_with(|| json!({}));
            if let Some(m) = 码元.as_object_mut() {
                m.insert("分隔符".into(), json!("製表符"));
                m.insert("第一列類型".into(), json!("字符"));
                if let Some(n) = 字数 {
                    m.insert("總字符數".into(), json!(n));
                }
                m.insert("哈希值".into(), json!(摘要));
            }
        }
    }

    let 文本 =
        serde_json::to_string_pretty(&档).map_err(|e| format!("序列化方案档案失败: {}", e))?;
    std::fs::create_dir_all(目录).ok();
    let 路径 = 目录.join(format!("{}.json", 标识));
    std::fs::write(&路径, 文本).map_err(|e| format!("写不了方案档案: {}", e))?;
    Ok(路径)
}
