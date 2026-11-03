// =========================================================================
// 📐 自动校准模块
// =========================================================================

use crate::config::TargetsConfig;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::types::{Metrics, ScaleConfig, SimpleMetrics, WeightConfig};
use rand::Rng;

/// 根据初始状态自动校准缩放因子
///
/// 使得不同量纲的指标在得分计算中具有相当的权重。
///
/// **口径：初态观测优先**（与上游 Dieken/code_genie 一致）。
/// `scale = 1 / 初态观测值`，即「一个初态量级的偏离 = 1 分」。
///
/// 早期 #8 版本曾改成「target 优先」，理由是 `1/初值` 会把口径绑死在一次**随机**初态上。
/// 但基准现已换成 `smart_init`（固定种子的贪心多起点，确定性），可复现的问题不复存在；
/// 而 target 优先会带来更严重的失真：`scale = 1/目标值`，目标定得越激进（如当量目标 0.12、
/// 初态 1.28），该项的有效权重就被放大约 10 倍，与上游完全不可比，也把优化器压到单一指标上。
///
/// 因此改为：
/// - 初态观测值 > eps → 用观测值（主路径）；
/// - 观测值≈0（如零重码）→ 退回配置 `[targets]` 的典型值；
/// - 两者都不可用 → `1.0` 兜底，避免 `1/eps` 爆炸。
pub fn calibrate_scales(
    initial_metrics: &Metrics,
    initial_simple: &SimpleMetrics,
    weights: &WeightConfig,
    targets: &TargetsConfig,
) -> ScaleConfig {
    let eps = 1e-9;

    // 参考值选择：observed 优先，target 兜底，最后 1.0。
    let pick = |target: f64, enabled: bool, observed: f64| -> f64 {
        if observed > eps {
            observed
        } else if enabled && target > eps {
            target
        } else {
            1.0
        }
    };

    // 计算活跃的全码指标数量
    let active_count = [
        weights.weight_collision_count,
        weights.weight_collision_rate,
        weights.weight_equivalence,
        weights.weight_equiv_cv,
        weights.weight_distribution,
    ]
    .iter()
    .filter(|&&w| w > 0.0)
    .count();

    let ft = &targets.full_code;

    // 如果只有一个活跃指标，使用默认缩放
    let base = if active_count <= 1 {
        ScaleConfig::default()
    } else {
        // 根据参考值计算缩放因子（scale = 1 / 参考值）
        ScaleConfig {
            collision_count: 1.0
                / pick(
                    ft.collision_count,
                    ft.enabled,
                    initial_metrics.collision_count as f64,
                ),
            collision_rate: 1.0
                / pick(
                    ft.collision_rate,
                    ft.enabled,
                    initial_metrics.collision_rate,
                ),
            equivalence: 1.0 / pick(ft.equivalence, ft.enabled, initial_metrics.equiv_mean),
            equiv_cv: 1.0 / pick(ft.equiv_cv, ft.enabled, initial_metrics.equiv_cv),
            distribution: 1.0 / pick(ft.distribution, ft.enabled, initial_metrics.dist_deviation),
            ..ScaleConfig::default()
        }
    };

    // 如果未启用简码，返回基础配置
    if !weights.enable_simple_code {
        return base;
    }

    let st = &targets.simple_code;

    // 简码频率覆盖损失 = 1 - 覆盖率。其目标参考应为 `1 - 目标覆盖率`（同为“损失”量纲），
    // 不能用覆盖率目标本身，否则量纲不一致。
    let freq_coverage_loss = 1.0 - initial_simple.weighted_freq_coverage;

    // 合并简码缩放因子
    ScaleConfig {
        simple_freq: 1.0
            / pick(
                1.0 - st.freq,
                st.enabled && st.freq > eps,
                freq_coverage_loss,
            ),
        simple_equiv: 1.0 / pick(st.equiv, st.enabled, initial_simple.equiv_mean),
        simple_dist: 1.0 / pick(st.dist, st.enabled, initial_simple.dist_deviation),
        simple_collision_count: 1.0
            / pick(
                st.collision_count,
                st.enabled,
                initial_simple.collision_count as f64,
            ),
        simple_collision_rate: 1.0
            / pick(st.collision_rate, st.enabled, initial_simple.collision_rate),
        ..base
    }
}

