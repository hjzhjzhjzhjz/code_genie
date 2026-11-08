// =========================================================================
// ⚡ 评估器
// =========================================================================

use rand::prelude::*;
use std::collections::HashMap;

use crate::bucket_store::{BucketStore, FullBucket};
use crate::context::OptContext;
use crate::types::{KeyDistConfig, MetricScores, Metrics, EQUIV_TABLE_SIZE};

/// 一个字根槽位的键位来源。
///
/// `字根键位表.txt` 的一行是「基名 + 该基名各槽位落到的键」，编码就是这些键拼成的串。
/// 固定字根（约束文件里一列只有一个键）直接给键位；其余字根属于某个根组，查该组的分配结果。
#[derive(Clone, Copy, Debug)]
enum 槽来源 {
    固定(u8),
    组(u32),
}

/// 预计算「基名 → 各槽位来源」，供编码种数统计使用。
///
/// 与 `output::build_root_encodings_sorted` 同口径（都按基名聚合、按槽位号排序），
/// 只是这里保留来源而不是当场解析键位，避免退火每步重建哈希表。
fn 构建根槽来源(ctx: &OptContext) -> (Vec<槽来源>, Vec<u32>) {
    use crate::types::{extract_base_name, extract_suffix_num};

    let mut 分组: HashMap<String, Vec<(i32, 槽来源)>> = HashMap::new();
    for (根, &键) in &ctx.fixed_roots {
        分组
            .entry(extract_base_name(根))
            .or_default()
            .push((extract_suffix_num(根), 槽来源::固定(键)));
    }
    for (gi, 组) in ctx.groups.iter().enumerate() {
        for 根 in &组.roots {
            分组
                .entry(extract_base_name(根))
                .or_default()
                .push((extract_suffix_num(根), 槽来源::组(gi as u32)));
        }
    }

    // 排序只为结果稳定（便于单测与重现），统计本身与顺序无关。
    let mut 基名们: Vec<String> = 分组.keys().cloned().collect();
    基名们.sort();

    let mut 来源: Vec<槽来源> = Vec::new();
    let mut 偏移: Vec<u32> = Vec::with_capacity(基名们.len() + 1);
    偏移.push(0);
    for 名 in &基名们 {
        let mut 槽们 = 分组.remove(名).unwrap_or_default();
        槽们.sort_by_key(|(号, _)| *号);
        for (_, s) in 槽们 {
            来源.push(s);
        }
        偏移.push(来源.len() as u32);
    }
    (来源, 偏移)
}

// =========================================================================
// 主评估器
// =========================================================================

/// 输出桶存储后端选择日志（需求 12）：在配置确认阶段单线程调用一次。
///
/// 全部规模数字（code_base / max_parts / code_space / 各级 code_base^L / n_chars / 预估内存）
/// 均在运行时据 `ctx` 与类型大小计算，除 `SPARSE_THRESHOLD` 外无任何硬编码规模常量（需求 12.6）。
pub(crate) fn log_bucket_backends(ctx: &OptContext) {
    use crate::bucket_store::SPARSE_THRESHOLD;
    let n_chars = ctx.char_infos.len();
    println!(
        "📦 桶存储后端选择 (code_base={}, max_parts={}, code_space={}, n_chars={}, 容量绝对上限={}):",
        ctx.code_base, ctx.max_parts, ctx.code_space, n_chars, SPARSE_THRESHOLD
    );
    log_one_backend(
        "全码桶",
        ctx.code_space,
        std::mem::size_of::<FullBucket>(),
        n_chars,
    );
}

/// 单个桶存储的后端选择日志行（需求 12.2/12.3/12.4）。
fn log_one_backend(label: &str, capacity: usize, elem_size: usize, n_chars: usize) {
    println!("{}", backend_log_line(label, capacity, elem_size, n_chars));
}

/// 构造单个桶存储后端选择日志行（纯函数，便于测试，需求 12.6）。
///
/// 与生产路径同判据（[`choose_backend_for`]）：容量上界、Dense 绝对占用、`n_chars` 三者联合，
/// 而不是只看容量或只看占用率——只看容量会让「4 码定长方案」预分配 48 MB，
/// 只看占用率会让「3 码方案」误走稀疏、主循环慢约三分之一。
pub(crate) fn backend_log_line(
    label: &str,
    capacity: usize,
    elem_size: usize,
    n_chars: usize,
) -> String {
    use crate::bucket_store::{
        choose_backend_detailed, BackendKind, BackendReason, DENSE_BUDGET_BYTES,
        SPARSE_ENTRY_FACTOR, SPARSE_THRESHOLD,
    };
    let mb = capacity.saturating_mul(elem_size) / (1024 * 1024);
    let budget_mb = DENSE_BUDGET_BYTES / (1024 * 1024);
    let sparse_bound = n_chars.saturating_mul(SPARSE_ENTRY_FACTOR);
    let (kind, reason) = choose_backend_detailed(capacity, n_chars, elem_size);
    match kind {
        BackendKind::Dense => {
            let why = match reason {
                BackendReason::WithinDenseBudget => {
                    format!("{capacity} 槽 × {elem_size} B ≈ {mb} MB/线程，在 {budget_mb} MB 预算内")
                }
                _ => format!(
                    "占用 ≈ {mb} MB/线程虽超 {budget_mb} MB 预算，但非空桶上界 \
                     n_chars({n_chars})×{SPARSE_ENTRY_FACTOR} = {sparse_bound} 与槽数相当，稀疏省不了内存"
                ),
            };
            format!("   · {label}: Dense ({why}；数组索引比哈希查找快)")
        }
        BackendKind::Sparse => {
            let why = if reason == BackendReason::OverAbsoluteLimit {
                format!(
                    "容量 {capacity} 超绝对上限 {SPARSE_THRESHOLD}（非空桶上界 \
                     n_chars({n_chars})×{SPARSE_ENTRY_FACTOR} = {sparse_bound} 远小于容量）"
                )
            } else {
                format!(
                    "占用 ≈ {mb} MB/线程 超 {budget_mb} MB 预算，且容量 {capacity} 远超非空桶上界 \
                     n_chars({n_chars})×{SPARSE_ENTRY_FACTOR} = {sparse_bound}"
                )
            };
            format!("   · {label}: Sparse ({why}；内存随非空桶数增长)")
        }
    }
}

