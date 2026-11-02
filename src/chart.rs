// =========================================================================
// 📈 收敛曲线出图
// =========================================================================

//! 把 `optimize` 日志里的「采样」行画成折线图。
//!
//! 采样行由 `annealing.rs` 每 1% 步数打印一次，格式固定：
//!
//! ```text
//!    [T0] 采样 step=5000 得分=0.987654 当前=1.234567 温度=0.001234
//! ```
//!
//! 解析部分不依赖 charts-rs，始终编译（可单测）；绘图部分在 `chart` feature 之下。
//! 之所以把出图放进可选 feature：charts-rs 依赖树含 resvg/fontdb 等几十个 crate，
//! 而日常跑优化根本不用出图，默认开着纯属拖慢每次 `cargo build`。
//!
//! 与 `tools/画收敛曲线.py`（零依赖出 SVG）并存：那个用于快速看，这个用于
//! 出正式报告需要的 PNG（Word 对 SVG 支持一般）。

/// 一个收敛采样点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct 采样点 {
    /// 已跑的步数
    pub 步数: u64,
    /// 历史最优得分（只降不升）
    pub 得分: f64,
    /// 当前解得分（会随温度回落，高温期远高于最优）
    pub 当前: f64,
    /// 当前温度
    pub 温度: f64,
}

/// 采样行的定位标记（行首缩进与线程号由 `find` 跳过）。
const 采样标记: &str = "采样 step=";

/// 从整份日志里抽出所有采样点；不合规的行静默跳过（日志里混着别的输出）。
pub fn 解析(文本: &str) -> Vec<采样点> {
    let mut 出 = Vec::new();
    for 行 in 文本.lines() {
        let Some(位) = 行.find(采样标记) else {
            continue;
        };
        let 尾 = &行[位 + 采样标记.len()..];
        let mut 步数 = None;
        let mut 得分 = None;
        let mut 当前 = None;
        let mut 温度 = None;
        for (序号, 段) in 尾.split_whitespace().enumerate() {
            // 标记本身吃掉了 "step="，所以尾部第一段是裸数字，其余是 key=value。
            let (键, 值) = match 段.split_once('=') {
                Some((k, v)) => (k, v),
                None if 序号 == 0 => ("step", 段),
                None => continue,
            };
            let Ok(数) = 值.parse::<f64>() else {
                continue;
            };
            match 键 {
                "step" => 步数 = Some(数.max(0.0) as u64),
                "得分" => 得分 = Some(数),
                "当前" => 当前 = Some(数),
                "温度" => 温度 = Some(数),
                // 未来加了字段也不会让解析失效
                _ => {}
            }
        }
        if let (Some(s), Some(b), Some(c), Some(t)) = (步数, 得分, 当前, 温度) {
            出.push(采样点 {
                步数: s,
                得分: b,
                当前: c,
                温度: t,
            });
        }
    }
    出
}

/// 步数缩写：5000 → `5k`，2_000_000 → `2M`。
///
/// x 轴要放上百个标签，写全 `2000000` 必然挤压重叠，缩写后 charts-rs 还能按需稀疏。
fn 缩写步数(步: u64) -> String {
    if 步 >= 1_000_000 {
        let 值 = 步 as f64 / 1_000_000.0;
        format!("{}M", 去尾零(值))
    } else if 步 >= 1_000 {
        let 值 = 步 as f64 / 1_000.0;
        format!("{}k", 去尾零(值))
    } else {
        步.to_string()
    }
}

/// `2.0` → `2`，`2.5` → `2.5`：保留一位小数，但整数不显示 `.0`。
fn 去尾零(值: f64) -> String {
    let 一位 = (值 * 10.0).round() / 10.0;
    if (一位 - 一位.round()).abs() < f64::EPSILON {
        format!("{}", 一位.round() as i64)
    } else {
        format!("{一位:.1}")
    }
}

/// 出图结果：SVG 一定有，PNG 只在调用方要时才生成（光栅化比排版慢）。
#[cfg(feature = "chart")]
#[derive(Debug)]
pub struct 图形 {
    pub svg: String,
    pub png: Option<Vec<u8>>,
}

