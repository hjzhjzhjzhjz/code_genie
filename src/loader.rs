// =========================================================================
// 📂 文件加载模块
// =========================================================================

use std::collections::HashMap;
use std::fs;

use crate::types::{char_to_key_index, KeyDistConfig, RootGroup, EQUIV_TABLE_SIZE};

/// 加载固定字根和受限字根组
///
/// # 返回值
/// - (固定字根映射, 受限字根组)
pub fn load_fixed(path: &str) -> Result<(HashMap<String, u8>, Vec<RootGroup>), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("无法读取固定字根文件 {}: {}", path, e))?;
    let mut truly_fixed: HashMap<String, u8> = HashMap::new();
    let mut constrained: Vec<RootGroup> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let roots: Vec<String> = parts[0].split_whitespace().map(|s| s.to_string()).collect();
            if roots.is_empty() {
                continue;
            }
            // #6：与 load_keymap 保持一致——把整列当作编码串，逐字符取键（小写化、忽略空白）。
            // 不再「按空白分词后只取每个 token 首字符」：那会把 `Wko` 误读成单键 `w`，
            // 与 load_keymap 的 `[w,k,o]` 矛盾（历史「15 死组」诱因）。
            let keys: Vec<u8> = parts[1]
                .trim()
                .to_lowercase()
                .chars()
                .filter_map(|c| char_to_key_index(c).map(|i| i as u8))
                .collect();

            if keys.len() == 1 {
                for root in roots {
                    truly_fixed.insert(root, keys[0]);
                }
            } else if keys.len() > 1 {
                constrained.push(RootGroup {
                    roots,
                    allowed_keys: keys,
                });
            }
        }
    }
    Ok((truly_fixed, constrained))
}

/// 加载拆分表
///
/// # 返回值
/// - Vec<(字符, 根名列表, 频率)>
pub fn load_splits(path: &str) -> Result<Vec<(char, Vec<String>, u64)>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("无法读取拆分表 {}: {}", path, e))?;
    let mut res = Vec::new();
    // 频次列解析失败时静默按 1 计，会把当量/重码率/分布的加权整体拉平，且屏幕上毫无迹象；
    // 这里统一收集后告警（逐行打会把十万行的表刷满屏，只留前几个样例 + 总数）。
    let mut 频次异常行数 = 0usize;
    let mut 频次异常样例: Vec<(usize, String)> = Vec::new();
    for (行号, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            // #10：首字段为空（如行首就是制表符）时跳过，避免 chars().next().unwrap() panic。
            let ch = match parts[0].trim().chars().next() {
                Some(c) => c,
                None => continue,
            };
            let roots: Vec<String> = parts[1].split_whitespace().map(|s| s.to_string()).collect();
            // 第三列可省略（默认 1）；给了但解析不了才告警。
            let freq: u64 = match parts.get(2).map(|s| s.trim()) {
                None | Some("") => 1,
                Some(原文) => match 原文.parse::<u64>() {
                    Ok(v) => v,
                    // 容忍 `1234.0` 这类浮点/带小数的字频写法（字频表常带小数点）
                    Err(_) => match 原文.parse::<f64>() {
                        Ok(f) if f.is_finite() && f >= 0.0 => f as u64,
                        _ => {
                            频次异常行数 += 1;
                            if 频次异常样例.len() < 5 {
                                频次异常样例.push((行号 + 1, 原文.to_string()));
                            }
                            1
                        }
                    },
                },
            };
            res.push((ch, roots, freq));
        }
    }
    if 频次异常行数 > 0 {
        let 样例: Vec<String> = 频次异常样例
            .iter()
            .map(|(号, 原)| format!("第{号}行「{原}」"))
            .collect();
        eprintln!(
            "⚠️ 拆分表 {} 有 {频次异常行数} 行频次无法解析，已按 1 计（样例：{}）。\
             若是「1,234」或带单位写法，请改成纯数字，否则当量/重码加权会失真。",
            path,
            样例.join("、")
        );
    }
    Ok(res)
}