/// 主评估器 - 评估整个编码方案
pub struct Evaluator {
    /// 当前编码列表
    current_codes: Vec<usize>,
    /// 当前等价值列表；`None` 表示该字没有有效按键对。
    current_equiv_val: Vec<Option<f64>>,
    /// 全码桶存储（取代基线的 code_to_chars/bucket_freq_sum/bucket_max_freq/bucket_first
    /// 四个 code_space 大小数组）。密集/稀疏后端按 code_space 阈值自适应（需求 1/5）。
    full_buckets: BucketStore<FullBucket>,
    /// 每个汉字在其桶中的位置（用于 O(1) swap_remove）
    char_bucket_pos: Vec<usize>,
    /// 总重码数
    total_collisions: usize,
    /// 重码频率
    collision_frequency: u64,

    /// 加权等价值总和
    total_equiv_weighted: f64,
    /// 加权等价值平方总和（仅统计有有效按键对的字）
    total_equiv_sq_weighted: f64,
    /// 有效当量字频分母
    equiv_frequency: u64,

    /// 键位加权使用统计
    pub key_weighted_usage: [f64; EQUIV_TABLE_SIZE],

    /// 总频率倒数
    pub inv_total_frequency: f64,
    /// 总键击次数倒数
    pub inv_total_key_presses: f64,

    /// 缓存的得分
    pub cached_score: f64,
    /// 得分是否需要重新计算
    pub score_dirty: bool,

    /// 缓存的全码分量得分
    pub cached_full_score: f64,
    /// 全码分量得分是否需要重新计算
    pub full_score_dirty: bool,

    /// 各基名的槽位来源（按基名连续存放）
    根槽来源: Vec<槽来源>,
    /// 每个基名的槽位区间起点，长度 = 基名数 + 1
    根槽偏移: Vec<u32>,
    /// 复用缓冲：各基名的编码指纹，排序去重后即编码种数
    编码指纹缓冲: Vec<u64>,
    /// 当前编码种数（字根键位组合去重后的个数）
    编码种数: usize,
}

impl Evaluator {
    /// 创建新的评估器。
    pub fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        let n = ctx.char_infos.len();
        let cs = ctx.code_space;
        let (根槽来源, 根槽偏移) = 构建根槽来源(ctx);
        let mut full_buckets: BucketStore<FullBucket> = BucketStore::new(cs, n);
        // 稀疏后端预留 n_chars 容量：避免退火期 rehash、降低负载因子（缩短探测、减少缓存缺失）。
        full_buckets.reserve_nonempty(n);
        let mut char_bucket_pos = vec![0usize; n];
        let mut current_codes = Vec::with_capacity(n);
        let mut current_equiv_val = Vec::with_capacity(n);

        let mut total_equiv_weighted = 0.0f64;
        let mut total_equiv_sq_weighted = 0.0f64;
        let mut equiv_frequency = 0u64;
        let mut key_weighted_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for ci in 0..n {
            let info = &ctx.char_infos[ci];
            let freq_f = info.frequency as f64;

            let code = ctx.calc_code_only(ci, assignment);
            let equiv = ctx.calc_equiv_from_parts(ci, assignment);

            current_codes.push(code);
            current_equiv_val.push(equiv);

            let b = full_buckets.get_mut_or_insert(code as u32);
            let pos = b.members.len();
            b.members.push(ci as u32);
            char_bucket_pos[ci] = pos;
            b.freq_sum += info.frequency;
            if info.frequency > b.max_freq {
                b.max_freq = info.frequency;
            }

            if let Some(eq) = equiv {
                total_equiv_weighted += eq * freq_f;
                total_equiv_sq_weighted += eq * eq * freq_f;
                equiv_frequency += info.frequency;
            }

            for &p in &info.parts {
                let k = ctx.resolve_key(p, assignment) as usize;
                key_weighted_usage[k] += freq_f;
            }
            total_key_presses += freq_f * info.parts.len() as f64;
        }

        // 碰撞统计：仅遍历非空桶（需求 7.1）。
        let mut total_collisions = 0usize;
        let mut collision_frequency = 0u64;
        full_buckets.for_each_nonempty(|_code, b| {
            let cnt = b.members.len();
            if cnt >= 2 {
                total_collisions += cnt - 1;
                collision_frequency += b.freq_sum - b.max_freq;
            }
        });

        let inv_tf = if ctx.total_frequency > 0 {
            1.0 / ctx.total_frequency as f64
        } else {
            0.0
        };
        let inv_tkp = if total_key_presses > 0.0 {
            1.0 / total_key_presses
        } else {
            0.0
        };

