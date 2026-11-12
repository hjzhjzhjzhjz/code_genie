//! 退火热点微基准（divan 0.1.21）
//!
//! 对准 2026-11-11 火焰图定标点名的路径：
//!   - `try_move`         —— 真实的退火单步单元（组级别改键 + 组内所有字 update_char + 全分重算），
//!                          是头条数字，未来任何重构都以它为对照基线。
//!   - `compute_full_score` —— 强制重算全分（含 `刷新编码种数`：368 基名指纹 sort/dedup），
//!                          隔离火焰图里那 2.66% 的路径。
//!   - `update_char`      —— 真实「组键翻转」后对该组每个字做增量桶迁移（非 no-op）。
//!   - `resolve_key` / `calc_code_only` / `calc_equiv_from_parts` —— 各为一次全表遍历，
//!                          用于拆出 `resolve_key`(5.74%) 与 `calc_equiv_from_parts`(5.43%) 的占比。
//!
//! 本项目是纯 binary crate，没有 lib target。这里用 `#[path]` 在 bench crate 内重建同一套
//! 模块树：模块内的 `crate::xxx` 解析到本 crate 同名模块，因此**不改 src/main.rs、不加 lib**。
//!
//! 运行：`cargo bench --bench annealing_hotspots -- --max-time 3`
//! 只看某个：`cargo bench --bench annealing_hotspots -- try_move`

#![allow(dead_code)]
#![allow(unused_imports)]

#[cfg(feature = "mimalloc")]
#[global_allocator]
static 全局分配器: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[path = "../src/annealing.rs"]
mod annealing;
#[path = "../src/assess.rs"]
mod assess;
#[path = "../src/bucket_store.rs"]
mod bucket_store;
#[path = "../src/calibrate.rs"]
mod calibrate;
#[path = "../src/checkpoint.rs"]
mod checkpoint;
#[path = "../src/clusters.rs"]
mod clusters;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/context.rs"]
mod context;
#[path = "../src/data_dir.rs"]
mod data_dir;
#[path = "../src/evaluator.rs"]
mod evaluator;
#[path = "../src/export_table.rs"]
mod export_table;
#[path = "../src/fsutil.rs"]
mod fsutil;
#[path = "../src/handfeel.rs"]
mod handfeel;
#[path = "../src/loader.rs"]
mod loader;
#[path = "../src/output.rs"]
mod output;
#[path = "../src/pipeline.rs"]
mod pipeline;
#[path = "../src/prepare/mod.rs"]
mod prepare;
#[path = "../src/rime_convert.rs"]
mod rime_convert;
#[path = "../src/roots_chart.rs"]
mod roots_chart;
#[path = "../src/roots_cluster.rs"]
mod roots_cluster;
#[path = "../src/run.rs"]
mod run;
#[path = "../src/schedule.rs"]
mod schedule;
#[path = "../src/sentence.rs"]
mod sentence;
#[path = "../src/shortcode.rs"]
mod shortcode;
#[path = "../src/types.rs"]
mod types;
#[path = "../src/validate.rs"]
mod validate;
#[path = "../src/zigen.rs"]
mod zigen;

use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::types::{ScaleConfig, WeightConfig};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static 全局素材: OnceLock<OptContext> = OnceLock::new();

fn 输入目录() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("dlrk/output-v8-新聚类103簇-40m/inputs")
}

fn 路径转文本(路径: &Path) -> &str {
    路径.to_str().expect("输入路径含非 UTF-8 字符")
}

fn 构建素材() -> OptContext {
    let 目录 = 输入目录();
    let (固定字根, 字根组) =
        loader::load_fixed(路径转文本(&目录.join("字根约束-新聚类103簇.txt")))
            .expect("字根约束加载失败");
    let 拆分 = loader::load_splits(路径转文本(
        &目录.join("单字拆分-星陈-全字集-602020.txt"),
    ))
    .expect("拆分表加载失败");
    // 项目未随输入目录提供 pair_equivalence.txt；传空串让 loader 使用内嵌当量表，
    // 与生产路径 `or_warn_default` 的兜底行为一致。
    let 当量表 = loader::load_pair_equivalence("").expect("内嵌当量表加载失败");
    let 用指分布 = loader::load_key_distribution(路径转文本(&目录.join("用指分布.txt")))
        .expect("用指分布加载失败");
    OptContext::new(
        &拆分,
        &固定字根,
        &字根组,
        当量表,
        用指分布,
        ScaleConfig::default(),
        WeightConfig::default(),
        crate::config::TargetsConfig::default(),
    )
}

