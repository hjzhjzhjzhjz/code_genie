// =========================================================================
// 📦 主上下文
// =========================================================================

use std::collections::{HashMap, HashSet};

use crate::config::{ConstraintsConfig, TargetsConfig};
use crate::types::{
    char_to_key_index, CharInfo, KeyDistConfig, RootGroup, ScaleConfig, WeightConfig,
    EQUIV_TABLE_SIZE, GROUP_MARKER, KEY_SPACE,
};

/// 等价表类型别名
pub type EquivTable = [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

/// 优化上下文 - 存储所有算法需要的数据
pub struct OptContext {
    /// 权重配置
    pub weights: WeightConfig,
    /// 键位显示顺序（用于「用指分布」等报告输出，取自 cfg.keys.display_order）
    pub display_order: String,
    /// 字根组数量
    pub num_groups: usize,
    /// 字根名到组索引的映射
    pub root_to_group: HashMap<String, usize>,
    /// 组索引到使用该组的汉字索引列表的映射
    pub group_to_chars: Vec<Vec<usize>>,
    /// 汉字信息列表
    pub char_infos: Vec<CharInfo>,
    /// 原始拆分数据 (字符, 根名列表, 频率)
    pub raw_splits: Vec<(char, Vec<String>, u64)>,
    /// 字根组列表
    pub groups: Vec<RootGroup>,
    /// 固定字根映射
    pub fixed_roots: HashMap<String, u8>,
    /// 当量表
    pub equiv_table: EquivTable,
    /// 全码终止键掩码：末键属于这些键时不追加虚拟空格当量。
    pub termination_key_mask: [bool; EQUIV_TABLE_SIZE],
    /// 键位分布配置
    pub key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    /// 总频率
    pub total_frequency: u64,
    /// 编码基数
    pub code_base: usize,
    /// 最大码长
    pub max_parts: usize,
    /// 编码空间大小
    pub code_space: usize,
    /// 缩放配置
    pub scale_config: ScaleConfig,
    /// 每个组的加权频率总和（用于 key_weighted_usage 的 O(1) 更新）
    pub group_freq_sum: Vec<f64>,
    /// 目标配置（用于目标偏差评分和 _max 硬约束检查）
    pub targets_config: TargetsConfig,
    /// 结构约束（编码种数上限等）；默认不启用
    pub constraints: ConstraintsConfig,
}

impl OptContext {
    /// 创建优化上下文。
    ///
    /// 生产路径走 [`OptContext::new_with_termination`]；本入口供测试直接构造（全码终止键默认 `_`）。
    #[allow(dead_code)]
    // 构造参数就是上下文各配置块本身；打包成参数结构体只会把同样的字段再抄一遍，
    // 且波及全部调用点（含大量测试），收益不抵改动。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: EquivTable,
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        weights: WeightConfig,
        targets_config: TargetsConfig,
    ) -> Self {
        Self::new_with_termination(
            splits,
            fixed_roots,
            groups,
            equiv_table,
            key_dist_config,
            scale_config,
            weights,
            targets_config,
            "_",
            ConstraintsConfig::default(),
        )
    }

    /// 创建优化上下文（含全码终止键配置与结构约束）。
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_termination(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: EquivTable,
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        weights: WeightConfig,
        targets_config: TargetsConfig,
        termination_keys: &str,
        constraints: ConstraintsConfig,
    ) -> Self {
        let mut termination_key_mask = [false; EQUIV_TABLE_SIZE];
        for c in termination_keys.chars() {
            if let Some(k) = char_to_key_index(c) {
                termination_key_mask[k] = true;
            }
        }
        let mut root_to_group: HashMap<String, usize> = HashMap::new();
        for (gi, g) in groups.iter().enumerate() {
            for r in &g.roots {
                root_to_group.insert(r.clone(), gi);
            }
        }

        let num_groups = groups.len();
        let mut group_to_chars = vec![Vec::new(); num_groups];
        let mut char_infos = Vec::with_capacity(splits.len());
        let mut total_frequency = 0u64;
        let mut max_parts = 0usize;

        for (ci, (_, roots, freq)) in splits.iter().enumerate() {
            let mut info = CharInfo {
                parts: crate::types::Parts::new(),
                frequency: *freq,
            };

            let mut seen_groups = HashSet::new();

            for root in roots {
                if let Some(&key) = fixed_roots.get(root) {
                    info.parts.push(key as u16);
                } else if let Some(&gi) = root_to_group.get(root) {
                    info.parts.push(gi as u16 + GROUP_MARKER);
                    seen_groups.insert(gi);
                }
            }

            if info.parts.len() > max_parts {
                max_parts = info.parts.len();
            }

            for &gi in &seen_groups {
                group_to_chars[gi].push(ci);
            }

            total_frequency += freq;

            char_infos.push(info);
        }

        let code_base = EQUIV_TABLE_SIZE + 1;
        let code_space = crate::types::pow_base(code_base, max_parts);

        // u32 编码键约束（需求 6.2/6.3）：BucketStore 以 u32 表示编码值与稀疏键。
        // code_space = code_base^max_parts，max_parts ≤ 6 时 < u32::MAX；≥ 7 溢出，构建期报错终止
        // 而非静默截断。规模数字（code_base/max_parts/code_space）均运行时计算（需求 12.6）。
        assert!(
            code_space <= u32::MAX as usize,
            "编码空间 code_space = code_base^max_parts = {code_base}^{max_parts} = {code_space} \
             超出 u32 表示范围（{}）。请减小 max_parts（当前 {max_parts}）使 code_base^max_parts ≤ u32::MAX。",
            u32::MAX
        );

        // 预计算每个组的加权频率总和
        // group_freq_sum[r] = sum of freq_f for each (ci, part) where part references group r
        let mut group_freq_sum = vec![0.0f64; num_groups];
        for ci in 0..char_infos.len() {
            let freq_f = char_infos[ci].frequency as f64;
            for &p in &char_infos[ci].parts {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    group_freq_sum[gi] += freq_f;
                }
            }
        }

        Self {
            weights,
            display_order: "qwertyuiopasdfghjklzxcvbnm".to_string(),
            num_groups,
            root_to_group,
            group_to_chars,
            char_infos,
            raw_splits: splits.to_vec(),
            groups: groups.to_vec(),
            fixed_roots: fixed_roots.clone(),
            equiv_table,
            termination_key_mask,
            key_dist_config,
            total_frequency,
            code_base,
            max_parts,
            code_space,
            scale_config,
            group_freq_sum,
            targets_config,
            constraints,
        }
    }

    /// 解析键位 - 将部分索引解析为实际键位
    #[inline(always)]
    pub fn resolve_key(&self, part: u16, assignment: &[u8]) -> u8 {
        if part >= GROUP_MARKER {
            assignment[(part - GROUP_MARKER) as usize]
        } else {
            part as u8
        }
    }

    /// 计算仅全码
    #[inline(always)]
    pub fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in &info.parts {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 从拆分计算全码等价值。返回 `None` 表示没有有效按键对，不参与当量统计。
    #[inline(always)]
    pub fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> Option<f64> {
        let info = &self.char_infos[ci];
        let n = info.parts.len();
        if n == 0 {
            return None;
        }

        let mut prev_key = self.resolve_key(info.parts[0], assignment) as usize;
        let mut total = 0.0;
        let mut pair_count = n.saturating_sub(1);

        for i in 1..n {
            let cur_key = self.resolve_key(info.parts[i], assignment) as usize;
            total += self.equiv_table[prev_key][cur_key];
            prev_key = cur_key;
        }

        if n < self.max_parts && prev_key != KEY_SPACE && !self.termination_key_mask[prev_key] {
            total += self.equiv_table[prev_key][KEY_SPACE];
            pair_count += 1;
        }

        if pair_count == 0 {
            None
        } else {
            Some(total / pair_count as f64)
        }
    }
}
