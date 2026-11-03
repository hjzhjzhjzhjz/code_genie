// =========================================================================
// 📐 自动校准模块
// =========================================================================

use crate::config::TargetsConfig;
use crate::types::{Metrics, ScaleConfig, SimpleMetrics, WeightConfig};

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