/// 内嵌当量表：编译期把 `assets/当量表.txt` 打进二进制。
///
/// 数据源为宇浩星陈的 2116 条键对当量（含标点键）。这样即使外部当量表文件缺失，
/// 也不会退化成「全 0 当量」导致评分失真——`pair_equiv` 留空即用本表。
const 内嵌当量表: &str = include_str!("../assets/当量表.txt");

/// 加载字根对当量表
///
/// 数据来源优先级：`path` 非空且文件存在 → 读文件；否则 → 用内嵌当量表。
/// 因此配置里 `pair_equiv = ""` 就表示「用内嵌表」，外部文件仍可覆盖。
///
/// # 返回值
/// - 31x31 当量矩阵
pub fn load_pair_equivalence(
    path: &str,
) -> Result<[[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE], String> {
    let mut table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
    let content: std::borrow::Cow<str> = if path.trim().is_empty() {
        std::borrow::Cow::Borrowed(内嵌当量表)
    } else {
        match fs::read_to_string(path) {
            Ok(s) => std::borrow::Cow::Owned(s),
            Err(e) => {
                eprintln!("⚠️ 读当量表 {} 失败（{}），改用内嵌当量表", path, e);
                std::borrow::Cow::Borrowed(内嵌当量表)
            }
        }
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let chars: Vec<char> = parts[0].chars().collect();
            if chars.len() == 2
                && let (Some(k1), Some(k2)) =
                    (char_to_key_index(chars[0]), char_to_key_index(chars[1]))
                && let Ok(equiv) = parts[1].trim().parse::<f64>()
                && k1 < EQUIV_TABLE_SIZE
                && k2 < EQUIV_TABLE_SIZE
            {
                table[k1][k2] = equiv;
            }
        }
    }
    Ok(table)
}

/// 从 keymap 文件加载字根到键位的映射
///
/// keymap 文件格式: 字根名\t编码\t使用次数
/// 编码格式: 首字母大写的键位字符串，如 Wko -> [w, k, o]
///
/// 需要同时传入 division 文件路径，以确定每个基础字根的实际子字根后缀列表。
/// 例如 keymap 中「口 制表符 Wko」表示口的编码为 [w, k, o]，
/// 而 division 中口的子字根为 口、口.1、口.2，
/// 因此映射为: 口=w, 口.1=k, 口.2=o
///
/// # 返回值
/// - HashMap<String, u8>: 子字根名 -> 键位索引
pub fn load_keymap(keymap_path: &str, division_path: &str) -> Result<HashMap<String, u8>, String> {
    use crate::types::{extract_base_name, extract_suffix_num};

    // 第一步：从 division 文件中提取每个基础字根的子字根后缀列表
    let splits = load_splits(division_path)?;
    let mut base_to_suffixes: HashMap<String, Vec<i32>> = HashMap::new();

    for (_, roots, _) in &splits {
        for root in roots {
            let base = extract_base_name(root);
            let suffix = extract_suffix_num(root);
            let entry = base_to_suffixes.entry(base).or_default();
            if !entry.contains(&suffix) {
                entry.push(suffix);
            }
        }
    }

    // 对每个基础字根的后缀列表排序
    for suffixes in base_to_suffixes.values_mut() {
        suffixes.sort();
    }

    // 第二步：解析 keymap 文件
    let content = fs::read_to_string(keymap_path)
        .map_err(|e| format!("无法读取 keymap 文件 {}: {}", keymap_path, e))?;
    let mut root_to_key: HashMap<String, u8> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let base_name = parts[0].trim();
        let encoding = parts[1].trim().to_lowercase();
        let keys: Vec<u8> = encoding
            .chars()
            .filter_map(|c| char_to_key_index(c).map(|i| i as u8))
            .collect();

        if keys.is_empty() {
            continue;
        }

        // 获取该基础字根的后缀列表
        let known_base = base_to_suffixes.contains_key(base_name);
        let suffixes = match base_to_suffixes.get(base_name) {
            Some(s) => s.clone(),
            None => {
                // #3：division 中不存在该字根（未知字根）。过去静默按 [-1, 1, 2, ...]
                // 虚构 `base.1` / `base.2` 等子字根名，会与真实字根名静默错配。
                // 现在只把该基础字根映射到编码首键，并显式告警；不再虚构子字根。
                if keys.len() > 1 {
                    eprintln!(
                        "⚠️ keymap 字根 '{}' 不在拆分表中（未知字根），仅取首键，忽略其余 {} 个编码键",
                        base_name,
                        keys.len() - 1
                    );
                }
                vec![-1]
            }
        };

        // #4：已知字根的编码键数与子字根后缀数不匹配时，过去静默截断/丢键（均无声）。
        // 现在显式告警：键多则多余键被忽略，键少则多出的子字根拿不到键。
        if known_base && keys.len() != suffixes.len() {
            eprintln!(
                "⚠️ keymap 字根 '{}' 编码键数 {} 与子字根后缀数 {} 不一致：{}",
                base_name,
                keys.len(),
                suffixes.len(),
                if keys.len() > suffixes.len() {
                    "多余编码键将被忽略"
                } else {
                    "多出的子字根将无键"
                }
            );
        }

        // 将键位按后缀顺序映射到子字根名
        for (i, &key) in keys.iter().enumerate() {
            if i < suffixes.len() {
                let suffix = suffixes[i];
                let sub_name = if suffix < 0 {
                    base_name.to_string()
                } else {
                    format!("{}.{}", base_name, suffix)
                };
                root_to_key.insert(sub_name, key);
            }
        }
    }

    Ok(root_to_key)
}

