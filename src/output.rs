// =========================================================================
// 📤 输出模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::config::Config;
use crate::context::OptContext;
use crate::types::{extract_base_name, extract_suffix_num, key_to_char, Metrics};

/// 统计字根使用频率
pub fn count_root_usage(ctx: &OptContext) -> HashMap<String, u64> {
    let mut usage: HashMap<String, u64> = HashMap::new();
    for (_, roots, freq) in &ctx.raw_splits {
        let mut seen_bases: HashSet<String> = HashSet::new();
        for root in roots {
            let base = extract_base_name(root);
            if seen_bases.insert(base.clone()) {
                *usage.entry(base).or_default() += freq;
            }
        }
    }
    usage
}

/// 构建排序后的根编码列表
pub fn build_root_encodings_sorted(
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) -> Vec<(String, Vec<u8>)> {
    let mut all_roots: Vec<(String, u8)> = Vec::new();

    for (root, &key) in fixed {
        all_roots.push((root.clone(), key));
    }
    for (gi, group) in groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            all_roots.push((root.clone(), key));
        }
    }

    let mut grouped: HashMap<String, Vec<(String, u8)>> = HashMap::new();
    for (name, key) in all_roots {
        let base = extract_base_name(&name);
        grouped.entry(base).or_default().push((name, key));
    }

    let mut result: Vec<(String, Vec<u8>)> = Vec::new();
    for (base, mut entries) in grouped {
        entries.sort_by(|a, b| {
            let sa = extract_suffix_num(&a.0);
            let sb = extract_suffix_num(&b.0);
            sa.cmp(&sb)
        });
        let keys: Vec<u8> = entries.iter().map(|(_, k)| *k).collect();
        result.push((base, keys));
    }

    // 按使用频率降序排序
    result.sort_by(|a, b| {
        let ua = root_usage.get(&a.0).copied().unwrap_or(0);
        let ub = root_usage.get(&b.0).copied().unwrap_or(0);
        ub.cmp(&ua).then_with(|| a.0.cmp(&b.0))
    });

    result
}

/// 格式化编码为可读字符串
pub fn format_encoding(keys: &[u8]) -> String {
    let mut s = String::with_capacity(keys.len());
    for (i, &k) in keys.iter().enumerate() {
        let c = key_to_char(k);
        if i == 0 {
            s.extend(c.to_uppercase());
        } else {
            s.push(c);
        }
    }
    s
}

/// 写入键位映射输出
pub fn write_keymap_output(
    root_out: &mut String,
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) {
    let encodings = build_root_encodings_sorted(fixed, groups, assignment, root_usage);
    for (base_name, keys) in &encodings {
        let enc = format_encoding(keys);
        let usage = root_usage.get(base_name).copied().unwrap_or(0);
        root_out.push_str(&format!("{}\t{}\t{}\n", base_name, enc, usage));
    }
}

pub fn save_combined_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let mut out = String::new();

    // 全码部分 - 所有字都输出全码（按槽位直取，与单字编码表同口径）
    let 字根表 = 根键位串(ctx, assignment);
    let n_chars = ctx.char_infos.len();
    let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
    sorted_chars.sort_by(|&a, &b| {
        ctx.char_infos[b]
            .frequency
            .cmp(&ctx.char_infos[a].frequency)
    });
    for &ci in &sorted_chars {
        let ch = ctx.raw_splits[ci].0;
        let (_, roots, _) = &ctx.raw_splits[ci];
        if let Ok(code) = 按槽位取码(roots, &字根表) {
            if !code.is_empty() {
                out.push_str(&format!("{}\t{}\n", ch, code));
            }
        }
    }

    crate::fsutil::write_output(format!("{}/合并码表.txt", dir), out);
}

