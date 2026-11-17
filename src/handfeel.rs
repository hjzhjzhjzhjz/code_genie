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

    // ── 负载均衡指标 ──────────────────────────────────────────────
    //
    // 口径统一为**八指绝对击键量**（不按各指可用键数归一），
    // 用于横向比较不同方案「负载是否压在少数手指上」。
    //
    // 算法来源（按要求复用推荐库）：
    //   · 变异系数     ← Yata `methods::StDev`（总体标准差 / 均值）
    //   · 中位绝对偏差 ← Yata `methods::MedianAbsDev`（标准 MAD 定义）
    //   · 基尼系数     ← TheAlgorithms/Rust `searching::quick_select`（选择/排序）
    //   · 归一化熵     ← TheAlgorithms/Rust `math::cross_entropy_loss`（log 基础式）
    //   · 左右失衡     ← 本仓 `手()` 映射

    /// 八指负载的变异系数（σ/μ，%）。越小越均衡。
    ///
    /// 对照 Yata `StDev`：其 `peek()` 返回总体标准差（`k = 1/(n−1)` 的样本口径），
    /// 这里元素仅 8 个且是**全体**手指（非抽样），故取总体标准差 `/n`。
    pub fn 变异系数(&self) -> f64 {
        let n = 8.0;
        let 均值 = self.合计() / n;
        if 均值 <= 0.0 {
            return 0.0;
        }
        let 方差 = self
            .各指
            .iter()
            .map(|x| (x - 均值) * (x - 均值))
            .sum::<f64>()
            / n;
        方差.sqrt() / 均值 * 100.0
    }

    /// 八指负载的相对中位绝对偏差（MAD / 均值，%）。抗离群。
    ///
    /// 分子按标准 MAD 实现——绝对偏差的**中位数**，对单个极端手指负载不敏感；
    /// Yata `MedianAbsDev` 的 `peek()` 用的是**平均**绝对偏差（`Σ|x−med|/n`），
    /// 不抗离群，故此处不沿用。
    ///
    /// 分母取**均值**而非中位数：手指负载天然偏斜（食指重、小指轻），中位数偏低
    /// 会把比值放大，导致「最均衡的方案反而 MAD 最高」。改成与变异系数同分母后
    /// 两者可直接对照：**相对MAD 明显小于 CV** ⇔ 分散主要来自少数极端手指，
    /// 而非整体摊薄。
    pub fn 中位绝对偏差(&self) -> f64 {
        let mut 值: Vec<f64> = self.各指.to_vec();
        值.sort_by(f64::total_cmp);
        let 中位 = 中位数(&值);
        let 均值 = self.合计() / 8.0;
        if 均值 <= 0.0 {
            return 0.0;
        }
        let mut 偏差: Vec<f64> = 值.iter().map(|x| (x - 中位).abs()).collect();
        偏差.sort_by(f64::total_cmp);
        中位数(&偏差) / 均值 * 100.0
    }

    /// 八指负载的基尼系数（0=完全均匀，→1=极度集中）。
    ///
    /// 对照 TheAlgorithms/Rust `quick_select`：那里为求第 k 小做 O(n) 选择；
    /// 这里仅 8 个元素，稳定排序后按标准公式
    /// `G = 2·Σ(i·xᵢ)/(n·Σx) − (n+1)/n`（i 从 1 起）计算。
    pub fn 基尼(&self) -> f64 {
        let mut 值: Vec<f64> = self.各指.to_vec();
        值.sort_by(f64::total_cmp);
        let 和: f64 = 值.iter().sum();
        if 和 <= 0.0 {
            return 0.0;
        }
        let n = 值.len() as f64;
        let 加权: f64 = 值
            .iter()
            .enumerate()
            .map(|(i, x)| (i as f64 + 1.0) * x)
            .sum();
        (2.0 * 加权) / (n * 和) - (n + 1.0) / n
    }

    /// 八指负载的归一化香农熵（0..1，1=完全均匀）。
    ///
    /// 对照 TheAlgorithms/Rust `cross_entropy_loss` 与 `kl_divergence_loss`
    /// 的 log 基础式；库内无独立 entropy 实现，按标准定义组合。
    pub fn 归一化熵(&self) -> f64 {
        let 总 = self.合计();
        if 总 <= 0.0 {
            return 0.0;
        }
        let h: f64 = self
            .各指
            .iter()
            .filter(|x| **x > 0.0)
            .map(|x| {
                let p = x / 总;
                -p * p.ln()
            })
            .sum();
        h / 8.0_f64.ln()
    }

    /// 左右手负载失衡度（%）：`|左−右| / (左+右) × 100`。0 = 完全均衡。
    ///
    /// 与「左右手交替率」互补：交替率衡量**节奏**，本项衡量**工作量分配**。
    pub fn 左右失衡(&self) -> f64 {
        let 左: f64 = self.各指[0..4].iter().sum();
        let 右: f64 = self.各指[4..8].iter().sum();
        let 总 = 左 + 右;
        if 总 <= 0.0 {
            return 0.0;
        }
        (左 - 右).abs() / 总 * 100.0
    }
}

