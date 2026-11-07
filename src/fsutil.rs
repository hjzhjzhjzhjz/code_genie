// =========================================================================
// 🛟 文件安全写入：写入/复制前若目标已存在，警告并归档旧文件（避免误覆盖）
// =========================================================================
//
// 用于 `optimize -d <dir>` 复用既有目录的场景：备份输入文件、写出优化结果文件时，
// 若目标文件已存在，则发出警告并把旧文件重命名为带「重命名时刻时间戳」后缀的归档名，
// 再写入新内容，从而既不丢失旧数据，也避免静默覆盖。

use std::path::{Path, PathBuf};

/// 在原路径后追加 `.{ts}` 形成归档名：`a/foo.txt` → `a/foo.txt.{ts}`。
fn with_timestamp_suffix(path: &Path, ts: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".{}", ts));
    PathBuf::from(s)
}

/// 若 `path` 是已存在的**普通文件**：警告并将其重命名为带时间戳后缀的归档名。
/// 重命名失败时仅告警（调用方随后会写入/覆盖）。
///
/// 目录一律跳过：`path.exists()` 对目录也为真，早期曾因调用方拼出空 basename
/// 而把 `inputs/` 目录本身当「旧文件」去 rename，Windows 必然报 os error 32，
/// 刷出一行与真实原因无关的告警，随后又跟着一行 os error 3。
pub fn archive_if_exists(path: &Path) {
    if path.is_file() {
        let ts = crate::checkpoint::now_timestamp_ms();
        let archived = with_timestamp_suffix(path, &ts);
        match std::fs::rename(path, &archived) {
            Ok(()) => eprintln!(
                "⚠️ 目标文件已存在，已重命名旧文件以避免覆盖: {} → {}",
                path.display(),
                archived.display()
            ),
            Err(e) => eprintln!(
                "⚠️ 重命名旧文件 {} 失败: {}（将直接写入/覆盖）",
                path.display(),
                e
            ),
        }
    }
}

/// 目标是目录时给出可读错误（而不是让底层抛 `os error 3` 让人猜）。
fn reject_directory(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("目标是一个目录，不能作为备份文件：{}", path.display()),
        ));
    }
    Ok(())
}

/// 安全写入：目标已存在时先归档旧文件（警告），再写入新内容。
pub fn write_with_backup<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    let path = path.as_ref();
    reject_directory(path)?;
    archive_if_exists(path);
    std::fs::write(path, contents)
}

/// 安全复制：目标已存在时先归档旧文件（警告），再复制。
pub fn copy_with_backup<S: AsRef<Path>, D: AsRef<Path>>(src: S, dst: D) -> std::io::Result<u64> {
    let dst = dst.as_ref();
    reject_directory(dst)?;
    archive_if_exists(dst);
    std::fs::copy(src, dst)
}

/// 写最终产物：失败时打印「哪个文件 + OS 原因」后退出，而不是 `unwrap()`。
///
/// release 构建是 `panic = abort`，`unwrap()` 只会抛一行 `panicked at ...`（有时连
/// 文件名都不在 panic 信息里），磁盘满 / 目录只读 / 文件被占用时用户完全看不出发生了什么。
pub fn write_output<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) {
    let path = path.as_ref();
    if let Err(e) = write_with_backup(path, contents) {
        eprintln!("❌ 写入 {} 失败：{}", path.display(), e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cg_fsutil_{}_{}_{}",
            tag,
            std::process::id(),
            crate::checkpoint::now_timestamp_ms().replace('.', "")
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_with_backup_archives_existing_and_writes_new() {
        let dir = tmp_dir("w");
        let path = dir.join("output-keymap.txt");
        // 首次写：无归档，内容为 v1
        write_with_backup(&path, b"v1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
        let count_before = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count_before, 1);

        // 再次写：旧文件被归档（多出一个 .{ts} 文件），规范文件为新内容 v2
        write_with_backup(&path, b"v2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 2, "应有规范文件 + 1 个归档: {entries:?}");
        // 归档文件含旧内容 v1
        let archived = entries
            .iter()
            .find(|n| n.as_str() != "output-keymap.txt")
            .expect("应存在归档文件");
        assert!(archived.starts_with("output-keymap.txt."));
        assert_eq!(std::fs::read_to_string(dir.join(archived)).unwrap(), "v1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_with_backup_archives_existing_target() {
        let dir = tmp_dir("c");
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        copy_with_backup(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
        // 旧 dst 被归档
        let archived = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .find(|n| n.starts_with("dst.txt."))
            .expect("应存在 dst 归档");
        assert_eq!(std::fs::read_to_string(dir.join(archived)).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_with_backup_no_archive_when_absent() {
        let dir = tmp_dir("a");
        let path = dir.join("fresh.txt");
        write_with_backup(&path, b"x").unwrap();
        // 仅有规范文件，无归档
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不能被当作「已存在的旧文件」去归档/改名。
    #[test]
    fn archive_if_exists_ignores_directory() {
        let dir = tmp_dir("dir");
        let inputs = dir.join("inputs");
        std::fs::create_dir_all(&inputs).unwrap();

        archive_if_exists(&inputs);

        assert!(inputs.is_dir(), "inputs/ 目录应原封不动");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "不应产生任何归档副本"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目标为目录时，错误应直接点名「目录」，而不是模糊的 os error 3。
    #[test]
    fn copy_with_backup_reports_directory_target() {
        let dir = tmp_dir("dircopy");
        let src = dir.join("a.txt");
        let dst = dir.join("inputs");
        std::fs::write(&src, b"x").unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        let err = copy_with_backup(&src, &dst).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("目录"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
