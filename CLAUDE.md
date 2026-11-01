# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

CodeGenie 是一个**字根编码优化器**，使用模拟退火算法优化汉字输入法的字根到键位映射方案。目标是最小化重码数、当量值和键位分布偏差，同时支持简码优化。

## 构建与运行命令

```bash
# 编译（Release 模式，推荐用于优化）
cargo build --release

# 运行优化（默认行为，等同于 optimize 子命令）
cargo run --release

# 使用指定配置文件 + 多轮播种
cargo run --release -- -c config.toml optimize -d out --rounds 3

# 断点续算 / 压测 / 编码 / 评估
cargo run --release -- resume -d out
cargo run --release -- bench --iters 20000 --steps 200000
cargo run --release -- encode -k 字根键位表.txt -d input-division.txt -o 单字编码表.txt
cargo run --release -- evaluate -k 字根键位表.txt

# 出码表 / Rime 方案 / 测评 / 诊断
cargo run --release -- export -k 字根键位表.txt -c moling/chaifen-all.txt -o 全码表.txt
cargo run --release -- rime-convert --base <星月底包> --output <算码目录> --chaifen moling/chaifen-all.txt
cargo run --release -- assess --dir runs --yuhao-dir yuhao-assess
cargo run --release -- sentence --full-dict pkg.full.dict.yaml --corpus 语料.txt
cargo run --release -- pipeline --keymap 字根键位表.txt --chaifen moling/chaifen-all.txt --top 5
cargo run --release -- merge --keymap 字根键位表.txt --chaifen moling/chaifen-all.txt --rules "还=人"
cargo run --release -- diagnose --dir runs
cargo run --release -- chart --keymap 字根键位表.txt

# 检查编译错误 / 跑测试
cargo check
cargo test --release
```

全局参数：`-c/--config`（默认 `config.toml`）、`--rounds N`（多轮播种，默认 1）。
子命令共 13 个：`optimize`(默认) / `resume` / `bench` / `encode` / `evaluate` / `export` /
`chart` / `assess` / `rime-convert` / `sentence` / `pipeline` / `merge` / `diagnose`。

## 核心架构

### 数据流

```
输入文件 → loader.rs → OptContext::new() → annealing.rs → Evaluator → output.rs → 输出文件
```

### 模块职责

| 模块 | 职责 |
|------|------|
| `main.rs` | CLI 入口（clap），13 个子命令的分发、多轮播种、尺度解析、收尾保存编排 |
| `annealing.rs` | 混合优化算法核心：4 种初始化策略、multi-start + hill climb 热身、冲突导向邻域搜索、SA 主循环 |
| `context.rs` | `OptContext` 构建所有优化数据：字根组、汉字信息、当量表、简码配置、反向索引 |
| `evaluator.rs` | `Evaluator` 计算重码数/率、当量、分布偏差等指标；`SimpleEvaluator` 处理简码评估；支持 `try_move()`/`try_swap()` 增量更新 |
| `loader.rs` | 加载固定字根、动态字根、拆分表、当量表、键位分布、keymap 文件 |
| `types.rs` | 核心类型：`CharInfo`、`RootGroup`、`Metrics`、`SimpleMetrics`、`LogicalRoot`、`CharSimpleInfo`、`SimpleCodeConfig` 等 |
| `config.rs` | TOML 配置解析（`Config` 及子结构体），权重校验，简码规则转换，默认配置后备 |
| `schedule.rs` | `TemperatureSchedule` 温度调度器，高斯舒适区自适应降温，预计算 LUT |
| `calibrate.rs` | `calibrate_scales()` 根据初始指标自动校准各维度缩放因子 |
| `simple.rs` | 解析独立简码规则文件（非 TOML 格式），用于 evaluate 子命令的 `--simple` 参数 |
| `output.rs` | 输出所有结果文件：字根键位表、单字编码表、简码表、合并码表、完整码表、键位分布、当量分布、汇总 |
| `bucket_store.rs` | 全码桶存储，按「Dense 绝对占用预算」+ `n_chars` 判定密集/稀疏后端（`choose_backend_for`） |
| `export_table.rs` | 测评码表导出；`取码序列` 实现星月 encoder 状态机（`export`/`pipeline` 共用） |
| `rime_convert.rs` | 以星月方案包为底包，只重算 chaifen/full/words/quick/short/roots 六张表的码 |
| `sentence.rs` | 整句准确率：逐单元首候选 + 连续码流解码；词典吃 dict.yaml 或纯 TSV；可调外部 librime 解码器 |
| `pipeline.rs` | 逐字取码推导表（Markdown）与字根归并对比 |
| `diagnose.rs` | 批量诊断码表目录（重码、当量、分布、选重） |
| `assess.rs` | 调 yuhao-assess 跑测评、出对比表、生成可导入网页版的方案档案 |
| `handfeel.rs` | 手感指标：速度当量、动态选重、同指连击率、换手率 |
| `root_chart.rs` | 字根键位图（HTML） |
| `checkpoint.rs` | 断点续算：线程检查点、元信息、原子写、历史归档 |
| `fsutil.rs` | 文件读写，覆盖前自动按时间戳归档旧文件 |
| `validate.rs` | 校验拆分表中所有字根是否已定义，输出缺失报告 |

