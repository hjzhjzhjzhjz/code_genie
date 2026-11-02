//! 配置文件烟雾测试（RON 格式）
//!
//! 检查 `config.ron.example` 与 `moling/config.ron`：
//!   1. 「简码评估性能优化」新增的 8 个配置项 + `checkpoint_interval_ratio` 均存在；
//!   2. 规范示例文件 `config.ron.example` 每项带行内注释（`//`）且取值等于代码默认；
//!   3. `moling/config.ron` 是机器生成的实验配置，只校验存在性，不断言注释与取值。
//!
//! 纯文本检查（按 `key:` 定位行），不依赖任何 RON/TOML 解析库。

use std::path::PathBuf;

#[derive(Clone, Copy)]
enum Expected {
    Float(f64),
    Str(&'static str),
    Int(i64),
}

const EXPECTED_ITEMS: &[(&str, Expected)] = &[
    ("simple_start_progress", Expected::Float(0.4)),
    ("simple_ramp_progress", Expected::Float(0.1)),
    ("simple_activation_reheat", Expected::Float(1.2)),
    ("simple_coverage_ratio", Expected::Float(1.0)),
    ("simple_active_coverage", Expected::Float(0.90)),
    ("reconcile_interval_ratio", Expected::Float(0.05)),
    ("simple_assign_mode", Expected::Str("efficiency")),
    ("simple_protect_top_n", Expected::Int(0)),
];

fn manifest_path(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(relative);
    p
}

/// 找形如 `key: ...` 的字段定义行（RON 字段）
fn find_field_line<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.lines().find(|line| {
        let t = line.trim_start();
        t.strip_prefix(key)
            .map(|rest| rest.trim_start().starts_with(':'))
            .unwrap_or(false)
    })
}

/// 从 `key: value, // comment` 里取出 `value`（去掉行尾逗号与注释）
fn extract_value(line: &str) -> &str {
    let after = line.split_once(':').map(|x| x.1).unwrap_or("").trim();
    let after = after.split("//").next().unwrap_or(after).trim();
    after.trim_end_matches(',').trim()
}

/// 校验配置文件的 8 个简码配置项。
///
/// `require_comment`：规范示例文件要求每项带 `//` 注释；
/// `check_value`：规范示例文件要求取值等于代码内置默认。
fn check_config_file(relative: &str, check_value: bool, require_comment: bool) {
    let path = manifest_path(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取配置文件 {}: {e}", path.display()));

    for (key, expected) in EXPECTED_ITEMS {
        let line =
            find_field_line(&raw, key).unwrap_or_else(|| panic!("[{relative}] 缺少配置项 `{key}`"));

        if require_comment {
            assert!(
                line.contains("//"),
                "[{relative}] 配置项 `{key}` 的定义行缺少注释: {line:?}"
            );
        }

        if check_value {
            let v = extract_value(line);
            match expected {
                Expected::Float(exp) => {
                    let got: f64 = v
                        .parse()
                        .unwrap_or_else(|_| panic!("[{relative}] `{key}` 不是浮点数: {v:?}"));
                    assert_eq!(got, *exp, "[{relative}] `{key}` 默认值与代码内置默认不一致");
                }
                Expected::Str(exp) => {
                    let got = v.trim_matches('"');
                    assert_eq!(got, *exp, "[{relative}] `{key}` 默认值与代码内置默认不一致");
                }
                Expected::Int(exp) => {
                    let got: i64 = v
                        .parse()
                        .unwrap_or_else(|_| panic!("[{relative}] `{key}` 不是整数: {v:?}"));
                    assert_eq!(got, *exp, "[{relative}] `{key}` 默认值与代码内置默认不一致");
                }
            }
        }
    }
}

/// 校验 `checkpoint_interval_ratio`（断点续算特性）。
fn check_checkpoint_interval_ratio(relative: &str, check_value: bool, require_comment: bool) {
    let path = manifest_path(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取配置文件 {}: {e}", path.display()));
    let line = find_field_line(&raw, "checkpoint_interval_ratio")
        .unwrap_or_else(|| panic!("[{relative}] 缺少 `checkpoint_interval_ratio`"));
    if require_comment {
        assert!(
            line.contains("//"),
            "[{relative}] `checkpoint_interval_ratio` 缺少注释: {line:?}"
        );
    }
    if check_value {
        let v = extract_value(line);
        let got: f64 = v.parse().unwrap_or_else(|_| {
            panic!("[{relative}] `checkpoint_interval_ratio` 不是浮点数: {v:?}")
        });
        assert_eq!(
            got, 0.05,
            "[{relative}] `checkpoint_interval_ratio` 默认值应为 0.05"
        );
    }
}

fn skip_if_absent(relative: &str) -> bool {
    if manifest_path(relative).is_file() {
        false
    } else {
        eprintln!("⏭️ 跳过 {relative}：文件不存在");
        true
    }
}

#[test]
fn config_ron_example_has_new_simple_code_items_with_defaults_and_comments() {
    // 规范示例文件：存在性 + 取值等于代码默认 + 带注释。
    check_config_file("config.ron.example", true, true);
}

#[test]
fn moling_config_ron_has_new_simple_code_items() {
    // 机器生成的实验配置：只校验存在性（不要求注释/取值）。
    if skip_if_absent("moling/config.ron") {
        return;
    }
    check_config_file("moling/config.ron", false, false);
}

#[test]
fn config_ron_example_has_checkpoint_interval_ratio() {
    check_checkpoint_interval_ratio("config.ron.example", true, true);
}

#[test]
fn moling_config_ron_has_checkpoint_interval_ratio() {
    if skip_if_absent("moling/config.ron") {
        return;
    }
    check_checkpoint_interval_ratio("moling/config.ron", false, false);
}
