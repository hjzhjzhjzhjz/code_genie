// =========================================================================
// 📊 宇浩测评封装
// =========================================================================
//
// 调 yuhao-assess 的 Node CLI（`pnpm cli`），把 JSON 结果解析成一张对比表。
// 方案之间的强弱只看这里输出的宇浩指标，不看 CodeGenie 内部得分。
// =========================================================================

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// 一份码表的测评指标
struct 指标 {
    名: String,
    速度全码: f64,
    速度简码: f64,
    选重全码: f64,
    选重简码: f64,
    码长500: f64,
    通用全码: (u64, u64),
    通用简码: (u64, u64),
    常用全码: (u64, u64),
    常用简码: (u64, u64),
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
fn 解析(名: &str, 结果: &Value) -> Option<指标> {
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

    Some(指标 {
        名: 名.to_string(),
        速度全码: 取数(r, &["速度當量分析", "北語簡體字頻全碼速度當量"])?,
        速度简码: 取数(r, &["速度當量分析", "北語簡體字頻全部簡碼速度當量"])?,
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

fn 打印(表: &[指标]) {
    println!(
        "{:<22}{:>9}{:>9}{:>10}{:>10}{:>9}{:>10}{:>10}{:>9}{:>9}",
        "方案", "速度全码", "速度简码", "选重全码", "选重简码", "码长N500", "通用全码", "通用简码", "常用全码", "常用简码"
    );
    println!("{}", "-".repeat(108));
    for m in 表 {
        println!(
            "{:<22}{:>9.4}{:>9.4}{:>10.6}{:>10.6}{:>9.4}{:>5}/{:<4}{:>5}/{:<4}{:>4}/{:<4}{:>4}/{:<4}",
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
            m.常用简码.1
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

/// 入口
pub fn run(
    表: Option<&str>,
    目录: Option<&str>,
    文件名: &str,
    方案: &str,
    yuhao目录: &str,
    保留: Option<&str>,
    汇总: Option<&str>,
) {
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
            let 名 = 路径
                .strip_prefix(&根)
                .ok()
                .and_then(|p| p.parent())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "码表".to_string());
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

    println!("=== 宇浩测评（{} 份）===", 清单.len());
    let mut 指标表: Vec<指标> = Vec::new();
    let mut 输出文件: Vec<PathBuf> = Vec::new();

    for (路径, 名) in &清单 {
        match 测评一张(路径, 名, &方案路径, &工作目录) {
            Ok((值, 文件)) => {
                if let Some(m) = 解析(名, &值) {
                    指标表.push(m);
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
    s.push_str("按加权码长 N=500 升序，越小越快。重码列写作「字数/组数」。\n\n");
    s.push_str("| 方案 | 速度当量(全码) | 速度当量(简码) | 动态选重(全码) | 码长 N=500 | 通用规范重码 | 常用国字重码 |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for m in 表 {
        s.push_str(&format!(
            "| {} | {:.4} | {:.4} | {:.6} | **{:.4}** | {}/{} | {}/{} |\n",
            m.名,
            m.速度全码,
            m.速度简码,
            m.选重全码,
            m.码长500,
            m.通用全码.0,
            m.通用全码.1,
            m.常用全码.0,
            m.常用全码.1
        ));
    }
    if let Some(最优) = 表.first() {
        s.push_str(&format!(
            "\n最优：**{}**（码长 {:.4}，速度当量 {:.4}）\n",
            最优.名, 最优.码长500, 最优.速度全码
        ));
    }
    Ok(s)
}
