// =========================================================================
// 🔧 拆分流水线与字根归并
// =========================================================================
//
// 流水线：把一个字从「拆分」走到「编码」的每一步摊开，便于查为什么是这个码。
// 归并  ：按「A=B」（整体并入）或「A 1=B 2」（码位并入）改键位，
//         重新编码后对比阶重与选重。只动键位，不重新拆分。
// =========================================================================

use std::collections::HashMap;

use crate::export_table::{
    加载拆分表, 加载简码表, 加载键位表, 生成表, 取码序列, 码位,
};
use crate::handfeel;

const 最大码长: usize = 4;

fn 码位名(位: 码位) -> &'static str {
    match 位 {
        码位::A => "A 首码",
        码位::S => "S 次码",
        码位::Y => "Y 末码",
    }
}

// -------------------------------------------------------------------------
// 流水线
// -------------------------------------------------------------------------

/// 打印若干字的取码链路，返回 Markdown
pub fn 流水线(
    keymap: &str,
    chaifen: &str,
    simple: Option<&str>,
    字们: &str,
    输出: Option<&str>,
) {
    let 键位 = 加载键位表(keymap);
    let 拆分 = 加载拆分表(chaifen);
    let 简码 = simple.map(加载简码表).unwrap_or_default();

    let 要查: Vec<&str> = 字们.split_whitespace().collect();
    let mut 索引: HashMap<&str, &Vec<String>> = HashMap::new();
    for (字, 根, _) in &拆分 {
        索引.insert(字.as_str(), 根);
    }

    let mut md = String::from("## 拆分流水线\n\n");
    md.push_str(&format!(
        "字根 {} 个，拆分条目 {} 条，简码 {} 条，最大码长 {}。\n",
        键位.len(),
        拆分.len(),
        简码.len(),
        最大码长
    ));

    for 字 in 要查 {
        md.push_str(&format!("\n### {}\n\n", 字));
        let Some(根) = 索引.get(*字).copied() else {
            md.push_str("拆分表里没有这个字。\n");
            continue;
        };
        md.push_str(&format!("**拆分**　{}\n\n", 根.join(" · ")));
        if 根.is_empty() {
            continue;
        }

        let 序列 = 取码序列(根, &键位, 最大码长);
        md.push_str("| 步 | 字根 | 码位 | 键位串 | 取第几键 | 得到 |\n");
        md.push_str("|---:|---|---|---|---:|---|\n");
        let mut 码 = String::new();
        let mut 缺 = false;
        for (i, (根名, 位)) in 序列.iter().enumerate() {
            match 键位.get(*根名) {
                None => {
                    md.push_str(&format!(
                        "| {} | {} | {} | — | — | 缺字根 |\n",
                        i + 1,
                        根名,
                        码位名(*位)
                    ));
                    缺 = true;
                    break;
                }
                Some(键) => {
                    if 键.is_empty() {
                        缺 = true;
                        break;
                    }
                    let 下标 = 位.序号().min(键.len() - 1);
                    let 字母 = 键[下标].to_ascii_lowercase();
                    码.push(字母);
                    let 串: String = 键.iter().collect();
                    md.push_str(&format!(
                        "| {} | {} | {} | `{}` | {} | `{}` |\n",
                        i + 1,
                        根名,
                        码位名(*位),
                        串,
                        下标 + 1,
                        字母
                    ));
                }
            }
        }
        if 缺 {
            continue;
        }

        md.push_str(&format!("\n**全码**　`{}`\n\n", 码));
        match 简码.get(*字) {
            Some(简) if 码.starts_with(简.as_str()) => {
                md.push_str(&format!("**简码**　`{}`（是全码前缀 ✓）\n", 简))
            }
            Some(简) => md.push_str(&format!("**简码**　`{}`（不是全码前缀，会被丢弃 ✗）\n", 简)),
            None => md.push_str("**简码**　无\n"),
        }
    }

    println!("{md}");
    if let Some(路径) = 输出 {
        match std::fs::write(路径, &md) {
            Ok(_) => println!("流水线已写入 {}", 路径),
            Err(e) => eprintln!("⚠️ 写不了流水线: {}", e),
        }
    }
}

// -------------------------------------------------------------------------
// 归并
// -------------------------------------------------------------------------

/// 一条归并规则
enum 规则 {
    整体 { 从: String, 到: String },
    码位 { 从: String, 从位: usize, 到: String, 到位: usize },
}

fn 解析规则(文: &str) -> Result<规则, String> {
    let (左, 右) = 文
        .split_once('=')
        .ok_or_else(|| format!("缺少等号：{}", 文))?;
    let 左列: Vec<&str> = 左.split_whitespace().collect();
    let 右列: Vec<&str> = 右.split_whitespace().collect();
    match (左列.len(), 右列.len()) {
        (1, 1) => Ok(规则::整体 {
            从: 左列[0].to_string(),
            到: 右列[0].to_string(),
        }),
        (2, 2) => {
            let 从位 = 左列[1]
                .parse::<usize>()
                .map_err(|_| format!("码位不是数字：{}", 文))?;
            let 到位 = 右列[1]
                .parse::<usize>()
                .map_err(|_| format!("码位不是数字：{}", 文))?;
            if 从位 == 0 || 到位 == 0 {
                return Err(format!("码位从 1 起：{}", 文));
            }
            Ok(规则::码位 {
                从: 左列[0].to_string(),
                从位,
                到: 右列[0].to_string(),
                到位,
            })
        }
        _ => Err(format!("规则写法不对：{}", 文)),
    }
}