        let mut e = Self {
            current_codes,
            current_equiv_val,
            full_buckets,
            char_bucket_pos,
            total_collisions,
            collision_frequency,
            total_equiv_weighted,
            total_equiv_sq_weighted,
            equiv_frequency,
            key_weighted_usage,
            inv_total_frequency: inv_tf,
            inv_total_key_presses: inv_tkp,
            cached_score: 0.0,
            score_dirty: true,
            cached_full_score: 0.0,
            full_score_dirty: true,
            根槽来源,
            根槽偏移,
            编码指纹缓冲: Vec::new(),
            编码种数: 0,
        };
        e.刷新编码种数(ctx, assignment);
        e.cached_full_score = e.compute_full_score(ctx);
        e.full_score_dirty = false;
        e.cached_score = e.cached_full_score;
        e.score_dirty = false;
        e
    }

    /// 重新扫描桶成员的最大频率。
    ///
    /// 仅供重码频率统计用（`max_freq`），不跟踪首选字。
    #[inline]
    fn rescan_bucket_max(ctx: &OptContext, members: &[u32]) -> u64 {
        let mut max_f = 0u64;
        for &ci in members {
            let f = ctx.char_infos[ci as usize].frequency;
            if f > max_f {
                max_f = f;
            }
        }
        max_f
    }

    /// 更新单个汉字的编码（增量更新碰撞计数）
    #[inline]
    pub fn update_char(&mut self, ctx: &OptContext, assignment: &[u8], ci: usize) {
        let old_code = self.current_codes[ci];
        let new_code = ctx.calc_code_only(ci, assignment);
        if old_code == new_code {
            return;
        }

        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        // 更新等价值，并同步有效当量字频分母。
        let old_eq = self.current_equiv_val[ci];
        let new_eq = ctx.calc_equiv_from_parts(ci, assignment);
        match (old_eq, new_eq) {
            (Some(old), Some(new)) => {
                self.total_equiv_weighted += (new - old) * freq_f;
                self.total_equiv_sq_weighted += (new * new - old * old) * freq_f;
            }
            (Some(old), None) => {
                self.total_equiv_weighted -= old * freq_f;
                self.total_equiv_sq_weighted -= old * old * freq_f;
                self.equiv_frequency -= freq;
            }
            (None, Some(new)) => {
                self.total_equiv_weighted += new * freq_f;
                self.total_equiv_sq_weighted += new * new * freq_f;
                self.equiv_frequency += freq;
            }
            (None, None) => {}
        }
        self.current_equiv_val[ci] = new_eq;

        let old_code_u = old_code as u32;
        let new_code_u = new_code as u32;

        // === 从旧桶移除 ===
        let pos = self.char_bucket_pos[ci];
        // 旧桶一定非空：访问 + 移除（空时）在稀疏后端单次哈希查找内完成，省去额外的 remove 查找。
        // 预借用 char_bucket_pos（与 full_buckets 为不相交字段），供闭包内重链 swap_remove 的尾元素。
        let char_bucket_pos = &mut self.char_bucket_pos;
        let (old_bucket_cc, old_bucket_cf, new_old_cc, new_old_cf) = self
            .full_buckets
            .modify_existing_remove_if_empty(old_code_u, |ob| {
                let old_len = ob.members.len();
                // 旧桶的碰撞贡献（移除前）
                let old_bucket_cc = old_len.saturating_sub(1);
                let old_bucket_cf = if old_len >= 2 {
                    ob.freq_sum - ob.max_freq
                } else {
                    0
                };

                // swap_remove: 用最后一个元素替换被移除的元素
                let last_idx = old_len - 1;
                if pos != last_idx {
                    let moved_ci = ob.members[last_idx];
                    ob.members[pos] = moved_ci;
                    char_bucket_pos[moved_ci as usize] = pos;
                }
                ob.members.pop();

                // 更新旧桶的频率统计：如果移除的是 max，需要重扫。
                ob.freq_sum -= freq;
                if freq >= ob.max_freq {
                    ob.max_freq = if ob.members.is_empty() {
                        0
                    } else {
                        Self::rescan_bucket_max(ctx, &ob.members)
                    };
                }

                let new_old_len = ob.members.len();
                let new_old_cc = new_old_len.saturating_sub(1);
                let new_old_cf = if new_old_len >= 2 {
                    ob.freq_sum - ob.max_freq
                } else {
                    0
                };
                (old_bucket_cc, old_bucket_cf, new_old_cc, new_old_cf)
            });
        // 旧桶变空时的移除已在 modify_existing_remove_if_empty 内（同一次查找）完成。

        // === 插入新桶 ===
        let (new_bucket_cc, new_bucket_cf, after_new_cc, after_new_cf) = {
            let nb = self.full_buckets.get_mut_or_insert(new_code_u);
            let new_len = nb.members.len();
            let new_bucket_cc = new_len.saturating_sub(1);
            let new_bucket_cf = if new_len >= 2 {
                nb.freq_sum - nb.max_freq
            } else {
                0
            };

            let new_pos = new_len;
            nb.members.push(ci as u32);
            self.char_bucket_pos[ci] = new_pos;
            nb.freq_sum += freq;
            if freq > nb.max_freq {
                nb.max_freq = freq;
            }

            let after_new_len = new_len + 1;
            let after_new_cc = after_new_len.saturating_sub(1);
            let after_new_cf = if after_new_len >= 2 {
                nb.freq_sum - nb.max_freq
            } else {
                0
            };
            (new_bucket_cc, new_bucket_cf, after_new_cc, after_new_cf)
        };

        // 更新全局碰撞计数
        self.total_collisions =
            (self.total_collisions + new_old_cc + after_new_cc) - (old_bucket_cc + new_bucket_cc);
        self.collision_frequency = (self.collision_frequency + new_old_cf + after_new_cf)
            - (old_bucket_cf + new_bucket_cf);
        self.current_codes[ci] = new_code;

        // 全码聚合已变化：标记全码分量缓存为脏（需求 10.1）。
        // `update_char` 是全码聚合的唯一变更入口（正向移动与回滚反向重放都经此），
        // 在此单点置脏可保证 `full_score_component` 永远基于最新聚合重算，
        // 无需在 `try_move`/`try_swap` 的多个分支重复维护。
        self.full_score_dirty = true;
    }

    /// 执行全码 _max 硬约束检查
    /// 返回 true 表示通过（未超限），false 表示超限需回滚
    #[inline(always)]
    fn check_full_code_max(&self, ctx: &OptContext) -> bool {
        let t = &ctx.targets_config.full_code;

        if t.collision_count_max > 0.0 && self.total_collisions as f64 > t.collision_count_max {
            return false;
        }

        if t.collision_rate_max > 0.0 {
            let rate = self.collision_frequency as f64 * self.inv_total_frequency;
            if rate > t.collision_rate_max {
                return false;
            }
        }

        if t.equivalence_max > 0.0 {
            let equiv = if self.equiv_frequency > 0 {
                self.total_equiv_weighted / self.equiv_frequency as f64
            } else {
                0.0
            };
            if equiv > t.equivalence_max {
                return false;
            }
        }

        if t.equiv_cv_max > 0.0 {
            let cv = self.calc_equiv_cv();
            if cv > t.equiv_cv_max {
                return false;
            }
        }

        if t.distribution_max > 0.0 {
            let dist = self.calc_distribution_deviation(&ctx.key_dist_config);
            if dist > t.distribution_max {
                return false;
            }
        }

        true
    }

    /// 全码 `*_max` 硬约束统一入口，供不走 `try_move`/`try_swap` 的邻域算子复用。
    ///
    /// 三交换、整键互换、坐标下降都是「自行改 assignment → 自行算分 → 自行回滚」的路径，
    /// 若不显式检查，`[targets]` 里配的硬上限在这些路径上可被静默突破
    /// （软惩罚拉不住时，解会跑到约束外）。
    #[inline(always)]
    pub fn constraints_ok(&self, ctx: &OptContext) -> bool {
        self.check_full_code_max(ctx)
    }

    /// 计算全码得分
    #[inline(always)]
    pub fn compute_full_score(&self, ctx: &OptContext) -> f64 {
        if ctx.targets_config.full_code.enabled {
            let t = &ctx.targets_config.full_code;
            let lw = t.low_weight;
            let mut score = 0.0;

            // collision_count
            {
                let v = self.total_collisions as f64;
                let s = ctx.scale_config.collision_count;
                let d = (v - t.collision_count).max(0.0) * s;
                score += ctx.weights.weight_collision_count * (d + d * d + lw * (v * s));
            }

            // collision_rate
            if ctx.weights.weight_collision_rate > 0.0 {
                let v = self.collision_frequency as f64 * self.inv_total_frequency;
                let s = ctx.scale_config.collision_rate;
                let d = (v - t.collision_rate).max(0.0) * s;
                score += ctx.weights.weight_collision_rate * (d + d * d + lw * (v * s));
            }

            // equivalence (equiv_mean)
            if ctx.weights.weight_equivalence > 0.0 {
                let v = if self.equiv_frequency > 0 {
                    self.total_equiv_weighted / self.equiv_frequency as f64
                } else {
                    0.0
                };
                let s = ctx.scale_config.equivalence;
                let d = (v - t.equivalence).max(0.0) * s;
                score += ctx.weights.weight_equivalence * (d + d * d + lw * (v * s));
            }

            // equiv_cv
            if ctx.weights.weight_equiv_cv > 0.0 {
                let v = self.calc_equiv_cv();
                let s = ctx.scale_config.equiv_cv;
                let d = (v - t.equiv_cv).max(0.0) * s;
                score += ctx.weights.weight_equiv_cv * (d + d * d + lw * (v * s));
            }

            // distribution
            if ctx.weights.weight_distribution > 0.0 {
                let v = self.calc_distribution_deviation(&ctx.key_dist_config);
                let s = ctx.scale_config.distribution;
                let d = (v - t.distribution).max(0.0) * s;
                score += ctx.weights.weight_distribution * (d + d * d + lw * (v * s));
            }

            score + self.编码罚分(ctx)
        } else {
            // 原有绝对值最小化模式（保持不变）
            let mut score = ctx.weights.weight_collision_count
                * self.total_collisions as f64
                * ctx.scale_config.collision_count;

            if ctx.weights.weight_collision_rate > 0.0 {
                let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
                score += ctx.weights.weight_collision_rate
                    * collision_rate
                    * ctx.scale_config.collision_rate;
            }

            if ctx.weights.weight_equivalence > 0.0 {
                let weighted_equiv = if self.equiv_frequency > 0 {
                    self.total_equiv_weighted / self.equiv_frequency as f64
                } else {
                    0.0
                };
                score +=
                    ctx.weights.weight_equivalence * weighted_equiv * ctx.scale_config.equivalence;
            }

            if ctx.weights.weight_equiv_cv > 0.0 {
                let equiv_cv = self.calc_equiv_cv();
                score += ctx.weights.weight_equiv_cv * equiv_cv * ctx.scale_config.equiv_cv;
            }

            if ctx.weights.weight_distribution > 0.0 {
                let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);
                score += ctx.weights.weight_distribution
                    * dist_deviation
                    * ctx.scale_config.distribution;
            }

            score + self.编码罚分(ctx)
        }
    }

    /// 重新统计编码种数（字根键位组合去重后的个数）。
    ///
    /// 只在 `[constraints].max_encodings > 0` 时统计：未启用时直接置 0，不付任何代价，
    /// 保证老配置的行为与性能完全不变。启用后每次求值多一次 368 项的小排序，
    /// 相对一次 `update_char` 可以忽略。
    fn 刷新编码种数(&mut self, ctx: &OptContext, assignment: &[u8]) {
        if !ctx.constraints.enabled() {
            self.编码种数 = 0;
            return;
        }
        let 基名数 = self.根槽偏移.len().saturating_sub(1);
        self.编码指纹缓冲.clear();
        self.编码指纹缓冲.reserve(基名数);
        for i in 0..基名数 {
            let 起 = self.根槽偏移[i] as usize;
            let 止 = self.根槽偏移[i + 1] as usize;
            if 起 == 止 {
                continue;
            }
            // 每槽 8 位：键位索引最大 30（26 字母 + 4 个标点键），8 位足够；
            // 一基名最多 8 槽，超出部分截断（星陈实测恒为 2 槽）。
            let mut 指纹 = 0u64;
            for (槽, 来源) in self.根槽来源[起..止].iter().enumerate().take(8) {
                let 键 = match 来源 {
                    槽来源::固定(k) => *k as u64,
                    槽来源::组(gi) => assignment[*gi as usize] as u64,
                };
                指纹 |= 键 << (8 * 槽);
            }
            self.编码指纹缓冲.push(指纹);
        }
        self.编码指纹缓冲.sort_unstable();
        self.编码指纹缓冲.dedup();
        self.编码种数 = self.编码指纹缓冲.len();
    }

    /// 当前编码种数（未启用编码种数约束时为 0）
    #[inline(always)]
    pub fn 编码种数(&self) -> usize {
        self.编码种数
    }

    /// 回滚时把编码种数恢复成移动前的值。
    ///
    /// 回滚路径为了让拒绝的移动尽快返回，不重新遍历整表统计；调用方在移动前已取过旧值，
    /// 这里直接写回，保证后续 `get_metrics` 读到的仍是真实状态的种数。
    #[inline(always)]
    pub fn 恢复编码种数(&mut self, 值: usize) {
        self.编码种数 = 值;
    }

    /// 编码种数超限罚分；未启用或未超限时为 0，因此达标解的得分与旧口径完全一致
    #[inline(always)]
    pub fn 编码罚分(&self, ctx: &OptContext) -> f64 {
        if !ctx.constraints.enabled() {
            return 0.0;
        }
        let 超 = self.编码种数.saturating_sub(ctx.constraints.max_encodings);
        if 超 == 0 {
            0.0
        } else {
            ctx.constraints.encoding_penalty * 超 as f64
        }
    }

    /// 获取得分
    #[inline(always)]
    pub fn get_score(&mut self, ctx: &OptContext, assignment: &[u8]) -> f64 {
        if self.score_dirty {
            self.刷新编码种数(ctx, assignment);
            self.cached_full_score = self.compute_full_score(ctx);
            self.full_score_dirty = false;
            self.cached_score = self.cached_full_score;
            self.score_dirty = false;
        }
        self.cached_score
    }

    /// 计算等价值变异系数
    #[inline(always)]
    pub fn calc_equiv_cv(&self) -> f64 {
        if self.equiv_frequency == 0 {
            return 0.0;
        }
        let inv_equiv_frequency = 1.0 / self.equiv_frequency as f64;
        let mean = self.total_equiv_weighted * inv_equiv_frequency;
        if mean <= 0.0 {
            return 0.0;
        }
        let mean_sq = self.total_equiv_sq_weighted * inv_equiv_frequency;
        let variance = mean_sq - mean * mean;
        if variance <= 0.0 {
            return 0.0;
        }
        variance.sqrt() / mean
    }

    /// 计算分布偏差
    #[inline(always)]
    pub fn calc_distribution_deviation(&self, kdc: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
        let mut dev = 0.0;
        for key in 0..EQUIV_TABLE_SIZE {
            let cfg = &kdc[key];
            if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
                continue;
            }
            let actual_pct = self.key_weighted_usage[key] * 100.0 * self.inv_total_key_presses;
            let diff = actual_pct - cfg.target_rate;
            if diff < 0.0 {
                dev += diff * diff * cfg.low_penalty;
            } else if diff > 0.0 {
                dev += diff * diff * cfg.high_penalty;
            }
        }
        dev
    }

    /// 获取评估指标
    pub fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        Metrics {
            collision_count: self.total_collisions,
            collision_rate: self.collision_frequency as f64 * self.inv_total_frequency,
            equiv_mean: if self.equiv_frequency > 0 {
                self.total_equiv_weighted / self.equiv_frequency as f64
            } else {
                0.0
            },
            equiv_cv: self.calc_equiv_cv(),
            dist_deviation: self.calc_distribution_deviation(&ctx.key_dist_config),
            encoding_count: self.编码种数,
        }
    }

    /// 获取各指标的分数分量（用于日志输出，复用算分逻辑，不重复公式）
    pub fn get_metric_scores(&self, ctx: &OptContext) -> MetricScores {
        // 复用 compute_full_score 的逻辑，但拆分为各指标分量
        let score_for = |v: f64, target: f64, scale: f64, weight: f64, lw: f64| -> f64 {
            if ctx.targets_config.full_code.enabled {
                let d = (v - target).max(0.0) * scale;
                weight * (d + d * d + lw * (v * scale))
            } else {
                weight * v * scale
            }
        };

        let t = &ctx.targets_config.full_code;
        let lw = t.low_weight;

        let s_collision_count = score_for(
            self.total_collisions as f64,
            t.collision_count,
            ctx.scale_config.collision_count,
            ctx.weights.weight_collision_count,
            lw,
        );
        let s_collision_rate = score_for(
            self.collision_frequency as f64 * self.inv_total_frequency,
            t.collision_rate,
            ctx.scale_config.collision_rate,
            ctx.weights.weight_collision_rate,
            lw,
        );
        let s_equivalence = score_for(
            self.total_equiv_weighted * self.inv_total_frequency,
            t.equivalence,
            ctx.scale_config.equivalence,
            ctx.weights.weight_equivalence,
            lw,
        );
        let s_equiv_cv = score_for(
            self.calc_equiv_cv(),
            t.equiv_cv,
            ctx.scale_config.equiv_cv,
            ctx.weights.weight_equiv_cv,
            lw,
        );
        let s_distribution = score_for(
            self.calc_distribution_deviation(&ctx.key_dist_config),
            t.distribution,
            ctx.scale_config.distribution,
            ctx.weights.weight_distribution,
            lw,
        );

        let total_full =
            s_collision_count + s_collision_rate + s_equivalence + s_equiv_cv + s_distribution;

        MetricScores {
            collision_count: s_collision_count,
            collision_rate: s_collision_rate,
            equivalence: s_equivalence,
            equiv_cv: s_equiv_cv,
            distribution: s_distribution,
            total_full,
        }
    }

    #[inline(always)]
    pub fn try_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
        temp: f64,
        rng: &mut impl Rng,
    ) -> bool {
        let old_key = assignment[r];
        if old_key == new_key {
            return false;
        }

        let old_score = self.get_score(ctx, assignment);
        // 回滚路径不重算编码种数（那是整表遍历），直接恢复移动前的值即可。
        let 旧种数 = self.编码种数;

        // O(1) key_weighted_usage 更新
        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;

        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }

        // _max 硬约束检查：超限则直接回滚，不进入得分计算
        if !self.check_full_code_max(ctx) {
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;
            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            self.编码种数 = 旧种数;
            return false;
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx, assignment);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.random::<f64>() < (-delta / temp).exp() {
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;

            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            self.编码种数 = 旧种数;
            false
        }
    }

    /// 尝试交换（交换两个组的键位）
    #[inline(always)]
    pub fn try_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
        temp: f64,
        rng: &mut impl Rng,
    ) -> bool {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 {
            return false;
        }

        let old_score = self.get_score(ctx, assignment);
        // 回滚路径不重算编码种数（那是整表遍历），直接恢复移动前的值即可。
        let 旧种数 = self.编码种数;

        // O(1) key_weighted_usage 更新
        let gfs1 = ctx.group_freq_sum[r1];
        let gfs2 = ctx.group_freq_sum[r2];
        self.key_weighted_usage[k1 as usize] -= gfs1;
        self.key_weighted_usage[k2 as usize] += gfs1;
        self.key_weighted_usage[k2 as usize] -= gfs2;
        self.key_weighted_usage[k1 as usize] += gfs2;

        assignment[r1] = k2;
        assignment[r2] = k1;
        for &ci in &ctx.group_to_chars[r1] {
            self.update_char(ctx, assignment, ci);
        }
        for &ci in &ctx.group_to_chars[r2] {
            self.update_char(ctx, assignment, ci);
        }

        // _max 硬约束检查：超限则直接回滚，不进入得分计算
        if !self.check_full_code_max(ctx) {
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;
            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            self.编码种数 = 旧种数;
            return false;
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx, assignment);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.random::<f64>() < (-delta / temp).exp() {
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;

            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            self.编码种数 = 旧种数;
            false
        }
    }
}