/// 中文字体只注册一次（`add_fonts` 是全局的，重复注册浪费时间）。
///
/// 不注册字体也能出图，只是中文会渲染成方框；找不到任何候选字体时返回 `None`，
/// 由调用方决定是否提示。
#[cfg(feature = "chart")]
fn 中文字体() -> Option<&'static str> {
    use std::sync::OnceLock;
    static 已注册: OnceLock<Option<String>> = OnceLock::new();
    已注册
        .get_or_init(|| {
            // (字体文件, family 名) —— msyh.ttc 内含 "Microsoft YaHei"
            const 候选: &[(&str, &str)] = &[
                (r"C:\Windows\Fonts\msyh.ttc", "Microsoft YaHei"),
                (r"C:\Windows\Fonts\msyh.ttf", "Microsoft YaHei"),
                (r"C:\Windows\Fonts\simhei.ttf", "SimHei"),
                ("/System/Library/Fonts/PingFang.ttc", "PingFang SC"),
                (
                    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                    "Noto Sans CJK SC",
                ),
                (
                    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
                    "WenQuanYi Micro Hei",
                ),
            ];
            for (路径, 名) in 候选 {
                let Ok(字节) = std::fs::read(路径) else {
                    continue;
                };
                if charts_rs::add_fonts(&[&字节]).is_ok() {
                    return Some((*名).to_string());
                }
            }
            None
        })
        .as_deref()
}

/// 画收敛曲线。
///
/// `含当前` 时把「当前得分」画在第二条 y 轴上——高温期它比最优高 2~5 倍，
/// 共用一根轴会把最优得分压成贴底直线。
#[cfg(feature = "chart")]
pub fn 出图(
    点: &[采样点], 含当前: bool, 标题: &str, 要_png: bool
) -> Result<图形, String> {
    use charts_rs::{svg_to_png, LineChart, Series};

    if 点.is_empty() {
        return Err("日志里没有采样点：确认日志来自带采样输出的 optimize 运行".to_string());
    }

    let 横标: Vec<String> = 点.iter().map(|p| 缩写步数(p.步数)).collect();
    let mut 序列 = vec![Series::new(
        "最优得分".to_string(),
        点.iter().map(|p| p.得分 as f32).collect(),
    )];
    if 含当前 {
        let mut 当前 = Series::new(
            "当前得分".to_string(),
            点.iter().map(|p| p.当前 as f32).collect(),
        );
        当前.y_axis_index = 1;
        序列.push(当前);
    }

    let mut 图 = LineChart::new(序列, 横标);
    let 末点 = 点.last().copied().unwrap_or(点[0]);
    图.title.text = 标题.to_string();
    // 把关键结论直接写在图上：最优得分是否还在降、总共跑了多少步。
    // 单轴模式下曲线常常是一条平线，光看图看不出「它到底是 6.59 还是 2.1」。
    图.sub_title.text = format!(
        "最优 {:.4} · 当前 {:.4} · 跑到 {} 步 · {} 个采样点",
        末点.得分,
        末点.当前,
        缩写步数(末点.步数),
        点.len()
    );
    图.width = 1200.0;
    图.height = 620.0;
    // 100 个采样点各画一个圆圈会连成一串珠子，线本身已经够看
    图.series.symbol = Some(charts_rs::Symbol::None);
    图.series.stroke_width = 2.0;
    if let Some(族) = 中文字体() {
        图.font_family = 族.to_string();
    }
    if 含当前 {
        // 第二条轴直接从第一条复制样式，只改轴名，省得手写一堆默认值。
        let mut 右轴 = 图.y_axis_configs.first().cloned().unwrap_or_default();
        右轴.title = Some("当前得分".to_string());
        图.y_axis_configs.push(右轴);
        图.y_axis_configs[0].title = Some("最优得分".to_string());
    }

    let svg = 图.svg().map_err(|e| format!("生成 SVG 失败：{e}"))?;
    let png = if 要_png {
        Some(svg_to_png(&svg).map_err(|e| format!("SVG 转 PNG 失败：{e}"))?)
    } else {
        None
    };
    Ok(图形 { svg, png })
}

/// `plot` 子命令的参数。
pub struct 参数 {
    /// optimize 日志路径
    pub 日志: String,
    /// 输出文件（按扩展名决定 PNG 还是 SVG）
    pub 输出: String,
    /// 是否加画「当前得分」（双轴）
    pub 含当前: bool,
    /// 图表标题，缺省取日志文件名
    pub 标题: Option<String>,
}

/// 默认标题：`收敛曲线 · 测试3.log`
fn 默认标题(日志: &str) -> String {
    let 名 = std::path::Path::new(日志)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| 日志.to_string());
    format!("收敛曲线 · {名}")
}

/// 跑 `plot`：读日志 → 出图 → 落盘。
#[cfg(feature = "chart")]
pub fn run(参: &参数) -> Result<(), String> {
    let 小写 = 参.输出.to_lowercase();
    if !小写.ends_with(".png") && !小写.ends_with(".svg") {
        return Err(format!("输出文件只支持 .png 或 .svg：{}", 参.输出));
    }
    let 文本 =
        std::fs::read_to_string(&参.日志).map_err(|e| format!("读不了日志 {}：{e}", 参.日志))?;
    let 点 = 解析(&文本);
    if 点.is_empty() {
        return Err(format!(
            "{} 里没有「采样 step=」行：日志要来自带采样输出的 optimize 运行",
            参.日志
        ));
    }
    let 题 = 参.标题.clone().unwrap_or_else(|| 默认标题(&参.日志));
    let 要_png = 小写.ends_with(".png");
    let 图 = 出图(&点, 参.含当前, &题, 要_png)?;
    let 内容: &[u8] = if 要_png {
        图.png.as_deref().unwrap_or(&[])
    } else {
        图.svg.as_bytes()
    };
    std::fs::write(&参.输出, 内容).map_err(|e| format!("写不了 {}：{e}", 参.输出))?;
    println!("✅ 已写出 {}（{} 个采样点）", 参.输出, 点.len());
    Ok(())
}

