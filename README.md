# CodeGenie

CodeGenie 是一个基于模拟退火算法的字根编码优化器，用于为汉字输入法搜索高质量的字根到键位映射方案。它的目标是同时优化重码率、当量和键位分布偏差，并支持简码规则与多级编码评估。

## 项目概述

该项目从输入的字根、汉字拆分表、当量表和键位分布目标出发，构建优化上下文，然后在多目标损失函数下迭代搜索最优键位分配方案。核心流程如下：

```text
输入文件 → loader.rs → OptContext::new() → annealing.rs → Evaluator → output.rs → 输出文件
```

它适用于：

- 字根键位布局优化
- 汉字编码方案评估
- 简码规则分析与分配
- 输入法键盘布局研究与实验

## 关键特性

- 模拟退火与局部优化混合搜索
- 多目标评估：重码数、重码率、当量、分布偏差
- 多线程并行优化（rayon）
- 增量评估与回滚，支持高性能局部移动/交换搜索
- 支持简码系统配置与评估
- 支持优化模式、编码模式和评估模式三种运行方式

## 快速开始

### 1. 安装 Rust

确保本机已安装 Rust 工具链：

```bash
rustc --version
cargo --version
```

### 2. 构建项目

```bash
git clone https://github.com/hjzhjzhjzhjz/code_genie.git
cd code_genie
cargo build --release
```

### 3. 运行优化

默认行为等同于 `optimize` 子命令：

```bash
cargo run --release
```

使用指定配置文件运行：

```bash
cargo run --release -- -c config.toml
```

## 命令说明

### 优化模式

```bash
cargo run --release
cargo run --release -- -c config.toml
```

该模式会读取字根与拆分表数据，执行模拟退火搜索，并输出最优键位映射结果。

### 编码模式

根据现有 keymap 为汉字生成编码：

```bash
cargo run --release -- encode -k output-keymap.txt -d input-division.txt -o output-encode.txt
```

### 评估模式

评估已有编码方案质量：

```bash
cargo run --release -- evaluate -k output-keymap.txt
cargo run --release -- evaluate -k output-keymap.txt -d input-division.txt --keydist key_distribution.txt --equiv pair_equivalence.txt --simple simple.txt -o output-evaluate.txt
```

### 编译检查

```bash
cargo check
```

## 输入文件

程序依赖以下输入文件：

| 文件 | 说明 | 格式 |
|------|------|------|
| `input-fixed.txt` | 固定字根 / 受限字根 | `字根名 [tab] 键位` |
| `input-roots.txt` | 动态字根分组 | `字根名1 字根名2 ...` |
| `input-division.txt` | 汉字拆分表 | `汉字 [tab] 字根1 字根2 ... [tab] 频率` |
| `pair_equivalence.txt` | 键对当量表 | `键对 [tab] 当量值` |
| `key_distribution.txt` | 键位分布目标 | `键位 [tab] 目标% [tab] 低惩罚 [tab] 高惩罚` |

## 配置文件

默认配置依赖 `config.toml`，可从 `config.toml.example` 复制生成：

```bash
cp config.toml.example config.toml
```

示例配置节选：

```toml
[files]
fixed = "input-fixed.txt"
dynamic = "input-roots.txt"
splits = "input-division.txt"
pair_equiv = "pair_equivalence.txt"
key_dist = "key_distribution.txt"

[annealing]
threads = 12
total_steps = 2000000
temp_start = 100.0
temp_end = 0.000001
comfort_temp = 0.4
comfort_width = 0.15

[keys]
allowed = "qwertyuiopasdfghjklzxcvbnm"
display_order = "qwertyuiopasdfghjklzxcvbnm"
```

## 简码系统

该项目支持简码规则配置，规则形式类似：

```text
AaBaCa
```

说明：

- 大写字母表示根选择器，如 `A=第1根, B=第2根, ...`
- 小写字母表示码位选择器，如 `a=第1码, b=第2码, ...`
- `code_num` 表示某一等级允许分配多少个简码
- 多个候选规则可按优先级尝试解析

## 输出文件

优化完成后，会在 `output-{YYYYMMDDHHMMSS}/` 目录中生成结果：

- `output-keymap.txt`：字根到键位映射
- `output-encode.txt`：汉字编码结果
- `output-simple-codes.txt`：简码分配表
- `output-combined.txt`：简码 + 全码合并输出
- `output-distribution.txt`：键位使用分布统计
- `output-equiv-dist.txt`：当量分布统计
- `summary.txt`：线程结果汇总

## 项目结构

```text
.
├── Cargo.toml
├── README.md
├── config.toml.example
├── input-fixed.txt
├── input-roots.txt
├── input-division.txt
├── pair_equivalence.txt
├── key_distribution.txt
├── src/
│   ├── main.rs
│   ├── annealing.rs
│   ├── calibrate.rs
│   ├── config.rs
│   ├── context.rs
│   ├── evaluator.rs
│   ├── loader.rs
│   ├── output.rs
│   ├── schedule.rs
│   ├── simple.rs
│   ├── types.rs
│   └── validate.rs
└── target/
```

## 核心模块说明

- `main.rs`：CLI 入口，定义 `optimize` / `encode` / `evaluate` 子命令
- `annealing.rs`：模拟退火与局部搜索核心实现
- `context.rs`：构建优化上下文，包含字根组、汉字信息和当量表
- `evaluator.rs`：计算重码数、重码率、当量和分布偏差
- `loader.rs`：加载固定字根、动态字根、拆分数据与元数据
- `simple.rs`：简码规则解析与评估
- `output.rs`：生成 keymap、编码和汇总文件
- `validate.rs`：校验拆分表中根是否定义完整

## 性能优化要点

- 使用 `rayon` 并行执行多个模拟退火线程
- 通过增量评估减少重复全量计算
- 碰撞桶和加权统计用于低成本更新键位使用情况
- 支持温度调度、重热机制和智能扰动
- Release 模式使用 LTO、单代码单元和较高优化级别

## 许可证

本项目采用 MIT License。详细内容请见 `LICENSE` 文件（如仓库中存在该文件）。

## 贡献

欢迎提交 issue、PR 或改进建议。若你正在开发新的编码规则或想改进优化器策略，建议先阅读 `src/annealing.rs` 与 `src/evaluator.rs` 中的实现逻辑后再进行修改。
