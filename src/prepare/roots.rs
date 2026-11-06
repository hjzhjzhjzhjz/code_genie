// =========================================================================
// 🌱 prepare 步骤 5–6：字根读音、声韵码
// =========================================================================
//
// 这两步的 Perl 逻辑密度最高，注释里保留了原始表达式，方便对照。
// 特别提醒：`s/[^aeuio]//` 只删**一个**字符（没有 `/g`），它承担了
// 「非常用字根不取声母」的全部职责，漏掉就会让低频字根的声码带上辅音。
// =========================================================================

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use crate::prepare::names;
use crate::prepare::schema::方案;
use crate::prepare::text::{self, 数值字段};
use crate::prepare::上下文;

/// 步骤 5：从万象拼音词典取字根读音，生成 `字根读音.txt`
///
/// 排序 `LC_ALL=C sort -u -k1,1 -k3,3nr -k2,2`：字根升序、词频降序、读音升序，
/// 并按这三个键去重（同一个字根保留排序最靠前的那条读音）。
pub fn 步骤5_字根读音(上: &上下文) -> Result<(), String> {
    println!("(5) 从万象拼音词典 chars.dict.yaml 生成字根读音 字根读音.txt ...");
    let 根集: HashSet<String> = 上
        .读(names::字根频率)?
        .into_iter()
        .filter_map(|行| 行.split_whitespace().next().map(str::to_string))
        .collect();

    let mut 行集: Vec<(String, String, String)> = Vec::new();
    for 行 in 上.读("chars.dict.yaml")? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.is_empty() || !根集.contains(列[0]) {
            continue;
        }
        let 末 = 列[列.len() - 1];
        let 计数字段 = if 末.starts_with(|c: char| c.is_numeric()) {
            末.to_string()
        } else {
            "0".to_string()
        };

        let mut 拼音集: BTreeSet<String> = BTreeSet::new();
        for 字段 in &列[1..] {
            if let Some(拼音) = 拼音前缀(字段) {
                if 拼音 != "无" {
                    拼音集.insert(拼音);
                }
            }
        }

        for 拼音 in 拼音集 {
            if 排除读音(列[0], &拼音) {
                continue;
            }
            if 列[0] == "土" && 上.方案.土用杜音 {
                行集.push(("土".to_string(), "dù".to_string(), "0".to_string()));
            } else {
                行集.push((列[0].to_string(), 拼音, 计数字段.clone()));
            }
        }
    }

    行集.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| {
                数值字段(&b.2)
                    .partial_cmp(&数值字段(&a.2))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    行集.dedup_by(|a, b| a.0 == b.0 && 数值字段(&a.2) == 数值字段(&b.2) && a.1 == b.1);

    let mut 出 = String::with_capacity(行集.len() * 16);
    for (根, 拼音, 计数) in 行集 {
        出.push_str(&format!("{根}\t{拼音}\t{计数}\n"));
    }
    上.写(names::字根读音, &出)
}

/// `/^([^\d\s]+)/`：取前导的「非数字、非空白」串
fn 拼音前缀(字段: &str) -> Option<String> {
    let 取: String = 字段
        .chars()
        .take_while(|c| !c.is_numeric() && !c.is_whitespace())
        .collect();
    (!取.is_empty()).then_some(取)
}

/// 三条黑名单读音改用更低频的那条（車用 chē、糸用 sī、長/长用 cháng）
fn 排除读音(根: &str, 拼音: &str) -> bool {
    if 根 == "車" && 拼音 == "jū" {
        return true;
    }
    if 根 == "糸" && 拼音 == "mì" {
        return true;
    }
    (根 == "長" || 根 == "长") && 拼音.starts_with("zh")
}