/// 将 keymap（子字根名 → 键位）据当前 `ctx` 折叠为退火初始分配（assignment），并做约束校验。
///
/// 用于「从既有结果给 optimize 播种」：把一份既有 `output-keymap.txt` 解析（`load_keymap`）
/// 得到的「子字根名 → 键位」映射，按**当前** `ctx` 的字根组结构折叠成 `assignment[group] = key`。
///
/// 校验（均基于当前 `ctx`，不依赖种子目录里备份的旧输入）：
/// - 固定字根以其在 `ctx.fixed_roots` 中的权威键位为准：种子给出不同键位则返回 `Err`
///   （`load_fixed` 与 `load_keymap` 两处输入矛盾），一致则跳过（不参与退火分配）；
/// - 键位必须 ∈ 该组 `allowed_keys`，否则返回 `Err`（含字根名与键位）；
/// - 同组多个子字根名映射到不同键位（组内不一致）返回 `Err`（含组信息）。
///
/// 缺失组（种子未覆盖到的组）从其 `allowed_keys` 用 `rng` 随机合法填充，使最终分配为
/// 当前方案下的合法分配；返回随机填充的组数 `filled` 供调用方日志提醒。
///
/// 所有运行时量（`ctx.num_groups`、各组 `allowed_keys`）均取自 `ctx`，不写死。
///
/// # 返回值
/// - `Ok((assignment, filled))`：`assignment` 长度为 `ctx.num_groups`，每组键位均合法；
///   `filled` 为随机填充的缺失组数。
/// - `Err(msg)`：键位非法、组内不一致，或固定字根键位与 `load_fixed` 矛盾。
pub fn keymap_to_assignment(
    ctx: &crate::context::OptContext,
    root_to_key: &HashMap<String, u8>,
    rng: &mut impl rand::Rng,
) -> Result<(Vec<u8>, usize), String> {
    let n = ctx.num_groups;
    // None = 未赋值；用于检测缺失组与组内不一致
    let mut chosen: Vec<Option<u8>> = vec![None; n];

    for (name, &key) in root_to_key {
        // #6：固定字根的键位由 load_fixed 权威指定；若种子 keymap 给出不同键位，
        // 说明 load_fixed 与 load_keymap 两处输入矛盾，显式报错（不再静默跳过）。
        if let Some(&fixed_key) = ctx.fixed_roots.get(name) {
            if fixed_key != key {
                return Err(format!(
                    "字根 '{}' 为固定字根（load_fixed 指定键位 {}），但 keymap 给出键位 {}，两者矛盾",
                    name, fixed_key, key
                ));
            }
            continue;
        }
        // 仅处理属于动态/受限组的字根名
        let gi = match ctx.root_to_group.get(name) {
            Some(&gi) => gi,
            None => continue,
        };
        // 约束 1：键位必须在该组 allowed_keys 内
        if !ctx.groups[gi].allowed_keys.contains(&key) {
            return Err(format!(
                "字根 '{}' 的键位 {} 不在组 {} 的 allowed_keys {:?} 内",
                name, key, gi, ctx.groups[gi].allowed_keys
            ));
        }
        // 约束 2：组内一致
        match chosen[gi] {
            Some(prev) if prev != key => {
                return Err(format!(
                    "组 {} 内字根映射到不同键位({} vs {})，种子与当前方案不一致",
                    gi, prev, key
                ));
            }
            _ => chosen[gi] = Some(key),
        }
    }

    // 缺失组：随机合法填充
    let mut filled = 0usize;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        match chosen[gi] {
            Some(k) => assignment[gi] = k,
            None => {
                let allowed = &ctx.groups[gi].allowed_keys;
                // allowed 恒非空（由 load_fixed 保证）
                assignment[gi] = allowed[rng.random_range(0..allowed.len())];
                filled += 1;
            }
        }
    }
    Ok((assignment, filled))
}