/// 升序切片的中位数（偶数取中间两值平均）。
fn 中位数(升序: &[f64]) -> f64 {
    let n = 升序.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        升序[n / 2]
    } else {
        (升序[n / 2 - 1] + 升序[n / 2]) / 2.0
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
        // 2024 edition 的 let chains：两个条件一次成型，省掉一层嵌套
        if let (Some(c), None) = (字.next(), 字.next())
            && let Some((指, 行)) = 指位(c)
        {
            各指[指 as usize] += 值;
            各行[行 as usize] += 值;
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
                分组.entry(c).or_default().push(右.trim().to_string());
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
    let 文件名 = "wordAbsoluteFrequencySC.json";
    let 候选 = [
        // 现在传入的是本仓 yuhao-data 平铺数据目录（无 public/data 中间层）
        yuhao目录.join(文件名),
        std::path::PathBuf::from("yuhao-data").join(文件名),
    ];
    候选.into_iter().find(|路| 路.is_file())
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
                分组.entry(c).or_default().push(右.trim().to_string());
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

// -------------------------------------------------------------------------
// 负载均衡指标测试
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn 造(各指: [f64; 8]) -> 指位分布 {
        指位分布 {
            各指,
            各行: [0.0; 3],
        }
    }

    #[test]
    fn 均匀负载各项归零() {
        let f = 造([10.0; 8]);
        assert!(f.变异系数().abs() < 1e-9);
        assert!(f.基尼().abs() < 1e-9);
        assert!(f.中位绝对偏差().abs() < 1e-9);
        assert!(f.左右失衡().abs() < 1e-9);
        assert!((f.归一化熵() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn 单指独占取极值() {
        let mut 各指 = [0.0; 8];
        各指[3] = 80.0;
        let f = 造(各指);
        // 基尼上限 (n-1)/n = 7/8
        assert!((f.基尼() - 7.0 / 8.0).abs() < 1e-9);
        // 只剩一个非零概率 → 熵为 0
        assert!(f.归一化熵().abs() < 1e-9);
        // 全在左手 → 失衡 100%
        assert!((f.左右失衡() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn 左右各半失衡为零() {
        let f = 造([10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0]);
        assert!(f.左右失衡().abs() < 1e-9);
    }

    #[test]
    fn mad对单个离群值不敏感() {
        let mut 离群 = [10.0; 8];
        离群[0] = 1000.0;
        let 坏 = 造(离群);
        // 中位数仍是 10，绝对偏差的中位数仍是 0
        assert!(坏.中位绝对偏差().abs() < 1e-9);
        // 但均值/标准差被拉爆
        assert!(坏.变异系数() > 100.0);
    }

    #[test]
    fn 中位数偶数取平均() {
        assert_eq!(中位数(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(中位数(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(中位数(&[]), 0.0);
    }

    #[test]
    fn 全零不除零() {
        let f = 造([0.0; 8]);
        assert_eq!(f.变异系数(), 0.0);
        assert_eq!(f.基尼(), 0.0);
        assert_eq!(f.中位绝对偏差(), 0.0);
        assert_eq!(f.归一化熵(), 0.0);
        assert_eq!(f.左右失衡(), 0.0);
    }
}
