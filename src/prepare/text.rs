// =========================================================================
// 🧾 prepare 的文本与排序工具
// =========================================================================
//
// bash 版靠 `LC_ALL=C sort` 与 Perl 的 `cmp` 定序，二者的共同语义是**字节序**；
// Rust 的 `&str`/`String` 的 `Ord` 正好也是 UTF-8 字节序，因此直接用它即可，
// 不需要额外比较函数。需要注意的只有三处：
//
// 1. 行尾：Perl 的 `-l` 只 chomp `\n`，在 CRLF 工作区上会把 `\r` 留在行尾，
//    导致 `/^\p{Han}$/`、`eq` 之类的锚定比较全部失配。`str::lines()` 同时吃掉
//    `\n` 与 `\r\n`，于是本地（CRLF）与 CI（LF）得到一致结果。
// 2. 数值：`sort -n` 与 Perl 的 `<=>` 都按十进制数比较，统一走 [`数值字段`]。
// 3. 键位串：`join(" ", split /\s*/, $s)` 的效果是「滤掉空白、逐字符用空格连接」。
// =========================================================================

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// 读取文本文件并切成行（同时兼容 LF 与 CRLF，末尾无换行也成行）
pub fn 读行(path: &Path) -> Result<Vec<String>, String> {
    let 文本 =
        fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{}", path.display(), e))?;
    Ok(文本.lines().map(str::to_string).collect())
}

/// 覆盖写文本（统一 LF 行尾，对应 bash 的 `>`）
///
/// 写前先确保父目录存在：prepare 的产物目录（如 `moling/`）不入库、常需由调用方
/// 在 CI 里自建，若缺失裸 `fs::write` 会报 ENOENT，故这里兜底 `create_dir_all`。
pub fn 写文本(path: &Path, 内容: &str) -> Result<(), String> {
    if let Some(父) = path.parent() {
        if !父.as_os_str().is_empty() {
            fs::create_dir_all(父)
                .map_err(|e| format!("创建目录 {} 失败：{}", 父.display(), e))?;
        }
    }
    fs::write(path, 内容).map_err(|e| format!("写入 {} 失败：{}", path.display(), e))
}

/// 追加写文本（对应 bash 的 `>>`）
///
/// 与 `写文本` 同理：写前确保父目录存在，避免 prepare 产物目录（如 `moling/`）缺失时
/// `OpenOptions::create` 因父目录不存在而 ENOENT。
pub fn 追加文本(path: &Path, 内容: &str) -> Result<(), String> {
    if let Some(父) = path.parent() {
        if !父.as_os_str().is_empty() {
            fs::create_dir_all(父)
                .map_err(|e| format!("创建目录 {} 失败：{}", 父.display(), e))?;
        }
    }
    let mut 文件 = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {} 失败：{}", path.display(), e))?;
    文件
        .write_all(内容.as_bytes())
        .map_err(|e| format!("写入 {} 失败：{}", path.display(), e))
}

/// GNU `sort -n` 的数值解析：字段都是纯数字，解析失败按 0 处理
pub fn 数值字段(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(0.0)
}

/// 是否单个汉字（近似 Unicode `Script=Han`）
///
/// 覆盖 `\p{Han}` 在本项目数据里出现的全部码位：康熙部首/部首补充、CJK 基本区、
/// 扩展 A、兼容汉字，以及第二/第三平面（扩展 B–H，整平面都是汉字）。
pub fn 是汉字(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x2EFF | 0x2F00..=0x2FDF | 0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B
            | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF
    )
}

/// 读取字根映射表（花括号名 → 显示字），供 `chart -d` 的字根键位图页面使用。
///
/// 兼容 tab 与任意空白分隔，跳过空行和 `#` 注释；同名只留最后一条（与 perl 版
/// 一致）。映射表是可选的展示层数据，读不到时返回空表，不阻断主流程。
///
/// 统一成兼容 tab 与任意空白分隔，供 `chart -d` 读同一份 tsv。
pub fn 加载映射(path: &Path) -> HashMap<String, String> {
    let mut 表 = HashMap::new();
    let Ok(文本) = fs::read_to_string(path) else {
        return 表;
    };
    for 行 in 文本.lines() {
        let 列: Vec<&str> = 行.split_whitespace().collect();
        if 列.len() >= 2 && !列[0].starts_with('#') {
            表.insert(列[0].to_string(), 列[1].to_string());
        }
    }
    表
}

/// 按 `\s*` 打散成单字符再用单空格连接（`join(" ", split /\s*/, $s)` 的等价形式）
pub fn 打散(s: &str) -> String {
    let 字: Vec<String> = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(String::from)
        .collect();
    字.join(" ")
}

/// 按「值降序、名字字节序升序」排序（Perl `sort { $h{$b} <=> $h{$a} || $a cmp $b }`）
pub fn 按值降序(表: &mut [(String, f64)]) {
    表.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 打散去掉空白后逐字符连接() {
        assert_eq!(
            打散("qwrtyp sdfghjkl xcvbnm"),
            "q w r t y p s d f g h j k l x c v b n m"
        );
        assert_eq!(打散("sdfghjkl"), "s d f g h j k l");
        assert_eq!(打散(""), "");
    }

    #[test]
    fn 汉字判定覆盖各平面() {
        // 扩展 B–H 整平面（0x20000..=0x3FFFF）都要算汉字：
        // 旧 sentence.rs 的私有判定只到 0x2A6DF，会漏掉这批字。
        for c in [
            '一',
            '的',
            '𠀀',
            '〇',
            '⺮',
            '豈',
            '\u{2A6E0}',
            '\u{30000}',
            '\u{3FFFF}',
        ] {
            assert!(是汉字(c), "{c} 应判为汉字");
        }
        for c in ['a', '1', '，', '\t', 'あ'] {
            assert!(!是汉字(c), "{c} 不应判为汉字");
        }
    }

    #[test]
    fn 加载映射兼容两种分隔符且跳过注释() {
        let 目录 = std::env::temp_dir().join("code_genie_prepare_mapping_test");
        fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join("mapping.tsv");
        fs::write(
            &路径,
            "# 这是注释\n{⺮右}\t⺮\n{日} 日\n{⺮右}\t⺀\n\n空行后不写\n",
        )
        .unwrap();
        let 表 = 加载映射(&路径);
        assert_eq!(表.get("{⺮右}").unwrap(), "⺀", "同名只留最后一条");
        assert_eq!(表.get("{日}").unwrap(), "日", "空格分隔也要读得进");
        assert!(!表.contains_key("#"), "注释行不能进表");
        let _ = fs::remove_dir_all(&目录);
    }

    #[test]
    fn 加载映射缺文件返回空表() {
        let 路径 = std::env::temp_dir().join("code_genie_不存在的映射表.tsv");
        assert!(加载映射(&路径).is_empty());
    }

    #[test]
    fn 数值字段容错() {
        assert_eq!(数值字段("40529650"), 40529650.0);
        assert_eq!(数值字段(" 12 "), 12.0);
        assert_eq!(数值字段("abc"), 0.0);
        assert_eq!(数值字段(""), 0.0);
    }

    #[test]
    fn 读行兼容_crlf_与末尾无换行() {
        let 目录 = std::env::temp_dir().join("code_genie_prepare_text_test");
        fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join("crlf.txt");
        fs::write(&路径, "甲\r\n乙\r\n丙").unwrap();
        assert_eq!(读行(&路径).unwrap(), vec!["甲", "乙", "丙"]);
        let _ = fs::remove_dir_all(&目录);
    }
}
