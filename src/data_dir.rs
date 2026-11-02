// =========================================================================
// 📦 上游素材目录：`yuhao-data/`
// =========================================================================
//
// 跟方案无关的上游数据统一放在仓库根的 `yuhao-data/`（宇浩测评器的数据也在那儿）：
//
//   yustar_chaifen.dict.yaml / yustar_chaifen_tw.dict.yaml   宇浩星陈拆分表
//   yuhao_charsets.lua                                       简繁常用字符集
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

/// 在 `base` 与素材目录里依次查找 `名`，返回第一个存在的**文件**
///
/// 先看 `base`：产物与方案专用文件（`字根声韵-<schema>.txt` 之类）都在那儿，
/// 读产物时必须优先命中，别被素材目录里的同名文件顶掉。
pub fn 找(base: &Path, 名: &str) -> Option<PathBuf> {
    let 本地 = base.join(名);
    if 本地.is_file() {
        return Some(本地);
    }
    let 素材 = 定位(base).join(名);
    素材.is_file().then_some(素材)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 临时目录(标签: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cg-data-dir-{标签}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn 优先命中子目录里的素材() {
        let 根 = 临时目录("sub");
        let 工作 = 根.join("moling");
        let 素材 = 根.join(目录名);
        std::fs::create_dir_all(&工作).unwrap();
        std::fs::create_dir_all(&素材).unwrap();
        std::fs::write(素材.join("yuhao-zigens.csv"), "x").unwrap();

        assert_eq!(定位(&工作), 素材);
        assert_eq!(
            找(&工作, "yuhao-zigens.csv"),
            Some(素材.join("yuhao-zigens.csv"))
        );
        std::fs::remove_dir_all(&根).ok();
    }

    #[test]
    fn 老布局平铺在工作目录也能读() {
        let 工作 = 临时目录("flat");
        std::fs::write(工作.join("beiyu-char-freq.txt"), "的 100\n").unwrap();

        assert_eq!(定位(&工作), 工作);
        assert_eq!(
            找(&工作, "beiyu-char-freq.txt"),
            Some(工作.join("beiyu-char-freq.txt"))
        );
        std::fs::remove_dir_all(&工作).ok();
    }

    #[test]
    fn 本地同名文件优先于素材目录() {
        let 根 = 临时目录("local");
        let 工作 = 根.join("moling");
        let 素材 = 根.join(目录名);
        std::fs::create_dir_all(&工作).unwrap();
        std::fs::create_dir_all(&素材).unwrap();
        std::fs::write(工作.join("字根声韵.txt"), "产物").unwrap();
        std::fs::write(素材.join("字根声韵.txt"), "素材").unwrap();

        assert_eq!(找(&工作, "字根声韵.txt"), Some(工作.join("字根声韵.txt")));
        std::fs::remove_dir_all(&根).ok();
    }

    #[test]
    fn 找不到时返回空() {
        let 工作 = 临时目录("none");
        assert_eq!(找(&工作, "不存在.csv"), None);
        std::fs::remove_dir_all(&工作).ok();
    }
}