/// 步骤 6：把读音折成「声母+韵腹」的声韵码表 `字根声韵.txt`
pub fn 步骤6_字根声韵码(上: &上下文) -> Result<(), String> {
    println!("(6) 从 字根读音.txt 修正并生成字根声码韵码表 字根声韵.txt ...");
    if 上.字根表 != names::字根声韵 {
        println!("    !!! 使用方案自行提供的字根声码韵码表 {} ...", 上.字根表);
        let 路径 = 上.路径(&上.字根表);
        let 已有 = std::fs::metadata(&路径)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if 已有 {
            println!(
                "    !!! 已存在 {}，跳过初始化，如果需要重新初始化，请清空它。",
                上.字根表
            );
            return Ok(());
        }
        println!("    !!! 使用宇浩灵明字根初始化 {} ...", 上.字根表);
        return 初始化灵明字根(上, &路径);
    }

    let mut 声韵码: HashMap<String, String> = HashMap::new();
    let mut 读音: HashMap<String, String> = HashMap::new();
    for 行 in 上.读(names::字根读音)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() < 2 || 声韵码.contains_key(列[0]) {
            continue;
        }
        读音.insert(列[0].to_string(), 列[1].to_string());

        let 归一 = 去调(列[1]);
        let 归一 = match 归一.as_str() {
            "er" => "0e".to_string(), // 与「二」归并
            "nv" => "nu".to_string(), // 女
            other => other.to_string(),
        };
        let Some((声母, 韵腹)) = 声韵(&归一) else {
            return Err(format!("Invalid pinyin: {行}"));
        };
        声韵码.insert(列[0].to_string(), format!("{声母}{韵腹}"));
    }

    // 常用字集（注意是 算码字集.txt，不是算码字集）
    let mut 常用: HashSet<String> = 上.读(names::算码字集)?.into_iter().collect();
    常用.insert("戸".to_string()); // 特殊处理，保持 hu 音

    let mut 出 = String::with_capacity(声韵码.len() * 16);
    let mut 表: Vec<(String, String, String)> = Vec::new();
    for 行 in 上.读(names::字根频率)? {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.is_empty() {
            continue;
        }
        let 根 = 列[0];
        let 码 = match 修正表(根, 上.方案) {
            Some(v) => v.to_string(),
            None => 声韵码.get(根).cloned().unwrap_or_else(|| "o".to_string()),
        };

        // 非常用字根不取声母：只删掉第一个非韵母字符
        let 码 = if 常用.contains(根) {
            码
        } else {
            删首个非韵母(&码)
        };

        // `OPTIMIZE_KEY_<声母>` 重映射（`OPTIMIZE_KEYS` 为空时条件恒成立）
        let 码 = 按优化键重映射(码, 上.方案);

        // 声韵码长度 > 1 才写读音，否则留空（行尾悬空 TAB）
        let 拼音 = if 码.chars().count() > 1 {
            读音.get(根).map(String::as_str).unwrap_or("")
        } else {
            ""
        };
        表.push((根.to_string(), 码, 拼音.to_string()));
    }

    // 对应 `LC_ALL=C sort -k2,2 -k1,1`：先按声韵码、再按字根（均为字节序）
    表.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    for (根, 码, 拼音) in &表 {
        出.push_str(&format!("{根}\t{码}\t{拼音}\n"));
    }
    上.写(names::字根声韵, &出)
}

/// `s/[^aeuio]//`：删掉第一个非韵母字符
fn 删首个非韵母(码: &str) -> String {
    if let Some(位置) = 码.find(|c: char| !"aeuio".contains(c)) {
        let 首 = &码[..位置];
        let 字: Vec<char> = 码[位置..].chars().collect();
        首.chars().chain(字[1..].iter().copied()).collect()
    } else {
        码.to_string()
    }
}

/// `s/^./$consonant/`：把首字符换成 `OPTIMIZE_KEY_<首字符>`
pub(crate) fn 按优化键重映射(码: String, 方案: &方案) -> String {
    let Some(首) = 码.chars().next() else {
        return 码;
    };
    if 方案.优化声母集.contains(首) {
        return 码;
    }
    match 方案.优化键(首) {
        Some(替) => format!("{替}{}", 码.chars().skip(1).collect::<String>()),
        None => 码,
    }
}