/// 自动标定退火温度（参考 libchai 的「调参」思想，用闭式估计，免试跑搜索）。
///
/// 做法：在初态上采样若干「随机单组移动」，估计平均 |Δscore|，再用 Metropolis 关系
/// `exp(-Δ/T)=p` 反解温度：
/// - `t_start = Δ̄ / ln(1/0.98)` ≈ 49.5·Δ̄（初温接受率≈0.98，充分探索）；
/// - `t_end   = Δ̄ / ln(1/0.02)` ≈ 0.256·Δ̄（末温接受率≈0.02，趋于贪心）。
///
/// 舒适区**不在这里定**：它由 `annealing.comfort_progress`（曲线上的位置 0~1）
/// 直接给出，`annealing::舒适区温度` 再按几何降温曲线换算成温度。
///
/// 采样失败（无合法移动/Δ̄≈0）返回 `None`，由调用方回退到配置温度。
pub fn auto_temperature(ctx: &OptContext, init: &[u8], rng: &mut impl Rng) -> Option<(f64, f64)> {
    let n = init.len();
    if n == 0 {
        return None;
    }
    let mut base_ev = Evaluator::new_full_only(ctx, init);
    let base = base_ev.get_score(ctx);

    const SAMPLES: usize = 100;
    let mut 采样差值: Vec<f64> = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let gi = rng.random_range(0..n);
        let allowed = &ctx.groups[gi].allowed_keys;
        if allowed.len() < 2 {
            continue;
        }
        let old = init[gi];
        let new = allowed[rng.random_range(0..allowed.len())];
        if new == old {
            continue;
        }
        let mut trial = init.to_vec();
        trial[gi] = new;
        let mut ev = Evaluator::new_full_only(ctx, &trial);
        采样差值.push(ev.get_score(ctx) - base);
    }
    if 采样差值.is_empty() {
        return None;
    }

    // 给定温度 T 时，样本集上的预期接受率：好/平走必收，坏走按 Metropolis 概率收。
    let 预期接受率 = |t: f64| -> f64 {
        if t <= 0.0 {
            return 采样差值.iter().filter(|&&d| d <= 0.0).count() as f64 / 采样差值.len() as f64;
        }
        采样差值
            .iter()
            .map(|&d| if d <= 0.0 { 1.0 } else { (-d / t).exp() })
            .sum::<f64>()
            / 采样差值.len() as f64
    };

    // 二分反解「使接受率等于目标值」的温度。接受率对 T 单调递增 → 二分必然收敛。
    let 解温度 = |目标: f64| -> f64 {
        // 上界：接受率已饱和到 1 的量级；下界：接受率退化为「必收比例」。
        let mut 低 = 1e-18f64;
        let mut 高 = {
            let mut h = 1.0f64;
            while 预期接受率(h) < 目标 && h < 1e18 {
                h *= 10.0;
            }
            h
        };
        for _ in 0..120 {
            let 中 = 0.5 * (低 + 高);
            if 预期接受率(中) < 目标 {
                低 = 中;
            } else {
                高 = 中;
            }
        }
        0.5 * (低 + 高)
    };

    // 旧口径按「平均 |Δ|」闭式反解（98%/2%），但均值被少数「巨幅恶化」样本拉爆：
    // 实测 t_start 达配置值的 6.5e5 倍、首窗口接受率 87% 以上，2M 步零改进。
    // 改为直接对齐接受率——首窗口 40%（充分探索）、末窗口 1%（纯贪心收尾），
    // 与上游 moling 实测的 39% / 19% 同量级。
    let t_start = 解温度(0.40);
    let mut t_end = 解温度(0.01);
    if !t_start.is_finite() || t_start <= 0.0 {
        return None;
    }
    // 末温不得高于初温；若「必收比例」本身就高于 1% 目标，解会趋近 0，做下限保护。
    t_end = t_end.min(t_start).max(t_start * 1e-6);
    Some((t_start, t_end.max(1e-12)))
}