/// 未启用 `chart` feature 时的占位：仍然先解析日志并把摘要报出来，
/// 免得用户拿到「要重编译」的提示却不确定数据本身有没有问题。
#[cfg(not(feature = "chart"))]
pub fn run(参: &参数) -> Result<(), String> {
    let 文本 =
        std::fs::read_to_string(&参.日志).map_err(|e| format!("读不了日志 {}：{e}", 参.日志))?;
    let 点 = 解析(&文本);
    if 点.is_empty() {
        return Err(format!(
            "{} 里没有「采样 step=」行：日志要来自带采样输出的 optimize 运行",
            参.日志
        ));
    }
    let 题 = 参.标题.clone().unwrap_or_else(|| 默认标题(&参.日志));
    let 首 = 点.first().map(|p| p.得分).unwrap_or(0.0);
    let 末 = 点.last().copied().unwrap_or(点[0]);
    Err(format!(
        "plot 需要图表依赖，当前二进制没编进去（数据没问题：{题} 有 {} 个采样点，\
         最优 {首:.4} → {:.4}，跑到 {} 步，将画{}到 {}）。\n\
         重新构建：cargo build --release --features chart",
        点.len(),
        末.得分,
        缩写步数(末.步数),
        if 参.含当前 { "双轴" } else { "单轴" },
        参.输出
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const 样例: &str = "\
   开始计算\n\
      [T0] 采样 step=5000 得分=6.589900 当前=12.345678 温度=0.001234\n\
      [T0] 进度 25%\n\
      [T0] 采样 step=10000 得分=6.589900 当前=8.765432 温度=0.000321\n";

    #[test]
    fn 解析抽出采样点并跳过其他行() {
        let 点 = 解析(样例);
        assert_eq!(点.len(), 2);
        assert_eq!(点[0].步数, 5000);
        assert!((点[0].得分 - 6.5899).abs() < 1e-9);
        assert!((点[0].当前 - 12.345678).abs() < 1e-9);
        assert!((点[0].温度 - 0.001234).abs() < 1e-9);
        assert_eq!(点[1].步数, 10000);
    }

    #[test]
    fn 缺字段的行整行丢弃() {
        // 少一个「温度」，不能产出半成品点（否则 x/y 长度对不上，图会错位）
        let 点 = 解析("   [T0] 采样 step=500 得分=1.0 当前=2.0\n");
        assert!(点.is_empty());
    }

    #[test]
    fn 无采样行返回空() {
        assert!(解析("普通日志\n没有采样\n").is_empty());
        assert!(解析("").is_empty());
    }

    #[test]
    fn 多余字段不影响解析() {
        // 以后采样行加字段（比如重码数）时，旧解析仍应工作
        let 点 = 解析("   [T0] 采样 step=500 得分=1.5 当前=2.5 温度=0.5 重码=3\n");
        assert_eq!(点.len(), 1);
        assert!((点[0].得分 - 1.5).abs() < 1e-9);
    }

    #[test]
    fn 步数缩写() {
        assert_eq!(缩写步数(0), "0");
        assert_eq!(缩写步数(500), "500");
        assert_eq!(缩写步数(5000), "5k");
        assert_eq!(缩写步数(15000), "15k");
        assert_eq!(缩写步数(2500), "2.5k");
        assert_eq!(缩写步数(2_000_000), "2M");
        assert_eq!(缩写步数(40_000_000), "40M");
    }

    /// 只在 `--features chart` 下跑：验的是「能出真图」，不只是编译过。
    /// 没装中文字体也能出图（中文变方块），所以这里不断言文字渲染。
    #[cfg(feature = "chart")]
    #[test]
    fn 出图产出有效_png() {
        let 点 = 解析(样例);
        let 图 = 出图(&点, true, "测试", true).expect("出图失败");
        assert!(图.svg.starts_with("<svg") || 图.svg.contains("<svg"));
        let png = 图.png.expect("要求了 PNG");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG 文件头不对");
    }

    #[cfg(feature = "chart")]
    #[test]
    fn 空数据出图报错而不是画空图() {
        let 错 = 出图(&[], false, "空", false).unwrap_err();
        assert!(错.contains("没有采样点"));
    }
}