/// 合并码表（覆盖全部字）：全量拆分表按星月 encoder 出的全码段。
///
/// 与 `完整码表.txt` 同口径（`rime_convert::单字码`），覆盖全量拆分表里的所有字。
pub fn save_combined_from_full(
    ctx: &OptContext,
    assignment: &[u8],
    全量: &[(String, Vec<String>, u64)],
    dir: &str,
) {
    let mut out = String::new();
    let 字根表 = 根键位串(ctx, assignment);
    for (字, 根们, _) in 全量 {
        if let Ok(code) = crate::rime_convert::单字码(根们, &字根表) {
            if !code.is_empty() {
                out.push_str(&format!("{}\t{}\n", 字, code));
            }
        }
    }
    crate::fsutil::write_output(format!("{}/合并码表.txt", dir), out);
}

/// 全码定稿后自动生成三级前缀简码（效率优先）：写 `简码表.txt`，并把简码段追加到
/// 已有的 `合并码表.txt`。**不碰** `完整码表.txt`（字根图 / Rime / 句子解码按纯全码读它）。
///
/// 只喂优化字集（`ctx.raw_splits`）：字频与退火评分同源，避免和全量拆分表的另一套
/// 字频混用；只在全量表里出现的冷僻字不参与分配，也不会白占前缀位。
///
/// `口径` 来自 `[shortcode] padding_space`，决定短码要不要按测评「补空格」计键。
pub fn save_shortcode_outputs(
    ctx: &OptContext,
    assignment: &[u8],
    dir: &str,
    口径: crate::shortcode::码长口径,
) -> crate::shortcode::简码统计 {
    let 字根表 = 根键位串(ctx, assignment);
    let mut 条目 = Vec::with_capacity(ctx.raw_splits.len());
    for (i, (ch, roots, _)) in ctx.raw_splits.iter().enumerate() {
        if let Ok(code) = 按槽位取码(roots, &字根表) {
            if !code.is_empty() {
                条目.push((*ch, code, ctx.char_infos[i].frequency));
            }
        }
    }
    let 结果 = crate::shortcode::生成_按口径(&条目, 口径);
    let 统计 = 结果.统计.clone();
    crate::shortcode::保存(dir, &结果, "files.splits（优化字集字频，与退火评分同源）");
    统计
}

/// 根 → 键位串（首字母大写、小码小写），与「字根键位表.txt」同口径
fn 根键位串(ctx: &OptContext, assignment: &[u8]) -> HashMap<String, String> {
    let mut 表: HashMap<String, String> = HashMap::new();
    let 无用量: HashMap<String, u64> = HashMap::new();
    for (根, 键们) in
        build_root_encodings_sorted(&ctx.fixed_roots, &ctx.groups, assignment, &无用量)
    {
        表.insert(根, format_encoding(&键们));
    }
    表
}

/// 按槽位取码：根名 `X.k` 取「X 的键位串」第 k 个字母（小写）。
///
/// 优化器的 `CharInfo.parts` 是**逐槽位取键位**（`白.0` / `白.1` 各算一条独立键位），
/// 而 `raw_splits` 正是带槽位的展开式（`的 = 白.0 勹.0 丶.0 丶.1`）。所以输出码表必须按
/// 槽位取，不能用只认**紧凑基名**的 `rime_convert::单字码`——否则 `白.0` 查不到 `白`、
/// 且根数也会按 4 根算（应为 3 根），导致整表为空或码错。
///
/// 无槽位后缀的根（`k == -1`）按第 0 码位处理，便于兼容紧凑拆分表。
fn 按槽位取码(
    根们: &[String], 字根表: &HashMap<String, String>
) -> Result<String, String> {
    let mut out = String::with_capacity(根们.len());
    for r in 根们 {
        let base = extract_base_name(r);
        let k = extract_suffix_num(r).max(0) as usize;
        let key = 字根表
            .get(&base)
            .ok_or_else(|| format!("未知字根「{base}」"))?;
        let ch = key
            .chars()
            .nth(k)
            .or_else(|| key.chars().next())
            .ok_or_else(|| format!("字根「{base}」无码位"))?;
        out.push(ch.to_ascii_lowercase());
    }
    Ok(out)
}

