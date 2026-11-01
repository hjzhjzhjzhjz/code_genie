// =========================================================================
// 🖐 指位与连击统计
// =========================================================================
//
// 宇浩把「键位熱力」单独列了一节，但没折算进排序指标。这里补上：
//   · 指位分布 → 八指负荷、食指/小指占比、行分布
//   · 字际连击 → 同指连击率、左右手交替率（按词频加权，只算字母键）
//   · 字内连击 → 单个编码内部相邻两击同指的比例
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

/// 手指编号：0–3 左手小/无名/中指/食指，4–7 右手食指/中指/无名/小指，8 拇指
pub const 指名: [&str; 9] = [
    "左小指",
    "左无名",
    "左中指",
    "左食指",
    "右食指",
    "右中指",
    "右无名",
    "右小指",
    "拇指",
];

pub const 行名: [&str; 3] = ["上排", "中排", "下排"];

/// 键位 → (手指, 行)
pub fn 指位(c: char) -> Option<(u8, u8)> {
    let 值 = match c {
        'q' => (0, 0),
        'a' => (0, 1),
        'z' => (0, 2),
        'w' => (1, 0),
        's' => (1, 1),
        'x' => (1, 2),
        'e' => (2, 0),
        'd' => (2, 1),
        'c' => (2, 2),
        'r' | 't' => (3, 0),
        'f' | 'g' => (3, 1),
        'v' | 'b' => (3, 2),
        'y' | 'u' => (4, 0),
        'h' | 'j' => (4, 1),
        'n' | 'm' => (4, 2),
        'i' => (5, 0),
        'k' => (5, 1),
        ',' => (5, 2),
        'o' => (6, 0),
        'l' => (6, 1),
        '.' => (6, 2),
        'p' => (7, 0),
        ';' => (7, 1),
        '/' => (7, 2),
        _ => return None,
    };
    Some(值)
}

/// 手别：0 左，1 右
fn 手(指: u8) -> u8 {
    if 指 <= 3 {
        0
    } else {
        1
    }
}

// -------------------------------------------------------------------------
// 指位分布（从测评 JSON 的键位热力算）
// -------------------------------------------------------------------------

pub struct 指位分布 {
    各指: [f64; 8],
    各行: [f64; 3],
}

impl 指位分布 {
    fn 合计(&self) -> f64 {
        self.各指.iter().sum::<f64>()
    }

    /// 食指负荷百分比
    pub fn 食指(&self) -> f64 {
        (self.各指[3] + self.各指[4]) / self.合计() * 100.0
    }

    /// 小指负荷百分比
    pub fn 小指(&self) -> f64 {
        (self.各指[0] + self.各指[7]) / self.合计() * 100.0
    }

    /// 中排占比
    pub fn 中排(&self) -> f64 {
        self.各行[1] / self.合计() * 100.0
    }

    /// 八指负荷标准差（越小越均衡）
    #[allow(dead_code)] // 观测辅助，当前未接入报告
    pub fn 标准差(&self) -> f64 {
        let 均 = self.合计() / 8.0;
        let 方: f64 = self.各指.iter().map(|x| (x - 均) * (x - 均)).sum::<f64>() / 8.0;
        方.sqrt()
    }

    /// 最重的手指编号
    pub fn 最重(&self) -> usize {
        let mut 位 = 0;
        for i in 1..8 {
            if self.各指[i] > self.各指[位] {
                位 = i;
            }
        }
        位
    }

    pub fn 指(&self, i: usize) -> f64 {
        self.各指[i]
    }

    pub fn 行(&self, i: usize) -> f64 {
        self.各行[i]
    }
}

fn 自己占比(部分: f64, 总: f64) -> f64 {
    if 总 > 0.0 {
        部分 / 总 * 100.0
    } else {
        0.0
    }
}

/// 从键位热力里抽指位分布。模式为 "全碼" 或 "簡碼"。
pub fn 从热力(热: &Value, 模式: &str) -> Option<指位分布> {
    // 传进来的可能是「測評結果」，也可能是「鍵位熱力」本身
    let 热 = 热.get("鍵位熱力").unwrap_or(热);
    let 表 = 热.get(模式)?.as_object()?;
    let mut 各指 = [0.0f64; 8];
    let mut 各行 = [0.0f64; 3];

    for (k, v) in 表 {
        if k == "更新時間" {
            continue;
        }
        let 值 = v.as_f64().unwrap_or(0.0);
        if k == "space" {
            // 空格不参与八指/三行分布统计，直接跳过
            continue;
        }
        let mut 字 = k.chars();
        if let (Some(c), None) = (字.next(), 字.next()) {
            if let Some((指, 行)) = 指位(c) {
                各指[指 as usize] += 值;
                各行[行 as usize] += 值;
            }
        }
    }

    if 各指.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    Some(指位分布 { 各指, 各行 })
}

// -------------------------------------------------------------------------
// 连击统计（码表 + 词频）
// -------------------------------------------------------------------------

pub struct 连击 {
    /// 同指连击率（%）
    pub 同指: f64,
    /// 换手率（%）
    pub 换手: f64,
    /// 字内同指率（%）
    pub 字内: f64,
}

