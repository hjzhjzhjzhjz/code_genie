//! 种子微调器：只以「全码当量」为目标对既有 keymap 做穷尽式局部搜索。
//!
//! 与 `optimize` 的区别：
//! - 目标单一：只最小化 `equiv_mean`（全码平均当量），不掺重码/分布/CV 的加权分。
//!   重码只作为**硬上限**（`max_collisions`）出现，防止为了当量把重码放到不可用的程度。
//! - 邻域穷尽：每轮先对所有合法移动/交换做 O(1) 代理估价，取代理最优的前 K 个
//!   再做精确试算（试算后回滚），而不是像 SA 那样随机抽一对碰运气。
//! - 只走下降：不做概率接受，因此不会出现「先变差再找路」的漂移，种子结构不会被打散。
//!
//! 当量口径沿用 `OptContext::calc_equiv_from_parts`：单字当量 = 相邻码位键对
//! `equiv_table[prev][cur]` 的平均值（末码非终止键时补一个虚拟空格键对）。因此改动某个
//! 组的键位只会影响「包含该组的字」，精确试算即重算这些字，成本 O(受影响字数)。

use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::types::EQUIV_TABLE_SIZE;

/// 微调参数。
pub struct TuneOpts {
    /// 最大轮数（每轮最多接受一次移动/交换）
    pub max_rounds: usize,
    /// 每轮精确试算的候选数（从代理估价最优里截取）
    pub candidates: usize,
    /// 重码硬上限：超过则放弃该次移动
    pub max_collisions: Option<usize>,
    /// 当量目标：达到即停止
    pub target_equiv: Option<f64>,
    /// 连续多少轮无改进后退出
    pub max_rounds_without_gain: usize,
    /// 是否允许「单组换键」（把一个组搬到别的键上，会改变每键的组数）
    pub allow_move: bool,
}

impl Default for TuneOpts {
    fn default() -> Self {
        Self {
            max_rounds: 5000,
            candidates: 400,
            max_collisions: None,
            target_equiv: None,
            max_rounds_without_gain: 3,
            allow_move: true,
        }
    }
}

/// 微调结果报告。
pub struct TuneReport {
    pub equiv_start: f64,
    pub equiv_end: f64,
    pub collisions_start: usize,
    pub collisions_end: usize,
    pub rounds: usize,
    pub applied: usize,
}

/// 一个候选操作。
#[derive(Clone, Copy)]
enum Cand {
    /// 两个组互换键位
    Swap(usize, usize),
    /// 单个组搬到另一个键位
    Move(usize, u8),
}

#[inline]
fn snapshot(ev: &Evaluator, ctx: &OptContext) -> (f64, usize) {
    let m = ev.get_metrics(ctx);
    (m.equiv_mean, m.collision_count)
}

/// 把组 `r` 的键位设为 `k` 并重算受影响的字。
#[inline]
fn set_key(ev: &mut Evaluator, ctx: &OptContext, asg: &mut [u8], r: usize, k: u8) {
    if asg[r] == k {
        return;
    }
    asg[r] = k;
    for &ci in &ctx.group_to_chars[r] {
        ev.update_char(ctx, asg, ci);
    }
}

/// 试算一次候选操作，返回 (当量, 重码)；返回前恢复原状。
fn trial(ev: &mut Evaluator, ctx: &OptContext, asg: &mut [u8], cand: Cand) -> (f64, usize) {
    match cand {
        Cand::Move(r, k) => {
            let old = asg[r];
            set_key(ev, ctx, asg, r, k);
            let snap = snapshot(ev, ctx);
            set_key(ev, ctx, asg, r, old);
            snap
        }
        Cand::Swap(r1, r2) => {
            let k1 = asg[r1];
            let k2 = asg[r2];
            set_key(ev, ctx, asg, r1, k2);
            set_key(ev, ctx, asg, r2, k1);
            let snap = snapshot(ev, ctx);
            set_key(ev, ctx, asg, r1, k1);
            set_key(ev, ctx, asg, r2, k2);
            snap
        }
    }
}

/// 应用一次候选操作（不回滚）。
fn apply(ev: &mut Evaluator, ctx: &OptContext, asg: &mut [u8], cand: Cand) {
    match cand {
        Cand::Move(r, k) => set_key(ev, ctx, asg, r, k),
        Cand::Swap(r1, r2) => {
            let k1 = asg[r1];
            let k2 = asg[r2];
            set_key(ev, ctx, asg, r1, k2);
            set_key(ev, ctx, asg, r2, k1);
        }
    }
}

/// 预计算每个键的「当量代价」：当量表第 k 行与第 k 列之和。
///
/// 单字当量是相邻键对代价的平均，所以某键被「当作前键」和「当作后键」的代价都要算上。
/// 这个量只用来给候选排序（代理），精确判定仍走 `trial`。
fn key_costs(ctx: &OptContext) -> [f64; EQUIV_TABLE_SIZE] {
    let mut cost = [0.0f64; EQUIV_TABLE_SIZE];
    for k in 0..EQUIV_TABLE_SIZE {
        let mut s = 0.0;
        for j in 0..EQUIV_TABLE_SIZE {
            s += ctx.equiv_table[k][j] + ctx.equiv_table[j][k];
        }
        cost[k] = s;
    }
    cost
}