/// 保存线程结果
#[allow(clippy::too_many_arguments)]
pub fn save_thread_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    thread_id: usize,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let thread_dir = format!("{}/thread-{:02}", output_dir, thread_id);
    fs::create_dir_all(&thread_dir).expect("无法创建线程输出目录");

    let mut root_out = String::new();
    root_out.push_str(&format!("# 线程: {}\n", thread_id));
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if metrics.encoding_count > 0 {
        root_out.push_str(&format!(
            "# 编码种数: {} / 上限 {}\n",
            metrics.encoding_count, ctx.constraints.max_encodings
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    crate::fsutil::write_output(format!("{}/字根键位表.txt", thread_dir), &root_out);

    save_key_distribution_to_dir(ctx, assignment, &thread_dir);
    save_equiv_distribution_to_dir(ctx, assignment, &thread_dir);
    save_combined_code_output(ctx, assignment, &thread_dir);
}

/// 保存键位分布到目录
pub fn save_key_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
    let mut out = String::new();
    out.push_str("# 用指分布统计\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    let display_order = ctx.display_order.as_str();
    for kc in display_order.chars() {
        if let Some(ki) = crate::types::char_to_key_index(kc) {
            if ki >= 31 {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * evaluator.inv_total_key_presses;
            let target = ctx.key_dist_config[ki].target_rate;
            let diff = actual - target;
            out.push_str(&format!(
                "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                kc,
                actual,
                target,
                diff,
                diff * diff
            ));
        }
    }

    let order_set: HashSet<char> = display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * evaluator.inv_total_key_presses;
                let target = ctx.key_dist_config[*ki].target_rate;
                let diff = actual - target;
                out.push_str(&format!(
                    "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                    kc,
                    actual,
                    target,
                    diff,
                    diff * diff
                ));
            }
        }
    }

    crate::fsutil::write_output(format!("{}/用指分布.txt", dir), out);
}

/// 保存当量分布到目录
pub fn save_equiv_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
    let m = evaluator.get_metrics(ctx);

    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        if let Some(equiv) = ctx.calc_equiv_from_parts(i, assignment) {
            char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
        }
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut out = String::new();
    out.push_str("# 当量分布统计\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", m.equiv_mean));
    out.push_str(&format!("# 变异系数(CV): {:.4}\n", m.equiv_cv));
    out.push_str(&format!("# 标准差: {:.4}\n", m.equiv_cv * m.equiv_mean));
    out.push_str("#\n# 当量最高的20个高频字 (字频>1000000):\n# 汉字\t当量\t字频\n");

    let mut count = 0;
    for (ch, eq, freq) in &char_equivs {
        if *freq > 1_000_000 && count < 20 {
            out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
            count += 1;
        }
    }

    out.push_str("#\n# 当量最低的20个高频字 (字频>1000000):\n");
    let high: Vec<_> = char_equivs
        .iter()
        .filter(|(_, _, f)| *f > 1_000_000)
        .collect();
    let start = high.len().saturating_sub(20);
    for (ch, eq, freq) in high.iter().skip(start) {
        out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
    }

    crate::fsutil::write_output(format!("{}/当量分布.txt", dir), out);
}