struct 单字 {
    首: u8,
    尾: u8,
    /// 编码内部相邻两击同指的比例
    同指比: f64,
}

/// 算一份码表的连击指标。只统计字母键，不含空格。
pub fn 算连击(码表: &Path, 词表: &Path) -> Result<连击, String> {
    let 文本 = std::fs::read_to_string(码表).map_err(|e| format!("读不了码表: {}", e))?;

    // 一个字可能有多行（简码 + 全码），按字归组
    let mut 分组: HashMap<char, Vec<String>> = HashMap::new();
    for 行 in 文本.lines() {
        if let Some((左, 右)) = 行.split_once('\t') {
            let mut 字 = 左.chars();
            if let (Some(c), None) = (字.next(), 字.next()) {
                分组
                    .entry(c)
                    .or_insert_with(Vec::new)
                    .push(右.trim().to_string());
            }
        }
    }

    let mut 字表: HashMap<char, 单字> = HashMap::new();
    for (c, mut 列) in 分组 {
        列.sort_by_key(|s| s.len());
        let 最短 = &列[0];
        let 指: Vec<u8> = 最短.chars().filter_map(|x| 指位(x).map(|t| t.0)).collect();
        if 指.is_empty() {
            continue;
        }
        let mut 同 = 0usize;
        for i in 0..指.len().saturating_sub(1) {
            if 指[i] == 指[i + 1] {
                同 += 1;
            }
        }
        let 总 = 指.len().saturating_sub(1);
        字表.insert(
            c,
            单字 {
                首: 指[0],
                尾: *指.last().unwrap(),
                同指比: if 总 > 0 {
                    同 as f64 / 总 as f64
                } else {
                    0.0
                },
            },
        );
    }

    // 词频
    let 词文本 = std::fs::read_to_string(词表).map_err(|e| format!("读不了词表: {}", e))?;
    let 词: serde_json::Map<String, Value> =
        serde_json::from_str(&词文本).map_err(|e| format!("解析词表失败: {}", e))?;

    let mut 权重和 = 0.0;
    let mut 同指和 = 0.0;
    let mut 换手和 = 0.0;
    let mut 字频: HashMap<char, f64> = HashMap::new();

    for (k, v) in &词 {
        let 频 = v.as_f64().unwrap_or(0.0);
        if 频 <= 0.0 {
            continue;
        }
        let 字: Vec<char> = k.chars().collect();
        for c in &字 {
            *字频.entry(*c).or_insert(0.0) += 频;
        }
        for i in 0..字.len().saturating_sub(1) {
            let (a, b) = (字[i], 字[i + 1]);
            let (左字, 右字) = match (字表.get(&a), 字表.get(&b)) {
                (Some(x), Some(y)) => (x, y),
                _ => continue,
            };
            权重和 += 频;
            if 左字.尾 == 右字.首 {
                同指和 += 频;
            } else if 手(左字.尾) != 手(右字.首) {
                换手和 += 频;
            }
        }
    }

    let mut 分子 = 0.0;
    let mut 分母 = 0.0;
    for (c, 频) in &字频 {
        if let Some(x) = 字表.get(c) {
            分子 += 频 * x.同指比;
            分母 += 频;
        }
    }

    Ok(连击 {
        同指: 自己占比(同指和, 权重和),
        换手: 自己占比(换手和, 权重和),
        字内: 自己占比(分子, 分母),
    })
}

/// 自动找词表：优先测评器数据目录，其次仓库内置
pub fn 找词表(yuhao目录: &Path) -> Option<std::path::PathBuf> {
    let 候选 = [
        yuhao目录
            .join("public")
            .join("data")
            .join("wordAbsoluteFrequencySC.json"),
        std::path::PathBuf::from("yuhao-data").join("wordAbsoluteFrequencySC.json"),
    ];
    for 路 in 候选 {
        if 路.is_file() {
            return Some(路);
        }
    }
    None
}

// -------------------------------------------------------------------------
// 选重个数（全表口径）
// -------------------------------------------------------------------------

/// 口径与宇浩一致，已对 593/289、125/60、115/57、26/13 四组数校验：
///   全码表 = 每字取最长码；简码表 = 每字取最短码（没有简码的字保留其全码）
///   重码字数 = 撞码桶里的字总数；重码组数 = 撞码的桶数
///   选重个数 = 重码字数 − 重码组数（首选字直接上屏，其余各多敲一次选重键）
/// 宇浩认的 CJK 区段（Unicode 17.0）。运行时会优先读 yuhao 自带的
/// cjkBlocks.json，读不到再用这里的兜底值。
pub const 默认区块: [(u32, u32); 11] = [
    (0x4E00, 0x9FFF),
    (0x3400, 0x4DBF),
    (0x20000, 0x2A6DF),
    (0x2A700, 0x2B73F),
    (0x2B740, 0x2B81D),
    (0x2B820, 0x2CEAD),
    (0x2CEB0, 0x2EBE0),
    (0x30000, 0x3134A),
    (0x31350, 0x323AF),
    (0x323B0, 0x33479),
    (0x2EBF0, 0x2EE5D),
];

