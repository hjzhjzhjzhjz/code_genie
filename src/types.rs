// =========================================================================
// 🚀 基础数据类型
// =========================================================================

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// 汉字拆分的字根列表内联容量。
///
/// `code_base = 32` 时 `max_parts ≤ 6`（`32^7 > u32::MAX`），故 6 足以覆盖所有合法规模；
/// 更小的 `code_base` 若出现更长的拆分，`SmallVec` 会自动溢出到堆（行为正确）。
/// 内联后 `parts` 与 `frequency` 同处 `CharInfo` 一条缓存行，`calc_code_only` /
/// `calc_equiv_from_parts` 的字根遍历不再额外解引用一次堆指针——退火热路径每字省一次缓存缺失。
pub type Parts = SmallVec<[u16; 6]>;

/// 空格键的键位索引（`_`）。注意它只是**一个键的索引**，不是键位数组的长度。
pub const KEY_SPACE: usize = 26;
/// 键位索引空间：`char_to_key_index` 的最大返回值 + 1（a-z 共 26 个，外加 `_ ; , . /` 5 个）。
///
/// 凡「按键位索引寻址」的数组都必须用这个长度。用 [`KEY_SPACE`] 当长度是历史遗留写法：
/// 允许键里一旦出现标点键，`k as usize` 会取到 26..=30，直接越界 panic
/// （release 配置 `panic = "abort"`，整轮退火无 checkpoint 直接终止）。
pub const KEY_INDEX_SPACE: usize = 31;
/// 当量表大小（与键位索引空间同宽：每个键都可能出现在末位转移里）
pub const EQUIV_TABLE_SIZE: usize = KEY_INDEX_SPACE;

// 编译期不变量：键位索引空间必须比空格键索引大（还要容纳 `; , . /`），当量表与之同宽。
const _: () = assert!(KEY_SPACE < KEY_INDEX_SPACE);
const _: () = assert!(EQUIV_TABLE_SIZE == KEY_INDEX_SPACE);
/// 分组标记起始值
pub const GROUP_MARKER: u16 = 1000;

/// 将字符转换为键位索引
/// - a-z: 0-25
/// - _: 26
/// - ;: 27
/// - ,: 28
/// - .: 29
/// - /: 30
pub fn char_to_key_index(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some((c as u8 - b'a') as usize),
        '_' => Some(KEY_SPACE),
        ';' => Some(27),
        ',' => Some(28),
        '.' => Some(29),
        '/' => Some(30),
        _ => None,
    }
}

/// 将键位索引转换为字符
pub fn key_to_char(key: u8) -> char {
    match key {
        0..=25 => (key + b'a') as char,
        26 => '_',
        27 => ';',
        28 => ',',
        29 => '.',
        30 => '/',
        _ => '?',
    }
}

/// 计算 base 的 exp 次方
pub fn pow_base(base: usize, exp: usize) -> usize {
    let mut result = 1;
    for _ in 0..exp {
        result *= base;
    }
    result
}

/// 权重配置 - 用于得分计算
#[derive(Clone, Copy, Debug)]
pub struct WeightConfig {
    pub weight_collision_count: f64,
    pub weight_collision_rate: f64,
    pub weight_equivalence: f64,
    pub weight_equiv_cv: f64,
    pub weight_distribution: f64,
}

impl Default for WeightConfig {
    fn default() -> Self {
        Self {
            weight_collision_count: 0.07,
            weight_collision_rate: 0.62,
            weight_equivalence: 0.2,
            weight_equiv_cv: 0.01,
            weight_distribution: 0.1,
        }
    }
}

/// 缩放配置 - 用于将不同量纲的指标归一化
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ScaleConfig {
    /// 重码数缩放因子
    pub collision_count: f64,
    /// 重码率缩放因子
    pub collision_rate: f64,
    /// 当量缩放因子
    pub equivalence: f64,
    /// 当量变异系数缩放因子
    pub equiv_cv: f64,
    /// 分布偏差缩放因子
    pub distribution: f64,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            collision_count: 1.0,
            collision_rate: 1.0,
            equivalence: 1.0,
            equiv_cv: 1.0,
            distribution: 1.0,
        }
    }
}

/// 键位分布配置
#[derive(Clone, Copy, Default)]
pub struct KeyDistConfig {
    /// 目标使用率 (%)
    pub target_rate: f64,
    /// 低于目标时的惩罚系数
    pub low_penalty: f64,
    /// 高于目标时的惩罚系数
    pub high_penalty: f64,
}

/// 汉字拆分信息
#[derive(Clone)]
pub struct CharInfo {
    /// 拆分后的字根列表（键位索引或分组标记）。内联小向量，热路径免一次堆解引用。
    pub parts: Parts,
    /// 使用频率
    pub frequency: u64,
}

/// 字根组 - 优化单元：同一组内的字根在退火中被当作整体分配到同一把键
#[derive(Clone)]
pub struct RootGroup {
    /// 字根列表
    pub roots: Vec<String>,
    /// 允许分配的键位
    pub allowed_keys: Vec<u8>,
}

/// 评估指标
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// 重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
    /// 平均当量
    pub equiv_mean: f64,
    /// 当量变异系数
    pub equiv_cv: f64,
    /// 分布偏差
    pub dist_deviation: f64,
    /// 编码种数（所有字根去重后的键位组合数）；未启用 `[constraints]` 时为 0。
    /// `serde(default)` 让旧 checkpoint 仍可读取。
    #[serde(default)]
    pub encoding_count: usize,
}

/// 全码各指标的分数分量（用于日志输出，避免在日志代码中重复算分公式）
#[derive(Clone, Copy, Default)]
pub struct MetricScores {
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
    pub total_full: f64,
}

/// 从名称中提取基础名（去掉数字后缀）
pub fn extract_base_name(name: &str) -> String {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..dot_pos].to_string();
        }
    }
    name.to_string()
}

/// 从名称中提取数字后缀
pub fn extract_suffix_num(name: &str) -> i32 {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if let Ok(n) = suffix.parse::<i32>() {
            return n;
        }
    }
    -1
}

#[cfg(test)]
mod key_index_tests {
    use super::*;

    /// 键位索引空间必须覆盖所有合法键：`KEY_SPACE`(26) 只是空格键的索引，
    /// 拿它当数组长度会让 `_ ; , . /`（26..=30）越界。
    #[test]
    fn 标点键索引落在键位空间内() {
        for c in ['a', 'm', 'z', '_', ';', ',', '.', '/'] {
            let i = char_to_key_index(c).expect("应为合法键");
            assert!(
                i < KEY_INDEX_SPACE,
                "键 '{c}' 的索引 {i} 超出 KEY_INDEX_SPACE={KEY_INDEX_SPACE}"
            );
        }
        assert_eq!(char_to_key_index('/'), Some(KEY_INDEX_SPACE - 1));
    }
}
