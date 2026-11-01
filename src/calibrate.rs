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
/// #8 修复：缩放因子不再单纯等于 `1 / 初值`。纯 `1/初值` 会把评分口径绑死在
/// 一次随机初态上（初态为 0 时更会放大成 `1/eps`），导致不同种子下不可复现。
/// 现改为「参考值」策略：
/// - 优先用配置 `[targets]` 中该指标的**典型值**（与初态解耦、可复现）；
/// - 未启用或典型值为 0 时，退回初值观测；
/// - 初值也接近 0 时用 `1.0` 兜底，避免 `1/eps` 爆炸。
pub fn calibrate_scales(
    initial_metrics: &Metrics,
    initial_simple: &SimpleMetrics,
    weights: &WeightConfig,
    targets: &TargetsConfig,
) -> ScaleConfig {
    let eps = 1e-9;

    // 参考值选择：target 优先（enabled 且 > eps），否则 observed，最后 1.0 兜底。
    let pick = |target: f64, enabled: bool, observed: f64| -> f64 {
        if enabled && target > eps {
            target
        } else if observed > eps {
            observed
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
    let mut sum = 0.0f64;
    let mut cnt = 0usize;
    for _ in 0..SAMPLES {
        let gi = rng.gen_range(0..n);
        let allowed = &ctx.groups[gi].allowed_keys;
        if allowed.len() < 2 {
            continue;
        }
        let old = init[gi];
        let new = allowed[rng.gen_range(0..allowed.len())];
        if new == old {
            continue;
        }
        let mut trial = init.to_vec();
        trial[gi] = new;
        let mut ev = Evaluator::new_full_only(ctx, &trial);
        sum += (ev.get_score(ctx) - base).abs();
        cnt += 1;
    }
    if cnt == 0 {
        return None;
    }
    let avg = sum / cnt as f64;
    // 平均 |Δ| 为 0（或非有限）说明初态已在局部极优、温度无从标定，交给调用方兜底。
    if avg.is_nan() || avg <= 0.0 {
        return None;
    }
    let t_start = avg / (1.0f64 / 0.98).ln();
    let t_end = avg / (1.0f64 / 0.02).ln();
    Some((t_start, t_end.max(1e-12)))
}