fn 在区块(c: char, 区块: &[(u32, u32)]) -> bool {
    let o = c as u32;
    区块.iter().any(|(a, z)| o >= *a && o <= *z)
}

fn 解十六(s: &str) -> Option<u32> {
    let t = s.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    u32::from_str_radix(t, 16).ok()
}

/// 读宇浩的 cjkBlocks.json，拿到它自己用的区段边界
pub fn 读区块(路径: &Path) -> Option<Vec<(u32, u32)>> {
    let 文本 = std::fs::read_to_string(路径).ok()?;
    let 值: Value = serde_json::from_str(&文本).ok()?;
    let 块 = 值.get("blocks")?.as_object()?;
    let mut 结果 = Vec::new();
    for (_, b) in 块 {
        let a = 解十六(b.get("start")?.as_str()?)?;
        let z = 解十六(b.get("end")?.as_str()?)?;
        结果.push((a, z));
    }
    if 结果.is_empty() {
        None
    } else {
        Some(结果)
    }
}

pub struct 选重统计 {
    pub 全码: u64,
    pub 简码: u64,
    /// 出简让全：出简字不占全码位时，全码空间剩下的选重个数
    pub 全码让全: u64,
    /// 0–3 阶重。零阶精确 Σ(n−1)，一阶以上 Σ n²/2 ÷ 键数^阶
    pub 阶重: [f64; 4],
    /// 方案实际用到的键数
    pub 用键数: u32,
}

/// 算一份码表在全表范围内的选重个数
pub fn 算选重(码表: &Path, 区块: &[(u32, u32)]) -> Result<选重统计, String> {
    let 文本 = std::fs::read_to_string(码表).map_err(|e| format!("读不了码表: {}", e))?;

    let mut 分组: HashMap<char, Vec<String>> = HashMap::new();
    for 行 in 文本.lines() {
        if let Some((左, 右)) = 行.split_once('\t') {
            let mut 字 = 左.chars();
            if let (Some(c), None) = (字.next(), 字.next()) {
                // 只统计宇浩认的 CJK 区段，否则和网页版对不上
                if !在区块(c, 区块) {
                    continue;
                }
                分组
                    .entry(c)
                    .or_insert_with(Vec::new)
                    .push(右.trim().to_string());
            }
        }
    }

    let mut 全桶: HashMap<String, u64> = HashMap::new();
    let mut 简桶: HashMap<String, u64> = HashMap::new();
    // 出简让全：出简字放弃全码位，全码桶里只剩没有简码的字
    let mut 让全桶: HashMap<String, u64> = HashMap::new();
    let mut 键集: HashSet<char> = HashSet::new();
    let mut 最长长度 = 0usize;

    for (_, mut 列) in 分组 {
        列.sort_by_key(|s| s.len());
        let 最长 = 列.last().unwrap().clone();
        let 最短 = 列.first().unwrap().clone();
        let 有简 = 列.len() > 1;

        for c in 最长.chars() {
            if c != '_' {
                键集.insert(c);
            }
        }
        最长长度 = 最长长度.max(最长.chars().count());

        *全桶.entry(最长.clone()).or_insert(0) += 1;
        if !有简 {
            *让全桶.entry(最长).or_insert(0) += 1;
        }
        *简桶.entry(最短).or_insert(0) += 1;
    }

    let 汇总 = |桶: &HashMap<String, u64>| -> (u64, u64) {
        let mut 组 = 0u64;
        let mut 字 = 0u64;
        for n in 桶.values() {
            if *n > 1 {
                组 += 1;
                字 += *n;
            }
        }
        (组, 字)
    };

    let (全组, 全字) = 汇总(&全桶);
    let (简组, 简字) = 汇总(&简桶);
    let (让全组, 让全字) = 汇总(&让全桶);

    // 阶重：按全码前 (最长长度 − 阶) 位分桶，短码用 ε 补位
    let 键数 = 键集.len().max(1) as f64;
    let mut 阶重 = [0.0f64; 4];
    for 阶 in 0..4usize {
        if 阶 >= 最长长度 {
            break;
        }
        let 保留 = 最长长度 - 阶;
        let mut 桶: HashMap<String, u64> = HashMap::new();
        for (码, 数) in &全桶 {
            let 前缀: String = 码
                .chars()
                .chain(std::iter::repeat('ε'))
                .take(保留)
                .collect();
            *桶.entry(前缀).or_insert(0) += *数;
        }
        if 阶 == 0 {
            阶重[0] = 桶
                .values()
                .filter(|n| **n > 1)
                .map(|n| (*n - 1) as f64)
                .sum();
        } else {
            let 空间 = 键数.powi(阶 as i32);
            阶重[阶] = 桶
                .values()
                .map(|n| {
                    let f = *n as f64;
                    f * f / 2.0 / 空间
                })
                .sum();
        }
    }

    Ok(选重统计 {
        全码: 全字 - 全组,
        简码: 简字 - 简组,
        全码让全: 让全字 - 让全组,
        阶重,
        用键数: 键集.len() as u32,
    })
}
