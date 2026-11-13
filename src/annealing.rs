// =========================================================================
// 🧠 混合优化算法（模拟退火 + 冲突导向邻域）
// =========================================================================

use rand::prelude::*;
use rayon::prelude::*;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::checkpoint::{self, ThreadCheckpoint};
use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::schedule::TemperatureSchedule;
use crate::types::{
    char_to_key_index, Metrics, EQUIV_TABLE_SIZE, GROUP_MARKER, KEY_INDEX_SPACE, KEY_SPACE,
};

/// 退火主循环结果（可续算变体返回）。
pub struct SaResult {
    pub assignment: Vec<u8>,
    pub score: f64,
    pub metrics: Metrics,
    /// 是否因 stop_flag（Ctrl-C）提前中断。
    pub interrupted: bool,
}

/// 退火主循环每隔多少步检查一次 stop_flag（Ctrl-C）。
const STOP_CHECK_STRIDE: usize = 10_000;

/// 计算 checkpoint 写出步数间隔 = max(1, floor(total_steps × ratio))。
///
/// 单位：`ratio` 为占 `total_steps` 的比例（与 `reconcile_interval` 同口径）。
/// 永不为 0（避免取模除零）。供退火主循环与属性测试调用。
pub(crate) fn checkpoint_interval(total_steps: usize, ratio: f64) -> usize {
    (((total_steps as f64) * ratio).floor() as i64).max(1) as usize
}

// =========================================================================
// 初始化策略
// =========================================================================

/// 原始贪心初始化 - 按组大小降序，均衡分配到键位
fn greedy_balance_init(ctx: &OptContext, cfg: &Config, rng: &mut impl Rng) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_groups];

    let mut group_freq: Vec<(usize, usize)> = ctx
        .group_to_chars
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    group_freq.sort_by_key(|x| std::cmp::Reverse(x.1));

    let max_ki = cfg
        .keys
        .allowed
        .chars()
        .filter_map(char_to_key_index)
        .max()
        .unwrap_or(25);
    let mut key_counts = vec![0usize; max_ki + 1];

    for (gi, _) in &group_freq {
        let gi = *gi;
        let allowed = &ctx.groups[gi].allowed_keys;
        let min_count = allowed
            .iter()
            .map(|&k| key_counts.get(k as usize).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);

        let candidates: Vec<u8> = allowed
            .iter()
            .filter(|&&k| key_counts.get(k as usize).copied().unwrap_or(0) == min_count)
            .copied()
            .collect();

        let best = if candidates.is_empty() {
            allowed[0]
        } else {
            candidates[rng.random_range(0..candidates.len())]
        };

        assignment[gi] = best;
        if (best as usize) < key_counts.len() {
            key_counts[best as usize] += 1;
        }
    }
    assignment
}

/// 频率感知贪心 - 按组权重降序，最小化碰撞代价
fn frequency_greedy_init(ctx: &OptContext, rng: &mut impl Rng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut group_info: Vec<(usize, f64)> = (0..n)
        .map(|gi| {
            let weight = ctx.group_to_chars[gi].len() as f64;
            let noise = 1.0 + rng.random::<f64>() * 0.25;
            (gi, weight * noise)
        })
        .collect();
    group_info.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // 按键位索引寻址，长度必须是键位索引空间（31），不能是 KEY_SPACE（26，仅空格键索引）：
    // 允许键含 `_ ; , . /` 时索引可达 30。
    let mut key_freq_load = vec![0.0f64; KEY_INDEX_SPACE];
    let mut key_group_count = vec![0usize; KEY_INDEX_SPACE];

    for &(gi, weight) in &group_info {
        let allowed = &ctx.groups[gi].allowed_keys;

        let mut best_key = allowed[0];
        let mut best_cost = f64::MAX;

        for &k in allowed {
            let ki = k as usize;
            let collision_cost = key_freq_load[ki] * weight;
            let balance_cost = key_group_count[ki] as f64 * 0.05;
            let cost = collision_cost + balance_cost;

            if cost < best_cost {
                best_cost = cost;
                best_key = k;
            }
        }

        assignment[gi] = best_key;
        key_freq_load[best_key as usize] += weight;
        key_group_count[best_key as usize] += 1;
    }

    assignment
}

/// 分散优先贪心
fn spread_greedy_init(ctx: &OptContext, rng: &mut impl Rng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);

    let mut key_last_used = vec![0usize; KEY_INDEX_SPACE];

    for (step, &gi) in order.iter().enumerate() {
        let allowed = &ctx.groups[gi].allowed_keys;
        let best_key = *allowed
            .iter()
            .min_by_key(|&&k| key_last_used[k as usize])
            .unwrap();

        assignment[gi] = best_key;
        key_last_used[best_key as usize] = step + 1;
    }

    assignment
}

/// 纯随机有效解
fn random_valid_init(ctx: &OptContext, rng: &mut impl Rng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        let allowed = &ctx.groups[gi].allowed_keys;
        assignment[gi] = allowed[rng.random_range(0..allowed.len())];
    }
    assignment
}

// =========================================================================
// 🔍 冲突分析
// =========================================================================

/// 构建编码到汉字的反向索引（只保留 ≥2 个汉字的桶）
///
/// 单个汉字独占的编码永远不可能冲突，全表 9868 条里绝大多数是这种；提前裁掉
/// 能让后续遍历/增量维护都只在「真正可能冲突」的几百个桶上进行。
///
/// 低温扰动移除后，正式路径已改由 [`CollisionCache`] 增量维护，本函数只服务测试。
#[cfg(test)]
fn build_code_to_chars(ctx: &OptContext, assignment: &[u8]) -> HashMap<usize, Vec<usize>> {
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..ctx.char_infos.len() {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }
    code_to_chars.retain(|_, v| v.len() >= 2);
    code_to_chars
}

/// 把一个冲突桶（同一编码下的若干汉字）折算成一条代表冲突对
///
/// 返回 `(组号下限, 组号上限, 权重)`；桶内字根组不足两个（同一组的两个码位自撞）
/// 时返回 `None`——那种冲突靠「移走一个组」拆不开。
fn entry_for_bucket(
    ctx: &OptContext,
    chars: &[usize],
    weight_by_freq: bool,
    scratch: &mut Vec<(usize, u64)>,
) -> Option<(usize, usize, usize)> {
    // 冲突权重：count 策略用 chars.len()；freq 策略用「字频和 − 最高字频」，
    // 与评估器 collision_frequency 的口径一致（首字不计损失）。
    let weight: usize = if weight_by_freq {
        let sum: u64 = chars.iter().map(|&ci| ctx.char_infos[ci].frequency).sum();
        let max = chars
            .iter()
            .map(|&ci| ctx.char_infos[ci].frequency)
            .max()
            .unwrap_or(0);
        // sum >= max 恒成立，减法不会下溢。
        (sum - max) as usize
    } else {
        chars.len()
    };

    // 统计每个字根组在该编码下的字频贡献，用于挑选代表对。
    //
    // 两点性能约束，都是被实测逼出来的（曾经每步重排一次，把吞吐从 316 万
    // 步/分打到 62 万）：
    // 1. 用线性查表而不是 HashMap——桶内通常 2~4 个汉字、至多四五个组，
    //    建哈希表反而更慢，且每个桶一次堆分配。
    // 2. 复用调用方传入的 `scratch`，整个重排过程零分配。
    scratch.clear();
    for &ci in chars {
        let info = &ctx.char_infos[ci];
        for &p in &info.parts {
            if p >= GROUP_MARKER {
                let gi = (p - GROUP_MARKER) as usize;
                match scratch.iter_mut().find(|e| e.0 == gi) {
                    Some(e) => e.1 += info.frequency,
                    None => scratch.push((gi, info.frequency)),
                }
            }
        }
    }

    // 贡献降序 + 组号升序裁决，保证可复现（不依赖遍历顺序）。
    scratch.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let ranked = &*scratch;

    if ranked.len() < 2 {
        return None;
    }
    let (g1, g2) = (ranked[0].0, ranked[1].0);
    // 规范化为 (min, max)，保证 g1 < g2，使排序次键稳定、配对去歧义
    let (lo, hi) = if g1 <= g2 { (g1, g2) } else { (g2, g1) };
    Some((lo, hi, weight))
}

/// 冲突条目排序：主键 weight 降序；次键 (g1, g2) 升序裁决（结果可复现）
fn sort_collisions(collisions: &mut [(usize, usize, usize)]) {
    collisions.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
}

/// 找出所有重码冲突的字根组对
///
/// `weight_by_freq` 控制排序权重（元组第三字段）：
/// - false：权重为参与该冲突编码的汉字数量（优化 collision_count）
/// - true ：权重为「字频和 − 最高字频」（优化 collision_rate）
///
/// # 为什么 freq 权重不是「字频和」
///
/// 评估器的 `collision_frequency` 是逐桶累加 `freq_sum - max_freq`：同一编码下
/// **首个候选字不受罚**，只有被挤到后面的字才算重码损失。所以「能(3125730) / 绲(67)」
/// 这种编码的字频和高达 312 万，实际可挽回损失只有 67 —— 按字频和排序会把它推到队首，
/// 让邻域预算全砸在几乎没有收益的编码上。改按 `freq_sum - max_freq` 排序，排序键
/// 才与要优化的指标同构：优先修的是「两个高频字撞在一起」的真·高频重码。
///
/// # 每个冲突编码只产出一条代表对
///
/// 原实现对同一编码下 k 个字根组枚举全部 `C(k,2)` 对，权重完全相同。4 个组的编码会
/// 一次塞进 6 条重复对，把采样窗口（默认取排序后前 20 个）占满在同一两个编码上，
/// 邻域预算被浪费在反复重试同一冲突，覆盖面收缩。改为每个编码只保留一条代表对
/// （该编码下字频贡献最大的两个组），使窗口内 20 条对应 20 个不同的冲突编码。
///
/// 排序：主键 weight 降序；次键 (g1, g2) 升序裁决，保证结果可复现、不依赖 HashMap 遍历顺序。
///
/// 低温扰动功能已移除，此函数现在只被测试（重码排序/权重）使用，正式路径改由
/// [`CollisionCache`] 增量维护冲突条目。
#[cfg(test)]
fn find_collision_groups(
    ctx: &OptContext,
    assignment: &[u8],
    weight_by_freq: bool,
) -> Vec<(usize, usize, usize)> {
    let code_to_chars = build_code_to_chars(ctx, assignment);
    let mut scratch: Vec<(usize, u64)> = Vec::with_capacity(8);
    let mut collisions: Vec<(usize, usize, usize)> = code_to_chars
        .values()
        .filter_map(|chars| entry_for_bucket(ctx, chars, weight_by_freq, &mut scratch))
        .collect();
    sort_collisions(&mut collisions);
    collisions
}

/// 冲突条目重建阈值：编码桶累计变化多少条才重排一次冲突列表。
///
/// 一次重排要遍历全部冲突桶（约 320 个）、逐桶折算权重再排序，而低温段每步
/// swap 就会让 20~40 个编码换桶——阈值太小等于每步都重排。取 64 是实测折中：
/// 重排已优化到零分配（复用 scratch），名义上约每 2~3 步一次，实际开销可忽略；
/// 而条目滞后控制在两个编码变化量级内，`try_resolve_conflict` 很少采到失效目标。
///
/// 反面教材：阈值 **256** 时代价没省多少（重排本身已不贵），却让条目长期滞后，
/// 冲突分支大量采到已消解的冲突而空转、白丢邻域预算——实测 4 个线程的得分
/// 从 0.4823~0.5398 全散布，方差是关闭冲突算子时的两倍。
const COLLISION_ENTRY_REBUILD_THRESHOLD: usize = 64;

