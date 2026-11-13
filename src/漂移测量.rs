//! 用任意精度有理数作「真值」，量化 f64 增量累加器的长跑漂移。
//!
//! 只挂 dev-dependencies、只在 `cfg(test)` 下编译：主循环的 f64 一行不改，
//! 也不消耗任何退火随机数。f64 是二进制有理数，可精确转成 `Rational`，
//! 因此这里的加减过程零舍入，只有最后一步按最近舍回 f64。
//!
//! 跑出数字：
//!   cargo test 漂移 -- --nocapture
#![cfg(test)]

use malachite::base::num::conversion::traits::RoundingFrom;
use malachite::base::rounding_modes::RoundingMode;
use malachite::Rational;

/// 把一串 f64 增量精确求和（中间过程零舍入），最后按最近舍回 f64。
pub(crate) fn 精确累加(增量: &[f64]) -> f64 {
    let mut 和 = Rational::from(0u8);
    for &d in 增量 {
        let 项 = Rational::try_from(d).expect("增量必须是有限 f64");
        和 += 项;
    }
    f64::rounding_from(&和, RoundingMode::Nearest).0
}

/// 确定性地造一串「接受 / 回滚」式增量，量级贴近真实字频 × 当量。
/// 不用 rng，避免与退火随机流扯上关系。
fn 造增量(步数: usize) -> Vec<f64> {
    let mut 状态 = 0x9E37_79B9_7F4A_7C15u64;
    let mut 出 = Vec::with_capacity(步数);
    for i in 0..步数 {
        状态 = 状态
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let 均匀 = (状态 >> 11) as f64 / (1u64 << 53) as f64;
        // 模拟 key_weighted_usage 的 += / -= 往返：正负交替、幅度不同
        let 幅度 = 1e6 * 均匀 + 1.0;
        let 符号 = if i % 2 == 0 { 1.0 } else { -1.0 };
        出.push(符号 * 幅度);
    }
    出
}

#[test]
fn 精确累加对已知序列给出精确结果() {
    // 1e16 + 1 在 f64 里会被压回 1e16（该量级 ulp=2），朴素累加丢掉那个 1。
    let 增量 = [1e16, 1.0, -1e16];
    let 朴素: f64 = 增量.iter().sum();
    assert_eq!(精确累加(&增量), 1.0);
    assert_ne!(朴素, 1.0, "朴素 f64 累加应当暴露大数吃小数");
}

#[test]
fn 增量累加长跑漂移对照() {
    let 步数 = 100_000;
    let 增量 = 造增量(步数);
    let 朴素 = 增量.iter().fold(0.0f64, |a, &b| a + b);
    let 真值 = 精确累加(&增量);
    let 绝对 = (朴素 - 真值).abs();
    let 相对 = 绝对 / 真值.abs().max(1.0);
    println!("步数={步数} 朴素={朴素:.6} 真值={真值:.6} 绝对漂移={绝对:.6e} 相对漂移={相对:.3e}");
    // 只守住「漂移不是灾难级」，具体数值交给上面的打印，避免阈值脆弱。
    assert!(相对 < 1e-6, "f64 累加漂移异常放大: {相对:.3e}");
}

#[test]
fn 周期性全量重建可消除累积漂移() {
    let 步数 = 100_000;
    let 增量 = 造增量(步数);
    let 朴素 = 增量.iter().fold(0.0f64, |a, &b| a + b);
    let 真值 = 精确累加(&增量);

    // 模拟「每 段长 步用真值重建一次累加器」：段内仍用 f64 增量，
    // 段末把起点重置为当前精确值，等价于 checkpoint 周期全量重算。
    let mut 分段结果 = Vec::new();
    for 段长 in [1_000usize, 10_000, 50_000] {
        let mut 和 = 0.0f64;
        let mut 精确起点 = Rational::from(0u8);
        for (i, &d) in 增量.iter().enumerate() {
            和 += d;
            精确起点 += Rational::try_from(d).expect("增量必须是有限 f64");
            if (i + 1) % 段长 == 0 {
                和 = f64::rounding_from(&精确起点, RoundingMode::Nearest).0;
            }
        }
        let 误差 = (和 - 真值).abs();
        let 无重建误差 = (朴素 - 真值).abs();
        println!("重建周期={段长:>6} 残余误差={误差:.6e}（无重建={无重建误差:.6e}）");
        分段结果.push(误差);
    }
    // 重建越频繁，残余漂移不应比完全不重建更差。
    assert!(
        分段结果
            .iter()
            .all(|&e| e <= (朴素 - 真值).abs() + f64::EPSILON),
        "周期性重建不应放大漂移"
    );
}
