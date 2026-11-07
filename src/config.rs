// =========================================================================
// 🔧 配置模块
// =========================================================================

use serde::{Deserialize, Serialize};
use std::fs;

use crate::types::WeightConfig;

// =========================================================================
// 📋 配置结构体定义
// =========================================================================

/// 主配置结构体
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub files: FilesConfig,
    pub keys: KeysConfig,
    pub weights: WeightsConfig,
    pub annealing: AnnealingConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<ScaleConfigToml>, // 可选，缺失则完全依赖 calibrate
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<TargetsConfig>, // 可选，缺失则使用默认（disabled）
}

/// 文件路径配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilesConfig {
    pub fixed: String,
    pub splits: String,
    /// 当量表（可选）：留空或省略时用内嵌 `assets/` 默认表。
    #[serde(default)]
    pub pair_equiv: String,
    /// 用指分布（可选）：留空或省略时用内嵌默认表。
    #[serde(default)]
    pub key_dist: String,
    /// 全量拆分表（可选）：优化为了速度通常只用常用字（`splits`），
    /// 但码表要覆盖全部字。配置此项后，收尾会用它另出一份 `完整码表.txt`。
    /// 为空表示不额外生成（沿用 `splits`）。
    #[serde(default)]
    pub full_splits: String,
}

/// 键位配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeysConfig {
    pub allowed: String,
    pub display_order: String,
    /// 全码末键属于这些键时，不追加虚拟空格转移当量。
    #[serde(default = "default_termination_keys")]
    pub termination_keys: String,
}

fn default_termination_keys() -> String {
    "_".to_string()
}

/// 权重配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WeightsConfig {
    pub full_code: FullCodeWeights,
}

/// 全码权重
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FullCodeWeights {
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
}

/// 模拟退火参数配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnnealingConfig {
    pub threads: usize,
    pub total_steps: usize,
    /// 初温：**直接取配置值**，运行时不再标定（与上游 Dieken/code_genie 同模式）。
    ///
    /// 默认 `0.01` 来自固定种子温度扫描：以上游 `0.005` 为基准，升初温至 `0.01`
    /// 并压末温至 `1e-5`（1000 倍跨度），重码率 -1.87%（种B 盆地更明显，种A 无响应）。
    /// 注意该收益未达统计显著（配对 t=-1.19, n=16），且分布偏差 +11%，属权衡取舍。
    #[serde(default = "default_temp_start")]
    pub temp_start: f64,
    /// 末温：直接取配置值。
    ///
    /// 与初温 `0.01` 配合得到 1000 倍跨度、曲线中点 `1e-3`。跨度太小 → 末段仍在
    /// 高温游走、来不及精修；跨度太大（如 1e6）→ 几何曲线的尺度被压坏，退火走到
    /// 一半就进入冻结状态，后半程的算力全部浪费。
    #[serde(default = "default_temp_end")]
    pub temp_end: f64,
    /// 舒适区在**降温曲线上的位置**（0~1，0=刚起步、1=临近结束）。
    /// 减速带（高斯）就以这里为中心，默认 0.5 = 曲线正中（探索/收敛各占一半）。
    /// 用进度而非温度表达：温度随评分尺度变，进度不会；也从「温度比例」这种
    /// 反直觉的量改成了「曲线走到几成」这种能直接看懂、能直接调的量。
    #[serde(default = "default_comfort_progress")]
    pub comfort_progress: f64,
    /// 舒适区温度的**绝对值**（可选，优先于 `comfort_progress`）。
    ///
    /// 与上游 Dieken/code_genie 的 `annealing.comfort_temp` 同义：给定时直接按绝对温度
    /// 定位减速带中心，不再按降温曲线位置换算。留空（或 <= 0）时回落到
    /// `comfort_progress`，保持原有「尺度无关」的默认行为。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfort_temp: Option<f64>,
    pub comfort_width: f64,
    pub comfort_slowdown: f64,
    pub swap_probability: f64,
    pub min_improve_steps_ratio: f64,
    pub reheat_factor: f64,

    /// 冲突导向移动的执行概率（0.0~1.0），0.0 表示关闭本特性（默认）
    ///
    /// 扫参（4 线程 × 4 轮、200 万步，见 `冲突导向邻域-字频排序改造.md` 5.5）：
    /// 0.1 收益约减半；0.2 是当前验证过的拐点；0.35 以上得分开始回退（冲突邻域
    /// 挤占了更有效的 swap/move 预算）。
    #[serde(default = "default_conflict_probability")]
    pub conflict_probability: f64,
    /// 冲突条目重建的兜底间隔（同步次数）；0 表示只按脏编码数自适应重建
    ///
    /// 编码桶改为增量维护后（见 `annealing::CollisionCache`），本项不再控制
    /// 「全量重建」，而只是防止条目长期不重排的兜底闸门。
    #[serde(default = "default_conflict_refresh_interval")]
    pub conflict_refresh_interval: usize,
    /// 在排序后冲突列表的前 N 个元素中采样
    #[serde(default = "default_conflict_sample_window")]
    pub conflict_sample_window: usize,
    /// 冲突组排序是否按频率加权（true=按字频之和，false=按汉字数量）
    #[serde(default = "default_conflict_weight_by_freq")]
    pub conflict_weight_by_freq: bool,
    /// 冲突组的新落点是否按键位剩余配额加权采样（默认 true）
    /// （true=避开已超目标使用率的键、用指分布更匀；false=均匀随机、综合得分略优）
    #[serde(default = "default_conflict_key_by_load")]
    pub conflict_key_by_load: bool,

    /// checkpoint 写出间隔比例：单位为占 total_steps 的比例，
    /// 实际间隔步数 = max(1, floor(total_steps × ratio))（默认 0.05，约每 5% 写一次，与汇报频率一致）
    #[serde(default = "default_checkpoint_interval_ratio")]
    pub checkpoint_interval_ratio: f64,

    /// 整键互换（full_key_swap）算子执行概率（0.0~1.0），0 关闭。缺省 0.01。
    #[serde(default = "default_full_key_swap_probability")]
    pub full_key_swap_probability: f64,
}