/// 冲突缓存：编码桶增量维护 + 冲突条目延迟重建
///
/// # 为什么不再每 N 步全量重建
///
/// 原实现每 `conflict_refresh_interval`（默认 1000）步调一次 `find_collision_groups`，
/// 那是「遍历 9868 个汉字重算码值 → 重新分桶 → 全量排序」的活。而低温段一千步内常有
/// 数十次成功移动，缓存均值已过期半个周期；`try_resolve_conflict` 的新鲜度校验只能把
/// 这些过期记录**跳过**，被跳过的采样机会直接白丢（等于这段时间的冲突算子预算打折）。
///
/// # 现在的做法
///
/// 1. **编码桶增量维护**：每次采样前对比组键位快照（约 650 次 `u8` 比较，实测开销
///    可忽略），只对真正移动过的组重算其名下汉字的码值并迁移桶——成本 ≈ 该组字符数
///    × 码长，而不是全表的 9868 次码值计算。
/// 2. **冲突条目延迟重建**：脏编码累计到 [`COLLISION_ENTRY_REBUILD_THRESHOLD`] 才从桶
///    重排一次条目。两次重建之间条目可能**少**几条新冲突，但不会多出错误目标
///    （采样前仍有同键校验兜底）。
///
/// 用「快照 diff」而不是「调用点手动标记」是有意的：`try_move` / `try_swap` /
/// `try_triple_swap` / `try_key_reorganization` / `try_full_key_swap` 都会改组键位，
/// 逐个调用点埋标记极易漏（漏了也不会报错，只会让缓存悄悄失真）。diff 一次 650 项
/// 就够，正确性不依赖任何调用点配合。
struct CollisionCache {
    /// 编码 → 该编码下的汉字下标（只保留 ≥2 的桶）
    buckets: HashMap<usize, Vec<usize>>,
    /// 汉字 → 当前所属编码
    char_code: Vec<usize>,
    /// 组 → 含该组标记的汉字下标（预计算一次，之后只读）
    group_chars: Vec<Vec<usize>>,
    /// 组 → 上次同步时的键位（diff 用）
    group_key: Vec<u8>,
    /// 排序后的冲突条目
    entries: Vec<(usize, usize, usize)>,
    /// 自上次重排条目以来发生变化的编码数
    changed_codes: usize,
    /// 自上次重排条目以来的同步次数（条目重建的步数兜底）
    syncs_since_rebuild: usize,
    /// 条目排序权重口径（`conflict_weight_by_freq`）
    weight_by_freq: bool,
    /// 条目重建的步数兜底间隔；0 表示只按脏编码数自适应
    entry_rebuild_interval: usize,
}

impl CollisionCache {
    fn new(
        ctx: &OptContext,
        assignment: &[u8],
        weight_by_freq: bool,
        entry_rebuild_interval: usize,
    ) -> Self {
        // 组 → 涉及汉字：一个汉字由多个字根组成，因此会出现在多个组的名下。
        let mut group_chars: Vec<Vec<usize>> = vec![Vec::new(); ctx.num_groups];
        for (ci, info) in ctx.char_infos.iter().enumerate() {
            for &p in &info.parts {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    if gi < group_chars.len() {
                        group_chars[gi].push(ci);
                    }
                }
            }
        }

        let mut cache = Self {
            buckets: HashMap::new(),
            char_code: vec![0; ctx.char_infos.len()],
            group_chars,
            group_key: vec![0; ctx.num_groups],
            entries: Vec::new(),
            changed_codes: 0,
            syncs_since_rebuild: 0,
            weight_by_freq,
            entry_rebuild_interval,
        };
        cache.rebuild(ctx, assignment);
        cache
    }

    /// 全量重建（初始化，或外部大范围改动后）。日常路径走 [`Self::sync`]。
    fn rebuild(&mut self, ctx: &OptContext, assignment: &[u8]) {
        self.buckets.clear();
        for ci in 0..ctx.char_infos.len() {
            let code = ctx.calc_code_only(ci, assignment);
            self.char_code[ci] = code;
            self.buckets.entry(code).or_default().push(ci);
        }
        self.buckets.retain(|_, v| v.len() >= 2);
        let n_groups = self.group_key.len();
        self.group_key.copy_from_slice(&assignment[..n_groups]);
        self.rebuild_entries(ctx);
        self.syncs_since_rebuild = 0;
    }

    /// 从编码桶重排冲突条目（不重算码值，是全量重建里最便宜的那半）
    fn rebuild_entries(&mut self, ctx: &OptContext) {
        let weight_by_freq = self.weight_by_freq;
        let mut scratch: Vec<(usize, u64)> = Vec::with_capacity(8);
        let mut entries: Vec<(usize, usize, usize)> = self
            .buckets
            .values()
            .filter_map(|chars| entry_for_bucket(ctx, chars, weight_by_freq, &mut scratch))
            .collect();
        sort_collisions(&mut entries);
        self.entries = entries;
        self.changed_codes = 0;
    }

    /// 把「组键位与快照不同」的改动增量同步进编码桶
    ///
    /// 只重算受影响组名下汉字的码值（平均每组十余个汉字），成本与改动规模成正比。
    /// 脏编码累计到阈值、或同步次数到兜底间隔时才重排条目。
    fn sync(&mut self, ctx: &OptContext, assignment: &[u8]) {
        let mut any = false;
        for gi in 0..self.group_key.len() {
            let key = assignment[gi];
            if self.group_key[gi] == key {
                continue;
            }
            self.group_key[gi] = key;
            any = true;

            for &ci in &self.group_chars[gi] {
                let new_code = ctx.calc_code_only(ci, assignment);
                let old_code = self.char_code[ci];
                if new_code == old_code {
                    continue;
                }

                // 从旧桶摘除；桶空了就删掉，避免桶表被空壳撑大
                let emptied = match self.buckets.get_mut(&old_code) {
                    Some(v) => {
                        if let Some(pos) = v.iter().position(|&x| x == ci) {
                            v.swap_remove(pos);
                        }
                        v.is_empty()
                    }
                    None => false,
                };
                if emptied {
                    self.buckets.remove(&old_code);
                }

                self.buckets.entry(new_code).or_default().push(ci);
                self.char_code[ci] = new_code;
                self.changed_codes += 1;
            }
        }

        if !any {
            return;
        }

        self.syncs_since_rebuild += 1;
        let over_interval = self.entry_rebuild_interval > 0
            && self.syncs_since_rebuild >= self.entry_rebuild_interval;
        if self.changed_codes >= COLLISION_ENTRY_REBUILD_THRESHOLD || over_interval {
            self.rebuild_entries(ctx);
            self.syncs_since_rebuild = 0;
        }
    }

    /// 当前冲突条目（可能滞后于最新键位，采样时另有同键校验兜底）
    fn entries(&self) -> &[(usize, usize, usize)] {
        &self.entries
    }
}

/// 找出特定键位上的所有字根组
/// 返回 SmallVec：同键组数常态 ≤ 每键平均组数（数十个），内联在栈上即可，
/// 避免 sa 主循环每步一次堆分配（长跑下分配器堆段只增不减，是内存膨胀主因）。
fn find_groups_on_key(ctx: &OptContext, assignment: &[u8], key: u8) -> SmallVec<[usize; 64]> {
    (0..ctx.num_groups)
        .filter(|&gi| assignment[gi] == key)
        .collect()
}

// =========================================================================
// 🎯 冲突导向的邻域操作
// =========================================================================

/// 从 `allowed_keys` 中按「剩余配额」加权挑一个落点键（排除当前键）。
///
/// # 为什么不再随机选键
///
/// 原实现把冲突组搬到**随机**新键，只关心「拆开重码」，完全不看目标键的负载：
/// 10 个冲突编码接连撞上同一把已经超标的键，分布偏差就从 2.99 涨到 5.07（+69%）。
/// 重码降了，键位均衡度赔进去。
///
/// # 权重取什么
///
/// 目标是 `calc_distribution_deviation` 里那一项 `(actual% − target%)² · penalty`。
/// 在 `low_penalty = 0`（只罚超标、不罚欠配额，是实际配置的常见形态）下，
/// 欠配额的键之间**没有**梯度差别，唯一有意义的信号是「这键还差多少才到目标」：
/// `headroom = target_rate − actual_pct`（百分点，正数=还欠配额）。它同时起两个作用——
/// 越过目标之前不加惩罚，越接近目标越该降低被选中的概率。所以拿它当抽样权重，
/// 而不是用绝对负载（会把 home 键这种「目标本来就高」的键误判成超标）。
///
/// `low_penalty > 0` 的配置才额外乘上它（那种口径下「欠得越多越该补」才成立）。
///
/// 全部键都已超标（或配置根本没给目标使用率）时回退均匀采样，与改造前行为一致。
/// 注意这是**静默**回退：没有目标配置时权重量全为 0，结果看起来和「加权生效」一样。
///
/// # 为什么不需要额外快照
///
/// `Evaluator::key_weighted_usage` 是 31 槽累加器，`try_move`/`try_swap` 里 O(1)
/// 增量维护（接受时加、拒绝时回滚），读它就是读**当前真实**负载：
/// 既不必每步扫全表（9868 字 × parts），也不会有「刷新间隔内快照过期」的问题。
/// 代价只是每步比较 21 个允许键的负载，相对 `try_move` 重算码值可以忽略。
fn pick_key_by_headroom(
    ctx: &OptContext,
    evaluator: &Evaluator,
    group: usize,
    current_key: u8,
    rng: &mut impl Rng,
) -> Option<u8> {
    let allowed = &ctx.groups[group].allowed_keys;
    let inv = evaluator.inv_total_key_presses;
    // 归一化系数无效（空表/未初始化）时无从判断配额，走均匀回退。
    let usable = inv.is_finite() && inv > 0.0;

    let mut keys = [0u8; EQUIV_TABLE_SIZE];
    let mut weights = [0.0f64; EQUIV_TABLE_SIZE];
    let mut n = 0usize;
    let mut total = 0.0f64;

    if usable {
        for &k in allowed {
            if k == current_key {
                continue;
            }
            let cfg = &ctx.key_dist_config[k as usize];
            // 与 calc_distribution_deviation 同口径：usage × 100 × inv_total_key_presses
            let actual = evaluator.key_weighted_usage[k as usize] * 100.0 * inv;
            let headroom = cfg.target_rate - actual;
            // 只排除「已经超标」的键；权重用 headroom 本身。
            //
            // 早期实现把 `low_penalty` 也乘进来，而它是「欠配额时的惩罚系数」，
            // 不是「这个键还能吃多少负载」。真实配置（`key_distribution.txt`）里
            // 第 4 列 low_penalty 普遍为 **0**（只罚超标、不罚欠配额），于是权重
            // 全部归零、所有键被跳过、每次都静默退回均匀随机——负载加权从头到尾
            // 没有生效过，日志上也看不出来。现在只在 low_penalty > 0 时才乘它
            // （那种配置下「欠得越多越该补」才成立）。
            if headroom <= 0.0 {
                continue;
            }
            let weight = if cfg.low_penalty > 0.0 {
                headroom * cfg.low_penalty
            } else {
                headroom
            };
            // 定长数组防越界：允许键若含重复项（约束文件手写重复列）会超过键位索引空间。
            if n >= KEY_INDEX_SPACE {
                break;
            }
            keys[n] = k;
            weights[n] = weight;
            total += weight;
            n += 1;
        }
    }

    if n == 0 || total <= 0.0 {
        // 所有键都已超标，或配置里没有目标使用率：回退均匀采样
        let mut others = [0u8; EQUIV_TABLE_SIZE];
        let mut m = 0usize;
        for &k in allowed {
            if k != current_key {
                if m >= KEY_INDEX_SPACE {
                    break;
                }
                others[m] = k;
                m += 1;
            }
        }
        if m == 0 {
            return None;
        }
        return Some(others[rng.random_range(0..m)]);
    }

    let mut r = rng.random::<f64>() * total;
    for i in 0..n {
        r -= weights[i];
        if r < 0.0 {
            return Some(keys[i]);
        }
    }
    // 浮点累加误差兜底：落到最后一个候选
    Some(keys[n - 1])
}