fn 取素材() -> &'static OptContext {
    全局素材.get_or_init(构建素材)
}

/// 代表性初始分配：把各组随机摊到其候选键上（固定种子）。
///
/// 真实退火用 `multi_start_init` 从「贪心均衡」起步，稳态重码仅 ~180/16153 字
/// （≈1 成员/桶，桶规模极小）。而「各组全取首键」会退化成 4 个巨型桶（最大 6967 成员），
/// 完全不是热路径的真实状态——用那种初始态测出来的桶迁移成本会严重失真。
/// 故基准统一用「随机摊开」的分配作初始态，使桶规模贴近稳态（小桶），测量才有意义。
fn 初始分配(ctx: &OptContext) -> Vec<u8> {
    let mut rng = SmallRng::seed_from_u64(0x5EED_2026);
    ctx.groups
        .iter()
        .map(|组| {
            let ls = &组.allowed_keys;
            if ls.is_empty() {
                0
            } else {
                ls[rng.random_range(0..ls.len())]
            }
        })
        .collect()
}

/// 选一个「大小最接近平均」、且翻转有意义的组：诊断基准用它做代表，
/// 使单组耗时与 update_char（轮转所有组）的每轮均值可直接对比。
fn 代表组(ctx: &OptContext) -> usize {
    let 总: usize = ctx.group_to_chars.iter().map(|v| v.len()).sum();
    let 均 = 总 as f64 / ctx.num_groups as f64;
    (0..ctx.num_groups)
        .filter(|&i| ctx.groups[i].allowed_keys.len() >= 2 && !ctx.group_to_chars[i].is_empty())
        .min_by_key(|&i| (((ctx.group_to_chars[i].len() as f64) - 均).abs() * 1000.0) as usize)
        .expect("无可用组")
}

/// 「可动组」列表（允许键 ≥2）：真实退火只会在这些组上尝试改键，
/// 按 0..num_groups 轮转会混入大量不可动组，把样本拖成 no-op。
fn 可动组(ctx: &OptContext) -> Vec<usize> {
    let v: Vec<usize> = (0..ctx.num_groups)
        .filter(|&i| ctx.groups[i].allowed_keys.len() >= 2 && !ctx.group_to_chars[i].is_empty())
        .collect();
    assert!(!v.is_empty(), "无可动组");
    v
}

/// 组规模统计（用于解读 try_move 数字）：平均每组多少字。
fn 平均组大小(ctx: &OptContext) -> f64 {
    let 总: usize = ctx.group_to_chars.iter().map(|v| v.len()).sum();
    总 as f64 / ctx.num_groups as f64
}

fn main() {
    let ctx = 取素材();
    eprintln!(
        "基准素材：{} 字 / {} 组 / 平均每组 {:.1} 字 / code_base {}",
        ctx.char_infos.len(),
        ctx.num_groups,
        平均组大小(ctx),
        ctx.code_base
    );
    {
        // 真实桶规模分布：按 calc_code_only 的 code 直方图（碰撞按「组序列」计）。
        let assignment: Vec<u8> = 初始分配(ctx);
        let mut hist: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for ci in 0..ctx.char_infos.len() {
            *hist.entry(ctx.calc_code_only(ci, &assignment)).or_insert(0) += 1;
        }
        let mut sizes: Vec<usize> = hist.values().copied().collect();
        sizes.sort_unstable();
        let max = sizes.last().copied().unwrap_or(0);
        let avg = ctx.char_infos.len() as f64 / sizes.len() as f64;
        eprintln!(
            "代表性分配桶分布：桶数={} 最大桶={} 平均桶={:.2} 中位桶={} P90={}",
            sizes.len(),
            max,
            avg,
            sizes.get(sizes.len() / 2).copied().unwrap_or(0),
            sizes.get(sizes.len() * 9 / 10).copied().unwrap_or(0)
        );
    }
    divan::main();
}

