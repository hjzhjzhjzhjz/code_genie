# CodeGenie

一个基于模拟退火算法的**字根编码优化器**，用于优化汉字输入法的字根到键位映射方案。

## 功能特性

- **自动调温**：温度不写死，每次运行按初态采样 `|Δscore|` 闭式标定 `t_start` / `t_end`，随权重与评分尺度走
- **多目标优化**：重码数 / 重码率 / 加权当量 / 当量 CV / 用指分布偏差，支持 `[targets]` 设目标值与 `_max` 硬约束
- **逃逸机制**：冲突导向邻域、低温智能扰动、平台期重热、整键互换（`full_key_swap`）
- **并行与多轮**：多线程独立退火 + `--rounds N` 内置多轮播种（每轮吃上一轮结果、复用尺度）
- **增量评估**：`Evaluator` 支持 `try_move()` / `try_swap()` 增量更新，热路径零全量重建
- **简码系统**：规则式简码（`AaBaCa`）+ 上屏键 / 异手过滤 / 固定简码
- **全量码表**：`[files] full_splits` 指定全量拆分表后，收尾自动输出覆盖 12 万字的 `完整码表.txt`
- **产物落地**：Rime 方案包（以星月为底包只换码）、宇浩测评对比、整句准确率、码表诊断

## 安装

```bash
git clone https://github.com/hjzhjzhjzhjz/code_genie.git
cd code_genie
cargo build --release
```

## 子命令

全局参数：`-c/--config <路径>`（默认 `config.toml`）、`--rounds N`（多轮播种，默认 1）。

| 命令 | 用途 | 最简示例 |
|------|------|----------|
| `optimize`（默认） | 跑模拟退火优化 | `code_genie -c config.toml optimize -d out --rounds 3` |
| `resume` | 从 checkpoint 断点续算 | `code_genie resume -d out` |
| `bench` | 压测 eval / 算子 / SA 吞吐 | `code_genie bench --iters 20000 --steps 200000` |
| `encode` | 按 keymap 给拆分表编码 | `code_genie encode -k 字根键位表.txt -d input-division.txt` |
| `evaluate` | 评估现有方案指标 | `code_genie evaluate -k 字根键位表.txt` |
| `export` | 导出测评码表（`字\t码`，简码在前） | `code_genie export -k 字根键位表.txt -c chaifen-all.txt -o 全码表.txt` |
| `chart` | 生成字根键位图（HTML） | `code_genie chart --keymap 字根键位表.txt` |
| `assess` | 宇浩测评对比 + 方案档案 | `code_genie assess --dir runs --yuhao-dir yuhao-assess` |
| `rime-convert` | 以星月方案包为底包只换码 | `code_genie rime-convert --base <星月底包> --output <算码目录> --chaifen chaifen-all.txt` |
| `sentence` | 整句准确率（逐单元 + 连续码流）；词典吃 dict.yaml 或 TSV | `code_genie sentence --full-dict 完整码表.txt --corpus 语料.txt` |
| `pipeline` | 逐字取码推导表（Markdown） | `code_genie pipeline --keymap 字根键位表.txt --chaifen chaifen-all.txt --top 5` |
| `merge` | 字根归并对比 | `code_genie merge --keymap 字根键位表.txt --chaifen chaifen-all.txt --rules "还=人"` |
| `diagnose` | 批量诊断码表目录 | `code_genie diagnose --dir runs` |

## 输入文件

| 文件 | 说明 | 格式 |
|------|------|------|
| `input-fixed.txt` | 固定 / 受限字根定义 | `字根名 [tab] 键位` |
| `input-roots.txt` | 动态字根组定义 | `字根名1 字根名2 ...`（同组同行） |
| `input-division.txt` | 汉字拆分表（优化用，通常只含常用字） | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `chaifen-all.txt` | 全量拆分表（出码表用，约 12 万字） | 同上 |
| `pair_equivalence.txt` | 键对当量表 | `键对 [tab] 当量值` |
| `key_distribution.txt` | 键位分布目标 | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

> 注意两份拆分表的写法差异：`input-division.txt` 里字根**已展开到码位**（`到 → 至.0 刂.0 刂.1`），
> `chaifen-all.txt` 是**纯根名**（`到 → 至 刂`）。码表一律用后者按编码规则现算。

## 配置文件

从 `config.toml.example` 复制为 `config.toml` 使用。

