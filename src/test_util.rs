//! 测试共用的小工具，仅在编译测试时可用。

#![cfg(test)]

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::TargetsConfig;
use crate::context::OptContext;
use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, WeightConfig, EQUIV_TABLE_SIZE};

/// 在系统临时目录下创建一个唯一的子目录，用于测试隔离。
///
/// 目录名形如 `cg_test_{tag}_{pid}_{timestamp}`，避免并发测试或重跑时互相干扰。
pub(crate) fn unique_temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "cg_test_{}_{}_{}",
        tag,
        std::process::id(),
        crate::fsutil::now_timestamp_ms().replace('.', "")
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 用「空 `fixed_roots` + 默认 `equiv_table`/`key_dist`/各配置」构造一份最小 `OptContext`。
///
/// 多个测试构造器（`造上下文` / `make_ctx_from_groups` 等）都用同一段默认尾部，
/// 抽到这里避免三处漂移。仅用于测试，不进入发布构建。
pub(crate) fn 造默认上下文(
    splits: &[(char, Vec<String>, u64)],
    groups: &[RootGroup],
) -> OptContext {
    let fixed_roots: HashMap<String, u8> = HashMap::new();
    let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
    let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
    OptContext::new(
        splits,
        &fixed_roots,
        groups,
        equiv_table,
        key_dist,
        ScaleConfig::default(),
        WeightConfig::default(),
        TargetsConfig::default(),
    )
}