// =========================================================================
// 🧪 目标偏差与硬约束属性测试
// =========================================================================
#[cfg(test)]
mod targets_proptest {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, WeightConfig, EQUIV_TABLE_SIZE};
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 5 个动态组，每组 1 根 1 字，允许 4 个键。
    /// 键位必然冲突（5 组挤 4 键）→ 一定有重码，供 `_max` 约束测试用。
    fn 造上下文() -> OptContext {
        let freqs = [5u64, 4, 3, 2, 1];
        let allowed: [u8; 4] = [0, 1, 2, 3];
        let n = freqs.len();
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for (i, &f) in freqs.iter().enumerate() {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: allowed.to_vec(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        )
    }

    /// 目标偏差公式（design.md）
    fn 目标偏差(v: f64, target: f64, scale: f64, weight: f64, lw: f64) -> f64 {
        let d = (v - target).max(0.0) * scale;
        weight * (d + d * d + lw * (v * scale))
    }

    fn 近似(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        // Property 2: 全码目标偏差公式正确性
        #[test]
        fn prop2_全码目标偏差公式(
            t_cc in 0.0f64..20.0,
            t_cr in 0.0f64..1.0,
            t_eq in 0.0f64..5.0,
            t_cv in 0.0f64..5.0,
            t_di in 0.0f64..50.0,
            s_cc in 0.001f64..10.0,
            s_cr in 0.001f64..10.0,
            s_eq in 0.001f64..10.0,
            s_cv in 0.001f64..10.0,
            s_di in 0.001f64..10.0,
            lw in 0.0f64..0.5,
        ) {
            let mut ctx = 造上下文();
            let mut sc = ctx.scale_config;
            sc.collision_count = s_cc;
            sc.collision_rate = s_cr;
            sc.equivalence = s_eq;
            sc.equiv_cv = s_cv;
            sc.distribution = s_di;
            ctx.scale_config = sc;

            let mut t = ctx.targets_config.full_code.clone();
            t.enabled = true;
            t.collision_count = t_cc;
            t.collision_rate = t_cr;
            t.equivalence = t_eq;
            t.equiv_cv = t_cv;
            t.distribution = t_di;
            t.low_weight = lw;
            ctx.targets_config.full_code = t;

            let assignment: Vec<u8> = vec![0, 1, 2, 3, 0];
            let ev = Evaluator::new(&ctx, &assignment);
            let m = ev.get_metrics(&ctx);

            let want = 目标偏差(m.collision_count as f64, t_cc, s_cc, ctx.weights.weight_collision_count, lw)
                + 目标偏差(m.collision_rate, t_cr, s_cr, ctx.weights.weight_collision_rate, lw)
                + 目标偏差(m.equiv_mean, t_eq, s_eq, ctx.weights.weight_equivalence, lw)
                + 目标偏差(m.equiv_cv, t_cv, s_cv, ctx.weights.weight_equiv_cv, lw)
                + 目标偏差(m.dist_deviation, t_di, s_di, ctx.weights.weight_distribution, lw);

            let got = ev.compute_full_score(&ctx);
            prop_assert!(近似(got, want), "全码得分 got={} want={}", got, want);

            // v <= target 时超出惩罚项必须为 0：只剩 low_weight 的线性项
            let v_low = t_cc * 0.5; // 恒 ≤ t_cc
            let want_low = ctx.weights.weight_collision_count * lw * (v_low * s_cc);
            let got_low = 目标偏差(v_low, t_cc, s_cc, ctx.weights.weight_collision_count, lw);
            prop_assert!(
                近似(got_low, want_low),
                "未超目标时应只剩线性项 got={} want={}",
                got_low,
                want_low
            );
        }

        // Property 3: 全码 enabled=false 时退化为原线性公式
        #[test]
        fn prop3_全码关闭时向后兼容(
            s_cc in 0.001f64..10.0,
            s_cr in 0.001f64..10.0,
            s_eq in 0.001f64..10.0,
            s_cv in 0.001f64..10.0,
            s_di in 0.001f64..10.0,
        ) {
            let mut ctx = 造上下文();
            let mut sc = ctx.scale_config;
            sc.collision_count = s_cc;
            sc.collision_rate = s_cr;
            sc.equivalence = s_eq;
            sc.equiv_cv = s_cv;
            sc.distribution = s_di;
            ctx.scale_config = sc;
            ctx.targets_config.full_code.enabled = false;

            let assignment: Vec<u8> = vec![0, 1, 2, 3, 0];
            let ev = Evaluator::new(&ctx, &assignment);
            let m = ev.get_metrics(&ctx);

            let want = ctx.weights.weight_collision_count * m.collision_count as f64 * s_cc
                + ctx.weights.weight_collision_rate * m.collision_rate * s_cr
                + ctx.weights.weight_equivalence * m.equiv_mean * s_eq
                + ctx.weights.weight_equiv_cv * m.equiv_cv * s_cv
                + ctx.weights.weight_distribution * m.dist_deviation * s_di;

            let got = ev.compute_full_score(&ctx);
            prop_assert!(近似(got, want), "关闭 targets 后应等于线性公式 got={} want={}", got, want);
        }

        // Property 6: 非零 _max 约束必须拒绝超限方案，并回滚键位
        #[test]
        fn prop6_非零_max_拒绝超限(新键 in 1u8..4) {
            let mut ctx = 造上下文();
            let mut t = ctx.targets_config.full_code.clone();
            // 只要有重码就超限（当前方案必有重码）
            t.collision_count_max = 0.5;
            ctx.targets_config.full_code = t;

            let mut assignment: Vec<u8> = vec![0, 1, 2, 3, 0];
            let mut ev = Evaluator::new(&ctx, &assignment);
            prop_assume!(ev.get_metrics(&ctx).collision_count >= 1);

            let 旧键 = assignment[0];
            let mut rng = rand::rng();
            // 温度极高：若无硬约束必定接受，故拒绝只能来自 _max
            let 接受 = ev.try_move(&ctx, &mut assignment, 0, 新键, 1e12, &mut rng);

            prop_assert!(!接受, "超出 collision_count_max 的移动必须被拒绝");
            prop_assert_eq!(assignment[0], 旧键, "被拒绝后键位必须回滚");
        }

        // Property 7: 所有 _max = 0.0 时不因硬约束拒绝方案
        #[test]
        fn prop7_全零_max_不拒绝(新键 in 1u8..4) {
            let mut ctx = 造上下文();
            let mut t = ctx.targets_config.full_code.clone();
            t.collision_count_max = 0.0;
            t.collision_rate_max = 0.0;
            t.equivalence_max = 0.0;
            t.equiv_cv_max = 0.0;
            t.distribution_max = 0.0;
            ctx.targets_config.full_code = t;

            let mut assignment: Vec<u8> = vec![0, 1, 2, 3, 0];
            let mut ev = Evaluator::new(&ctx, &assignment);

            let mut rng = rand::rng();
            // 温度极高 → 接受概率 ≈ 1，若被拒只可能是硬约束误判
            let 接受 = ev.try_move(&ctx, &mut assignment, 0, 新键, 1e12, &mut rng);
            prop_assert!(接受, "全部 _max=0 时不应因硬约束拒绝");
        }
    }
}