/// 声母 + 韵腹：`/^([^aeuio]).*?(?:[iu])?([aeuio])/`
pub(crate) fn 声韵(拼音: &str) -> Option<(char, char)> {
    let mut 字符 = 拼音.chars();
    let 声母 = 字符.next()?;
    if "aeuio".contains(声母) {
        return None;
    }
    let 余: Vec<char> = 字符.collect();
    // 非贪婪的 `.*?` 逐字符展开；同一位置上先试 `(?:[iu])?` 命中、再试直接命中韵腹
    for k in 0..余.len() {
        if "iu".contains(余[k]) && k + 1 < 余.len() && "aeuio".contains(余[k + 1]) {
            return Some((声母, 余[k + 1]));
        }
        if "aeuio".contains(余[k]) {
            return Some((声母, 余[k]));
        }
    }
    None
}

/// NFKD + 去 `\p{M}`：pinyin 数据里只会出现带调元音，逐个映射即可
pub(crate) fn 去调(拼音: &str) -> String {
    拼音
        .chars()
        .filter_map(|c| {
            if ('\u{300}'..='\u{36f}').contains(&c) {
                return None; // 组合用的附加符号
            }
            Some(match c {
                'ā' | 'á' | 'ǎ' | 'à' => 'a',
                'ē' | 'é' | 'ě' | 'è' | 'ê' => 'e',
                'ī' | 'í' | 'ǐ' | 'ì' => 'i',
                'ō' | 'ó' | 'ǒ' | 'ò' => 'o',
                'ū' | 'ú' | 'ǔ' | 'ù' | 'ü' => 'u',
                'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' => 'u',
                'ń' | 'ň' | 'ǹ' => 'n',
                'ḿ' => 'm',
                其他 => 其他,
            })
        })
        .collect()
}

/// 脚本里的 `%fixes` 硬编码表：数据错误的字根读音人工归并
pub(crate) fn 修正表(根: &str, 方案: &方案) -> Option<&'static str> {
    Some(match 根 {
        "冫" | "⺀" | "{飞右}" => "0e", // bi / o，与「二」归并
        "屮" => "ca",                   // ce，取 cao
        "丶" | "乀" => "da",            // zu / fu，取 dian
        "土" => {
            if 方案.土用杜音 {
                "du"
            } else {
                "tu"
            }
        }
        "口" => {
            if 方案.口用vou音 {
                "vo"
            } else {
                "ko"
            }
        }
        "朩" => "mu",        // 与「木」归并
        "丨" => "su",        // gu，取 shu
        "丆" | "巜" => "ca", // ha / ka，分别与厂、巛归并
        "乚" => "yi",
        "車" => "ce",                // ju，取 che
        "糸" => "si",                // mi，取 si
        "{丄丶}" => "sa",            // 与「丄」归并
        "リ" | "{周框}" => "ba",     // 与「丷」「勹」归并
        "⺊" => "bo",                // 与「卜」归并
        "{即左}" => "ge",            // 与「艮」归并
        "{荒下}" => "ca",            // 与「川」归并
        "ᅲ" => "ji",                  // 与「丌」归并
        "⺽" => "ju",                // 与「臼」归并
        "{奉下}" => "ka",            // 与「㐄」归并
        "{贏框}" => "lo",            // 取 luo
        "{曾中}" | "{横日}" => "ri", // 与「日」归并
        "龶" => "se",                // 取 sheng
        "{眉上}" | "{豕下}" => "si", // 与「尸」「豕」归并
        "⺶" => "ya",                // 与「羊」归并
        "ナ" => "zo",                // 取 zuo
        "𡗗" => "di",                // pe，取 di
        "丅" => "di",                // 与「丁」归并
        "ㄩ" => "ka",                // yu，取 kan
        "长" | "長" => "ca",         // za，取 chang
        "ㄗ" => "je",                // zi，与「卩」归并
        "艹" => "na",                // ca，与「卄」归并
        "彳" => "i",                 // ci，常用字但不取声母
        "亍" => "u",                 // cu，常用字但不取声母
        "咼" => "a",                 // ga，不取声母以与「骨」首笔分开
        "尢" => "o",                 // yo，不取声母以与「尤」首笔分开
        _ => return None,
    })
}