/// 对种子做当量微调，返回新的 assignment 与报告。
pub fn tune(ctx: &OptContext, init: Vec<u8>, opts: &TuneOpts) -> (Vec<u8>, TuneReport) {
    let n = ctx.num_groups;
    let mut asg = init;
    if asg.len() < n {
        asg.resize(n, 0);
    }

    // 种子可能给出非法键位（不在该组的 allowed_keys 里）：投影到第一个允许键，
    // 保证后续判定不会基于一个本来就不合法的解。
    let mut repaired = 0usize;
    for r in 0..n {
        let allowed = &ctx.groups[r].allowed_keys;
        if allowed.is_empty() {
            continue;
        }
        if !allowed.contains(&asg[r]) {
            asg[r] = allowed[0];
            repaired += 1;
        }
    }
    if repaired > 0 {
        println!("⚠️ 种子有 {} 个组落在非法键位上，已投影到首个允许键", repaired);
    }

    let mut ev = Evaluator::new_full_only(ctx, &asg);
    let (equiv_start, collisions_start) = snapshot(&ev, ctx);
    let mut cur_e = equiv_start;

    let cost = key_costs(ctx);
    let mut rounds = 0usize;
    let mut applied = 0usize;
    let mut without_gain = 0usize;

    let movable: Vec<usize> = (0..n)
        .filter(|&r| ctx.groups[r].allowed_keys.len() > 1)
        .collect();
    let m = movable.len();
    println!("  可动码位: {} / {}（其余被约束锁死为单键）", m, n);

    while rounds < opts.max_rounds {
        rounds += 1;
        if let Some(t) = opts.target_equiv {
            if cur_e <= t {
                break;
            }
        }

        // === 1) 代理扫描：所有合法移动/交换，按 O(1) 近似增量挑出最可能降当量的 ===
        let mut cand: Vec<(f64, Cand)> = Vec::with_capacity(m * 32);
        for a in 0..m {
            let r1 = movable[a];
            let k1 = asg[r1];
            let f1 = ctx.group_freq_sum[r1];
            let allow1 = &ctx.groups[r1].allowed_keys;

            // 单组移动：r1 从 k1 搬到另一个允许键（组数分布会变，故重码涨得快，靠上限兜住）
            if opts.allow_move {
                for &k in allow1 {
                    if k == k1 {
                        continue;
                    }
                    let d = f1 * (cost[k as usize] - cost[k1 as usize]);
                    if d < 0.0 {
                        cand.push((d, Cand::Move(r1, k)));
                    }
                }
            }

            // 两组互换：保持每键组数不变，只在两组频率差较大时才有收益
            for b in (a + 1)..m {
                let r2 = movable[b];
                let k2 = asg[r2];
                if k1 == k2 {
                    continue;
                }
                if !allow1.contains(&k2) || !ctx.groups[r2].allowed_keys.contains(&k1) {
                    continue;
                }
                let f2 = ctx.group_freq_sum[r2];
                let d = (f1 - f2) * (cost[k2 as usize] - cost[k1 as usize]);
                if d < 0.0 {
                    cand.push((d, Cand::Swap(r1, r2)));
                }
            }
        }
        if cand.is_empty() {
            break;
        }
        cand.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        cand.truncate(opts.candidates);

        // === 2) 精确试算：取当量最低、且不违反重码上限的那一个 ===
        let mut best: Option<(f64, usize, Cand)> = None;
        for (_, c) in &cand {
            let (e, col) = trial(&mut ev, ctx, &mut asg, *c);
            if e > cur_e - 1e-12 {
                continue;
            }
            if let Some(mc) = opts.max_collisions {
                if col > mc {
                    continue;
                }
            }
            if best.as_ref().is_none_or(|b| e < b.0) {
                best = Some((e, col, *c));
            }
        }

        match best {
            Some((e, col, c)) => {
                apply(&mut ev, ctx, &mut asg, c);
                cur_e = e;
                applied += 1;
                without_gain = 0;
                if applied.is_multiple_of(20) || applied <= 5 {
                    let desc = match c {
                        Cand::Move(r, k) => format!("移动 组{}→键{}", r, k),
                        Cand::Swap(a, b) => format!("交换 组{}↔组{}", a, b),
                    };
                    println!(
                        "    第 {:>4} 次 {} → 当量 {:.6}  重码 {}",
                        applied, desc, cur_e, col
                    );
                }
            }
            None => {
                without_gain += 1;
                if without_gain >= opts.max_rounds_without_gain {
                    break;
                }
            }
        }
    }

    let (equiv_end, collisions_end) = snapshot(&ev, ctx);
    (
        asg,
        TuneReport {
            equiv_start,
            equiv_end,
            collisions_start,
            collisions_end,
            rounds,
            applied,
        },
    )
}