/// 冲突导向邻域的采样参数：窗口大小 + 落点是否按负载加权。
///
/// 聚成一个结构而非两个裸参数，一是避免参数表过长（clippy `too_many_arguments`），
/// 二是这两个量总是一起从配置里取、一起传下去。
#[derive(Clone, Copy, Debug)]
struct ConflictSampling {
    /// 在排序后冲突列表的前 N 条里抽样
    window: usize,
    /// 落点是否按剩余配额加权（false = 均匀随机）
    key_by_load: bool,
}

/// 尝试解决冲突：将冲突组中的一个移动到新键位
///
/// `sampling.key_by_load = true` 时新键按剩余配额加权采样（见 [`pick_key_by_headroom`]），
/// 否则退回均匀随机；其余逻辑（窗口抽样、缓存新鲜度校验、两组的先后顺序）不变。
fn try_resolve_conflict(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    collisions: &[(usize, usize, usize)],
    sampling: ConflictSampling,
    temp: f64,
    rng: &mut impl Rng,
) -> bool {
    let sample_window = sampling.window;
    let key_by_load = sampling.key_by_load;
    if collisions.is_empty() || sample_window == 0 {
        return false;
    }

    // 有效窗口 = min(sample_window, 列表长度)
    let window = sample_window.min(collisions.len());
    let start = rng.random_range(0..window);

    // 缓存新鲜度校验：`collisions` 每 conflict_refresh 步才重建一次，而每一步都可能
    // 移动字根组。若某条冲突里的两个组已经不在同一键，说明该冲突早已被消解——
    // 继续拿这条记录去移动，只会把预算花在过期的目标上（低温段尤其明显：
    // 刷新间隔 1000 步内常有数十次成功移动，缓存均值已过期半个周期）。
    // 这里从随机起点向后扫整个窗口，取第一条仍然有效的冲突；全部失效则本步放弃，
    // 由下一步重新抽样（下次刷新会重建列表）。
    let mut picked: Option<(usize, usize)> = None;
    for k in 0..window {
        let (g1, g2, _) = collisions[(start + k) % window];
        // 两个组同键是「仍可能冲突」的必要条件（真正是否重码还取决于汉字拆分是否撞码）。
        if assignment[g1] == assignment[g2] {
            picked = Some((g1, g2));
            break;
        }
    }
    let (g1, g2) = match picked {
        Some(p) => p,
        None => return false,
    };

    // 先搬哪一组：50/50 随机。
    //
    // 这里刻意不做「哪组所在键更超标就先搬它」的源侧加权：能进冲突列表的两组按定义
    // 就在**同一键**上（`find_collision_groups` 按编码分组，采样前还要校验同键），
    // 所以两组的 source pressure 恒等，加了也只会退化成随机——试过一轮（频序LS-*），
    // 结果与纯随机落点无异。要真做源侧权衡，得用「该组自身承担的按键负载」
    // （搬走小负载组损失小、搬走大负载组降载多），那是另一个维度，另开实验。
    let groups_to_try: [usize; 2] = if rng.random_bool(0.5) {
        [g1, g2]
    } else {
        [g2, g1]
    };

    for &gi in groups_to_try.iter() {
        let current_key = assignment[gi];
        let allowed = &ctx.groups[gi].allowed_keys;

        let new_key = if key_by_load {
            match pick_key_by_headroom(ctx, evaluator, gi, current_key, rng) {
                Some(k) => k,
                None => continue,
            }
        } else {
            // 键集最多 KEY_SPACE 个，栈上内联，消除每步堆分配。
            let other_keys: SmallVec<[u8; KEY_SPACE]> = allowed
                .iter()
                .filter(|&&k| k != current_key)
                .copied()
                .collect();

            if other_keys.is_empty() {
                continue;
            }
            other_keys[rng.random_range(0..other_keys.len())]
        };

        if evaluator.try_move(ctx, assignment, gi, new_key, temp, rng) {
            return true;
        }
    }

    false
}

/// 尝试键位重组
fn try_key_reorganization(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut impl Rng,
) -> bool {
    let n = assignment.len();
    if n < 2 {
        return false;
    }

    let k1 = assignment[rng.random_range(0..n)];
    let groups_on_k1 = find_groups_on_key(ctx, assignment, k1);

    if groups_on_k1.is_empty() {
        return false;
    }

    let gi = groups_on_k1[rng.random_range(0..groups_on_k1.len())];
    let allowed = &ctx.groups[gi].allowed_keys;

    // 同上：栈上内联，随机序列与 collect 顺序完全不变。
    let other_keys: SmallVec<[u8; KEY_SPACE]> =
        allowed.iter().filter(|&&k| k != k1).copied().collect();

    if other_keys.is_empty() {
        return false;
    }

    let new_key = other_keys[rng.random_range(0..other_keys.len())];
    evaluator.try_move(ctx, assignment, gi, new_key, temp, rng)
}

/// 三组循环交换 (g1←k2, g2←k3, g3←k1) — 增量评估
fn try_triple_swap(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut impl Rng,
) -> bool {
    let n = assignment.len();
    if n < 3 {
        return false;
    }

    let indices: Vec<usize> = (0..n).choose_multiple(rng, 3);
    if indices.len() < 3 {
        return false;
    }

    let [g1, g2, g3] = [indices[0], indices[1], indices[2]];
    let [k1, k2, k3] = [assignment[g1], assignment[g2], assignment[g3]];

    // 三个键都相同则无意义
    if k1 == k2 && k2 == k3 {
        return false;
    }

    // 检查循环交换的合法性: g1→k2, g2→k3, g3→k1
    if !ctx.groups[g1].allowed_keys.contains(&k2)
        || !ctx.groups[g2].allowed_keys.contains(&k3)
        || !ctx.groups[g3].allowed_keys.contains(&k1)
    {
        return false;
    }

    let old_score = evaluator.get_score(ctx, assignment);
    // 回滚时直接写回，省一次整表遍历（回滚路径要尽快返回）
    let 旧种数 = evaluator.编码种数();

    // 更新 key_weighted_usage
    for &(gi, old_k, new_k) in &[(g1, k1, k2), (g2, k2, k3), (g3, k3, k1)] {
        if old_k == new_k {
            continue;
        }
        for &ci in &ctx.group_to_chars[gi] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                    evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                    evaluator.key_weighted_usage[new_k as usize] += freq_f;
                }
            }
        }
    }

    // 执行交换
    assignment[g1] = k2;
    assignment[g2] = k3;
    assignment[g3] = k1;

    // 增量更新受影响的汉字编码
    for &gi in &[g1, g2, g3] {
        for &ci in &ctx.group_to_chars[gi] {
            evaluator.update_char(ctx, assignment, ci);
        }
    }

    evaluator.score_dirty = true;
    let new_score = evaluator.get_score(ctx, assignment);
    let delta = new_score - old_score;

    // 硬约束：[targets] 里的 *_max 在自定义邻域（三交换/整键互换）上同样要生效。
    // 随机数照常抽取（先算「概率是否命中」再判约束），否则会改变随机流、
    // 让既有种子的结果无法复现。
    let 概率命中 = delta <= 0.0 || rng.random::<f64>() < (-delta / temp).exp();
    if 概率命中 && evaluator.constraints_ok(ctx) {
        true
    } else {
        // 回滚 key_weighted_usage
        for &(gi, old_k, new_k) in &[(g1, k2, k1), (g2, k3, k2), (g3, k1, k3)] {
            if old_k == new_k {
                continue;
            }
            for &ci in &ctx.group_to_chars[gi] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in &ctx.char_infos[ci].parts {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                        evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                        evaluator.key_weighted_usage[new_k as usize] += freq_f;
                    }
                }
            }
        }

        // 回滚 assignment
        assignment[g1] = k1;
        assignment[g2] = k2;
        assignment[g3] = k3;

        // 回滚编码
        for &gi in &[g1, g2, g3] {
            for &ci in &ctx.group_to_chars[gi] {
                evaluator.update_char(ctx, assignment, ci);
            }
        }

        evaluator.cached_score = old_score;
        evaluator.score_dirty = false;
        evaluator.恢复编码种数(旧种数);
        false
    }
}

/// 整键互换（full_key_swap）：把键 k1 上的所有组与键 k2 上的所有组整体对调。
///
/// 大邻域移动，用于单组 swap/move 跳不出局部最优时一步跨越很大范围。
/// 合法性要求：k1 上的每个组都允许 k2、k2 上的每个组都允许 k1，否则放弃本次移动。
/// 增量评估（key_weighted_usage + update_char），接受/回滚模式与 try_triple_swap 一致。
pub(crate) fn try_full_key_swap(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut impl Rng,
) -> bool {
    let n = assignment.len();
    if n < 2 {
        return false;
    }
    // 选两个当前被占用、且键位不同的键
    let g1 = rng.random_range(0..n);
    let k1 = assignment[g1];
    let g2 = rng.random_range(0..n);
    let k2 = assignment[g2];
    if k1 == k2 {
        return false;
    }

    let groups_a = find_groups_on_key(ctx, assignment, k1);
    let groups_b = find_groups_on_key(ctx, assignment, k2);
    if groups_a.is_empty() || groups_b.is_empty() {
        return false;
    }
    // 合法性：整键对调后每组仍在其 allowed_keys 内
    if !groups_a
        .iter()
        .all(|&gi| ctx.groups[gi].allowed_keys.contains(&k2))
        || !groups_b
            .iter()
            .all(|&gi| ctx.groups[gi].allowed_keys.contains(&k1))
    {
        return false;
    }

    let affected: SmallVec<[usize; 64]> = groups_a.iter().chain(groups_b.iter()).copied().collect();

    let old_score = evaluator.get_score(ctx, assignment);
    // 回滚时直接写回，省一次整表遍历（回滚路径要尽快返回）
    let 旧种数 = evaluator.编码种数();

    // 更新 key_weighted_usage
    for &gi in &affected {
        let old_k = assignment[gi];
        let new_k = if old_k == k1 { k2 } else { k1 };
        let gfs = ctx.group_freq_sum[gi];
        evaluator.key_weighted_usage[old_k as usize] -= gfs;
        evaluator.key_weighted_usage[new_k as usize] += gfs;
    }
    // 执行对调
    for &gi in &affected {
        assignment[gi] = if assignment[gi] == k1 { k2 } else { k1 };
    }
    // 增量更新受影响汉字的编码
    for &gi in &affected {
        for &ci in &ctx.group_to_chars[gi] {
            evaluator.update_char(ctx, assignment, ci);
        }
    }
    evaluator.score_dirty = true;
    let new_score = evaluator.get_score(ctx, assignment);
    let delta = new_score - old_score;

    // 硬约束：[targets] 里的 *_max 在自定义邻域（三交换/整键互换）上同样要生效。
    // 随机数照常抽取（先算「概率是否命中」再判约束），否则会改变随机流、
    // 让既有种子的结果无法复现。
    let 概率命中 = delta <= 0.0 || rng.random::<f64>() < (-delta / temp).exp();
    if 概率命中 && evaluator.constraints_ok(ctx) {
        true
    } else {
        // 回滚 usage：从「新键」扣、给「旧键」加
        for &gi in &affected {
            let cur_k = assignment[gi];
            let old_k = if cur_k == k1 { k2 } else { k1 };
            let gfs = ctx.group_freq_sum[gi];
            evaluator.key_weighted_usage[cur_k as usize] -= gfs;
            evaluator.key_weighted_usage[old_k as usize] += gfs;
        }
        // 回滚 assignment
        for &gi in &affected {
            assignment[gi] = if assignment[gi] == k1 { k2 } else { k1 };
        }
        // 回滚编码
        for &gi in &affected {
            for &ci in &ctx.group_to_chars[gi] {
                evaluator.update_char(ctx, assignment, ci);
            }
        }
        evaluator.cached_score = old_score;
        evaluator.score_dirty = false;
        evaluator.恢复编码种数(旧种数);
        false
    }
}

