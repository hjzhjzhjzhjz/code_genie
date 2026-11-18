// =========================================================================
// 拆分表加载
// =========================================================================
//
// 读取「字 <TAB> 根 根 … <TAB> 频次」格式的拆分表，供 optimize 收尾生成完整码表。
// 完整码表由 optimize 收尾产出（`output.rs` / `shortcode.rs`）。
// =========================================================================

use std::fs;

/// 读取纯拆分表：`字 <TAB> 根 根 … <TAB> 频次`
pub(crate) fn 加载拆分表(path: &str) -> Vec<(String, Vec<String>, u64)> {
    let 文本 = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("无法读取拆分表 {path}：{e}（非 UTF-8 文件请先转码）"));
    加载拆分表内容(&文本)
}

/// 从内存文本解析拆分表（供内嵌的全量拆分表使用）
pub(crate) fn 加载拆分表内容(文本: &str) -> Vec<(String, Vec<String>, u64)> {
    let mut 表 = Vec::new();
    for 行 in 文本.lines() {
        let 行 = 行.trim();
        if 行.is_empty() || 行.starts_with('#') {
            continue;
        }
        let 列: Vec<&str> = 行.split('\t').collect();
        if 列.len() < 2 {
            continue;
        }
        // 首列可能带变体选择符（如「直」+ VS），按原字符串保留，避免截成同一个字
        let 字 = 列[0].to_string();
        let 根: Vec<String> = 列[1].split_whitespace().map(|s| s.to_string()).collect();
        let 频次 = 列
            .get(2)
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        表.push((字, 根, 频次));
    }
    表
}