/// 头条数字：一次真实退火单步（组级别改键 → 组内所有字 update_char → 全分重算）。
/// temp=∞ 让移动恒定被接受，状态持续推进，避免回滚分支干扰计时。
#[divan::bench]
fn try_move(bencher: divan::Bencher) {
    let ctx = 取素材();
    let mut assignment: Vec<u8> = 初始分配(ctx);
    let mut 评估器 = Evaluator::new(ctx, &assignment);
    let mut rng = SmallRng::seed_from_u64(42);
    // 只在「可动组」（允许键 ≥2）之间轮转：单键组在真实退火里永远不会被选中，
    // 轮转全组会让绝大多数迭代变成 no-op，样本中位数被计时底板淹没。
    let 组表 = 可动组(ctx);
    eprintln!("可动组 {} / {} 组", 组表.len(), ctx.num_groups);
    let mut 序 = 0usize;
    bencher.bench_local(|| {
        let r = 组表[序 % 组表.len()];
        序 += 1;
        let 当前 = assignment[r];
        let 新键 = ctx.groups[r]
            .allowed_keys
            .iter()
            .find(|&&k| k != 当前)
            .copied()
            .unwrap_or(当前);
        if 新键 != 当前 {
            评估器.try_move(ctx, &mut assignment, r, 新键, f64::INFINITY, &mut rng);
        }
    });
}

/// 隔离路径：强制重算全分（含 `刷新编码种数` 的 368 基名指纹 sort/dedup）。
/// compute_full_score 不读 score_dirty，每次调用都做完整聚合，正好暴露那条 2.66% 的 cost。
#[divan::bench]
fn compute_full_score(bencher: divan::Bencher) {
    let ctx = 取素材();
    let assignment: Vec<u8> = 初始分配(ctx);
    let 评估器 = Evaluator::new(ctx, &assignment);
    bencher.bench_local(|| black_box(评估器.compute_full_score(ctx)));
}

/// 真实的增量桶迁移：翻转某组的键位后，对该组每个字调用 update_char（非 no-op）。
#[divan::bench]
fn update_char(bencher: divan::Bencher) {
    let ctx = 取素材();
    let mut assignment: Vec<u8> = 初始分配(ctx);
    let mut 评估器 = Evaluator::new(ctx, &assignment);
    let 组表 = 可动组(ctx);
    eprintln!("可动组 {} / {} 组", 组表.len(), ctx.num_groups);
    let mut 序 = 0usize;
    bencher.bench_local(|| {
        let r = 组表[序 % 组表.len()];
        序 += 1;
        let 当前 = assignment[r];
        let 新键 = ctx.groups[r]
            .allowed_keys
            .iter()
            .find(|&&k| k != 当前)
            .copied()
            .unwrap_or(当前);
        assignment[r] = 新键;
        for &ci in &ctx.group_to_chars[r] {
            评估器.update_char(ctx, &assignment, ci);
        }
    });
}

/// 诊断：只算「编码 + 当量」，不做任何桶操作——隔离 `calc_code_only` + `calc_equiv_from_parts`。
/// 与 `update_char` 的差值即桶迁移的净开销。
#[divan::bench]
fn update_char_compute(bencher: divan::Bencher) {
    let ctx = 取素材();
    let mut assignment: Vec<u8> = 初始分配(ctx);
    let r = 代表组(ctx);
    eprintln!("诊断组 r={} 大小={} 字", r, ctx.group_to_chars[r].len());
    let k0 = assignment[r];
    let k1 = *ctx.groups[r]
        .allowed_keys
        .iter()
        .find(|&&k| k != k0)
        .unwrap();
    let mut 朝k1 = true;
    let mut 累加 = 0u64;
    bencher.bench_local(|| {
        assignment[r] = if 朝k1 { k1 } else { k0 };
        朝k1 = !朝k1;
        let mut 局部 = 0u64;
        for &ci in &ctx.group_to_chars[r] {
            局部 = 局部.wrapping_add(ctx.calc_code_only(ci, &assignment) as u64);
            if let Some(e) = ctx.calc_equiv_from_parts(ci, &assignment) {
                局部 = 局部.wrapping_add(e.to_bits());
            }
        }
        累加 = 累加.wrapping_add(局部);
    });
    black_box(累加);
}