/// 方案自带字根表的初始化（对应 bash 里 `curl zigen-ling.csv | perl ...` 那段）
fn 初始化灵明字根(上: &上下文, 输出路径: &Path) -> Result<(), String> {
    let 内容 = 下载文本("https://shurufa.app/zigen-ling.csv")?;

    // yuhao-zigens.csv：第 3 列 → 第 2 列
    let mut 映射: HashMap<String, String> = HashMap::new();
    for (序号, 行) in 上.读("yuhao-zigens.csv")?.into_iter().enumerate() {
        if 序号 == 0 {
            continue;
        }
        let 列: Vec<&str> = 行.split(',').collect();
        if 列.len() < 3 {
            continue;
        }
        映射.insert(列[2].to_string(), 列[1].to_string());
    }
    映射.insert("竹".to_string(), "竹".to_string()); // 替代 ⺮

    let mut 行集: Vec<(String, String, String)> = Vec::new();
    for (序号, 行) in 内容.lines().enumerate() {
        if 序号 == 0 {
            continue;
        }
        let 列: Vec<&str> = 行.split(',').collect();
        if 列.len() < 2 {
            continue;
        }
        let 名 = 映射.get(列[0]).cloned().unwrap_or_default();
        let 韵: String = 列[1].chars().skip(1).collect();
        let 尾 = 列[2..].join(" ");
        行集.push((名.clone(), 韵.clone(), 尾.clone()));
        if 名 == "艹" {
            行集.push(("卄".to_string(), 韵, 尾));
        }
    }

    // `LC_ALL=C sort -u -k2,2 -k1,1`：韵升序、名升序、整行兜底，按 (韵, 名) 去重
    行集.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.2.cmp(&b.2))
    });
    行集.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);

    let mut 出 = String::with_capacity(行集.len() * 16);
    for (名, 韵, 尾) in 行集 {
        出.push_str(&format!("{名}\t{韵}\t{尾}\n"));
    }
    text::写文本(输出路径, &出)
}

fn 下载文本(网址: &str) -> Result<String, String> {
    let 结果 = Command::new("curl").args(["-s", 网址]).output();
    match 结果 {
        Ok(输出) if 输出.status.success() => {
            Ok(String::from_utf8_lossy(&输出.stdout).into_owned())
        }
        Ok(输出) => Err(format!(
            "下载 {网址} 失败（curl 退出码 {}）",
            输出.status.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!("无法执行 curl 下载 {网址}（{e}）")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 声韵取韵腹() {
        assert_eq!(声韵("zhang"), Some(('z', 'a')));
        assert_eq!(声韵("chuang"), Some(('c', 'a')));
        assert_eq!(声韵("liu"), Some(('l', 'u')));
        assert_eq!(声韵("nu"), Some(('n', 'u')));
        assert_eq!(声韵("ji"), Some(('j', 'i')));
        assert_eq!(声韵("er"), None); // 纯元音开头，靠 "0e" 兜底
    }

    #[test]
    fn 去调覆盖拼音带调元音() {
        assert_eq!(去调("zhǎng"), "zhang");
        assert_eq!(去调("nǚ"), "nu");
        assert_eq!(去调("lüe"), "lue");
        assert_eq!(去调("shù"), "shu");
    }

    #[test]
    fn 删首个非韵母只删一个() {
        assert_eq!(删首个非韵母("ca"), "a");
        assert_eq!(删首个非韵母("0e"), "e");
        assert_eq!(删首个非韵母("i"), "i");
        assert_eq!(删首个非韵母("ko"), "o");
    }

    #[test]
    fn 优化键重映射只换首字符() {
        assert_eq!(
            按优化键重映射("ki".into(), &crate::prepare::schema::魔灵),
            "ki"
        );
        assert_eq!(
            按优化键重映射("yi".into(), &crate::prepare::schema::魔灵),
            "ki"
        );
        assert_eq!(
            按优化键重映射("0e".into(), &crate::prepare::schema::魔灵),
            "we"
        );
        assert_eq!(
            按优化键重映射("a".into(), &crate::prepare::schema::魔灵),
            "a"
        );
    }
}