/// 默认 **关闭**（0.0）：这是新增算子，不应改变未显式开启的用户的搜索轨迹。
/// 实测开启有益（0.2 时得分 −3.7%/t≈2.2、重码率 −2.8%），但那是相对消融基线的
/// 结论，不是「所有配置都该默认打开」的理由——概率与其它邻域的配比需按配置调。
fn default_conflict_probability() -> f64 {
    0.0
}
fn default_conflict_refresh_interval() -> usize {
    1000
}
/// 默认 20：实测 20/25/30 三档无统计学差异（见 `冲突导向邻域-字频排序改造.md` 5.3），
/// 20 的得分与重码均值最优，故取 20。
fn default_conflict_sample_window() -> usize {
    20
}
fn default_conflict_weight_by_freq() -> bool {
    true
}
/// 默认 **开启**：实测（n=32/臂）启用后用指分布偏差 −6.4%（t=−0.94），
/// 代价是重码 +1.2%（t=1.71）、CV +1.5%（t=2.28）、综合得分 +2.3%（t=1.58）。
/// 即拿综合分换更均匀的用指分布（分布项权重只有 0.02，而限制落点会挤掉
/// 拆重码最有效的键位）。若只追求综合得分，配置里置 `false` 回到均匀随机。
/// 详见 `冲突导向邻域-字频排序改造.md` 5.4。
fn default_conflict_key_by_load() -> bool {
    true
}
fn default_checkpoint_interval_ratio() -> f64 {
    0.05
}
fn default_temp_start() -> f64 {
    0.01
}
fn default_temp_end() -> f64 {
    1e-5
}
fn default_comfort_progress() -> f64 {
    0.5
}
fn default_full_key_swap_probability() -> f64 {
    0.01
}

/// 全码目标配置（对应 [targets.full_code] 段）
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FullCodeTargets {
    pub enabled: bool,
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
    pub low_weight: f64,
    // _max 硬约束（0.0 = 不启用）
    pub collision_count_max: f64,
    pub collision_rate_max: f64,
    pub equivalence_max: f64,
    pub equiv_cv_max: f64,
    pub distribution_max: f64,
}

impl Default for FullCodeTargets {
    fn default() -> Self {
        Self {
            enabled: false,
            collision_count: 0.0,
            collision_rate: 0.0,
            equivalence: 0.0,
            equiv_cv: 0.0,
            distribution: 0.0,
            low_weight: 0.01,
            collision_count_max: 0.0,
            collision_rate_max: 0.0,
            equivalence_max: 0.0,
            equiv_cv_max: 0.0,
            distribution_max: 0.0,
        }
    }
}