fn 应用规则(键位: &mut HashMap<String, Vec<char>>, 规则文: &str) -> Vec<String> {
    let mut 报错 = Vec::new();
    for 一条 in 规则文.split([';', '\n']) {
        let 一条 = 一条.trim();
        if 一条.is_empty() {
            continue;
        }
        let 规 = match 解析规则(一条) {
            Ok(r) => r,
            Err(e) => {
                报错.push(e);
                continue;
            }
        };
        match 规 {
            规则::整体 { 从, 到 } => {
                let Some(目标) = 键位.get(&到).cloned() else {
                    报错.push(format!("「{}」不在字根池里", 到));
                    continue;
                };
                if !键位.contains_key(&从) {
                    报错.push(format!("「{}」不在字根池里", 从));
                    continue;
                }
                键位.insert(从, 目标);
            }
            规则::码位 {
                从,
                从位,
                到,
                到位,
            } => {
                let Some(目标键) = 键位.get(&到).and_then(|v| v.get(到位 - 1)).copied() else {
                    报错.push(format!("「{}」没有第 {} 个键位", 到, 到位));
                    continue;
                };
                match 键位.get_mut(&从) {
                    None => 报错.push(format!("「{}」不在字根池里", 从)),
                    Some(键) => {
                        if 键.len() < 从位 {
                            报错.push(format!("「{}」没有第 {} 个键位", 从, 从位));
                        } else {
                            键[从位 - 1] = 目标键;
                        }
                    }
                }
            }
        }
    }
    报错
}

/// 按多组归并规则各生成一个变体，比较结构指标
pub fn 归并(
    keymap: &str,
    chaifen: &str,
    simple: Option<&str>,
    规则组: &[String],
    目录: &str,
    汇总: Option<&str>,
) {
    let 基线 = 加载键位表(keymap);
    let 拆分 = 加载拆分表(chaifen);
    let 简码 = simple.map(加载简码表).unwrap_or_default();

    let mut 变体们: Vec<(String, HashMap<String, Vec<char>>, Vec<String>)> = Vec::new();
    变体们.push(("基线".to_string(), 基线.clone(), Vec::new()));
    for (i, 规则文) in 规则组.iter().enumerate() {
        let mut 键位 = 基线.clone();
        let 报错 = 应用规则(&mut 键位, 规则文);
        变体们.push((format!("变体{}", i + 1), 键位, 报错));
    }

    let mut 摘要 = String::from("## 字根归并对比\n\n");
    摘要.push_str("只改键位，不重新拆分。阶重口径同宇浩侧：零阶精确 Σ(n−1)，一阶以上 Σ n²/2 ÷ 键数^阶。\n\n");
    摘要.push_str("| 变体 | 规则 | 全码选重 | 让全后 | 简码选重 | 0阶 | 1阶 | 2阶 | 3阶 |\n");
    摘要.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|\n");

    println!("=== 字根归并对比 ===");
    for (名, 键位, 报错) in &变体们 {
        let 规则说明 = if 名 == "基线" {
            "—".to_string()
        } else {
            规则组[名.trim_start_matches("变体").parse::<usize>().unwrap_or(1) - 1].replace('\n', " ")
        };

        let (文本, 全条, 简条, 缺根) = 生成表(键位, &拆分, &简码);
        let 子目录 = std::path::Path::new(目录).join(名);
        std::fs::create_dir_all(&子目录).ok();
        let 路径 = 子目录.join("全码表.txt");
        if let Err(e) = std::fs::write(&路径, &文本) {
            eprintln!("⚠️ 写不了 {}: {}", 路径.display(), e);
            continue;
        }

        match handfeel::算选重(&路径, &handfeel::默认区块) {
            Ok(s) => {
                摘要.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {:.0} | {:.2} | {:.2} | {:.2} |\n",
                    名, 规则说明, s.全码, s.全码让全, s.简码, s.阶重[0], s.阶重[1], s.阶重[2], s.阶重[3]
                ));
                println!(
                    "  {:<8} 全码选重 {:>7}  让全后 {:>7}  简码选重 {:>7}  用键 {}  条 {} / {} / 缺根 {}",
                    名, s.全码, s.全码让全, s.简码, s.用键数, 全条, 简条, 缺根
                );
            }
            Err(e) => eprintln!("  {} 统计失败: {}", 名, e),
        }

        for 错 in 报错 {
            println!("    ⚠️ {}", 错);
            摘要.push_str(&format!("\n> {} 有规则未生效：{}\n", 名, 错));
        }
    }

    if let Some(路径) = 汇总 {
        match std::fs::write(路径, &摘要) {
            Ok(_) => println!("归并对比已写入 {}", 路径),
            Err(e) => eprintln!("⚠️ 写不了归并对比: {}", e),
        }
    }
}