/// 保存结果到输出目录
pub fn save_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let mut root_out = String::new();
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if metrics.encoding_count > 0 {
        root_out.push_str(&format!(
            "# 编码种数: {} / 上限 {}\n",
            metrics.encoding_count, ctx.constraints.max_encodings
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    crate::fsutil::write_output(format!("{}/字根键位表.txt", output_dir), &root_out);

    save_key_distribution_to_dir(ctx, assignment, output_dir);
    save_equiv_distribution_to_dir(ctx, assignment, output_dir);
    save_combined_code_output(ctx, assignment, output_dir);

    println!(
        "结果已保存至 {}/字根键位表.txt, 合并码表.txt 等",
        output_dir
    );
}

/// 缺根诊断里最多列出几个字根
const 缺根样例数: usize = 8;

/// 全量码表的生成统计
#[derive(Debug, Default)]
pub struct 全量码表统计 {
    /// 全码段条数（覆盖 `全量拆分` 里所有能编码的字）
    pub 全码条数: usize,
    /// 因字根缺键位（或键位串为空）跳过的字数
    pub 缺根条数: usize,
    /// 缺失最频繁的字根 `(根名, 涉及字数)`，按字数降序，最多 [`缺根样例数`] 个。
    ///
    /// 大面积缺失通常意味着 `files.full_splits` 与 `files.splits` 不是同一套根名体系
    /// （例如一个用真实部件名、另一个用映射后的根名），而不是真的存在 12 万个无法编码的字。
    pub 缺失字根: Vec<(String, usize)>,
}

/// 生成「全量码表」文本：每字一行全码。
///
/// 优化为了速度通常只喂常用字（`files.splits`），而码表要覆盖全部字（如 12 万字）。
/// 这里用优化得到的键位（`assignment`）对全量拆分重新编码，全码覆盖全量。
pub(crate) fn 全量码表文本(
    ctx: &OptContext,
    assignment: &[u8],
    全量拆分: &[(String, Vec<String>, u64)],
) -> (String, 全量码表统计) {
    let 字根表 = 根键位串(ctx, assignment);
    let mut 全码段 = String::with_capacity(全量拆分.len() * 16);
    let mut 全条 = 0usize;
    let mut 缺根 = 0usize;
    let mut 缺根计数: HashMap<&str, usize> = HashMap::new();
    for (字, 根们, _) in 全量拆分 {
        match crate::rime_convert::单字码(根们, &字根表) {
            Ok(code) if !code.is_empty() => {
                全码段.push_str(&format!("{}\t{}\n", 字, code));
                全条 += 1;
            }
            _ => {
                缺根 += 1;
                // 记下究竟是哪个根没有键位：根名体系不匹配时能一眼看出来。
                if let Some(根) = 根们.iter().find(|r| !字根表.contains_key(r.as_str())) {
                    *缺根计数.entry(根.as_str()).or_insert(0) += 1;
                }
            }
        }
    }

    let mut 缺失字根: Vec<(String, usize)> = 缺根计数
        .into_iter()
        .map(|(根, 数)| (根.to_string(), 数))
        .collect();
    缺失字根.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    缺失字根.truncate(缺根样例数);

    (
        全码段,
        全量码表统计 {
            全码条数: 全条,
            缺根条数: 缺根,
            缺失字根,
        },
    )
}

/// 保存「全量码表」到 `{output_dir}/完整码表.txt`：格式 `字<TAB>码`（不带频次列）。
pub fn save_full_code_table(
    ctx: &OptContext,
    assignment: &[u8],
    全量拆分: &[(String, Vec<String>, u64)],
    output_dir: &str,
) -> 全量码表统计 {
    let (out, 统计) = 全量码表文本(ctx, assignment, 全量拆分);
    crate::fsutil::write_output(format!("{}/完整码表.txt", output_dir), out);
    统计
}

/// 保存汇总信息
pub fn save_summary(
    cfg: &Config,
    results: &[(usize, Vec<u8>, f64, Metrics)],
    best_thread: usize,
    output_dir: &str,
    elapsed: std::time::Duration,
) {
    let mut summary = String::new();
    summary.push_str("# 优化结果汇总\n");
    summary.push_str(&format!("# 输出目录: {}\n", output_dir));
    summary.push_str(&format!("# 线程数: {}\n", cfg.annealing.threads));
    summary.push_str(&format!("# 总步数: {}\n", cfg.annealing.total_steps));
    summary.push_str(&format!("# 总耗时: {:?}\n", elapsed));
    summary.push_str(&format!("# 最优线程: {}\n", best_thread));
    summary.push_str("#\n");

    summary.push_str(&format!(
        "{:<8} {:<12} {:<10} {:<12} {:<10} {:<10} {:<12}\n",
        "线程", "得分", "重码数", "重码率%", "当量", "CV", "分布偏差"
    ));
    summary.push_str(&format!("{}\n", "-".repeat(80)));

    // 按得分排序
    let mut sorted: Vec<&(usize, Vec<u8>, f64, Metrics)> = results.iter().collect();
    sorted.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    for (tid, _, score, m) in &sorted {
        let marker = if *tid == best_thread { " 🏆" } else { "" };
        summary.push_str(&format!(
            "T{:<7} {:<12.4} {:<10} {:<12.6} {:<10.4} {:<10.4} {:<12.4}{}\n",
            tid,
            score,
            m.collision_count,
            m.collision_rate * 100.0,
            m.equiv_mean,
            m.equiv_cv,
            m.dist_deviation,
            marker
        ));
    }

    // 编码种数是设计约束（不在五项指标里），单独列在表后，避免撑宽主表破坏既有解析。
    if cfg
        .constraints
        .as_ref()
        .map(|c| c.enabled())
        .unwrap_or(false)
    {
        let 上限 = cfg
            .constraints
            .as_ref()
            .map(|c| c.max_encodings)
            .unwrap_or(0);
        summary.push_str(&format!(
            "\n# 编码种数（字根键位组合去重数，上限 {上限}）\n"
        ));
        for (tid, _, _, m) in &sorted {
            let 标记 = if m.encoding_count > 上限 {
                " 超限"
            } else {
                ""
            };
            summary.push_str(&format!("T{tid}\t{}{标记}\n", m.encoding_count));
        }
    }

    crate::fsutil::write_output(format!("{}/汇总.txt", output_dir), summary);
}

// =========================================================================
// 🧪 全量码表测试
// =========================================================================
#[cfg(test)]
mod 全量码表测试 {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, WeightConfig, EQUIV_TABLE_SIZE};
    use std::collections::HashMap;

    /// 每个字 2 个根（各自成组、允许键位 [0,1]）；assignment 全 0 时两槽都取键 0。
    fn 造上下文(freqs: &[u64]) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(freqs.len());
        for (i, &f) in freqs.iter().enumerate() {
            let r0 = format!("r{i}_0");
            let r1 = format!("r{i}_1");
            groups.push(RootGroup {
                roots: vec![r0.clone()],
                allowed_keys: vec![0, 1],
            });
            groups.push(RootGroup {
                roots: vec![r1.clone()],
                allowed_keys: vec![0, 1],
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![r0, r1], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE],
            [KeyDistConfig::default(); EQUIV_TABLE_SIZE],
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        )
    }

    /// 全量拆分替身：直接取 `ctx` 的优化拆分（同根名体系），保证全码段有内容。
    fn 全量拆分(ctx: &OptContext) -> Vec<(String, Vec<String>, u64)> {
        ctx.raw_splits
            .iter()
            .map(|(ch, roots, freq)| (ch.to_string(), roots.clone(), *freq))
            .collect()
    }

    /// 每字恰好一行，且条数与全量拆分一致。
    #[test]
    fn 全码段覆盖全量拆分() {
        let ctx = 造上下文(&[1000, 800, 600, 400]);
        let asg = vec![0u8; ctx.num_groups];

        let (文本, 统计) = 全量码表文本(&ctx, &asg, &全量拆分(&ctx));
        assert_eq!(统计.全码条数, ctx.raw_splits.len(), "全码应覆盖全量拆分");
        assert_eq!(统计.缺根条数, 0);
        assert_eq!(文本.lines().count(), 统计.全码条数);
    }

    /// 缺根时要把「是哪个字根缺键位」记下来——两套根名体系混用时能一眼定位。
    #[test]
    fn 缺根诊断记录缺失字根名() {
        let ctx = 造上下文(&[1000, 800]);
        let asg = vec![0u8; ctx.num_groups];

        let mut 全量 = 全量拆分(&ctx);
        全量.push(("囧".to_string(), vec!["查无此根".to_string()], 7));
        全量.push(("囧".to_string(), vec!["查无此根".to_string()], 7));

        let (文本, 统计) = 全量码表文本(&ctx, &asg, &全量);
        assert_eq!(统计.缺根条数, 2, "两条缺根都应计入");
        assert_eq!(
            统计.缺失字根,
            vec![("查无此根".to_string(), 2)],
            "缺失字根应按涉及字数聚合"
        );
        assert!(!文本.contains("囧\t"), "缺根的字不应写进码表");
    }
}
