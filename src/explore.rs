// =========================================================================
// 🧭 `explore` 子命令：权重空间探索
// =========================================================================
//
// 取代已删的 `moling/analyze-results-of-batch-test-weights.sh`：把「同一份配置、每次
// 换一套全码权重」跑出来的一批 optimize 日志汇总成
//
//   · 一张 TSV（每行一轮：权重 | 输出目录 | 首选线程指标 | 各线程最小 | 各线程最大）
//   · 一张 Plotly 平行坐标 HTML（20 根轴：5 个权重 + 5 项指标 × 最小/首选/最大）
//
// 输入：`code_genie optimize` 自己打印的日志。每份日志里取两行
//
//   全码权重: 重码数=1.00, 重码率=0.50, 当量=0.30, CV=0.10, 分布=0.10
//   输出目录: moling-output/output-20261102
//
// 再读该目录下的 `summary.txt`，取所有 `T<编号>` 行的第 3~7 列
// （重码数 / 重码率% / 当量 / CV / 分布偏差）。
//
// 与 shell 版的差异：shell 版从 `输出目录` 行里正则截 `output\S+` 再拼相对路径，
// 换个工作目录就找不到文件；这里直接用冒号后的完整路径。
// =========================================================================

use std::fs;
use std::path::Path;

use serde_json::json;

/// 权重维（顺序与 `全码权重:` 那行一致）
const 权重维: [&str; 5] = [
    "重码数权重",
    "重码率权重",
    "当量权重",
    "CV权重",
    "分布偏差权重",
];

/// 指标维（顺序与 summary.txt 的列一致）
const 指标维: [&str; 5] = ["重码数", "重码率%", "当量", "CV", "分布偏差"];

/// 命令行参数
pub struct 参数 {
    /// 输入日志（通常是 `test-*.log`）
    pub 日志: Vec<String>,
    /// 汇总表输出（TSV）
    pub 表: String,
    /// 平行坐标网页输出（HTML）
    pub 网页: String,
    /// 网页标题
    pub 标题: String,
}

/// 一轮优化：一套权重 + 该轮的指标分布
#[derive(Debug, Clone)]
struct 一轮 {
    来源: String,
    权重: [f64; 5],
    目录: String,
    首发: [f64; 5],
    最小: [f64; 5],
    最大: [f64; 5],
    线程数: usize,
}

pub fn run(参: &参数) -> Result<(), String> {
    if 参.日志.is_empty() {
        return Err("没有输入日志；用法：code_genie explore test-*.log".to_string());
    }
    println!("=== 权重空间探索 ===");

    let mut 轮们 = Vec::new();
    for 路径 in &参.日志 {
        match 解析一份(Path::new(路径)) {
            Ok(Some(r)) => {
                println!(
                    "  ✓ {}：权重 {}，{} 个线程",
                    r.来源,
                    权重串(&r.权重),
                    r.线程数
                );
                轮们.push(r);
            }
            Ok(None) => println!("  ⚠️ {路径}：没有输出目录或缺 summary.txt，跳过"),
            Err(e) => println!("  ⚠️ {路径}：{e}，跳过"),
        }
    }
    if 轮们.is_empty() {
        return Err("所有日志都没解析出结果".to_string());
    }

    let 表 = 生成表(&轮们);
    fs::write(&参.表, &表).map_err(|e| format!("写 {} 失败：{e}", 参.表))?;
    println!("  已写入 {}（{} 轮）", 参.表, 轮们.len());

    let 网页 = 生成网页(&轮们, &参.标题);
    fs::write(&参.网页, &网页).map_err(|e| format!("写 {} 失败：{e}", 参.网页))?;
    println!("  已写入 {}", 参.网页);

    if let Some(行) = 最优行(&轮们) {
        println!(
            "  首选指标最好的一轮：{}（重码数 {:.0}，当量 {:.4}）",
            行.来源, 行.首发[0], 行.首发[2]
        );
    }
    Ok(())
}