// =========================================================================
// 🔧 增强版爬山算法
// =========================================================================

/// 增强版爬山：结合冲突导向的邻域操作
fn enhanced_hill_climb(
    ctx: &OptContext,
    cfg: &Config,
    init: Vec<u8>,
    rng: &mut impl Rng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let n = assignment.len();
    if n == 0 {
        let 分 = evaluator.get_score(ctx, &assignment);
        return (assignment, 分);
    }

    let zero_temp = 1e-15;
    // 冲突算子的三个参数与主循环同源：精炼段是同一份搜索的收尾，不该自带一套口径。
    // 原先这里硬编码 window=20 + 按个数排序 + 均匀落点，配置改了它不跟随。
    let conflict_weight_by_freq = cfg.annealing.conflict_weight_by_freq;
    let conflict_sampling = ConflictSampling {
        window: cfg.annealing.conflict_sample_window,
        key_by_load: cfg.annealing.conflict_key_by_load,
    };
    let mut no_improve_count = 0usize;
    // 冲突缓存懒构建：warmup 阶段大量调用本函数，若一步都没走到冲突分支，
    // 就不该为它付一次全表分桶的代价（原实现在函数开头无条件算一遍）。
    let mut conflict_cache: Option<CollisionCache> = None;

    for step in 0..max_steps {
        let op_type = step % 10;

        let success = match op_type {
            0..=3 => {
                let cache = conflict_cache.get_or_insert_with(|| {
                    CollisionCache::new(
                        ctx,
                        &assignment,
                        conflict_weight_by_freq,
                        cfg.annealing.conflict_refresh_interval,
                    )
                });
                // 与主循环同一套增量同步：精炼段每一步都可能移动组，缓存不再等 200 步。
                cache.sync(ctx, &assignment);
                let collisions = cache.entries();
                if !collisions.is_empty() {
                    try_resolve_conflict(
                        ctx,
                        &mut assignment,
                        &mut evaluator,
                        collisions,
                        conflict_sampling,
                        zero_temp,
                        rng,
                    )
                } else {
                    false
                }
            }
            4..=6 => {
                if n >= 2 {
                    let r1 = rng.random_range(0..n);
                    let r2 = rng.random_range(0..n - 1);
                    let r2 = if r2 >= r1 { r2 + 1 } else { r2 };
                    let k1 = assignment[r1];
                    let k2 = assignment[r2];
                    if ctx.groups[r1].allowed_keys.contains(&k2)
                        && ctx.groups[r2].allowed_keys.contains(&k1)
                    {
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, zero_temp, rng)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            7 => try_triple_swap(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            8 => try_key_reorganization(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            _ => {
                let r = rng.random_range(0..n);
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.random_range(0..allowed.len())];
                evaluator.try_move(ctx, &mut assignment, r, new_k, zero_temp, rng)
            }
        };

        // At zero_temp, success means score improved (only strict improvements accepted)
        if success {
            no_improve_count = 0;
        } else {
            no_improve_count += 1;
        }

        if no_improve_count > n * 10 {
            break;
        }
    }

    let 末分 = evaluator.get_score(ctx, &assignment);
    (assignment, 末分)
}

fn hill_climb_warmup(
    ctx: &OptContext,
    cfg: &Config,
    init: Vec<u8>,
    rng: &mut impl Rng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    enhanced_hill_climb(ctx, cfg, init, rng, max_steps)
}

// =========================================================================
// 坐标下降
// =========================================================================

fn coordinate_descent(ctx: &OptContext, init: Vec<u8>) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let n = assignment.len();
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut improved = true;

    while improved {
        improved = false;
        for gi in 0..n {
            let current_key = assignment[gi];
            let current_score = evaluator.get_score(ctx, &assignment);

            let mut best_key = current_key;
            let mut best_score = current_score;

            for &k in &ctx.groups[gi].allowed_keys {
                if k == current_key {
                    continue;
                }

                // 增量前向：移动到候选键
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[assignment[gi] as usize] -= freq_f;
                            evaluator.key_weighted_usage[k as usize] += freq_f;
                        }
                    }
                }

                let prev_key = assignment[gi];
                assignment[gi] = k;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.score_dirty = true;
                let score = evaluator.get_score(ctx, &assignment);

                // 硬约束：越界的候选即使分数更低也不接受（软惩罚拉不住时解会跑到约束外），
                // 与 try_move / 三交换 / 整键互换的判定保持一致。
                if score < best_score - 1e-12 && evaluator.constraints_ok(ctx) {
                    best_score = score;
                    best_key = k;
                }

                // 回滚
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[k as usize] -= freq_f;
                            evaluator.key_weighted_usage[prev_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = prev_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.cached_score = current_score;
                evaluator.score_dirty = false;
            }

            if best_key != current_key {
                // 应用最优移动
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[current_key as usize] -= freq_f;
                            evaluator.key_weighted_usage[best_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = best_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                evaluator.score_dirty = true;
                improved = true;
            }
        }
    }

    let score = evaluator.get_score(ctx, &assignment);
    (assignment, score)
}

// =========================================================================
// 🎯 多起点初始化入口
// =========================================================================

// =========================================================================
// 🎲 可复现随机源（`--rng-seed`）
// =========================================================================
/// 用途标签：让同一线程里的不同随机流互不相关。
pub const 用途_初始解: u64 = 0xA11C_0000_0000_0001;
pub const 用途_主循环: u64 = 0xA11C_0000_0000_0002;
pub const 用途_播种补全: u64 = 0xA11C_0000_0000_0003;

/// 从 CLI 的 `--rng-seed` 派生「线程 × 用途」独立的确定性种子。
///
/// 线程号必须参与派生：否则 N 个线程会从同一条随机流起步，
/// 多线程优化退化成 N 份完全相同的搜索轨迹。
pub fn 派生种子(主种子: u64, 线程号: usize, 用途: u64) -> u64 {
    主种子 ^ (线程号 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 用途
}

/// 校准基准（尺度 / 温度）专用的固定种子：换它等于换一整套校准口径，务必别乱动。
#[allow(dead_code)]
const 校准种子: u64 = 0x636F_6465_6765_6E69;

pub fn multi_start_init(ctx: &OptContext, cfg: &Config, thread_id: usize) -> Vec<u8> {
    let mut rng = rand::rng();
    multi_start_init_with(ctx, cfg, thread_id, &mut rng)
}

/// 固定种子的**确定性**多起点初始化。
///
/// 供尺度校准当基准用：既保住 #8 要的「跨 run 可复现」
/// （同输入同配置必得同一初解），又不会像 `deterministic_init` 那样给出劣质基准
/// ——后者是 `allowed[gi % len]` 轮转分配，实测重码数是贪心解的 3 倍以上，
/// 拿它当基准会把 `scale = 1/基准` 带偏。
pub fn multi_start_init_seeded(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
    seed: u64,
) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    multi_start_init_with(ctx, cfg, thread_id, &mut rng)
}

fn multi_start_init_with(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
    rng: &mut impl Rng,
) -> Vec<u8> {
    let n = ctx.num_groups;
    if n == 0 {
        return vec![];
    }

    // 多起点候选数：组数不多时多撒几把（每次爬山成本低），规模大时减半以控总耗时。
    let n_candidates: usize = if n < 1000 { 50 } else { 30 };
    let warmup_steps = (n * 30).clamp(2000, 50_000);

    // 先按顺序把「每个候选的独立随机种子」抽出来。
    //
    // 这样每个候选各自持有一份独立的 `StdRng`，候选之间再无任何共享可变状态，
    // 于是既能并行跑，又保住了 #8 的「同输入同配置 → 同一结果」：
    // 种子序列由传入的 `rng` 顺序生成，与并行调度顺序完全无关。
    // （代价是候选 i+1 的初始解不再受候选 i 消耗 rng 的影响——多起点本来就要求
    //   各候选相互独立，这里反而更符合本意。）
    let seeds: Vec<u64> = (0..n_candidates).map(|_| rng.random()).collect();

    // Init T0 的全部成本就是「n_candidates × warmup_steps 步爬山」，而候选之间彼此独立
    // （各自的 Evaluator、各自的 rng，`ctx`/`cfg` 只读）。按候选并行后，
    // 4 核时 Init 段墙钟约降到 1/4，而初始解质量一字不变——这是纯提速、零风险的一刀。
    //
    // 注：外层 SA 也是 rayon 并行，这里是嵌套并行；rayon 的 work-stealing 支持嵌套，
    // 不会死锁，且空闲 worker 会来帮忙，实际并行度仍受全局线程池大小约束。
    let results: Vec<(usize, &'static str, Vec<u8>, f64)> = (0..n_candidates)
        .into_par_iter()
        .map(|trial| {
            let mut crng = StdRng::seed_from_u64(seeds[trial]);
            let (strategy_name, candidate) = match trial % 5 {
                0 => ("均衡贪心", greedy_balance_init(ctx, cfg, &mut crng)),
                1 | 2 => ("频率贪心", frequency_greedy_init(ctx, &mut crng)),
                3 => ("分散贪心", spread_greedy_init(ctx, &mut crng)),
                _ => ("纯随机", random_valid_init(ctx, &mut crng)),
            };

            let (refined, score) = hill_climb_warmup(ctx, cfg, candidate, &mut crng, warmup_steps);
            (trial, strategy_name, refined, score)
        })
        .collect();

    // 按 trial 顺序回放择优。`into_par_iter().collect::<Vec<_>>()` 保证保序，
    // 因此「严格更优才替换」的语义与打印顺序都与串行版逐字一致（含多候选同分时取靠前者）。
    let mut best_assignment: Option<Vec<u8>> = None;
    let mut best_score = f64::MAX;

    for (trial, strategy_name, refined, score) in results {
        if score < best_score {
            best_score = score;
            best_assignment = Some(refined);

            if thread_id == 0 {
                println!(
                    "   [Init T0] 候选 {}/{} 策略={} → 预热后得分: {:.4} ✓",
                    trial + 1,
                    n_candidates,
                    strategy_name,
                    score
                );
            }
        }
    }

    let best = best_assignment.unwrap();

    if n <= 500 {
        let (polished, polished_score) = coordinate_descent(ctx, best.clone());
        if thread_id == 0 {
            println!(
                "   [Init T0] 坐标下降: {:.4} → {:.4}",
                best_score, polished_score
            );
        }
        if polished_score < best_score {
            return polished;
        }
    }

    best
}

/// 校准基准用的初始解：**确定性的贪心多起点**（固定种子）。
///
/// #8 一开始改成 `deterministic_init`，为了「改种子不改 scale」——方向对，但取错了基准：
/// 轮转分配是劣质解（实测 1204 重码 / 2.53%，而贪心解 379 / 0.60%）。由于
/// `scale = 1/基准`，劣质基准会把评分口径整体带偏。
/// 现在改为「固定种子的 multi_start_init」：确定性照样成立，基准质量与上游一致。
#[allow(dead_code)]
pub fn smart_init(ctx: &OptContext, cfg: &Config) -> Vec<u8> {
    multi_start_init_seeded(ctx, cfg, usize::MAX, 校准种子)
}

// =========================================================================
// 🔥 模拟退火主循环
// =========================================================================

/// 舒适区温度（绝对值）。
///
/// 两种口径，**绝对值优先**：
/// - 配置里给了 `comfort_temp`（> 0）→ 直接用它，与上游 Dieken/code_genie 的
///   `annealing.comfort_temp` 语义完全一致，两边可用同一个数直接对照；
/// - 否则按「降温曲线上的位置」`comfort_progress`（0~1）换算：
///   `T(p) = t_start · (t_end/t_start)^p`。用进度而不是温度比例表达，是因为温度本身
///   随评分尺度变（由配置显式给出），而「减速带落在曲线的第几成」是尺度无关、且能直接调的量。
pub(crate) fn 舒适区温度(cfg: &Config) -> f64 {
    // 绝对温度优先：显式配置时不参与任何曲线换算。
    if let Some(t) = cfg.annealing.comfort_temp
        && t.is_finite()
        && t > 0.0
    {
        return t;
    }
    let ts = cfg.annealing.temp_start;
    let te = cfg.annealing.temp_end;
    // 温度未标定（NaN 或非正数）时不换算，直接返回 t_start 由调用方兜底。
    if ts.is_nan() || ts <= 0.0 || te.is_nan() || te <= 0.0 {
        return ts;
    }
    let p = if cfg.annealing.comfort_progress.is_nan() {
        // NaN 会让整条曲线退化成线性（gaussian 传染 NaN），按默认中点处理更可控。
        0.5
    } else {
        cfg.annealing.comfort_progress.clamp(0.0, 1.0)
    };
    ts * (te / ts).powf(p)
}

/// 可断点续算的模拟退火主循环。
///
/// - `stop_flag`：Ctrl-C 共享停止标志，置位时线程写出最新 checkpoint 并提前返回（`interrupted=true`）。
/// - `resume`：若为 `Some(tc)`，从该线程检查点恢复（跳过 multi_start_init），从 `tc.current_step` 续算。
/// - `ckpt_dir`：checkpoint 目录；为 `None` 时不写 checkpoint（向后兼容入口）。
/// - `checkpoint_interval`：每隔多少步写一次 checkpoint（`usize::MAX` 实际等于不周期写）。
/// - `seed`：若为 `Some(s)`（仅 fresh 路径生效），用 `s` 作为退火初始分配替代 `multi_start_init`
///   （从既有结果播种微调）；走 fresh 路径（起始步 0），
///   与 resume 续算互斥（`resume.is_some()` 时忽略 `seed`）。
/// - `rng_seed`：`Some(s)` 时用 `s` 派生确定性的 `StdRng`，使**同配置 + 同种子 +
///   同线程数**的 fresh 运行逐位可复现；`None` 时沿用系统熵（历史行为）。
///   注意 checkpoint 不序列化 RNG 状态，故「一次跑完」与「中断后续算」仍不等价。
#[allow(clippy::too_many_arguments)]
pub fn simulated_annealing_resumable(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
    stop_flag: &Arc<AtomicBool>,
    resume: Option<&ThreadCheckpoint>,
    ckpt_dir: Option<&Path>,
    checkpoint_interval: usize,
    seed: Option<&[u8]>,
    rng_seed: Option<u64>,
) -> SaResult {
    let mut rng: StdRng = match rng_seed {
        Some(s) => StdRng::seed_from_u64(派生种子(s, thread_id, 用途_主循环)),
        None => StdRng::from_rng(&mut rand::rng()),
    };

    let mut assignment;
    let start_step: usize;
    // 上一次写出（或 resume 恢复）的 checkpoint 时间戳，供归档命名（需求 13）。
    let mut prev_ts: Option<String> = resume.map(|tc| tc.timestamp.clone());

    // 初始化或从检查点恢复（需求 11.1）。
    // - fresh：multi_start_init 起步。
    // - resume：用检查点 assignment 重建评估器，从 current_step 续算。
    let mut evaluator;
    if let Some(tc) = resume {
        assignment = tc.assignment.clone();
        start_step = tc.current_step;
        evaluator = Evaluator::new(ctx, &assignment);
    } else {
        // fresh：seed 存在则用种子作为初始解（从既有结果播种），否则 multi_start_init。
        assignment = match seed {
            Some(s) => s.to_vec(),
            None => match rng_seed {
                Some(s) => multi_start_init_seeded(
                    ctx,
                    cfg,
                    thread_id,
                    派生种子(s, thread_id, 用途_初始解),
                ),
                None => multi_start_init(ctx, cfg, thread_id),
            },
        };
        start_step = 0;
        evaluator = Evaluator::new(ctx, &assignment);
    }

    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx, &assignment);
    let mut best_metrics = evaluator.get_metrics(ctx);

    if thread_id == 0 {
        let m = &best_metrics;
        let scores = evaluator.get_metric_scores(ctx);
        println!(
            "   [T0] 初始化完成 | 得分: {:.4} | 重码: {}({:.4}) 重码率: {:.4}%({:.4}) 当量: {:.4}({:.4}) CV: {:.4}({:.4}) 分布: {:.4}({:.4})",
            best_score,
            m.collision_count, scores.collision_count,
            m.collision_rate * 100.0, scores.collision_rate,
            m.equiv_mean, scores.equivalence,
            m.equiv_cv, scores.equiv_cv,
            m.dist_deviation, scores.distribution,
        );
    }

    let steps = cfg.annealing.total_steps;
    let n_groups = assignment.len();
    if n_groups == 0 {
        return SaResult {
            assignment: best_assignment,
            score: best_score,
            metrics: best_metrics,
            interrupted: false,
        };
    }

    let schedule = TemperatureSchedule::build(
        cfg.annealing.temp_start,
        cfg.annealing.temp_end,
        舒适区温度(cfg),
        cfg.annealing.comfort_width,
        cfg.annealing.comfort_slowdown,
    );

    if thread_id == 0 {
        schedule.print_preview(steps);
    }

    let mut temp_multiplier = 1.0f64;
    let min_improve_steps = cfg.min_improve_steps();
    let reheat_decay = if min_improve_steps > 0 {
        (0.01f64).powf(1.0 / min_improve_steps as f64)
    } else {
        0.99
    };

    let mut steps_since_improve = 0usize;
    let mut last_best_score = best_score;

    let report_interval = (steps / 20).max(1);

    let swap_prob_base = cfg.annealing.swap_probability;

    // 冲突导向算子状态（仅在启用时维护；关闭时零开销且不消耗额外随机数，保持 RNG 序列与原实现一致）
    let conflict_prob = cfg.annealing.conflict_probability;
    let conflict_refresh = cfg.annealing.conflict_refresh_interval;
    let conflict_window = cfg.annealing.conflict_sample_window;
    let conflict_weight_by_freq = cfg.annealing.conflict_weight_by_freq;
    // 落点是否按负载（剩余配额）加权；关闭则退回改造前的均匀随机
    let conflict_key_by_load = cfg.annealing.conflict_key_by_load;
    let conflict_enabled = conflict_prob > 0.0;
    // 冲突缓存：编码桶增量维护 + 条目延迟重建（见 `CollisionCache` 文档）。
    // 关闭时不构建；`conflict_prob = 0` 时也从不在采样路径 sync，保持零开销。
    let mut conflict_cache = if conflict_enabled {
        Some(CollisionCache::new(
            ctx,
            &assignment,
            conflict_weight_by_freq,
            conflict_refresh,
        ))
    } else {
        None
    };

    let sa_start = Instant::now();

    // === 进度日志速率窗口状态 ===
    // 用「自上次进度汇报以来」的窗口统计计算吞吐与接受率，而非自退火开始的累计平均：
    // - 窗口吞吐：能反映当前阶段的真实速度（累计平均会被早期高温段稀释、迟钝）。
    // - 窗口接受率：能读出当前退火阶段（高温接近 1、趋冻接近 0），累计接受率被早期主导、几乎不动。
    // 窗口锚点（起始时刻/步号）在每次进度汇报后重置。窗口锚点初始化为本次会话起点
    //（resume 时 = start_step / 恢复时刻），从而续算的窗口速率只统计「本次会话新增」，
    // 不会把续算前的历史步数算进来（修复 resume 路径吞吐爆表）。
    let mut window_start = sa_start;
    let mut window_start_step = start_step;
    let mut win_accepted: u64 = 0;
    let mut win_proposed: u64 = 0;

    // === 从检查点恢复控制状态（需求 11.1/11.2/11.3）===
    if let Some(tc) = resume {
        best_assignment = tc.best_assignment.clone();
        best_score = tc.best_score;
        best_metrics = tc.best_metrics;
        temp_multiplier = tc.temp_multiplier;
        steps_since_improve = tc.steps_since_improve;
        last_best_score = tc.last_best_score;
        if thread_id == 0 {
            println!(
                "   [T0] 从检查点恢复 | 步数: {}/{} | 最优得分: {:.4}",
                start_step, steps, best_score
            );
        }
    }

    // checkpoint 写出闭包：组装当前线程检查点并归档旧版后原子写（需求 1/13）。
    // 写失败仅告警、不中断退火（需求 10.2）。`step` 为当前步（即下次从此步续算）。
    let write_ckpt = |step: usize,
                      assignment: &[u8],
                      best_assignment: &[u8],
                      best_score: f64,
                      best_metrics: Metrics,
                      temp_multiplier: f64,
                      steps_since_improve: usize,
                      last_best_score: f64,
                      prev_ts: &mut Option<String>| {
        if let Some(dir) = ckpt_dir {
            let tc = ThreadCheckpoint {
                thread_id,
                timestamp: checkpoint::now_timestamp_ms(),
                assignment: assignment.to_vec(),
                current_step: step,
                best_assignment: best_assignment.to_vec(),
                best_score,
                best_metrics,
                temp_multiplier,
                steps_since_improve,
                last_best_score,
            };
            match checkpoint::save_thread_checkpoint(&tc, dir, prev_ts.as_deref()) {
                Ok(()) => *prev_ts = Some(tc.timestamp),
                Err(e) => eprintln!("⚠️ [T{}] 写 checkpoint 失败: {}", thread_id, e),
            }
        }
    };

    // 主循环（从 start_step 开始，支持断点续算）
    for step in start_step..steps {
        // === Ctrl-C 停止检查（每 STOP_CHECK_STRIDE 步）：写最新 checkpoint 后提前返回（需求 8.2）===
        if step % STOP_CHECK_STRIDE == 0 && step > start_step && stop_flag.load(Ordering::Relaxed) {
            if thread_id == 0 {
                println!("\n   [T0] 收到停止信号，正在写出 checkpoint...");
            }
            write_ckpt(
                step,
                &assignment,
                &best_assignment,
                best_score,
                best_metrics,
                temp_multiplier,
                steps_since_improve,
                last_best_score,
                &mut prev_ts,
            );
            return SaResult {
                assignment: best_assignment,
                score: best_score,
                metrics: best_metrics,
                interrupted: true,
            };
        }
        // === 周期写 checkpoint（每 checkpoint_interval 步，需求 1.2）===
        if ckpt_dir.is_some() && step > start_step && step % checkpoint_interval == 0 {
            write_ckpt(
                step,
                &assignment,
                &best_assignment,
                best_score,
                best_metrics,
                temp_multiplier,
                steps_since_improve,
                last_best_score,
                &mut prev_ts,
            );
        }

        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        // 邻域分发：启用时每步抽取一次 r 决定走冲突路径还是既有 swap/move；
        // 关闭时跳过抽样，直接走既有分发，保持 RNG 消耗序列与原实现一致。
        // 每步恰好提议一次邻域移动；`proposal_accepted` 记录本步是否被（含概率）接受，用于窗口接受率。
        let mut proposal_accepted = false;
        let did_conflict = if conflict_enabled {
            let r = rng.random::<f64>();
            if r < conflict_prob {
                // 采样前增量同步：把上一步可能的移动并进编码桶，脏编码够多才重排条目。
                // 这是替代「每 1000 步全量重建」的关键——缓存不再成块过期。
                let cache = conflict_cache
                    .as_mut()
                    .expect("conflict_enabled 为真时缓存必然已构建");
                cache.sync(ctx, &assignment);
                let collisions = cache.entries();
                if !collisions.is_empty() {
                    proposal_accepted = try_resolve_conflict(
                        ctx,
                        &mut assignment,
                        &mut evaluator,
                        collisions,
                        ConflictSampling {
                            window: conflict_window,
                            key_by_load: conflict_key_by_load,
                        },
                        temp,
                        &mut rng,
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !did_conflict {
            let swap_prob =
                swap_prob_base + (1.0 - swap_prob_base) * (step as f64 / steps as f64) * 0.3;

            // 整键互换（大邻域）：低概率触发，用于跳出单组 swap/move 的局部最优。
            let fks_prob = cfg.annealing.full_key_swap_probability;
            if fks_prob > 0.0 && n_groups >= 2 && rng.random::<f64>() < fks_prob {
                proposal_accepted =
                    try_full_key_swap(ctx, &mut assignment, &mut evaluator, temp, &mut rng);
            } else if rng.random::<f64>() < swap_prob && n_groups >= 2 {
                let r1 = rng.random_range(0..n_groups);
                let r2 = rng.random_range(0..n_groups - 1);
                let r2 = if r2 >= r1 { r2 + 1 } else { r2 };

                let k1 = assignment[r1];
                let k2 = assignment[r2];
                if k1 != k2
                    && ctx.groups[r1].allowed_keys.contains(&k2)
                    && ctx.groups[r2].allowed_keys.contains(&k1)
                {
                    proposal_accepted =
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
                } else {
                    let r = r1;
                    let allowed = &ctx.groups[r].allowed_keys;
                    let new_k = allowed[rng.random_range(0..allowed.len())];
                    proposal_accepted =
                        evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
                }
            } else {
                let r = rng.random_range(0..n_groups);
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.random_range(0..allowed.len())];
                proposal_accepted =
                    evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
            }
        }

        // 窗口接受率累计（每步一次提议）。
        win_proposed += 1;
        if proposal_accepted {
            win_accepted += 1;
        }

        let current_score = evaluator.get_score(ctx, &assignment);
        if current_score < best_score {
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            steps_since_improve = 0;

            if thread_id == 0 && best_score <= last_best_score - 0.9 {
                let m = best_metrics;
                let elapsed = sa_start.elapsed().as_secs_f64();
                // 自本次会话起的平均吞吐：用 (step - start_step) 而非绝对 step，
                // 否则 resume 续算时 step 从历史步号起算、elapsed 从 0 起算会使速率瞬间爆表。
                let speed = if elapsed > 0.0 {
                    (step - start_step) as f64 / elapsed
                } else {
                    0.0
                };
                let scores = evaluator.get_metric_scores(ctx);
                println!(
                    "   [T0] 步数 {}/{} | 均速 {:.1} 万步/分钟 | 温度 {:.6} | 重码:{}({:.4}) 重码率:{:.4}%({:.4}) 当量:{:.4}({:.4}) CV:{:.4}({:.4}) 分布:{:.4}({:.4}) | 得分: {:.4}",
                    step, steps, speed * 60.0 / 10000.0, temp,
                    m.collision_count, scores.collision_count,
                    m.collision_rate * 100.0, scores.collision_rate,
                    m.equiv_mean, scores.equivalence,
                    m.equiv_cv, scores.equiv_cv,
                    m.dist_deviation, scores.distribution,
                    best_score
                );
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        if steps_since_improve > min_improve_steps {
            temp_multiplier = cfg.annealing.reheat_factor;
            steps_since_improve = 0;

            if thread_id == 0 {
                let elapsed = sa_start.elapsed().as_secs_f64();
                // 自本次会话起的平均吞吐（同上，修正 resume 爆表）。
                let speed = if elapsed > 0.0 {
                    (step - start_step) as f64 / elapsed
                } else {
                    0.0
                };
                println!(
                    "   [T0] 步数 {} | 均速 {:.1} 万步/分钟: Reheat ×{:.1} (基温 {:.6})",
                    step,
                    speed * 60.0 / 10000.0,
                    cfg.annealing.reheat_factor,
                    base_temp
                );
            }
        }

        if thread_id == 0 && step % report_interval == 0 && step > 0 {
            let pct = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            let scores = evaluator.get_metric_scores(ctx);

            // === 窗口速率（自上次进度汇报以来，区别于事件行的会话均速）===
            // win_steps/win_elapsed 为本汇报区间内的步数与耗时，反映「当前阶段」真实吞吐；
            // accept_rate 为本区间内（含概率）接受的提议占比，反映退火当前所处阶段。
            let now = Instant::now();
            let win_elapsed = now.duration_since(window_start).as_secs_f64();
            let win_steps = step - window_start_step;
            let win_speed = if win_elapsed > 0.0 {
                win_steps as f64 / win_elapsed
            } else {
                0.0
            };
            let accept_rate = if win_proposed > 0 {
                win_accepted as f64 / win_proposed as f64
            } else {
                0.0
            };
            // 总吞吐：自本次会话起的累计平均速度（= (step - start_step)/总耗时），即原进度行的速度字段。
            // 用 (step - start_step) 而非绝对 step，避免 resume 续算时把历史步数算进来导致爆表。
            let sa_elapsed = sa_start.elapsed().as_secs_f64();
            let avg_speed = if sa_elapsed > 0.0 {
                (step - start_step) as f64 / sa_elapsed
            } else {
                0.0
            };

            let total = evaluator.get_score(ctx, &assignment);
            // 编码种数是非五项指标的设计约束；未启用时不追加，保持既有日志格式与解析脚本可用。
            let 编码项 = if ctx.constraints.enabled() {
                format!(
                    " 编码={}/{}",
                    evaluator.编码种数(),
                    ctx.constraints.max_encodings
                )
            } else {
                String::new()
            };
            println!(
                "   [T0] 进度: {:>3}% | 基温: {:.6} | 重码={}({:.4}) 重码率={:.4}%({:.4}) 当量={:.4}({:.4}) CV={:.4}({:.4}) 分布={:.4}({:.4}) | 当前: {:.4} 🏆最优: {:.4}{}",
                pct, base_temp,
                m.collision_count, scores.collision_count,
                m.collision_rate * 100.0, scores.collision_rate,
                m.equiv_mean, scores.equivalence,
                m.equiv_cv, scores.equiv_cv,
                m.dist_deviation, scores.distribution,
                total, best_score, 编码项
            );
            // 速率行：左起加进度百分比（与进度行对齐）；先「总吞吐」（会话累计均速）
            // 后「窗口吞吐」（自上次汇报以来），再窗口接受率（附原始计数）。
            println!(
                "   [T0] 速率: {:>3}% | 总吞吐 {:>5.1} 万步/分钟 | 窗口吞吐 {:>5.1} 万步/分钟 | 窗口接受率 {:>5.2}% (接受 {}/{} 步)\n",
                pct, avg_speed * 60.0 / 10000.0, win_speed * 60.0 / 10000.0,
                accept_rate * 100.0, win_accepted, win_proposed
            );

            // 重置窗口锚点，使下个汇报区间重新计速/计接受率。
            window_start = now;
            window_start_step = step;
            win_accepted = 0;
            win_proposed = 0;
        }

        // === 收敛采样：每 1% 步数输出一行结构化数据，供 `tools/画收敛曲线.py` 解析 ===
        //
        // 与上面 5% 一次的人工汇报解耦：汇报行是给人看的（字段多、列对齐），
        // 采样行只保留画曲线要用的四个数，格式固定、key=value，改汇报格式不会打断脚本。
        // 40M 步的默认配置下约 100 个采样点，足以看出收敛趋势与后期震荡。
        if thread_id == 0 && step % (steps / 100).max(1) == 0 {
            println!(
                "   [T0] 采样 step={} 得分={:.6} 当前={:.6} 温度={:.6}",
                step, best_score, current_score, temp
            );
        }
    }

    // 重算最佳解基线：`best_score` / `best_metrics` 一律以 `best_assignment` 的真实评估为准，
    // 使精炼的 `final_score < best_score` / `cd_score < best_score` 比较在同一口径下正确
    //（主循环里的 best_score 是搜索过程中记录的，续算/外部播种时可能与当前分配不同步）。
    {
        let mut best_eval = Evaluator::new(ctx, &best_assignment);
        best_score = best_eval.get_score(ctx, &best_assignment);
    }

    // 最终精炼
    if thread_id == 0 {
        println!("   [T0] SA 完成，执行最终精炼...");
    }

    let final_warmup_steps = (n_groups * 50).clamp(5000, 100_000);
    let (final_assignment, final_score) = hill_climb_warmup(
        ctx,
        cfg,
        best_assignment.clone(),
        &mut rng,
        final_warmup_steps,
    );

    if final_score < best_score {
        best_assignment = final_assignment;
        best_score = final_score;
        if thread_id == 0 {
            println!("   [T0] 最终爬山改进 → 得分: {:.4}", best_score);
        }
    }

    if n_groups <= 500 {
        let score_before_cd = best_score;
        let (cd_assignment, cd_score) = coordinate_descent(ctx, best_assignment.clone());
        if cd_score < best_score {
            best_assignment = cd_assignment;
            best_score = cd_score;
            if thread_id == 0 {
                println!(
                    "   [T0] 坐标下降精炼: {:.4} → {:.4}",
                    score_before_cd, best_score
                );
            }
        }
    }

    // 最终上报指标在精炼之后统一重算一次，使日志总结、summary.txt 与 output 文件头三者一致。
    {
        let mut best_eval = Evaluator::new(ctx, &best_assignment);
        best_score = best_eval.get_score(ctx, &best_assignment);
        best_metrics = best_eval.get_metrics(ctx);

        if thread_id == 0 {
            println!(
                "   [T0] 最终得分: {:.4} 重码: {}",
                best_score, best_metrics.collision_count
            );
        }
    }

    SaResult {
        assignment: best_assignment,
        score: best_score,
        metrics: best_metrics,
        interrupted: false,
    }
}

// ===== end simulated_annealing_resumable =====

// =========================================================================
// 🧪 冲突导向算子测试（sa-conflict-operators）
// =========================================================================
#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::config::{Config, TargetsConfig};
    use crate::context::OptContext;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, EQUIV_TABLE_SIZE};
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建最小 OptContext：每个频率对应一个动态组，每组含 1 个字根、1 个单部件汉字。
    /// 不同组的汉字被分到同一键位时即产生重码（用于驱动 find_collision_groups）。
    fn make_ctx(freqs: &[u64], allowed: &[u8]) -> OptContext {
        make_ctx_with_dist(freqs, allowed, [KeyDistConfig::default(); EQUIV_TABLE_SIZE])
    }

    /// 同 `make_ctx`，但可指定各键的目标使用率/惩罚系数（驱动落点加权采样）。
    fn make_ctx_with_dist(
        freqs: &[u64],
        allowed: &[u8],
        key_dist: [KeyDistConfig; EQUIV_TABLE_SIZE],
    ) -> OptContext {
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
        let cfg = Config::default();
        let weights = cfg.get_weight_config();
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            weights,
            TargetsConfig::default(),
        )
    }

    const FREQS: [u64; 5] = [100, 90, 80, 70, 60];
    const ALLOWED: [u8; 3] = [0, 1, 2];

    proptest! {
        // Feature: sa-conflict-operators, Property 1: 采样窗口落在有效范围内
        // 空列表或 window=0 时返回 false 且不修改 assignment；任意 window（含大于列表长度）不 panic。
        #[test]
        fn prop1_sampling_window_bounds(window in 0usize..12) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            let mut rng = rand::rng();

            // 空冲突列表 + 任意 window → false 且 assignment 不变
            let mut a = vec![0u8; FREQS.len()];
            let before = a.clone();
            let mut ev = Evaluator::new(&ctx, &a);
            let r_empty = try_resolve_conflict(&ctx, &mut a, &mut ev, &[], ConflictSampling { window, key_by_load: false }, 1.0, &mut rng);
            prop_assert!(!r_empty);
            prop_assert_eq!(&a, &before);

            // 非空列表（assignment 全 0 → 所有组同键，必有冲突对）
            let cols = find_collision_groups(&ctx, &a, false);
            prop_assert!(!cols.is_empty());

            // window=0 → false 且不变
            let mut a0 = a.clone();
            let mut ev0 = Evaluator::new(&ctx, &a0);
            let r0 = try_resolve_conflict(&ctx, &mut a0, &mut ev0, &cols, ConflictSampling { window: 0, key_by_load: false }, 1.0, &mut rng);
            prop_assert!(!r0);
            prop_assert_eq!(&a0, &a);

            // 任意 window（包含 > len）不 panic：window+1 ∈ [1,12]，覆盖大于 len 的情形
            let mut a1 = a.clone();
            let mut ev1 = Evaluator::new(&ctx, &a1);
            let _ = try_resolve_conflict(&ctx, &mut a1, &mut ev1, &cols, ConflictSampling { window: window + 1, key_by_load: false }, 1.0, &mut rng);
        }

        // Feature: sa-conflict-operators, Property 2: 排序按所选权重降序
        #[test]
        fn prop2_descending_sort(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            for wbf in [false, true] {
                let cols = find_collision_groups(&ctx, &asg, wbf);
                for w in cols.windows(2) {
                    prop_assert!(w[0].2 >= w[1].2);
                }
            }
        }

        // Feature: sa-conflict-operators, Property 3: 排序结果可复现（裁决确定性）
        #[test]
        fn prop3_reproducible(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            for wbf in [false, true] {
                let c1 = find_collision_groups(&ctx, &asg, wbf);
                let c2 = find_collision_groups(&ctx, &asg, wbf);
                prop_assert_eq!(c1, c2);
            }
        }

        // Feature: sa-conflict-operators, Property 4: 两种排序策略产出相同冲突组集合
        #[test]
        fn prop4_same_set(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            let cf = find_collision_groups(&ctx, &asg, false);
            let ct = find_collision_groups(&ctx, &asg, true);
            let mut sf: Vec<(usize, usize)> = cf.iter().map(|t| (t.0, t.1)).collect();
            let mut st: Vec<(usize, usize)> = ct.iter().map(|t| (t.0, t.1)).collect();
            sf.sort_unstable();
            st.sort_unstable();
            prop_assert_eq!(sf, st);
        }
    }

    // 采样窗口边界（需求 4.3 / 4.4）显式单元用例
    #[test]
    fn test_window_zero_and_empty_return_false() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        let mut rng = rand::rng();
        let mut a = vec![0u8; FREQS.len()];
        let before = a.clone();
        let mut ev = Evaluator::new(&ctx, &a);

        // 空列表
        assert!(!try_resolve_conflict(
            &ctx,
            &mut a,
            &mut ev,
            &[],
            ConflictSampling {
                window: 5,
                key_by_load: false,
            },
            1.0,
            &mut rng
        ));
        assert_eq!(a, before);

        // window = 0
        let cols = find_collision_groups(&ctx, &a, false);
        assert!(!cols.is_empty());
        assert!(!try_resolve_conflict(
            &ctx,
            &mut a,
            &mut ev,
            &cols,
            ConflictSampling {
                window: 0,
                key_by_load: false
            },
            1.0,
            &mut rng
        ));
        assert_eq!(a, before);
    }

    // 两种排序策略权重含义（需求 5.1 / 5.2）：全 0 分配下，count 权重为组数、
    // freq 权重为「字频和 − 最高字频」（= 被挤到候选位之后的字频损失）
    #[test]
    fn test_weight_semantics_all_same_key() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        let a = vec![0u8; FREQS.len()]; // 所有组同键，单一冲突编码，所有组互相冲突
        let cnt = find_collision_groups(&ctx, &a, false);
        let frq = find_collision_groups(&ctx, &a, true);
        // count 策略：每个冲突对权重 = 共享该编码的汉字数 = 5
        assert!(cnt.iter().all(|t| t.2 == FREQS.len()));
        // freq 策略：权重 = 字频和 − 最高字频 = 400 − 100 = 300
        let sum: usize = FREQS.iter().map(|&f| f as usize).sum();
        let max: usize = FREQS.iter().copied().max().unwrap() as usize;
        assert!(frq.iter().all(|t| t.2 == sum - max));
        // 每个冲突编码只产出一条代表对（不再按 C(k,2) 枚举重复对）
        assert_eq!(cnt.len(), 1);
        assert_eq!(frq.len(), 1);
        // 代表对取字频贡献最大的两个组：FREQS[0]=100 与 FREQS[1]=90
        assert_eq!((cnt[0].0, cnt[0].1), (0, 1));
    }

    // 缓存新鲜度：冲突记录里的两组已不在同一键时，不应驱动任何移动
    #[test]
    fn test_stale_collision_skipped() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        // 组 0 在键 0、组 1 在键 1 → 这条 (0,1) 冲突记录已过期
        let mut a = vec![0u8, 1, 1, 1, 1];
        let stale = vec![(0usize, 1usize, 100usize)];
        let before = a.clone();
        let mut ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();
        let moved = try_resolve_conflict(
            &ctx,
            &mut a,
            &mut ev,
            &stale,
            ConflictSampling {
                window: 5,
                key_by_load: false,
            },
            1.0,
            &mut rng,
        );
        assert!(!moved, "过期的冲突记录不应驱动移动");
        assert_eq!(a, before, "过期记录不应改动 assignment");
    }

    // 每个冲突编码只产出一条代表对：2 字编码与 3 字编码各一条，不按 C(k,2) 膨胀
    #[test]
    fn test_one_pair_per_collision_code() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        // 前两组同占键 0（2 字重码）、后三组同占键 1（3 字重码）→ 两个冲突编码
        let a = vec![0u8, 0, 1, 1, 1];
        let cols = find_collision_groups(&ctx, &a, false);
        assert_eq!(cols.len(), 2, "每个冲突编码只应产出一条代表对");
        // count 策略权重 = 该编码下的汉字数 → {2, 3}
        let mut weights: Vec<usize> = cols.iter().map(|t| t.2).collect();
        weights.sort_unstable();
        assert_eq!(weights, vec![2, 3]);

        // 未去重时应为 C(2,2)+C(3,2)=4 条，去重后只剩 2 条
        let frq = find_collision_groups(&ctx, &a, true);
        assert_eq!(frq.len(), 2);
    }

    // =========================================================================
    // 🎯 落点按负载（剩余配额）加权采样
    // =========================================================================

    /// 键 0/2 目标 50%（欠配额），键 1 目标 1%（必然超标）
    fn lopsided_dist() -> [KeyDistConfig; EQUIV_TABLE_SIZE] {
        let mut kd = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        for k in [0usize, 2] {
            kd[k] = KeyDistConfig {
                target_rate: 50.0,
                low_penalty: 1.0,
                high_penalty: 1.0,
            };
        }
        kd[1] = KeyDistConfig {
            target_rate: 1.0,
            low_penalty: 1.0,
            high_penalty: 1.0,
        };
        kd
    }

    /// 与真实 `key_distribution.txt` 同形：只罚超标、不罚欠配额（`low_penalty = 0`）。
    fn zero_low_penalty_dist() -> [KeyDistConfig; EQUIV_TABLE_SIZE] {
        let mut kd = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        for k in [0usize, 2] {
            kd[k] = KeyDistConfig {
                target_rate: 50.0,
                low_penalty: 0.0,
                high_penalty: 1.0,
            };
        }
        kd[1] = KeyDistConfig {
            target_rate: 1.0,
            low_penalty: 0.0,
            high_penalty: 1.0,
        };
        kd
    }

    // low_penalty = 0 时也必须真的按配额加权。
    //
    // 这是曾静默失效的回归测试：早期权重写成 `headroom × low_penalty`，
    // 而实际配置里 low_penalty 全是 0，权重恒为 0 → 全部键被跳过 → 每次
    // 采样都退回均匀随机，负载加权从未生效且不留任何痕迹。
    #[test]
    fn test_load_pick_works_with_zero_low_penalty() {
        let ctx = make_ctx_with_dist(&FREQS, &ALLOWED, zero_low_penalty_dist());
        // 5 组全在键 1：键 1 实际 100% 远超 1% 目标，键 0/2 实际 0%、还差 50%
        let a = vec![1u8; FREQS.len()];
        let ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();

        let mut seen = [false; 3];
        for _ in 0..500 {
            let k = pick_key_by_headroom(&ctx, &ev, 0, 1, &mut rng).expect("应有候选键");
            assert_ne!(k, 1, "low_penalty=0 时同样不该选超标键");
            seen[k as usize] = true;
        }
        assert!(
            seen[0] && seen[2],
            "两个欠配额键都应被采到，说明没有退化成固定落点"
        );
    }

    #[test]
    fn test_load_pick_avoids_overloaded_keys() {
        let ctx = make_ctx_with_dist(&FREQS, &ALLOWED, lopsided_dist());
        // 5 组全在键 1 → 键 1 实际 100%（远超 1% 目标），键 0/2 为 0%（欠 50%）
        let a = vec![1u8; FREQS.len()];
        let ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();

        let mut seen = [false; 3];
        for _ in 0..500 {
            let k = pick_key_by_headroom(&ctx, &ev, 0, 1, &mut rng).expect("应有候选键");
            assert_ne!(k, 1, "已超目标使用率的键不应被选中");
            seen[k as usize] = true;
        }
        assert!(seen[0] && seen[2], "两个欠配额的键都应被采到");
    }

    #[test]
    fn test_load_pick_fallback_when_all_key_over() {
        // 所有键目标 0% → 无一欠配额，应回退均匀采样（而非返回 None 卡死）
        let mut kd = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        for slot in kd.iter_mut() {
            *slot = KeyDistConfig {
                target_rate: 0.0,
                low_penalty: 1.0,
                high_penalty: 1.0,
            };
        }
        let ctx = make_ctx_with_dist(&FREQS, &ALLOWED, kd);
        let a = vec![0u8; FREQS.len()];
        let ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();

        let mut seen = [false; 3];
        for _ in 0..300 {
            let k = pick_key_by_headroom(&ctx, &ev, 0, 0, &mut rng).expect("回退也应给出候选");
            assert_ne!(k, 0, "回退采样同样要排除当前键");
            seen[k as usize] = true;
        }
        assert!(seen[1] && seen[2], "全部超标时应覆盖所有非当前键");
    }

    #[test]
    fn test_load_pick_none_when_no_other_key() {
        // 组只允许键 0 且当前就在键 0 → 无落点
        let ctx = make_ctx(&FREQS, &[0u8]);
        let a = vec![0u8; FREQS.len()];
        let ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();
        assert!(pick_key_by_headroom(&ctx, &ev, 0, 0, &mut rng).is_none());
    }

    // 负载加权路径端到端：欠配额的键存在时，冲突移动能真正落子
    #[test]
    fn test_load_weighted_conflict_move_happens() {
        let ctx = make_ctx_with_dist(&FREQS, &ALLOWED, lopsided_dist());
        let mut a = vec![1u8; FREQS.len()];
        let cols = find_collision_groups(&ctx, &a, false);
        assert!(!cols.is_empty());
        let mut ev = Evaluator::new(&ctx, &a);
        let mut rng = rand::rng();

        let moved = try_resolve_conflict(
            &ctx,
            &mut a,
            &mut ev,
            &cols,
            ConflictSampling {
                window: 5,
                key_by_load: true,
            },
            1.0,
            &mut rng,
        );
        assert!(moved, "欠配额键存在时冲突移动应被接受");
        assert!(a.iter().any(|&k| k != 1), "至少一个组应离开超标的键 1");
    }

    // 增量桶维护的正确性：任意一串移动之后，缓存重排出的条目必须与全量重建逐条一致。
    // 这是把「每 1000 步全量重建」换成增量同步的全部风险所在——桶漏删/漏加会直接
    // 让冲突目标错位，而这种错误不会抛异常、只会悄悄拉低收益。
    #[test]
    fn test_collision_cache_matches_full_rebuild() {
        let freqs = [900u64, 700, 500, 300, 100];
        let ctx = make_ctx(&freqs, &ALLOWED);
        let mut a = vec![0u8; freqs.len()];
        let mut rng = StdRng::seed_from_u64(20261103);
        let mut cache = CollisionCache::new(&ctx, &a, true, 0);

        for _ in 0..60 {
            let gi = rng.random_range(0..freqs.len());
            let allowed = &ctx.groups[gi].allowed_keys;
            a[gi] = allowed[rng.random_range(0..allowed.len())];
            cache.sync(&ctx, &a);
        }

        // 强制重排条目后逐条比对（阈值内的条目滞后不是错误，这里只验桶的正确性）
        cache.rebuild_entries(&ctx);
        let mut got = cache.entries().to_vec();
        sort_collisions(&mut got);
        assert_eq!(got, find_collision_groups(&ctx, &a, true));
    }

    // 脏编码累计到阈值应当自动重排，让条目追平全量结果（不需要外部再触发一次重建）
    #[test]
    fn test_collision_cache_auto_rebuild_after_threshold() {
        let freqs = [900u64, 700, 500, 300];
        let ctx = make_ctx(&freqs, &ALLOWED);
        let mut a = vec![0u8; freqs.len()];
        let mut cache = CollisionCache::new(&ctx, &a, true, 0);

        // 反复移动同一组：每次只有 1 个汉字换编码，累计到阈值以上自动触发重排
        let mut auto_rebuilt = false;
        for i in 0..(COLLISION_ENTRY_REBUILD_THRESHOLD + 8) {
            a[0] = if i % 2 == 0 { 1 } else { 0 };
            cache.sync(&ctx, &a);
            // `changed_codes` 归零 = 本次 sync 刚刚重排过条目
            if cache.changed_codes == 0 {
                auto_rebuilt = true;
                break;
            }
        }
        assert!(auto_rebuilt, "脏编码累计到阈值后应自动重排条目");

        let mut got = cache.entries().to_vec();
        sort_collisions(&mut got);
        assert_eq!(
            got,
            find_collision_groups(&ctx, &a, true),
            "脏编码超阈值后条目应已自动追平全量结果"
        );
    }

    // 关闭口径（weight_by_freq = false）下增量路径同样成立
    #[test]
    fn test_collision_cache_count_weight_consistency() {
        let freqs = [900u64, 700, 500, 300];
        let ctx = make_ctx(&freqs, &ALLOWED);
        let mut a = vec![0u8; freqs.len()];
        let mut cache = CollisionCache::new(&ctx, &a, false, 0);

        for i in 0..5 {
            a[i % freqs.len()] = if i % 2 == 0 { 1 } else { 2 };
            cache.sync(&ctx, &a);
        }
        cache.rebuild_entries(&ctx);

        let mut got = cache.entries().to_vec();
        sort_collisions(&mut got);
        assert_eq!(got, find_collision_groups(&ctx, &a, false));
    }
}