/// 目标配置容器（对应 [targets] 段）
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
pub struct TargetsConfig {
    #[serde(default)]
    pub full_code: FullCodeTargets,
}

/// TOML 可选 scale 配置（所有字段 Option<f64>，对应 [scale] 段）
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ScaleConfigToml {
    pub collision_count: Option<f64>,
    pub collision_rate: Option<f64>,
    pub equivalence: Option<f64>,
    pub equiv_cv: Option<f64>,
    pub distribution: Option<f64>,
}

// =========================================================================
// 📥 配置加载
// =========================================================================

impl Config {
    /// 从指定路径加载配置（支持 `.ron` 与 `.toml`，按扩展名决定先试哪个）
    ///
    /// #1：解析/读取失败不再静默回退默认配置，而是返回 `Err`，由调用方 `exit(1)`。
    /// 这样「配置写错却没生效」不会再被默认值掩盖。
    ///
    /// #2：两种格式互为兜底。`optimize` 会把配置备份成 `inputs/config.<源扩展名>`，
    /// 但历史输出目录里备份件可能叫 `config.ron` 却是 TOML 内容（反过来也一样），
    /// 所以不能只靠扩展名硬判——先按扩展名试，失败再试另一种，两种都失败才报错。
    pub fn load_from_path(path: &str) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("无法读取配置文件 {}: {}", path, e))?;
        let 优先_toml = std::path::Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"));

        let 先 = |文本: &str| -> Result<Self, String> {
            if 优先_toml {
                toml::from_str(文本).map_err(|e| e.to_string())
            } else {
                ron::from_str(文本).map_err(|e| e.to_string())
            }
        };
        let 后 = |文本: &str| -> Result<Self, String> {
            if 优先_toml {
                ron::from_str(文本).map_err(|e| e.to_string())
            } else {
                toml::from_str(文本).map_err(|e| e.to_string())
            }
        };

        let (首选名, 备用名) = if 优先_toml {
            ("TOML", "RON")
        } else {
            ("RON", "TOML")
        };
        match 先(&content) {
            Ok(c) => {
                println!("✅ 已加载配置文件: {}（{}）", path, 首选名);
                Ok(c)
            }
            Err(e1) => match 后(&content) {
                Ok(c) => {
                    println!(
                        "✅ 已加载配置文件: {}（{}，扩展名与实际格式不符）",
                        path, 备用名
                    );
                    Ok(c)
                }
                Err(e2) => Err(format!(
                    "配置文件解析失败 {}\n  按{}解析: {}\n  按{}解析: {}",
                    path, 首选名, e1, 备用名, e2
                )),
            },
        }
    }

    /// 验证权重配置是否合理
    ///
    /// #11：权重和明显偏离 1.0 时不再仅告警，而是返回 `Err`，由调用方 `exit(1)`，
    /// 避免配置笔误被「照常跑下去」掩盖。
    pub fn validate_weights(&self) -> Result<(), String> {
        let mut problems: Vec<String> = Vec::new();

        let total_full = self.weights.full_code.collision_count
            + self.weights.full_code.collision_rate
            + self.weights.full_code.equivalence
            + self.weights.full_code.equiv_cv
            + self.weights.full_code.distribution;
        if (total_full - 1.0).abs() > 0.001 {
            problems.push(format!("全码权重总和不为 1.0 (当前: {:.3})", total_full));
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "权重配置不合法（请修正后重跑）:\n  - {}",
                problems.join("\n  - ")
            ))
        }
    }

    /// 验证退火温度与舒适区参数是否可用
    ///
    /// 温度必须为正、初温应高于末温：`TemperatureSchedule::build` 用 `t_start * (t_end/t_start)^q`
    /// 生成曲线，`t_start = 0` 时 `0 * inf^q = NaN`；NaN 温度使 Metropolis 判据
    /// `rng < (-delta/temp).exp()` 恒为假，整轮退火静默退化成严格爬山（不报错、结果莫名其妙变差）。
    /// 舒适区宽度为 0 或负数会让高斯权重变 NaN。这里提前拦下，避免跑完几天才发现。
    pub fn validate_annealing(&self) -> Result<(), String> {
        let a = &self.annealing;
        let mut problems: Vec<String> = Vec::new();

        if !(a.temp_start.is_finite() && a.temp_start > 0.0) {
            problems.push(format!("temp_start 必须为正数（当前: {}）", a.temp_start));
        }
        if !(a.temp_end.is_finite() && a.temp_end > 0.0) {
            problems.push(format!("temp_end 必须为正数（当前: {}）", a.temp_end));
        }
        if a.temp_start.is_finite() && a.temp_end.is_finite() && a.temp_end > a.temp_start {
            problems.push(format!(
                "temp_end ({}) 大于 temp_start ({})：温度会反向上升",
                a.temp_end, a.temp_start
            ));
        }
        if !(a.comfort_width.is_finite() && a.comfort_width > 0.0) {
            problems.push(format!(
                "comfort_width 必须为正数（当前: {}）",
                a.comfort_width
            ));
        }
        if !(a.comfort_slowdown.is_finite() && (0.0..=1.0).contains(&a.comfort_slowdown)) {
            problems.push(format!(
                "comfort_slowdown 必须在 [0, 1] 内（当前: {}）",
                a.comfort_slowdown
            ));
        }
        if let Some(ct) = a.comfort_temp {
            if !ct.is_finite() {
                problems.push(format!("comfort_temp 不是有限数（当前: {}）", ct));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "退火参数不合法（请修正后重跑）:\n  - {}",
                problems.join("\n  - ")
            ))
        }
    }

    /// 计算最小改进步数
    pub fn min_improve_steps(&self) -> usize {
        (self.annealing.total_steps as f64 * self.annealing.min_improve_steps_ratio) as usize
    }

    /// 获取 TargetsConfig，缺失时返回默认值（全部 disabled）
    pub fn get_targets_config(&self) -> TargetsConfig {
        self.targets.clone().unwrap_or_default()
    }

    /// 获取权重配置
    pub fn get_weight_config(&self) -> WeightConfig {
        WeightConfig {
            weight_collision_count: self.weights.full_code.collision_count,
            weight_collision_rate: self.weights.full_code.collision_rate,
            weight_equivalence: self.weights.full_code.equivalence,
            weight_equiv_cv: self.weights.full_code.equiv_cv,
            weight_distribution: self.weights.full_code.distribution,
        }
    }
}