/// 内嵌用指分布：编译期把 `assets/用指分布.txt` 打进二进制。
///
/// 46 个键位的目标使用率与上下惩罚系数。外部文件缺失时回落到本表，避免退化成默认 0
/// 导致分布评分失真——`key_dist` 留空即用本表。
const 内嵌用指分布: &str = include_str!("../assets/用指分布.txt");

/// 加载键位分布配置
///
/// 数据来源优先级：`path` 非空且文件存在 → 读文件；否则 → 用内嵌用指分布。
/// 配置里 `key_dist = ""` 即表示「用内嵌表」。
///
/// # 返回值
/// - 31 个键位的分布配置
pub fn load_key_distribution(path: &str) -> Result<[KeyDistConfig; EQUIV_TABLE_SIZE], String> {
    let mut cfg = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
    let content: std::borrow::Cow<str> = if path.trim().is_empty() {
        std::borrow::Cow::Borrowed(内嵌用指分布)
    } else {
        match fs::read_to_string(path) {
            Ok(s) => std::borrow::Cow::Owned(s),
            Err(e) => {
                eprintln!("⚠️ 读用指分布 {} 失败（{}），改用内嵌用指分布", path, e);
                std::borrow::Cow::Borrowed(内嵌用指分布)
            }
        }
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4
            && let Some(ki) = parts[0].chars().next().and_then(char_to_key_index)
            && ki < EQUIV_TABLE_SIZE
        {
            cfg[ki] = KeyDistConfig {
                target_rate: parts[1].trim().parse().unwrap_or(0.0),
                low_penalty: parts[2].trim().parse().unwrap_or(0.0),
                high_penalty: parts[3].trim().parse().unwrap_or(0.0),
            };
        }
    }
    Ok(cfg)
}