```toml
[files]
fixed = "input-fixed.txt"
dynamic = "input-roots.txt"
splits = "input-division.txt"          # 优化用（常用字，快）
full_splits = "moling/chaifen-all.txt" # 全量拆分表（手动指定；留空 = 不生成完整码表）

[annealing]
threads = 12
total_steps = 2000000
# 温度不手工配置：运行时按初态 |Δscore| 自动标定 t_start / t_end
comfort_progress = 0.5   # 舒适区在降温曲线上的位置（0~1）：0.5 = 曲线正中
comfort_width = 0.15       # 舒适区宽度
comfort_slowdown = 0.7     # 舒适区减速因子
swap_probability = 0.35
perturb_interval_ratio = 0.05  # 低温扰动间隔（0=关闭）
perturb_strength = 0.03        # 扰动强度（0=关闭）
perturb_trigger_temp_ratio = 0.01  # 扰动触发阈值 = 舒适区温度 × 本比例
perturb_temp_multiplier = 2.0      # 扰动时温度倍率
perturb_collision_pairs_limit = 10 # 扰动优先采样的冲突对上限
reheat_factor = 1.3            # 平台期重热倍率（1.0=关闭）
max_parts = 3                  # 最大码长
conflict_probability = 0.25    # 冲突导向算子概率（0=关闭）
full_key_swap_probability = 0.01   # 整键互换算子概率（0=关闭）

[weights.full_code]
collision_count = 0.07
collision_rate = 0.62
equivalence = 0.2
equiv_cv = 0.01
distribution = 0.1

[targets.full_code]      # 目标偏差模式：enabled=false 时退化为原线性公式
enabled = false
collision_count = 0.0
# ... 各指标目标值 + *_max 硬约束（0 = 不启用）
low_weight = 0.01
```

> `[scale]` 段可选手动固化缩放因子；不写则由初态指标自动校准（推荐）。
> 写 `[scale]` 时全码 5 个字段必须齐全，简码 5 个字段在 `simple_code.enabled = true` 时也必须齐全。

## 输出文件

优化完成后在 `output-{时间戳}/` 目录生成：

| 文件 | 内容 |
|------|------|
| `字根键位表.txt` | 字根 → 键位串 + 使用频次 |
| `单字编码表.txt` | 简码在前、全码在后（`字/码/频次`），覆盖**优化字集** |
| `完整码表.txt` | 同上格式，覆盖**全量拆分表**（配置 `full_splits` 时生成） |
| `简码表.txt` | 简码分配（按级别、按频率排序） |
| `合并码表.txt` | 简码与全码合并输出 |
| `用指分布.txt` / `当量分布.txt` | 用指分布 / 当量分布统计 |
| `汇总.txt` | 各线程结果排名 |
| `尺度.json` | 固化缩放因子（供 `--scale` 复用） |
| `checkpoint/` | 断点续算数据（Ctrl-C 自动落盘） |
| `inputs/` | 本次运行所用输入文件的备份（含 config.toml） |
| `thread-XX/` | 每个线程的独立输出 |

## 项目结构

```
src/
├── main.rs          # CLI 入口、子命令分发、多轮播种、尺度解析
├── annealing.rs     # 模拟退火主循环、邻域算子、初始化策略
├── evaluator.rs     # 全码/简码评估器（增量更新、目标偏差、硬约束）
├── bucket_store.rs  # 全码桶存储（按容量+字数自适应密集/稀疏）
├── context.rs       # OptContext：字根组、汉字信息、当量表、反向索引
├── config.rs        # TOML 配置解析与默认值
├── calibrate.rs     # 缩放因子校准 + 自动调温
├── schedule.rs      # 温度调度（高斯舒适区 + LUT）
├── simple.rs        # 独立简码规则文件解析
├── loader.rs        # 各类输入文件加载
├── output.rs        # 结果输出（含全量码表）
├── export_table.rs  # 测评码表导出（星月 encoder 取码）
├── rime_convert.rs  # 星月方案包只换码
├── sentence.rs      # 整句准确率（内置 Viterbi / 外部 librime）
├── pipeline.rs      # 取码推导表 + 字根归并对比
├── diagnose.rs      # 码表诊断
├── assess.rs        # 宇浩测评对比 + 方案档案
├── handfeel.rs      # 手感指标（速度当量、选重、同指连击）
├── root_chart.rs    # 字根键位图（HTML）
├── checkpoint.rs    # 断点续算
├── fsutil.rs        # 文件读写（自动归档旧文件）
├── types.rs         # 核心类型
└── validate.rs      # 输入校验
```

## 技术细节

### 字根分类

1. **固定字根**：预定义的单键映射
2. **受限字根**：在限定键位集中选择
3. **动态字根**：需要优化分配的字根

### 编码规则（星月 / 宇浩）

| 根数 | 码 |
|------|-----|
| 1 根 | `A + S + S` |
| 2 根 | `A1 A2 S2 S1` |
| 3 根 | `A1 A2 A3 S3` |
| ≥4 根 | `A1 A2 A3 A_last` |

其中 A=键位串首字母（大码）、S=第 2 个字母（不足两位时回落大码）。词码：2 字 `AaAbBaBb`、3 字 `AaBaCaCb`、4+ 字 `AaBaCaZa`。

### 简码规则

规则串如 `AaBaCa`：
- 大写字母 `A-Z`：根选择器（`A`=第 0 个根）
- 小写字母 `a-z`：编码选择器（`a`=第 0 个码）

### 性能

- `rayon` 多线程并行退火
- 增量更新 `try_move()` / `try_swap()`，周期对账（reconcile）代替每步全量重建
- 温度调度预计算 LUT
- Release 启用 LTO + 单 codegen-unit + `panic=abort`
- `cargo run --release -- bench` 可压测各算子成本与 SA 步速

## 许可证

MIT License