// =========================================================================
// 🧪 全码评估器不变量测试
// =========================================================================
#[cfg(test)]
mod full_evaluator_tests {
    use super::*;
    use crate::config::{ConstraintsConfig, TargetsConfig};
    use crate::context::OptContext;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, WeightConfig, EQUIV_TABLE_SIZE};
    use std::collections::HashMap;

    /// n 个单根单字、各自一个独立组，键集固定。相同频率 + 少键 → 必然出重码桶。
    fn 造上下文(n: usize, 键数: u8) -> OptContext {
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for i in 0..n {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: (0..键数).collect(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], 10 + (i as u64 % 3)));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        )
    }

    /// 碰撞数必须等于「每个非空桶成员数 − 1」之和，且等于 Σ(桶内除最大频率外的字频)。
    #[test]
    fn 碰撞统计与桶结构一致() {
        let ctx = 造上下文(9, 3);
        let asg: Vec<u8> = (0..9).map(|i| (i % 3) as u8).collect();
        let ev = Evaluator::new(&ctx, &asg);
        let m = ev.get_metrics(&ctx);

        let mut 桶: HashMap<usize, Vec<usize>> = HashMap::new();
        for ci in 0..ctx.char_infos.len() {
            let code = ctx.calc_code_only(ci, &asg);
            桶.entry(code).or_default().push(ci);
        }
        let mut want_cc = 0usize;
        let mut want_cf = 0u64;
        for members in 桶.values() {
            if members.len() >= 2 {
                want_cc += members.len() - 1;
                let 最大 = members
                    .iter()
                    .map(|&ci| ctx.char_infos[ci].frequency)
                    .max()
                    .unwrap();
                want_cf += members
                    .iter()
                    .map(|&ci| ctx.char_infos[ci].frequency)
                    .sum::<u64>()
                    - 最大;
            }
        }
        assert_eq!(m.collision_count, want_cc, "碰撞数应与桶结构一致");
        assert!(
            (m.collision_rate - want_cf as f64 / ctx.total_frequency as f64).abs() < 1e-12,
            "碰撞率应为碰撞字频占比"
        );
    }

    /// 逐字 `update_char` 走到目标分配后，所有聚合与直接从零构建等同。
    #[test]
    fn 增量更新与全量重建一致() {
        let ctx = 造上下文(12, 4);
        let 初始: Vec<u8> = (0..12).map(|i| (i % 2) as u8).collect();
        let 目标: Vec<u8> = (0..12).map(|i| ((i * 3 + 1) % 4) as u8).collect();

        let mut ev = Evaluator::new(&ctx, &初始);
        let mut asg = 初始.clone();
        for ci in 0..asg.len() {
            asg[ci] = 目标[ci];
            ev.update_char(&ctx, &asg, ci);
        }
        let 全量 = Evaluator::new(&ctx, &目标);

        assert_eq!(ev.total_collisions, 全量.total_collisions, "碰撞数应一致");
        assert_eq!(
            ev.collision_frequency, 全量.collision_frequency,
            "碰撞字频应一致"
        );
        assert!(
            (ev.get_metrics(&ctx).equiv_mean - 全量.get_metrics(&ctx).equiv_mean).abs() < 1e-9,
            "当量应一致"
        );
        // 用指负载由调用方（try_move / 坐标下降）随移动同步维护，`update_char` 只负责编码
        // 桶与当量，故这里不比较 key_weighted_usage。
    }

    /// 被拒绝或回滚的移动必须把评估器恢复到移动前状态（逐字段可复现）。
    #[test]
    fn 拒绝移动后状态复原() {
        let ctx = 造上下文(10, 3);
        let 初始: Vec<u8> = (0..10).map(|i| (i % 3) as u8).collect();
        let mut ev = Evaluator::new(&ctx, &初始);
        let 原始分 = ev.get_score(&ctx, &初始);
        let 原始碰撞 = ev.total_collisions;

        let mut asg = 初始.clone();
        // 温度 0：只接受改进，非改进必回滚
        let mut rng = rand::rng();
        let _ = ev.try_move(&ctx, &mut asg, 0, 2, 0.0, &mut rng);

        let 重建 = Evaluator::new(&ctx, &asg);
        assert_eq!(
            ev.total_collisions, 重建.total_collisions,
            "回滚后碰撞数应一致"
        );
        assert_eq!(
            ev.key_weighted_usage, 重建.key_weighted_usage,
            "回滚后用指应一致"
        );
        if asg == 初始 {
            assert!(
                (ev.get_score(&ctx, &asg) - 原始分).abs() < 1e-12,
                "未接受任何移动时得分应保持不变"
            );
            assert_eq!(ev.total_collisions, 原始碰撞);
        }
    }

    /// 同一分配重复构建评估器应完全等价（构建过程无隐藏状态）。
    #[test]
    fn 重复构建结果一致() {
        let ctx = 造上下文(8, 3);
        let asg: Vec<u8> = (0..8).map(|i| (i % 3) as u8).collect();
        let mut a = Evaluator::new(&ctx, &asg);
        let mut b = Evaluator::new(&ctx, &asg);
        assert_eq!(a.total_collisions, b.total_collisions);
        assert_eq!(a.collision_frequency, b.collision_frequency);
        assert_eq!(a.get_score(&ctx, &asg), b.get_score(&ctx, &asg));
    }

    /// 分项得分之和必须等于总分（日志口径与算分公式不能分叉）。
    #[test]
    fn 分项得分之和等于总分() {
        let ctx = 造上下文(10, 4);
        let asg: Vec<u8> = (0..10).map(|i| (i % 4) as u8).collect();
        let ev = Evaluator::new(&ctx, &asg);
        let s = ev.get_metric_scores(&ctx);
        let 和 = s.collision_count + s.collision_rate + s.equivalence + s.equiv_cv + s.distribution;
        assert!(
            (和 - s.total_full).abs() < 1e-9,
            "分项和 {} 与总分 {}",
            和,
            s.total_full
        );
        assert!((s.total_full - ev.compute_full_score(&ctx)).abs() < 1e-9);
    }

    /// 选中全部字时，平均当量必须落在单字当量区间内；无有效按键对时当量均值为 0。
    #[test]
    fn 无按键对时当量均值为零() {
        // 每个字只有 1 个部件且 max_parts == 1 → 既无内部键对也无尾随空格对
        let mut groups = Vec::new();
        let mut splits = Vec::new();
        for i in 0..4 {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: vec![0, 1],
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], 10u64));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[1.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let ctx = OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        );
        assert_eq!(ctx.max_parts, 1);
        let asg: Vec<u8> = vec![0, 1, 0, 1];
        let ev = Evaluator::new(&ctx, &asg);
        assert_eq!(ev.get_metrics(&ctx).equiv_mean, 0.0);
        assert_eq!(ev.get_metrics(&ctx).equiv_cv, 0.0);
    }

    /// 未启用 `[constraints]` 时编码种数恒为 0、罚分恒为 0，老配置的评分与性能都不受影响。
    #[test]
    fn 未启用约束时编码种数不参与评分() {
        let ctx = 造上下文(4, 2);
        let asg: Vec<u8> = vec![0, 0, 1, 1];
        let ev = Evaluator::new(&ctx, &asg);
        assert_eq!(ev.编码种数(), 0);
        assert_eq!(ev.编码罚分(&ctx), 0.0);
    }

    /// 编码种数 = 各基名「两槽键位拼成的串」去重后的个数。
    ///
    /// 星陈的每个字根有 `.0` / `.1` 两个槽、各选一键，所以种数不是字根数，而是键位组合数。
    #[test]
    fn 编码种数按两槽键位组合去重() {
        let allowed: Vec<u8> = (0..3).collect();
        let groups: Vec<RootGroup> = ["a.0", "a.1", "b.0", "b.1"]
            .iter()
            .map(|r| RootGroup {
                roots: vec![(*r).to_string()],
                allowed_keys: allowed.clone(),
            })
            .collect();
        let splits: Vec<(char, Vec<String>, u64)> = vec![
            ('甲', vec!["a.0".to_string(), "a.1".to_string()], 1),
            ('乙', vec!["b.0".to_string(), "b.1".to_string()], 1),
        ];
        let mut ctx = OptContext::new(
            &splits,
            &HashMap::new(),
            &groups,
            [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
            [KeyDistConfig::default(); EQUIV_TABLE_SIZE],
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        );
        ctx.constraints = ConstraintsConfig {
            max_encodings: 1,
            encoding_penalty: 0.5,
        };

        // a = (0,1)、b = (0,2) → 两种不同键位组合
        let ev = Evaluator::new(&ctx, &[0, 1, 0, 2]);
        assert_eq!(ev.编码种数(), 2);

        // 两个基名落在同一组合 → 只算一种
        let ev = Evaluator::new(&ctx, &[0, 1, 0, 1]);
        assert_eq!(ev.编码种数(), 1);
    }

    /// 罚分只在超出上限时按个数线性计费，达标时为 0（因此最终得分与旧口径可比）。
    #[test]
    fn 编码罚分达标为零超限线性() {
        let mut ctx = 造上下文(4, 3);
        ctx.constraints = ConstraintsConfig {
            max_encodings: 3,
            encoding_penalty: 0.25,
        };
        let asg: Vec<u8> = vec![0, 1, 2, 0];
        let ev = Evaluator::new(&ctx, &asg);
        assert_eq!(ev.编码种数(), 3);
        assert_eq!(ev.编码罚分(&ctx), 0.0, "未超限不该有罚分");

        ctx.constraints.max_encodings = 2;
        assert!(
            (ev.编码罚分(&ctx) - 0.25).abs() < 1e-12,
            "超 1 种应罚一个系数"
        );
    }

    /// 被拒绝的移动必须把编码种数恢复原值，否则汇总与进度行会读到移动中途的脏值。
    #[test]
    fn 拒绝移动后编码种数复原() {
        let mut ctx = 造上下文(4, 3);
        ctx.constraints = ConstraintsConfig {
            max_encodings: 1,
            encoding_penalty: 0.5,
        };
        let 初始: Vec<u8> = vec![0, 0, 0, 0];
        let mut ev = Evaluator::new(&ctx, &初始);
        assert_eq!(ev.编码种数(), 1);

        let mut asg = 初始.clone();
        let mut rng = rand::rng();
        // 温度 0 → 只接受改进；把一组挪到新键会让种数与罚分变差，必然回滚。
        let _ = ev.try_move(&ctx, &mut asg, 0, 2, 0.0, &mut rng);
        assert_eq!(ev.编码种数(), 1, "回滚后编码种数应复原");
        assert_eq!(asg, 初始, "回滚后键位应复原");
    }
}