// =========================================================================
// 🧪 keymap → assignment 转换/校验测试（seed-optimize-from-result, 需求 4/5）
// =========================================================================
#[cfg(test)]
mod keymap_assignment_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{ScaleConfig, WeightConfig, EQUIV_TABLE_SIZE};
    use proptest::prelude::*;
    use rand::Rng;

    /// 由一组 `RootGroup` 构造最小 OptContext。
    fn make_ctx_from_groups(groups: Vec<RootGroup>) -> OptContext {
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(groups.len());
        for (gi, g) in groups.iter().enumerate() {
            let ch = char::from_u32(0x4e00 + gi as u32).unwrap();
            splits.push((ch, vec![g.roots[0].clone()], 100u64));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            WeightConfig::default(),
            TargetsConfig::default(),
        )
    }

    /// 由「每组的 allowed_keys」构造 ctx，组名为 g0/g1/...（每组单根）。
    fn make_ctx(group_allowed: &[Vec<u8>]) -> OptContext {
        let groups: Vec<RootGroup> = group_allowed
            .iter()
            .enumerate()
            .map(|(gi, allowed)| RootGroup {
                roots: vec![format!("g{gi}")],
                allowed_keys: allowed.clone(),
            })
            .collect();
        make_ctx_from_groups(groups)
    }

    /// 非空键位子集（取自 [0,1,2,3]，去重）。
    fn subset() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(0u8..4, 1..5).prop_map(|mut v| {
            v.sort();
            v.dedup();
            v
        })
    }

    /// 1..6 个组，每组一个非空 allowed_keys 子集。
    fn groups_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
        prop::collection::vec(subset(), 1..6)
    }

    #[test]
    fn group_internal_inconsistency_errors() {
        // 同组两个子根映射到不同键 → Err（需求 4.4）
        let groups = vec![RootGroup {
            roots: vec!["a".to_string(), "b".to_string()],
            allowed_keys: vec![0, 1, 2],
        }];
        let ctx = make_ctx_from_groups(groups);
        let mut rng = rand::rng();
        let mut m: HashMap<String, u8> = HashMap::new();
        m.insert("a".to_string(), 0);
        m.insert("b".to_string(), 1);
        assert!(keymap_to_assignment(&ctx, &m, &mut rng).is_err());
    }

    #[test]
    fn fixed_root_name_skipped() {
        // 不属于任何组的字根名被跳过，不触发 allowed_keys 校验（需求 4.2）
        let groups = vec![RootGroup {
            roots: vec!["a".to_string()],
            allowed_keys: vec![0, 1, 2],
        }];
        let ctx = make_ctx_from_groups(groups);
        let mut rng = rand::rng();
        let mut m: HashMap<String, u8> = HashMap::new();
        m.insert("不在任何组的固定根".to_string(), 99); // 非法键但被跳过
        let (asg, filled) = keymap_to_assignment(&ctx, &m, &mut rng).unwrap();
        assert_eq!(filled, 1, "唯一组未覆盖应随机填充");
        assert!(ctx.groups[0].allowed_keys.contains(&asg[0]));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: seed-optimize-from-result, Property 1: 合法种子转换得到合法且一致的分配
        #[test]
        fn prop_full_coverage_legal(group_allowed in groups_strategy()) {
            let ctx = make_ctx(&group_allowed);
            let mut rng = rand::rng();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            let mut chosen = vec![0u8; group_allowed.len()];
            for gi in 0..group_allowed.len() {
                let allowed = &group_allowed[gi];
                let k = allowed[rng.random_range(0..allowed.len())];
                chosen[gi] = k;
                root_to_key.insert(format!("g{gi}"), k);
            }
            let (asg, filled) = keymap_to_assignment(&ctx, &root_to_key, &mut rng).unwrap();
            prop_assert_eq!(filled, 0);
            for gi in 0..group_allowed.len() {
                prop_assert_eq!(asg[gi], chosen[gi]);
                prop_assert!(group_allowed[gi].contains(&asg[gi]));
            }
        }

        // Feature: seed-optimize-from-result, Property 2: 非法键位被拒绝
        #[test]
        fn prop_illegal_key_rejected(group_allowed in groups_strategy(), pick in 0usize..100) {
            let ctx = make_ctx(&group_allowed);
            let mut rng = rand::rng();
            let gi = pick % group_allowed.len();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            // 99 永不在 allowed_keys（⊆ [0,3]）内 → 必为非法
            root_to_key.insert(format!("g{gi}"), 99u8);
            prop_assert!(keymap_to_assignment(&ctx, &root_to_key, &mut rng).is_err());
        }

        // Feature: seed-optimize-from-result, Property 3: 缺失组随机填充后分配合法且计数正确
        #[test]
        fn prop_partial_fill(group_allowed in groups_strategy(), mask in any::<u64>()) {
            let n = group_allowed.len();
            let ctx = make_ctx(&group_allowed);
            let mut rng = rand::rng();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            let mut covered = 0usize;
            for gi in 0..n {
                if (mask >> gi) & 1 == 1 {
                    let allowed = &group_allowed[gi];
                    let k = allowed[rng.random_range(0..allowed.len())];
                    root_to_key.insert(format!("g{gi}"), k);
                    covered += 1;
                }
            }
            let (asg, filled) = keymap_to_assignment(&ctx, &root_to_key, &mut rng).unwrap();
            prop_assert_eq!(filled, n - covered);
            for gi in 0..n {
                prop_assert!(group_allowed[gi].contains(&asg[gi]));
            }
        }
    }
}