### 关键数据结构

- **OptContext**: 优化上下文，包含 `groups`（字根组）、`chars`（汉字信息）、`group_to_chars`（反向索引）、`equiv_table`（当量矩阵）、`simple_config`、`char_simple_info`（逻辑根+级别指令）、`group_to_simple_affected`、`group_freq_sum`（预计算的组加权频率，用于 O(1) 键位使用量更新）等
- **Evaluator**: 维护 `code_to_chars` 碰撞桶（Vec 直接索引，大小 = code_space）、`char_bucket_pos`（桶内位置索引）、`bucket_freq_sum`/`bucket_max_freq`（桶频率统计）、`key_weighted_usage` 键位使用统计，支持增量 move/swap 操作和 Boltzmann 接受判定
- **SimpleEvaluator**: 按级别跟踪简码分配，计算简码重码数/率、频率覆盖、当量、分布
- **RootGroup**: 字根组，包含 `roots`（字根名列表）和 `allowed_keys`（允许分配的键位）
- **CharInfo**: 汉字拆分信息，`parts: Vec<u16>` 中值 < 1000 为直接键位索引，>= 1000 为组标记（GROUP_MARKER + group_index）
- **LogicalRoot**: 逻辑根，桥接拆分表与简码系统，包含 base_name 和有序子根列表

### 字根分类

1. **固定字根**: 单键映射，从 `input-fixed.txt` 加载，不参与优化
2. **有约束字根**: 多键选择（也在 `input-fixed.txt` 中定义），作为 `RootGroup` 参与优化但键位有约束
3. **动态字根**: 需要优化分配，从 `input-roots.txt` 加载，同行字根为一组

### 简码系统

简码规则格式为 `AaBaCa`，每两个字符为一组：
- 大写字母（根选择器）: A=第1根, B=第2根, ... Z=最后一根
- 小写字母（码位选择器）: a=第1码, b=第2码, ... z=最后一码

配置中 `code_num` 表示该级别可分配的简码数量（0 表示不限制，按频率贪心分配）。每个级别可配置多个候选规则（`rules` 数组），系统选择能成功解析的第一个。

### 模拟退火算法

**初始化阶段** (`multi_start_init`):
- 生成 12-32 个候选解，轮流使用 4 种策略：均衡贪心、频率贪心、分散贪心、随机
- 每个候选经 `enhanced_hill_climb` 精炼（混合操作：40% 冲突解决、30% 交换、10% 三路交换、10% 键位重组、10% 随机移动）
- 小规模问题额外执行坐标下降

**SA 主循环** (`simulated_annealing_resumable`):
- **自动调温**：进入主循环前按初态采样 100 次随机单组移动估 `Δ̄`，闭式给出
  `t_start = Δ̄ / ln(1/0.98)`、`t_end = Δ̄ / ln(1/0.02)` —— 温度随权重/scale 自动标定，配置里不写死
