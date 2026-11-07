// =========================================================================
// 📐 自动校准模块
// =========================================================================

use crate::config::TargetsConfig;
use crate::types::{Metrics, ScaleConfig, WeightConfig};

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
#[allow(dead_code)]
pub fn calibrate_scales(
    initial_metrics: &Metrics,
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
    if active_count <= 1 {
        return ScaleConfig::default();
    }

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
    }
}