/// 解析一份日志 → 一轮结果；日志里没有可用输出目录时返回 `Ok(None)`
fn 解析一份(路径: &Path) -> Result<Option<一轮>, String> {
    let 文本 = fs::read_to_string(路径).map_err(|e| format!("读不了：{e}"))?;

    let mut 权重: Option<[f64; 5]> = None;
    let mut 目录: Option<String> = None;
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 权重.is_none() && 行.starts_with("全码权重") {
            let 数 = 抓浮点(行);
            if 数.len() >= 5 {
                权重 = Some([数[0], 数[1], 数[2], 数[3], 数[4]]);
            }
        } else if 目录.is_none() {
            if let Some(后) = 行.strip_prefix("输出目录:") {
                let 后 = 后.trim();
                if !后.is_empty() {
                    目录 = Some(后.to_string());
                }
            }
        }
    }

    let (Some(权重), Some(目录)) = (权重, 目录) else {
        return Ok(None);
    };
    let 汇总路径 = Path::new(&目录).join("summary.txt");
    let 汇总 = match fs::read_to_string(&汇总路径) {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let (首发, 最小, 最大, 线程数) = 解析汇总(&汇总);
    if 线程数 == 0 {
        return Ok(None);
    }
    Ok(Some(一轮 {
        来源: 路径
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| 路径.display().to_string()),
        权重,
        目录,
        首发,
        最小,
        最大,
        线程数,
    }))
}

/// 从一行文字里按出现顺序抽出所有浮点数（`1.00`、`0.5`…）
fn 抓浮点(行: &str) -> Vec<f64> {
    let mut 出 = Vec::new();
    let mut 当前 = String::new();
    for c in 行.chars() {
        if c.is_ascii_digit() || c == '.' {
            当前.push(c);
        } else if !当前.is_empty() {
            if let Ok(v) = 当前.parse::<f64>() {
                出.push(v);
            }
            当前.clear();
        }
    }
    if !当前.is_empty() {
        if let Ok(v) = 当前.parse::<f64>() {
            出.push(v);
        }
    }
    出
}

/// 解析 `summary.txt`：返回（得分最高那行、逐列最小、逐列最大、线程数）
///
/// 行格式：`T1 1.2345 12345 0.123456 1.2345 0.1234 0.1234 [🏆]`，
/// 取第 3~7 列（0 基 2..=6）为重码数、重码率%、当量、CV、分布偏差。
/// `summary.txt` 按得分升序排，所以第一行就是首选线程。
fn 解析汇总(文本: &str) -> ([f64; 5], [f64; 5], [f64; 5], usize) {
    let mut 首发 = [0.0f64; 5];
    let mut 最小 = [f64::INFINITY; 5];
    let mut 最大 = [f64::NEG_INFINITY; 5];
    let mut 线程数 = 0usize;

    for 行 in 文本.lines() {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 7 || !列[0].starts_with('T') {
            continue;
        }
        let 数: Vec<f64> = match 列[2..=6].iter().map(|s| s.parse::<f64>()).collect() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if 线程数 == 0 {
            首发.copy_from_slice(&数);
        }
        for i in 0..5 {
            最小[i] = 最小[i].min(数[i]);
            最大[i] = 最大[i].max(数[i]);
        }
        线程数 += 1;
    }
    (首发, 最小, 最大, 线程数)
}

fn 权重串(w: &[f64; 5]) -> String {
    format!("{}/{}/{}/{}/{}", w[0], w[1], w[2], w[3], w[4])
}