- **舒适区位置**：配置给的是 `comfort_progress`（曲线上的位置 0~1，默认 0.5 = 正中），
  `annealing::舒适区温度()` 按 `T(p) = t_start·(t_end/t_start)^p` 换算成温度。
  用进度而非温度比例，是因为温度随尺度变、进度不变，且「曲线走到几成」能直接看懂
- 按 `swap_probability`（随时间线性增长）选择 swap 或 move 操作
- 邻域算子：冲突导向（`conflict_probability`）、整键互换（`full_key_swap_probability`）、
  三路交换、键位重组、智能扰动（`perturb_*`）
- `Evaluator::try_move/try_swap` 内部完成增量评估 + Boltzmann 接受/回滚 + `_max` 硬约束检查
- 重热机制：连续 `min_improve_steps` 步无改进时乘以 `reheat_factor`
- 智能扰动：低温时周期性扰动碰撞相关组
- 最终精炼：对最优解执行 hill climb + 坐标下降
- 生产入口只有 `simulated_annealing_resumable`；旧的 `simulated_annealing` 已降为 `#[cfg(test)]`

## 输入文件格式

| 文件 | 格式 |
|------|------|
| `input-fixed.txt` | `字根名 [tab] 键位`（单键=固定，多键=约束） |
| `input-roots.txt` | `字根名1 字根名2 ...`（同行为一组，空格分隔） |
| `input-division.txt` | `汉字 [tab] 字根1 字根2 ... [tab] 频率`（字根**已展开到码位**：`至.0 刂.0 刂.1`） |
| `chaifen-all.txt` | 同上格式，但字根是**纯根名**（`至 刂`），全量约 12 万字，供出码表用 |
| `pair_equivalence.txt` | `键对 [tab] 当量值` |
| `key_distribution.txt` | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件 (config.toml)

从 `config.toml.example` 复制为 `config.toml` 使用。

关键配置项：

```toml
[files]
fixed = "input-fixed.txt"
dynamic = "input-roots.txt"
splits = "input-division.txt"          # 优化用（常用字，快）
full_splits = "moling/chaifen-all.txt" # 全量拆分表（手动指定；留空 = 不生成完整码表）
pair_equiv = "pair_equivalence.txt"    # 字根对当量表
key_dist = "key_distribution.txt"      # 用指分布目标

[annealing]
threads = 12              # 并行线程数
total_steps = 2000000     # 总迭代步数
# 温度不手工配置：运行时按初态采样 |Δscore| 自动标定 t_start / t_end
comfort_progress = 0.5   # 舒适区在降温曲线上的位置（0~1）：0.5 = 曲线正中
comfort_width = 0.15      # 舒适区宽度（高斯 sigma）
comfort_slowdown = 0.7    # 舒适区减速因子 (0-1)
swap_probability = 0.35   # 交换操作概率
min_improve_steps_ratio = 0.1  # 无改进触发重热的步数比例
perturb_interval_ratio = 0.05  # 扰动间隔比例（0=关闭）
perturb_strength = 0.03        # 扰动强度（0=关闭）
perturb_trigger_temp_ratio = 0.01  # 扰动触发阈值 = 舒适区温度 × 本比例
perturb_temp_multiplier = 2.0      # 扰动时温度倍率
perturb_collision_pairs_limit = 10 # 扰动优先采样的冲突对上限
reheat_factor = 1.3            # 重热倍率（1.0=关闭）
conflict_probability = 0.25    # 冲突导向算子概率（0=关闭）
full_key_swap_probability = 0.01   # 整键互换算子概率（0=关闭）
checkpoint_interval_ratio = 0.05   # checkpoint 间隔（占 total_steps 比例）

[keys]
allowed = "qwertyuiopasdfghjklzxcvbnm"
display_order = "qwertyuiopasdfghjklzxcvbnm"

[weights.full_code]
collision_count = 0.19   # 重码数权重
collision_rate = 0.3     # 重码率权重
equivalence = 0.3        # 当量权重
equiv_cv = 0.01          # 当量变异系数权重
distribution = 0.2       # 分布偏差权重

[weights.simple_code]
enabled = true
full_code_weight = 0.6   # 全码目标总权重
simple_code_weight = 0.4 # 简码目标总权重
collision_count = 0.1    # 简码重码数权重
collision_rate = 0.3     # 简码重码率权重
freq = 0.2               # 频率覆盖权重
equiv = 0.2              # 当量权重
dist = 0.2               # 分布偏差权重

[[simple_levels]]
level = 1
code_num = 1             # 该级别分配的简码数
rules = ["Aa"]           # 取码规则

[[simple_levels]]
level = 2
code_num = 1
rules = ["AaBa"]

[[simple_levels]]
level = 3
code_num = 0             # 0 = 不分配该级别简码
rules = ["AaAbZz", "AaBaZz"]  # 多候选规则
```

