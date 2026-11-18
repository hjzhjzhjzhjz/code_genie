// =========================================================================
// 📦 上游素材目录：`yuhao-data/`
// =========================================================================
//
// 跟方案无关的上游数据统一放在仓库根的 `yuhao-data/`（宇浩测评器的数据也在那儿）：
//
//   yustar_chaifen.dict.yaml / yustar_chaifen_tw.dict.yaml   宇浩星陈拆分表
//   yuhao-zigens.csv                                         字根字形变体
//   chars.dict.yaml                                          万象拼音词典（字根读音）
//   beiyu-char-freq.txt                                      北语字频（纯简体模式）
//   charAbsoluteFrequency*.json                              测评器字频（简繁混合模式）
//
// 定位规则：`<base>/yuhao-data` → `<base>/../yuhao-data` → `<base>`。
// 最后一项兼容「素材平铺在 `--dir` 下」的老布局，两种摆法都能跑；
// 产物一律只写 `--dir`，不会落到素材目录里。
// =========================================================================

use std::path::{Path, PathBuf};

/// 素材目录名
pub const 目录名: &str = "yuhao-data";

/// 按「`<base>/yuhao-data` → `<base>` 的上一级 `/yuhao-data` → `<base>`」定位素材目录
///
/// `--dir moling` 时命中仓库根的 `yuhao-data/`（第二项）；`--dir .` 时命中第一项。
/// 用 `parent()` 而不是 `join("..")`，路径里不会留 `moling/..` 这种字样。
pub fn 定位(base: &Path) -> PathBuf {
    let 候选: Vec<PathBuf> = std::iter::once(base.join(目录名))
        .chain(base.parent().map(|上| 上.join(目录名)))
        .collect();
    for p in 候选 {
        if p.is_dir() {
            return p;
        }
    }
    base.to_path_buf()
}