/// 生成 TSV：表头 + 每轮一行
fn 生成表(轮们: &[一轮]) -> String {
    let mut s = String::new();
    let mut 头: Vec<String> = 权重维.iter().map(|s| s.to_string()).collect();
    头.push("输出目录".to_string());
    头.push("线程数".to_string());
    for 前缀 in ["首选", "最小", "最大"] {
        for 名 in 指标维 {
            头.push(format!("{前缀}{名}"));
        }
    }
    s.push_str(&头.join("\t"));
    s.push('\n');

    for r in 轮们 {
        let mut 列: Vec<String> = r.权重.iter().map(|v| format!("{v:.2}")).collect();
        列.push(r.目录.clone());
        列.push(r.线程数.to_string());
        for 段 in [&r.首发, &r.最小, &r.最大] {
            for v in 段 {
                列.push(format!("{v}"));
            }
        }
        s.push_str(&列.join("\t"));
        s.push('\n');
    }
    s
}

/// 首选指标（当量）最好的一轮
fn 最优行(轮们: &[一轮]) -> Option<&一轮> {
    轮们.iter().min_by(|a, b| {
        a.首发[2]
            .partial_cmp(&b.首发[2])
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// 生成平行坐标 HTML
fn 生成网页(轮们: &[一轮], 标题: &str) -> String {
    let mut 维 = Vec::new();
    for (i, 名) in 权重维.iter().enumerate() {
        维.push(json!({
            "label": 名,
            "values": 轮们.iter().map(|r| r.权重[i]).collect::<Vec<_>>(),
        }));
    }
    for (段名, 取) in [
        ("最小", (|r: &一轮| r.最小) as fn(&一轮) -> [f64; 5]),
        ("首选", (|r: &一轮| r.首发) as fn(&一轮) -> [f64; 5]),
        ("最大", (|r: &一轮| r.最大) as fn(&一轮) -> [f64; 5]),
    ] {
        for (i, 名) in 指标维.iter().enumerate() {
            维.push(json!({
                "label": format!("{名}({段名})"),
                "values": 轮们.iter().map(|r| 取(r)[i]).collect::<Vec<_>>(),
            }));
        }
    }

    let trace = json!([{
        "type": "parcoords",
        "line": { "color": "blue" },
        "dimensions": 维,
    }]);
    let trace文本 = serde_json::to_string_pretty(&trace).unwrap_or_else(|_| "[]".to_string());

    format!(
        r#"<!DOCTYPE html>
<html lang="zh" dir="ltr">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0"/>
  <title>{标题}</title>
  <script src="https://cdn.plot.ly/plotly-3.4.0.min.js" charset="utf-8"></script>
</head>
<body>
  <h1>{标题}</h1>
  <div id="chart"></div>
  <p>使用说明：</p>
  <ol>
    <li>用意是通过多维指标范围的过滤，得出最优的权重取值范围（前 5 根轴），据此指导优化时的权重调整；</li>
    <li>在数轴上拖动即可设置过滤范围，点击过滤条取消过滤；</li>
    <li>每项指标画了三根轴（最小 / 首选 / 最大），用来判断这一轮线程间的波动是否过大，波动大说明步数偏少、结论不可靠；</li>
    <li>过滤时以「最小」轴为准：正式优化步数更大，指标通常还会更低；</li>
    <li>字根或单字拆分方案变化、或码灵版本升级后，最好重新探索一遍权重空间。</li>
  </ol>
  <script>
  Plotly.newPlot("chart", {trace文本});
  </script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一份日志 + 对应输出目录下的 summary.txt
    fn 造一轮(根: &Path, 名: &str, 权重: &str, 线程: &[&str]) -> String {
        let 目录 = 根.join(名);
        fs::create_dir_all(&目录).unwrap();
        let mut 汇总 = String::from("# 优化结果汇总\n\n线程 得分 重码数\n");
        for 行 in 线程 {
            汇总.push_str(行);
            汇总.push('\n');
        }
        fs::write(目录.join("summary.txt"), 汇总).unwrap();

        let 日志 = 根.join(format!("{名}.log"));
        fs::write(
            &日志,
            format!("全码权重: {权重}\n输出目录: {}\n", 目录.display()),
        )
        .unwrap();
        日志.display().to_string()
    }

    #[test]
    fn 抓浮点只取数字与小数点() {
        assert_eq!(
            抓浮点("全码权重: 重码数=1.00, 重码率=0.50, 当量=0.30, CV=0.10, 分布=0.10"),
            vec![1.0, 0.50, 0.30, 0.10, 0.10]
        );
        assert!(抓浮点("全码权重: 无").is_empty());
    }

    #[test]
    fn 解析汇总取首发与极值() {
        let 文本 = "# 汇总\n\
                    T1  1.0000  100  0.100000  1.2000  0.1000  0.1000\n\
                    T2  2.0000  200  0.200000  1.3000  0.2000  0.3000\n\
                    T3  3.0000  150  0.150000  1.2500  0.1500  0.2000 🏆\n";
        let (首发, 最小, 最大, n) = 解析汇总(文本);
        assert_eq!(n, 3);
        assert_eq!(首发, [100.0, 0.1, 1.2, 0.1, 0.1]);
        assert_eq!(最小, [100.0, 0.1, 1.2, 0.1, 0.1]);
        assert_eq!(最大, [200.0, 0.2, 1.3, 0.2, 0.3]);
    }

    #[test]
    fn 一条链跑通并生成表() {
        let 根 = std::env::temp_dir().join(format!("cg-explore-{}", std::process::id()));
        let _ = fs::remove_dir_all(&根);
        fs::create_dir_all(&根).unwrap();

        let a = 造一轮(
            &根,
            "output-a",
            "重码数=1.00, 重码率=0.50, 当量=0.30, CV=0.10, 分布=0.10",
            &[
                "T1  1.0000  100  0.100000  1.2000  0.1000  0.1000",
                "T2  2.0000  200  0.200000  1.3000  0.2000  0.3000",
            ],
        );
        let b = 造一轮(
            &根,
            "output-b",
            "重码数=0.80, 重码率=0.60, 当量=0.40, CV=0.10, 分布=0.10",
            &["T1  1.0000  90  0.090000  1.1000  0.1000  0.1000"],
        );

        断言一轮(&a, "output-a");
        断言一轮(&b, "output-b");

        let 参 = 参数 {
            日志: vec![a, b],
            表: 根.join("表.tsv").display().to_string(),
            网页: 根.join("网页.html").display().to_string(),
            标题: "测试".to_string(),
        };
        run(&参).unwrap();

        let 表 = fs::read_to_string(&参.表).unwrap();
        let 行们: Vec<&str> = 表.lines().collect();
        assert_eq!(行们.len(), 3, "表头 + 2 轮");
        assert!(行们[0].contains("重码数权重\t重码率权重"));
        assert!(行们[1].contains("1.00\t0.50\t0.30\t0.10\t0.10"));
        assert_eq!(
            行们[1].split('\t').count(),
            5 + 2 + 15,
            "权重 5 + 目录/线程数 2 + 指标 15"
        );

        let 网页 = fs::read_to_string(&参.网页).unwrap();
        assert!(网页.contains("Plotly.newPlot"));
        assert!(网页.contains("重码数(最小)"));
        assert!(网页.contains("分布偏差(最大)"));

        fs::remove_dir_all(&根).ok();
    }

    fn 断言一轮(日志: &str, 名: &str) {
        let r = 解析一份(Path::new(日志)).unwrap().expect("应解析出结果");
        assert!(r.目录.ends_with(名), "{}", r.目录);
    }

    #[test]
    fn 缺_summary_的日志被跳过() {
        let 根 = std::env::temp_dir().join(format!("cg-explore-miss-{}", std::process::id()));
        let _ = fs::remove_dir_all(&根);
        fs::create_dir_all(&根).unwrap();
        let 日志 = 根.join("x.log");
        fs::write(
            &日志,
            format!(
                "全码权重: 重码数=1.00, 重码率=0.50, 当量=0.30, CV=0.10, 分布=0.10\n输出目录: {}\n",
                根.join("不存在").display()
            ),
        )
        .unwrap();
        assert!(解析一份(&日志).unwrap().is_none());
        fs::remove_dir_all(&根).ok();
    }
}