## 性能优化要点

- 使用 `rayon` 并行执行多线程模拟退火（每线程独立 SA 实例）
- `Evaluator` 支持增量更新（move/swap 只重算受影响的汉字），避免全量重算
- 碰撞桶走 `BucketStore`（按 `code_space` 阈值自适应密集数组/稀疏哈希），配合 `char_bucket_pos` 实现 O(1) swap_remove
- `bucket_freq_sum`/`bucket_max_freq` 实现碰撞频率的增量维护，避免每次遍历桶
- `group_freq_sum` 预计算组加权频率，`key_weighted_usage` 更新从 O(chars) 降为 O(1)
- `try_triple_swap`、`try_full_key_swap`、`coordinate_descent` 均为增量评估+回滚，不重建 Evaluator
- 简码出简走增量（阶段 1 候选集 + 阶段 2 受影响桶），热路径零全量重建，靠 `reconcile` 周期对账兜底
- SA 主循环输出实时速度统计（万步/分钟）
- 温度调度使用预计算查找表 (LUT, 100001 个采样点 + 线性插值)
- Release 编译启用 `opt-level=3`、LTO、单代码单元、`panic=abort`
- `calibrate_scales` 自动校准使各指标在初始状态贡献均衡

## 输出文件

优化完成后生成 `output-{YYYYMMDDHHMMSS}/` 目录：
- `字根键位表.txt`: 字根 → 键位串 + 使用频次
- `单字编码表.txt`: 简码在前、全码在后（`字/码/频次`），覆盖**优化字集**
- `完整码表.txt`: 同上格式，覆盖**全量拆分表**（配置 `files.full_splits` 时生成）
- `简码表.txt`: 简码分配（按级别、按频率排序）
- `合并码表.txt`: 简码+全码合并输出
- `用指分布.txt` / `当量分布.txt`: 用指分布 / 当量分布统计
- `汇总.txt`: 各线程结果排名汇总
- `尺度.json`: 固化缩放因子（供 `--scale` 复用）
- `checkpoint/`: 断点续算数据（Ctrl-C 自动落盘）
- `inputs/`: 本次运行所用输入文件的备份（含 config.toml）
- `thread-XX/`: 每个线程的独立输出子目录（包含上述同名文件）

> 旧版本产出的 `output-keymap.txt` / `output-encode.txt` / `output-simple-codes.txt` /
> `output-combined.txt` / `summary.txt` 已改名为上表的中文名；旧目录里仍是旧文件名。

## 依赖

| crate | 用途 |
|-------|------|
| `clap` (4, derive) | CLI 参数解析 |
| `rayon` (1.10) | 并行迭代 |
| `rand` (0.8) | 随机数生成 |
| `ctrlc` (3.4) | Ctrl-C 中断并写 checkpoint |
| `serde` (1.0, derive) + `toml` (0.8) | TOML 配置反序列化 |
| `chrono` (0.4) | 输出目录时间戳 |
| `hashbrown` (0.14) / `rustc-hash` (1.1) | 高性能哈希容器 |
| `smallvec` (1) | 小数组免堆分配 |
| `serde_json` (1.0) | JSON 支持（尺度/方案档案/检查点） |
| `sha2` (0.10) | 方案档案校验 |
| `proptest` (1, dev) | 属性测试 |