// =========================================================================
// 🧪 断点续算测试（annealing-checkpoint-resume, Property 5 / 需求 7/8）
// =========================================================================
#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, EQUIV_TABLE_SIZE};
    use std::collections::HashMap;

    /// 最小 OptContext：n 个单字根组、单部件汉字，仅 2 个允许键制造重码。
    pub(crate) fn make_ctx(n: usize) -> OptContext {
        let allowed: Vec<u8> = vec![0, 1];
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for i in 0..n {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: allowed.clone(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], (100 - i as u64).max(1)));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let cfg = Config::default();
        let weights = cfg.get_weight_config();
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            weights,
            TargetsConfig::default(),
        )
    }

    pub(crate) fn tiny_cfg(total_steps: usize) -> Config {
        let mut cfg = Config::default();
        cfg.annealing.total_steps = total_steps;
        cfg.annealing.checkpoint_interval_ratio = 0.1; // interval = total/10
        cfg
    }

    fn tmp_ckpt_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cg_resume_{}_{}_{}",
            tag,
            std::process::id(),
            checkpoint::now_timestamp_ms().replace('.', "")
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // Feature: annealing-checkpoint-resume, Property 5: resume 续算不重置进度且最优不退化
    #[test]
    fn fresh_run_writes_checkpoint_then_resume_continues() {
        let ctx = make_ctx(8);
        let total_steps = 3000usize;
        let cfg = tiny_cfg(total_steps);
        let interval = checkpoint_interval(total_steps, cfg.annealing.checkpoint_interval_ratio);
        let ckpt_dir = tmp_ckpt_dir("e2e");
        let stop = Arc::new(AtomicBool::new(false));

        // 1) fresh 运行至完成：应周期写出 thread-00.json。
        let r0 = simulated_annealing_resumable(
            &ctx,
            &cfg,
            0,
            &stop,
            None,
            Some(&ckpt_dir),
            interval,
            None,
            None,
        );
        assert!(!r0.interrupted);
        let tpath = checkpoint::thread_path(&ckpt_dir, 0);
        assert!(tpath.exists(), "fresh 运行应写出 thread-00.json");
        let tc = checkpoint::load_thread_checkpoint(&tpath).unwrap();
        // 最后一次周期写发生在 < total_steps 的某个 interval 倍数处。
        assert!(tc.current_step > 0 && tc.current_step < total_steps);
        assert_eq!(tc.current_step % interval, 0);

        // 2) 从该检查点 resume 续算：best 不退化（单调），返回有效结果。
        let r1 = simulated_annealing_resumable(
            &ctx,
            &cfg,
            0,
            &stop,
            Some(&tc),
            Some(&ckpt_dir),
            interval,
            None,
            None,
        );
        assert!(!r1.interrupted);
        assert!(
            r1.score <= tc.best_score + 1e-9,
            "resume 后 best 退化: {} > {}",
            r1.score,
            tc.best_score
        );

        let _ = std::fs::remove_dir_all(&ckpt_dir);
    }

    // 需求 7.9：current_step >= total_steps 的检查点 resume 时跳过主循环、直接收尾，不报错。
    #[test]
    fn resume_finished_checkpoint_skips_loop() {
        let ctx = make_ctx(6);
        let total_steps = 2000usize;
        let cfg = tiny_cfg(total_steps);
        let ckpt_dir = tmp_ckpt_dir("done");
        let stop = Arc::new(AtomicBool::new(false));

        // 先跑一次拿到一个合法 best_assignment 与分量。
        let r0 = simulated_annealing_resumable(
            &ctx,
            &cfg,
            0,
            &stop,
            None,
            Some(&ckpt_dir),
            usize::MAX,
            None,
            None,
        );
        let mut tc = checkpoint::load_thread_checkpoint(&checkpoint::thread_path(&ckpt_dir, 0))
            .unwrap_or(ThreadCheckpoint {
                thread_id: 0,
                timestamp: checkpoint::now_timestamp_ms(),
                assignment: vec![0u8; ctx.num_groups],
                current_step: 0,
                best_assignment: vec![0u8; ctx.num_groups],
                best_score: r0.score,
                best_metrics: r0.metrics,
                temp_multiplier: 1.0,
                steps_since_improve: 0,
                last_best_score: r0.score,
            });
        // 标记为「已完成」：current_step = total_steps。
        tc.current_step = total_steps;

        let r1 = simulated_annealing_resumable(
            &ctx,
            &cfg,
            0,
            &stop,
            Some(&tc),
            Some(&ckpt_dir),
            usize::MAX,
            None,
            None,
        );
        assert!(!r1.interrupted);
        println!(
            "DEBUG r0={} r1={} tc={} fresh(r0)={} fresh(tc)={}",
            r0.score,
            r1.score,
            tc.best_score,
            Evaluator::new(&ctx, &r0.assignment).get_score(&ctx, &r0.assignment),
            Evaluator::new(&ctx, &tc.best_assignment).get_score(&ctx, &tc.best_assignment)
        );
        assert!(r1.score <= tc.best_score + 1e-9);
        let _ = std::fs::remove_dir_all(&ckpt_dir);
    }

    // Feature: seed-optimize-from-result, Property 5: 播种起点不退化（种子被忠实采用）
    #[test]
    fn seed_start_does_not_regress() {
        let ctx = make_ctx(8);
        // 构造一个合法 Seed_Assignment：每组取其 allowed_keys[0]。
        let seed: Vec<u8> = (0..ctx.num_groups)
            .map(|gi| ctx.groups[gi].allowed_keys[0])
            .collect();
        // 种子起点的初始得分（以该种子直接构建评估器）。
        let seed_init_score = Evaluator::new(&ctx, &seed).get_score(&ctx, &seed);

        let total_steps = 500usize; // 极小步数
        let cfg = tiny_cfg(total_steps);
        let stop = Arc::new(AtomicBool::new(false));

        // 以 seed 播种走 fresh 路径（resume=None, ckpt_dir=None）。
        let r = simulated_annealing_resumable(
            &ctx,
            &cfg,
            0,
            &stop,
            None,
            None,
            usize::MAX,
            Some(&seed),
            None,
        );
        assert!(!r.interrupted);
        assert_eq!(r.assignment.len(), ctx.num_groups, "返回分配长度应等于组数");
        assert!(
            r.score <= seed_init_score + 1e-9,
            "退火最优不应差于种子起点: {} > {}",
            r.score,
            seed_init_score
        );
    }
}

