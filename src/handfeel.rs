// =========================================================================
// 🖐 指位与连击统计
// =========================================================================
//
// 宇浩把「键位熱力」单独列了一节，但没折算进排序指标。这里补上：
//   · 指位分布 → 八指负荷、食指/小指占比、行分布
//   · 字际连击 → 同指连击率、左右手交替率（按词频加权，只算字母键）
//   · 字内连击 → 单个编码内部相邻两击同指的比例
// =========================================================================

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// 手指编号：0–3 左手小/无名/中指/食指，4–7 右手食指/中指/无名/小指，8 拇指
pub const 指名: [&str; 9] = [
    "左小指", "左无名", "左中指", "左食指", "右食指", "右中指", "右无名", "右小指", "拇指",
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
    空格: f64,
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

    pub fn 空格值(&self) -> f64 {
        self.空格
    }

    /// 拇指（空格）在总击键中的占比
    pub fn 空格占比(&self) -> f64 {
        自己占比(self.空格, self.合计() + self.空格)
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
    let 表 = 热.get(模式)?.as_object()?;
    let mut 各指 = [0.0f64; 8];
    let mut 各行 = [0.0f64; 3];
    let mut 空格 = 0.0;

    for (k, v) in 表 {
        if k == "更新時間" {
            continue;
        }
        let 值 = v.as_f64().unwrap_or(0.0);
        if k == "space" {
            空格 = 值;
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
    Some(指位分布 { 各指, 各行, 空格 })
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
    /// 参与统计的连击总权重
    pub 权重: f64,
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
                同指比: if 总 > 0 { 同 as f64 / 总 as f64 } else { 0.0 },
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
        权重: 权重和,
    })
}

/// 自动找词表：优先测评器数据目录，其次仓库内置
pub fn 找词表(yuhao目录: &Path) -> Option<std::path::PathBuf> {
    let 候选 = [
        yuhao目录.join("public").join("data").join("wordAbsoluteFrequencySC.json"),
        std::path::PathBuf::from("yuhao-data").join("wordAbsoluteFrequencySC.json"),
    ];
    for 路 in 候选 {
        if 路.is_file() {
            return Some(路);
        }
    }
    None
}