#[cfg(test)]
mod 内嵌当量表_tests {
    use super::*;

    #[test]
    fn 空路径走内嵌当量表且非全零() {
        let t = load_pair_equivalence("").unwrap();
        assert!(
            t.iter().flatten().any(|&v| v != 0.0),
            "内嵌当量表不应全 0（否则当量评分失真）"
        );
    }

    #[test]
    fn 内嵌与读文件逐项一致() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/当量表.txt");
        let 内嵌 = load_pair_equivalence("").unwrap();
        let 文件 = load_pair_equivalence(p).unwrap();
        assert_eq!(内嵌, 文件);
    }

    #[test]
    fn 缺失文件回落内嵌() {
        let t = load_pair_equivalence("不存在的路径/当量表.txt").unwrap();
        assert!(t.iter().flatten().any(|&v| v != 0.0));
    }
}

#[cfg(test)]
mod 内嵌用指分布_tests {
    use super::*;

    fn 快照(cfg: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> Vec<(f64, f64, f64)> {
        cfg.iter()
            .map(|c| (c.target_rate, c.low_penalty, c.high_penalty))
            .collect()
    }

    #[test]
    fn 空路径走内嵌用指分布且非全默认() {
        let cfg = load_key_distribution("").unwrap();
        assert!(
            cfg.iter().any(|c| c.target_rate != 0.0),
            "内嵌用指分布不应全为默认 0（否则分布评分失真）"
        );
    }

    #[test]
    fn 内嵌与读文件逐项一致() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/用指分布.txt");
        let 内嵌 = load_key_distribution("").unwrap();
        let 文件 = load_key_distribution(p).unwrap();
        assert_eq!(快照(&内嵌), 快照(&文件));
    }

    #[test]
    fn 缺失文件回落内嵌() {
        let cfg = load_key_distribution("不存在的路径/用指分布.txt").unwrap();
        assert!(cfg.iter().any(|c| c.target_rate != 0.0));
    }
}

#[cfg(test)]
mod splits_freq_tests {
    use super::*;

    fn 临时拆分表(内容: &str) -> std::path::PathBuf {
        let 目录 = std::env::temp_dir().join(format!("cg_splits_{}", std::process::id()));
        std::fs::create_dir_all(&目录).unwrap();
        let 路径 = 目录.join("splits.txt");
        std::fs::write(&路径, 内容).unwrap();
        路径
    }

    /// 频次列：纯整数、浮点写法、带千分位、缺列。
    /// 浮点要能正确取整（字频表常带小数点）；真解析不了才回落 1 并告警，
    /// 不能像原来那样一律静默按 1 计。
    #[test]
    fn 频次支持浮点且异常值不再静默() {
        let 路径 = 临时拆分表("一\ta b\t1234.0\n二\tc d\n三\te f\t1,234\n四\tg h\t37\n");
        let 行 = load_splits(路径.to_str().unwrap()).unwrap();
        assert_eq!(行.len(), 4);
        assert_eq!(行[0].2, 1234, "浮点字频应取整，而不是回落 1");
        assert_eq!(行[1].2, 1, "缺第三列按 1（原有约定）");
        assert_eq!(行[2].2, 1, "无法解析的频次按 1，但会告警");
        assert_eq!(行[3].2, 37);
    }
}