/// 诊断：只做「桶迁移」——预计算该组每字在 k0/k1 下的新旧码，计时循环内不调用 `calc_*`。
/// 同一组在 k0↔k1 反复翻转，其余组不动，状态恒一致。得出纯桶迁移（稀疏后端两次哈希查找）的耗时。
#[divan::bench]
fn update_char_migrate(bencher: divan::Bencher) {
    let ctx = 取素材();
    let mut assignment: Vec<u8> = 初始分配(ctx);
    let r = 代表组(ctx);
    eprintln!("诊断组 r={} 大小={} 字", r, ctx.group_to_chars[r].len());
    let k0 = assignment[r];
    let k1 = *ctx.groups[r]
        .allowed_keys
        .iter()
        .find(|&&k| k != k0)
        .unwrap();
    let mut 评估器 = Evaluator::new(ctx, &assignment);
    // 预计算该组每字：(ci, code@k0, code@k1, freq)；码相同者不迁移（对应 update_char 的 early-out）。
    let mut 行: Vec<(usize, usize, usize, u64)> = Vec::new();
    for &ci in &ctx.group_to_chars[r] {
        let c0 = ctx.calc_code_only(ci, &assignment);
        assignment[r] = k1;
        let c1 = ctx.calc_code_only(ci, &assignment);
        assignment[r] = k0;
        if c0 != c1 {
            行.push((ci, c0, c1, ctx.char_infos[ci].frequency));
        }
    }
    let mut 朝k1 = true;
    bencher.bench_local(|| {
        assignment[r] = if 朝k1 { k1 } else { k0 };
        let 新是k1 = 朝k1;
        朝k1 = !朝k1;
        for &(ci, c0, c1, freq) in &行 {
            let (old, new) = if 新是k1 { (c0, c1) } else { (c1, c0) };
            评估器.migrate_char_bucket(ctx, old, new, ci, freq);
        }
    });
}

/// 全表所有字根的键位解析（火焰图 5.74%）。
#[divan::bench]
fn resolve_key(bencher: divan::Bencher) {
    let ctx = 取素材();
    let assignment: Vec<u8> = 初始分配(ctx);
    let 部们: Vec<u16> = ctx
        .char_infos
        .iter()
        .flat_map(|字| 字.parts.iter().copied())
        .collect();
    bencher.bench_local(|| {
        let mut 累加 = 0u64;
        for &部 in &部们 {
            累加 += ctx.resolve_key(black_box(部), black_box(assignment.as_slice())) as u64;
        }
        black_box(累加)
    });
}

/// 全表全码计算（每字一次，主要走 resolve_key + 乘加）。
#[divan::bench]
fn calc_code_only(bencher: divan::Bencher) {
    let ctx = 取素材();
    let assignment: Vec<u8> = 初始分配(ctx);
    let 字数 = ctx.char_infos.len();
    bencher.bench_local(|| {
        let mut 累加 = 0usize;
        for 序 in 0..字数 {
            累加 = 累加.wrapping_add(ctx.calc_code_only(序, black_box(assignment.as_slice())));
        }
        black_box(累加)
    });
}

/// 全表当量计算（火焰图 5.43%，含等效键表查表）。
#[divan::bench]
fn calc_equiv_from_parts(bencher: divan::Bencher) {
    let ctx = 取素材();
    let assignment: Vec<u8> = 初始分配(ctx);
    let 字数 = ctx.char_infos.len();
    bencher.bench_local(|| {
        let mut 累加 = 0.0f64;
        for 序 in 0..字数 {
            if let Some(值) = ctx.calc_equiv_from_parts(序, black_box(assignment.as_slice())) {
                累加 += 值;
            }
        }
        black_box(累加)
    });
}