// =========================================================================
// 🧪 固定种子可复现性（`--rng-seed`）
// =========================================================================
#[cfg(test)]
mod 种子复现测试 {
    use super::resume_tests::{make_ctx, tiny_cfg};
    use super::*;

    #[test]
    fn 派生种子按线程号与用途隔离() {
        assert_eq!(派生种子(42, 0, 用途_主循环), 派生种子(42, 0, 用途_主循环));
        // 不按线程号派生的话，多线程会从同一条随机流起步，退化成 N 份相同轨迹。
        assert_ne!(派生种子(42, 0, 用途_主循环), 派生种子(42, 1, 用途_主循环));
        assert_ne!(派生种子(42, 0, 用途_主循环), 派生种子(42, 0, 用途_初始解));
    }

    #[test]
    fn 不同主种子产生不同随机流() {
        let mut a = StdRng::seed_from_u64(派生种子(42, 0, 用途_主循环));
        let mut b = StdRng::seed_from_u64(派生种子(43, 0, 用途_主循环));
        assert_ne!(a.random::<u64>(), b.random::<u64>());
    }

    /// 初始解（multi_start_init）与主循环两条随机流都固定后，
    /// 得分与最终分配应逐位相同——这正是参数扫描需要的信噪比前提。
    #[test]
    fn 同种子完全复现() {
        let ctx = make_ctx(8);
        let cfg = tiny_cfg(1200);
        let stop = Arc::new(AtomicBool::new(false));
        let 跑 = |s: u64| {
            simulated_annealing_resumable(
                &ctx,
                &cfg,
                0,
                &stop,
                None,
                None,
                usize::MAX,
                None,
                Some(s),
            )
        };
        let a = 跑(20261107);
        let b = 跑(20261107);
        assert_eq!(a.score.to_bits(), b.score.to_bits(), "同种子得分应逐位相同");
        assert_eq!(a.assignment, b.assignment, "同种子最终分配应相同");
    }
}