// =========================================================================
// 🔧 默认配置（后备）
// =========================================================================

impl Default for Config {
    fn default() -> Self {
        Config {
            files: FilesConfig {
                // 与 src/prepare/names.rs 保持同一套产物名
                fixed: crate::prepare::names::字根约束.to_string(),
                splits: crate::prepare::names::单字拆分.to_string(),
                pair_equiv: "pair_equivalence.txt".to_string(),
                key_dist: "key_distribution.txt".to_string(),
                full_splits: String::new(),
            },
            keys: KeysConfig {
                allowed: "qwertyuiopasdfghjklzxcvbnm".to_string(),
                display_order: "qwertyuiopasdfghjklzxcvbnm".to_string(),
                termination_keys: default_termination_keys(),
            },
            weights: WeightsConfig {
                full_code: FullCodeWeights {
                    collision_count: 0.07,
                    collision_rate: 0.62,
                    equivalence: 0.2,
                    equiv_cv: 0.01,
                    distribution: 0.1,
                },
            },
            annealing: AnnealingConfig {
                threads: 16,
                total_steps: 10_000,
                temp_start: default_temp_start(),
                temp_end: default_temp_end(),
                comfort_progress: default_comfort_progress(),
                comfort_temp: None,
                comfort_width: 0.15,
                comfort_slowdown: 0.8,
                swap_probability: 0.3,
                min_improve_steps_ratio: 0.1,
                reheat_factor: 1.25,
                conflict_probability: default_conflict_probability(),
                conflict_refresh_interval: default_conflict_refresh_interval(),
                conflict_sample_window: default_conflict_sample_window(),
                conflict_weight_by_freq: default_conflict_weight_by_freq(),
                conflict_key_by_load: default_conflict_key_by_load(),
                checkpoint_interval_ratio: default_checkpoint_interval_ratio(),
                full_key_swap_probability: default_full_key_swap_probability(),
            },
            scale: None,
            targets: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ScaleConfigToml 解析测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_scale_config_toml_parse() {
        let toml_str = r#"
[files]
fixed = "字根约束.txt"
splits = "单字拆分.txt"
pair_equiv = "pair_equivalence.txt"
key_dist = "key_distribution.txt"

[keys]
allowed = "qwertyuiopasdfghjklzxcvbnm"
display_order = "qwertyuiopasdfghjklzxcvbnm"

[weights.full_code]
collision_count = 0.07
collision_rate = 0.62
equivalence = 0.2
equiv_cv = 0.01
distribution = 0.1

[annealing]
threads = 4
total_steps = 1000
temp_start = 1.0
temp_end = 0.000001
comfort_progress = 0.5
comfort_width = 0.15
comfort_slowdown = 0.8
swap_probability = 0.3
min_improve_steps_ratio = 0.1
reheat_factor = 1.0

[scale]
collision_count = 0.001
collision_rate = 10.0
equivalence = 1.5
equiv_cv = 5.0
distribution = 0.2
"#;
        let cfg: Config = toml::from_str(toml_str).expect("解析失败");
        let scale = cfg.scale.expect("[scale] 段应存在");
        assert_eq!(scale.collision_count, Some(0.001));
        assert_eq!(scale.collision_rate, Some(10.0));
        assert_eq!(scale.equivalence, Some(1.5));
        assert_eq!(scale.equiv_cv, Some(5.0));
        assert_eq!(scale.distribution, Some(0.2));
    }

    #[test]
    fn test_scale_config_missing() {
        // 不含 [scale] 段的最小配置
        let cfg: Config = toml::from_str(minimal_config_prefix()).expect("解析失败");
        assert!(cfg.scale.is_none(), "缺失 [scale] 段时应为 None");
    }

    // -----------------------------------------------------------------------
    // 旧配置兼容：已移除的简码段仍能被忽略而不是解析失败
    // -----------------------------------------------------------------------

    #[test]
    fn 旧配置里的简码段被忽略() {
        let toml_str = format!(
            "{}\nsimple_levels = [{{ level = 1, code_num = 1, rules = [\"Aa\"] }}]\n\n\
             [weights.simple_code]\nenabled = true\nfull_code_weight = 0.7\n\
             simple_code_weight = 0.3\nfreq = 0.5\nequiv = 0.15\ndist = 0.05\n\
             collision_count = 0.05\ncollision_rate = 0.25\n\n\
             [fixed_simple_codes]\n\"不\" = \"u\"\n",
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("旧配置应能被忽略简码段后解析");
        assert_eq!(cfg.annealing.total_steps, 100);
    }

    // -----------------------------------------------------------------------
    // TargetsConfig 解析测试
    // -----------------------------------------------------------------------

    fn minimal_config_prefix() -> &'static str {
        r#"
[files]
fixed = "f"
splits = "s"
pair_equiv = "p"
key_dist = "k"

[keys]
allowed = "abc"
display_order = "abc"

[weights.full_code]
collision_count = 0.5
collision_rate = 0.5
equivalence = 0.0
equiv_cv = 0.0
distribution = 0.0

[annealing]
threads = 1
total_steps = 100
temp_start = 1.0
temp_end = 0.001
comfort_progress = 0.5
comfort_width = 0.1
comfort_slowdown = 0.8
swap_probability = 0.3
min_improve_steps_ratio = 0.1
reheat_factor = 1.0
"#
    }

    #[test]
    fn test_targets_full_code_parse() {
        let toml_str = format!(
            r#"{}
[targets.full_code]
enabled = true
collision_count = 50.0
collision_rate = 0.005
equivalence = 1.5
equiv_cv = 0.3
distribution = 5.0
low_weight = 0.02
collision_count_max = 200.0
collision_rate_max = 0.02
equivalence_max = 2.0
equiv_cv_max = 0.5
distribution_max = 10.0
"#,
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("解析失败");
        let targets = cfg.targets.expect("[targets] 段应存在");
        let fc = &targets.full_code;
        assert!(fc.enabled);
        assert_eq!(fc.collision_count, 50.0);
        assert_eq!(fc.collision_rate, 0.005);
        assert_eq!(fc.equivalence, 1.5);
        assert_eq!(fc.equiv_cv, 0.3);
        assert_eq!(fc.distribution, 5.0);
        assert_eq!(fc.low_weight, 0.02);
        assert_eq!(fc.collision_count_max, 200.0);
        assert_eq!(fc.collision_rate_max, 0.02);
        assert_eq!(fc.equivalence_max, 2.0);
        assert_eq!(fc.equiv_cv_max, 0.5);
        assert_eq!(fc.distribution_max, 10.0);
    }

    #[test]
    fn test_targets_defaults() {
        // 不含 [targets] 段时，get_targets_config() 应返回默认值
        let toml_str = minimal_config_prefix();
        let cfg: Config = toml::from_str(toml_str).expect("解析失败");
        assert!(cfg.targets.is_none(), "缺失 [targets] 段时应为 None");

        let targets = cfg.get_targets_config();
        assert!(!targets.full_code.enabled);
        assert_eq!(targets.full_code.collision_count, 0.0);
        assert_eq!(targets.full_code.low_weight, 0.01);
        assert_eq!(targets.full_code.collision_count_max, 0.0);
    }

    // -----------------------------------------------------------------------
    // 冲突导向配置项：默认值与向后兼容解析测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_conflict_fields_default_when_absent() {
        let cfg: Config = toml::from_str(minimal_config_prefix()).expect("解析失败");
        let a = &cfg.annealing;
        assert_eq!(a.conflict_probability, 0.0);
        assert_eq!(a.conflict_refresh_interval, 1000);
        assert_eq!(a.conflict_sample_window, 20);
        assert!(a.conflict_weight_by_freq);
        assert!(a.conflict_key_by_load); // 默认开启（拿综合分换更均匀的用指分布）
    }

    #[test]
    fn test_conflict_fields_partial_present() {
        let toml_str = format!(
            "{}\nconflict_probability = 0.25\nconflict_weight_by_freq = true\n",
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("解析失败");
        let a = &cfg.annealing;
        assert_eq!(a.conflict_probability, 0.25); // 显式
        assert!(a.conflict_weight_by_freq); // 显式
        assert_eq!(a.conflict_refresh_interval, 1000); // 默认
        assert_eq!(a.conflict_sample_window, 20); // 默认
        assert!(a.conflict_key_by_load); // 默认开启
    }

    #[test]
    fn test_温度直接取自配置且默认跨度一千倍() {
        // 默认值是固定种子温度扫描选出的那一组（初温 0.01 / 末温 1e-5），跨度 1000 倍，
        // 曲线中点温度 1e-3。注意它**不再等于上游 0.005/5e-5**，是有意偏离。
        let d = Config::default().annealing;
        assert_eq!(d.temp_start, default_temp_start());
        assert_eq!(d.temp_end, default_temp_end());
        let 跨度 = d.temp_start / d.temp_end;
        assert!(
            (跨度 - 1000.0).abs() < 1e-6,
            "默认末温应为初温的 1/1000，实际跨度 {}",
            跨度
        );

        // 配置里写了什么就生效什么（不再有运行时标定覆盖这一步）。
        let 改过 = minimal_config_prefix().replace("temp_start = 1.0", "temp_start = 0.02");
        let cfg: Config = toml::from_str(&改过).expect("解析失败");
        assert_eq!(cfg.annealing.temp_start, 0.02);
        // 另一项里写的是 0.001，原样生效（不会再被任何运行时逻辑覆盖）。
        assert_eq!(cfg.annealing.temp_end, 0.001);
    }

    #[test]
    fn test_conflict_fields_default_impl() {
        let a = &Config::default().annealing;
        // 逐项与 `default_*` 函数比对，而不是硬编码字面量：这样 `Config::default()`
        // 与 serde 缺省值一旦分叉，测试会立刻失败（此前二者各写一份 0.0/0.25 就分叉过）。
        assert_eq!(a.conflict_probability, default_conflict_probability());
        assert_eq!(
            a.conflict_refresh_interval,
            default_conflict_refresh_interval()
        );
        assert_eq!(a.conflict_sample_window, default_conflict_sample_window());
        assert_eq!(a.conflict_weight_by_freq, default_conflict_weight_by_freq());
        assert_eq!(a.conflict_key_by_load, default_conflict_key_by_load());
    }

    #[test]
    fn 权重只有全码一项且和为1() {
        // 简码权重删除后，`[weights]` 只剩 `full_code`：默认值必须自洽。
        let cfg = Config::default();
        assert!(cfg.validate_weights().is_ok(), "默认权重应合法");
        let w = cfg.get_weight_config();
        let 和 = w.weight_collision_count
            + w.weight_collision_rate
            + w.weight_equivalence
            + w.weight_equiv_cv
            + w.weight_distribution;
        assert!((和 - 1.0).abs() < 1e-9, "全码权重之和应为 1，实际 {}", 和);
    }

    #[test]
    fn ron_配置往返一致() {
        // 序列化再解析回来，验证 RON 路径与结构体定义一致（含默认字段）
        let cfg = Config::default();
        let ron_text = ron::ser::to_string_pretty(&cfg, ron::ser::PrettyConfig::default()).unwrap();
        let back: Config = ron::from_str(&ron_text).expect("RON 解析失败");
        assert_eq!(back.annealing.total_steps, cfg.annealing.total_steps);
        assert_eq!(
            back.weights.full_code.collision_count,
            cfg.weights.full_code.collision_count
        );
    }

    /// 加载器要认两种格式：`.ron` 走 RON、`.toml` 走 TOML，且同一份配置两种写法
    /// 解析出来的结构必须逐字段相同（否则「换成 TOML」就成了改配置）。
    #[test]
    fn load_from_path_两种格式解析结果一致() {
        let 基 = Config::default();
        let ron文本 = ron::ser::to_string_pretty(&基, ron::ser::PrettyConfig::default()).unwrap();
        let toml文本 = toml::to_string(&基).expect("Config 应能序列化成 TOML");

        let 目录 = std::env::temp_dir().join(format!("cg_cfg_fmt_{}", std::process::id()));
        std::fs::create_dir_all(&目录).unwrap();
        let ron路径 = 目录.join("a.ron");
        let toml路径 = 目录.join("a.toml");
        std::fs::write(&ron路径, &ron文本).unwrap();
        std::fs::write(&toml路径, &toml文本).unwrap();

        let 甲 = Config::load_from_path(&ron路径.to_string_lossy()).expect("RON 加载失败");
        let 乙 = Config::load_from_path(&toml路径.to_string_lossy()).expect("TOML 加载失败");
        // Config 没实现 PartialEq，用序列化文本比，等价于逐字段比较。
        assert_eq!(
            ron::ser::to_string(&甲).unwrap(),
            ron::ser::to_string(&乙).unwrap(),
            "RON 与 TOML 两种写法解析出的配置不一致"
        );

        // 扩展名与实际格式不符时也要能兜底（resume 的备份件曾出现这种情况）。
        let 错名路径 = 目录.join("b.toml");
        std::fs::write(&错名路径, &ron文本).unwrap();
        let 丙 = Config::load_from_path(&错名路径.to_string_lossy())
            .expect("扩展名与实际格式不符时应兜底成功");
        assert_eq!(
            ron::ser::to_string(&丙).unwrap(),
            ron::ser::to_string(&甲).unwrap()
        );

        let _ = std::fs::remove_dir_all(&目录);
    }
}
#[cfg(test)]
mod annealing_validate_tests {
    use super::*;

    /// 初温/末温/舒适区宽度非法时必须报错：`temp_start = 0` 会让降温曲线出 NaN，
    /// Metropolis 判据恒假，整轮退火静默退化成严格爬山。
    #[test]
    fn 退火参数校验() {
        let mut cfg = Config::default();
        assert!(cfg.validate_annealing().is_ok(), "默认配置应合法");

        cfg.annealing.temp_start = 0.0;
        assert!(cfg.validate_annealing().is_err(), "初温 0 应被拒");

        cfg.annealing.temp_start = 1.0;
        cfg.annealing.temp_end = 2.0;
        assert!(
            cfg.validate_annealing().is_err(),
            "末温高于初温（温度反向上升）应被拒"
        );

        cfg.annealing.temp_end = 1e-3;
        cfg.annealing.comfort_width = 0.0;
        assert!(cfg.validate_annealing().is_err(), "舒适区宽度 0 应被拒");

        cfg.annealing.comfort_width = 0.15;
        cfg.annealing.comfort_slowdown = 1.5;
        assert!(
            cfg.validate_annealing().is_err(),
            "减速深度超出 [0,1] 应被拒"
        );
    }
}
