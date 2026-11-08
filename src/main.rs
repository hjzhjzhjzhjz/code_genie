// =========================================================================
// 🚀 字根编码优化器 - 主入口
// =========================================================================

// 索引循环在本项目大量用于「一组等长数组按下标并行访问」（如 `ku[k]`/`kd[k]`、
// `max_len[li]`/`levels[li]`、`果[k]`/`r.手感[k]`）。改写为 enumerate/zip 只能消掉
// 其中一个下标，其余数组仍要裸下标，读起来反而更绕；故全局关闭这条纯风格 lint。
#![allow(clippy::needless_range_loop)]

// 全局分配器：退火主循环每步都有大量短命分配（邻域候选、增量维护缓冲），
// Windows/mingw 的 CRT 堆分配后只扩段、不把空闲页还给 OS，长跑时私有内存与
// 页文件峰值会单调抬高（120 万步实测 62MB → 852MB）。mimalloc 用线程本地堆、
// 按 arena 回收，配合 release 的 LTO 能同时改善吞吐与高水位。
// 关掉 feature（`--no-default-features`）即回退系统分配器，便于 A/B。
#[cfg(feature = "mimalloc")]
#[global_allocator]
static 全局分配器: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use clap::{Parser, Subcommand};
use rand::SeedableRng;
use rayon::prelude::*;

mod annealing;
mod assess;
mod bucket_store;
mod calibrate;
mod checkpoint;
mod clusters;
mod config;
mod context;
mod data_dir;
mod diagnose;
mod evaluator;
mod export_table;
mod fsutil;
mod handfeel;
mod loader;
mod output;
mod pipeline;
mod prepare;
mod rime_convert;
mod root_chart;
mod roots_chart;
mod roots_cluster;
mod run;
mod schedule;
mod sentence;
mod shortcode;
mod types;
mod validate;
mod zigen;

use crate::annealing::{simulated_annealing_resumable, SaResult};
use crate::calibrate::calibrate_scales;
use crate::checkpoint::ThreadCheckpoint;
use crate::config::{Config, TargetsConfig};
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::output::{save_results, save_summary, save_thread_results};
use crate::types::{key_to_char, KeyDistConfig, ScaleConfig, EQUIV_TABLE_SIZE};

// =========================================================================
// CLI 定义
// =========================================================================

#[derive(Parser)]
#[command(name = "CodeGenie", about = "字根编码优化器", version)]
struct Cli {
    /// 配置文件路径
    #[arg(short = 'c', long, default_value = "config.ron")]
    config: String,
    /// 固定退火随机种子（u64）：同配置 + 同种子 + 同线程数的 fresh 运行逐位可复现。
    /// 不给则沿用系统熵。注意 --seed-dir / --seed-keymap 是「播种」（初始解），与此不同。
    /// checkpoint 不存 RNG 状态，故中断续算与一次跑完仍不等价。
    #[arg(long = "rng-seed", value_name = "U64")]
    rng_seed: Option<u64>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行模拟退火优化（默认行为）
    Optimize {
        /// 输出目录：不存在则自动创建；存在则复用（允许非空），逐文件防覆盖（旧文件加时间戳归档）。
        /// 缺省时使用 output-{时间戳}
        #[arg(short = 'd', long = "output-dir")]
        output_dir: Option<String>,

        /// 从既有 output 目录播种：读取 {DIR}/thread-NN/字根键位表.txt 作为各线程初始解
        /// （按 thread-NN 编号轮询映射）。与 --seed-keymap 互斥。
        #[arg(long = "seed-dir")]
        seed_dir: Option<String>,

        /// 从单个 keymap 文件播种：所有退火线程使用同一初始解。与 --seed-dir 互斥。
        #[arg(long = "seed-keymap")]
        seed_keymap: Option<String>,

        /// 复用尺度文件（尺度.json）：跳过校准，保证跨运行口径一致
        #[arg(long = "scale")]
        scale: Option<String>,
    },

    /// 跑一轮完整优化：备份输入 → prepare → optimize/resume，写 COMMENT.txt 与日志
    ///
    /// 原 `moling/optimize.sh` 的 Rust 实现（该脚本已删除，功能完全由本子命令取代）。
    #[command(name = "run")]
    Run {
        /// 工作目录：prepare/optimize 的 cwd，config*.ron 与产物都在这里
        #[arg(long = "dir", default_value = "moling")]
        dir: String,

        /// 方案名：moling（魔靈）/ yuling（靈明）/ yustar（星陳）
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 上游素材目录；缺省按 `<dir>/yuhao-data` → `<dir>/../yuhao-data` 自动探测
        #[arg(long = "data-dir")]
        data_dir: Option<String>,

        /// 输出目录；缺省 `<schema>-output/output-<时间戳>`（相对工作目录）
        #[arg(short = 'd', long = "output-dir")]
        output_dir: Option<String>,

        /// 断点续算：走 resume 而不是 optimize（必须配合 -d 指定既有目录）
        #[arg(long)]
        resume: bool,

        /// 一句话备注，追加写入 `<OUT>/COMMENT.txt`；缺省时交互式提问
        #[arg(long)]
        comment: Option<String>,

        /// 跳过 prepare，直接用工作目录里已有的产物
        #[arg(long = "no-prepare")]
        no_prepare: bool,

        /// 只打印将执行的命令，不建目录、不写备注、不备份、不跑
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// macOS 上不在命令前套 caffeinate（默认会套，防长时间退火时休眠）
        #[arg(long = "no-caffeinate")]
        no_caffeinate: bool,

        /// 从既有 output 目录播种（转给 optimize）
        #[arg(long = "seed-dir")]
        seed_dir: Option<String>,

        /// 从单个 keymap 文件播种（转给 optimize）
        #[arg(long = "seed-keymap")]
        seed_keymap: Option<String>,

        /// 复用尺度文件（转给 optimize）
        #[arg(long)]
        scale: Option<String>,
    },

    /// 根据 keymap 为汉字编码
    Encode {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 编码输出文件
        #[arg(short = 'o', long, default_value = "单字编码表.txt")]
        output: String,
    },

    /// 全方位评估编码方案
    Evaluate {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 目标键位分布文件
        #[arg(long)]
        keydist: Option<String>,

        /// 当量数据文件
        #[arg(long)]
        equiv: Option<String>,

        /// 评估输出文件
        #[arg(short = 'o', long, default_value = "output-evaluate.txt")]
        output: String,
    },

    /// 从既有 output 目录的 checkpoint 断点续算
    Resume {
        /// 之前运行的 output 目录（含 inputs/ 与 checkpoint/）
        #[arg(short = 'd', long)]
        dir: String,
    },

    /// 导出测评用码表（字<TAB>码，每字一行全码），并统计全码长 / 动重
    #[command(name = "export")]
    ExportTable {
        /// 键位映射文件（字根键位表.txt / output-keymap.txt）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 拆分表（拆分表.txt 或 拆分全表.txt：字<TAB>根 根 …<TAB>频次）
        #[arg(short = 'c', long)]
        chaifen: String,

        /// 方案名：moling（魔靈）/ yuling（靈明）/ yustar（星陳），决定取码规则
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 覆盖方案自带的最大码长（默认取方案值，三套方案都是 4）
        #[arg(long = "max-code-len")]
        max_code_len: Option<usize>,

        /// 输出文件
        #[arg(short = 'o', long, default_value = "全码表.txt")]
        output: String,
    },
    /// 生成字根图
    #[command(name = "chart")]
    Chart {
        /// 键位映射文件（简易模式：直接从 keymap 出单页字根图）
        #[arg(short = 'k', long)]
        keymap: Option<String>,

        /// 字根映射表（可选，花括号名 → PUA 显示字）
        #[arg(short = 'm', long)]
        mapping: Option<String>,

        /// 优化输出目录（完整模式：产 roots/chaifen/mabiao/.js/.html 全套，取代 generate-root-chart.sh）
        #[arg(short = 'd', long)]
        dir: Option<String>,

        /// 完整模式的方案名：moling（魔靈）/ yuling（靈明）/ yustar（星陳）
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 页面标题（完整模式，缺省「<方案>字根表 <日期>」）
        #[arg(long)]
        title: Option<String>,

        /// Unihan 数据目录（含 Unihan_Readings.txt / Unihan_Variants.txt）
        #[arg(long = "unihan-dir")]
        unihan_dir: Option<String>,

        /// 字根字体文件（.woff/.ttf）；文件存在则 base64 内嵌，否则当字体名
        #[arg(long)]
        font: Option<String>,

        /// 每页字数（必须 ≥ 1；0 会让分页整除直接 panic）
        #[arg(long = "page-size", default_value_t = 500, value_parser = 正整数)]
        page_size: usize,

        /// 每个字根最多几个例字
        #[arg(long = "max-examples", default_value_t = 10)]
        max_examples: usize,

        /// 统计前多少个高频字
        #[arg(long = "top-chars", default_value_t = 8000)]
        top_chars: usize,

        /// 只出 .js，不出 .html
        #[arg(long = "no-page")]
        no_page: bool,

        /// 简易模式的输出文件
        #[arg(short = 'o', long, default_value = "字根图.html")]
        output: String,
    },
    /// 跑宇浩测评并输出对比表（内部调 yuhao-assess 的 CLI）
    #[command(name = "assess")]
    Assess {
        /// 单张码表
        #[arg(long)]
        table: Option<String>,

        /// 目录（递归找 全码表.txt）
        #[arg(long)]
        dir: Option<String>,

        /// 目录里要匹配的文件名
        #[arg(long = "file-name", default_value = "全码表.txt")]
        file_name: String,

        /// 方案配置（jsonc）
        #[arg(long, default_value = "codegenie-s4.jsonc")]
        scheme: String,

        /// yuhao-assess 仓库目录
        #[arg(long = "yuhao-dir", default_value = "../yuhao-assess")]
        yuhao_dir: String,

        /// 保留每份测评 JSON 到该目录
        #[arg(long)]
        keep: Option<String>,

        /// 汇总表写到该文件（Markdown）
        #[arg(long)]
        summary: Option<String>,

        /// 把每份结果另存成可导入宇浩测评网页版的「方案档案」，写到该目录
        #[arg(long = "profile")]
        profile: Option<String>,

        /// 方案档案里的作者
        #[arg(long, default_value = "CodeGenie")]
        author: String,

        /// 方案档案里的版本号（留空用当天日期）
        #[arg(long, default_value = "")]
        version: String,

        /// 额外打印「合格方案」：按 compare-optimization-results.sh 的阈值
        /// （通用重码率 ≤0.00031、常用国字 ≤0.00028、动态选重 <0.0020、速度当量 <1.270）筛选并按重码率排序
        #[arg(long)]
        qualify: bool,
    },

    /// 以星月方案包为底包，只把词典里的码换掉，其余（文件名/schema/lua/词表）原样沿用
    #[command(name = "rime-convert")]
    RimeConvert {
        /// 底包目录（starmoon-ime 根目录，含 <前缀>_chaifen.dict.yaml 与 yuhao/）
        #[arg(long)]
        base: String,

        /// 算码输出目录（含字根键位表与 zigen-<前缀>.csv）
        #[arg(long)]
        output: String,

        /// 字根键位表；缺省按 <output>/output-keymap.txt → roots.tsv 顺序自动找
        #[arg(long)]
        keymap: Option<String>,

        /// 拆分表（拆分全表.txt：字<TAB>根 根 …<TAB>频次）
        #[arg(long, default_value = crate::prepare::names::拆分全表)]
        chaifen: String,

        /// 输出目录
        #[arg(long, default_value = "rime-out")]
        out: String,

        /// 底包的方案前缀（同时用于 zigen-<前缀>.csv）
        #[arg(long, default_value = "starmoon")]
        prefix: String,
    },

    /// 整句准确率：逐单元首候选 + 连续码流解码，两个口径一起给
    #[command(name = "sentence")]
    Sentence {
        /// 全码词典（Rime dict.yaml 或 TSV：字 / 码 / 权重）
        #[arg(long)]
        full_dict: String,
        /// 词组词典（可选；格式同上）
        #[arg(long)]
        words_dict: Option<String>,
        /// 语料文件，一行一句；也接受整段，按非汉字切开
        #[arg(long)]
        corpus: String,
        /// 改用外部解码器（librime），给了就不走内置 Viterbi
        #[arg(long)]
        decoder: Option<String>,
        /// librime 共享数据目录（仅外部解码器用）
        #[arg(long, default_value = ".")]
        shared: String,
        /// librime 用户数据目录（仅外部解码器用）
        #[arg(long, default_value = "rime-user")]
        user: String,
        /// librime 方案 id（仅外部解码器用）
        #[arg(long, default_value = "xingyun_test")]
        schema: String,
        /// 最多测多少句，0 = 全部
        #[arg(long, default_value = "0")]
        limit: usize,
        /// 句长上限
        #[arg(long, default_value = "20")]
        max_len: usize,
        /// 导出中间文件的目录
        #[arg(long)]
        dump: Option<String>,
        /// 结果 Markdown
        #[arg(long)]
        out: Option<String>,
        /// 让外部解码器输出诊断
        #[arg(long)]
        verbose: bool,
    },

    /// 摊开一个字的取码链路：拆分 → 码位 → 键位 → 编码
    #[command(name = "pipeline")]
    Pipeline {
        /// 字根键位表
        #[arg(long)]
        keymap: String,

        /// 纯拆分表
        #[arg(long)]
        chaifen: String,

        /// 同键组表 字根约束.txt（可选），用来标出归并关系
        #[arg(long)]
        fixed: Option<String>,

        /// 要查看的字，空格分隔
        #[arg(long, default_value = "")]
        chars: String,

        /// 不给 --chars 时，按频次取前 N 个字
        #[arg(long, default_value = "0")]
        top: usize,

        /// 写到该文件（Markdown）
        #[arg(long)]
        output: Option<String>,
    },

    /// 按归并规则改键位，重新编码后对比结构指标
    #[command(name = "merge")]
    Merge {
        /// 字根键位表
        #[arg(long)]
        keymap: String,

        /// 纯拆分表
        #[arg(long)]
        chaifen: String,

        /// 归并规则，可重复。写法：`还=人` 或 `你 1=人 2`，多条用分号分隔
        #[arg(long = "rules")]
        rules: Vec<String>,

        /// 变体码表输出目录
        #[arg(long, default_value = "归并对比")]
        out: String,

        /// 对比表写到该文件（Markdown）
        #[arg(long)]
        summary: Option<String>,
    },

    /// 按形码测评口径做诊断：手感七项、字频分段、词组测评、二码离散图
    #[command(name = "diagnose")]
    Diagnose {
        /// 码表目录（递归找 全码表.txt）
        #[arg(long)]
        dir: String,

        /// 测评数据目录（当量表／字频表／词表）
        #[arg(long, default_value = "ceping-data")]
        data: String,

        /// 目录里要匹配的文件名
        #[arg(long = "file-name", default_value = "全码表.txt")]
        file_name: String,

        /// 报告写到该文件（Markdown）
        #[arg(long)]
        summary: Option<String>,
    },

    /// 从上游 yaml/lua/csv 数据加工出优化器要的输入文件
    /// （`moling/prepare-inputs.sh` 的原生子命令实现，共 13 个步骤）
    #[command(name = "prepare")]
    Prepare {
        /// 工作目录，相当于脚本里的 cwd
        #[arg(long, default_value = "moling")]
        dir: String,

        /// 上游素材目录；缺省按 `--dir/yuhao-data` → `--dir/../yuhao-data` 自动探测，
        /// 都找不到就认为素材平铺在 `--dir` 下
        #[arg(long = "data-dir")]
        data_dir: Option<String>,

        /// 方案名：moling（魔靈）/ yuling（靈明）/ yustar（星陳）
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 起始步骤（0~13），用于分段调试
        #[arg(long = "from-step", default_value_t = 0)]
        from_step: usize,

        /// 结束步骤（0~13）
        #[arg(long = "to-step", default_value_t = 13)]
        to_step: usize,

        /// 基线目录：给了就把产物与之逐字节对照，有差异即报错
        #[arg(long = "baseline-dir")]
        baseline_dir: Option<String>,
    },

    /// 聚类重码分析：某一行聚类压成一个大码，要付出多少重码
    #[command(name = "clusters")]
    Clusters {
        /// 方案名：moling（魔靈）/ yuling（靈明）/ yustar（星陳），决定编码规则
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 纯根名拆分表（字<TAB>根 根 …<TAB>频次）
        #[arg(long, default_value = crate::prepare::names::拆分表)]
        chaifen: String,

        /// 字根声韵表（根<TAB>声韵<TAB>拼音）
        #[arg(long, default_value = crate::prepare::names::字根声韵)]
        roots: String,

        /// 聚类表（RON）；留空用内嵌表（assets/roots_cluster.ron），
        /// 也可直接写 "根1 根2 ; 根3 根4"
        #[arg(long, default_value = "")]
        cluster: String,

        /// 明细只列超过这么多重的组（全码那一档不受限）
        #[arg(long = "max-dups", default_value_t = 9)]
        max_dups: usize,

        /// 逐行代入聚类表，与「全不聚类」基线做差
        #[arg(long)]
        per_row: bool,
    },

    /// 字形变体表：生成 zigen-<方案>.csv / roots-mapping.tsv / zigen-trainer-<方案>.json
    #[command(name = "zigen")]
    Zigen {
        /// 方案名：只用于拼产物文件名
        #[arg(long, default_value = "moling")]
        schema: String,

        /// 字根键位表（output-keymap.txt 或 字根键位表.txt）
        #[arg(long, default_value = "output-keymap.txt")]
        keymap: String,

        /// 源目录：取 字根声韵.txt / roots-cluster.ron；yuhao-zigens.csv 是上游素材，
        /// 源目录里没有就到 yuhao-data/ 里找
        #[arg(long, default_value = ".")]
        source: String,

        /// 输出目录
        #[arg(long, default_value = ".")]
        out: String,

        /// 字根数与 yuhao-zigens.csv 不一致时按失败退出（原脚本行为）
        #[arg(long)]
        strict: bool,
    },

    /// 给已经算完的输出目录补生成三级前缀简码（不重跑优化）
    #[command(name = "shortcode")]
    ShortCode {
        /// 输出目录（含 完整码表.txt / 合并码表.txt）
        #[arg(short = 'd', long)]
        dir: String,

        /// 字频表（字<TAB>…<TAB>字频）；缺省自动探测 inputs/ 下的拆分表、
        /// 全字频表.txt，最后回退到内嵌全量拆分表
        #[arg(long)]
        freq: Option<String>,

        /// 按「实际按键数」口径分配：未满 4 码的短码补一个 `_` 计键（三级简码 + 空格
        /// = 4 键不省键故不分配，一级简码写成单字母）。等价于主配置
        /// `[shortcode] padding_space = true`。本项目按「每级简码都带空格上屏」口径，
        /// 优化测评「簡碼效率」统一用本档；测评源码该指标用纯编码长度，数值不可直接比。
        /// 【缺省即本档】传 `--padding-space=false` 可切回旧的码串长度档。
        #[arg(
            long = "padding-space",
            default_value_t = true,
            num_args = 0..=1,
            default_missing_value = "true",
            action = clap::ArgAction::Set
        )]
        padding_space: bool,
    },
}

// =========================================================================
// 主入口
// =========================================================================

/// 必需输入：加载失败直接报错退出（替代 loader 内部的 panic）。
fn must<T>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
    }
}

/// 可选输入（#5 当量/分布）：加载失败告警并用默认值，不阻断。
fn or_warn_default<T: Default>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            eprintln!("❌ {}（按「不阻断」策略继续）", e);
            T::default()
        }
    }
}

fn main() {
    let cli = Cli::parse();

    // prepare 不依赖主配置（它跑在输入文件还不存在的时候），因此在加载 config 之前先处理
    if let Some(Commands::Prepare {
        dir,
        data_dir,
        schema,
        from_step,
        to_step,
        baseline_dir,
    }) = &cli.command
    {
        let 参 = prepare::参数 {
            目录: dir.clone(),
            素材目录: data_dir.clone(),
            方案: schema.clone(),
            从步骤: *from_step,
            到步骤: *to_step,
            基线目录: baseline_dir.clone(),
        };
        if let Err(e) = prepare::run(&参) {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
        return;
    }

    // run 是 prepare + optimize 的外层事务，两个子命令都由它再调一遍自己，
    // 所以父进程不需要加载配置（配置在工作目录里，由子进程按 -c 解释）
    if let Some(Commands::Run {
        dir,
        schema,
        data_dir,
        output_dir,
        resume,
        comment,
        no_prepare,
        dry_run,
        no_caffeinate,
        seed_dir,
        seed_keymap,
        scale,
    }) = &cli.command
    {
        let 参 = run::参数 {
            工作目录: dir.clone(),
            方案: schema.clone(),
            素材目录: data_dir.clone(),
            输出目录: output_dir.clone(),
            续算: *resume,
            备注: comment.clone(),
            跳过准备: *no_prepare,
            试运行: *dry_run,
            不防休眠: *no_caffeinate,
            播种目录: seed_dir.clone(),
            播种键位: seed_keymap.clone(),
            尺度: scale.clone(),
            随机种子: cli.rng_seed,
        };
        if let Err(e) = run::run(&参) {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
        return;
    }

    // chart 的完整模式只读产物文件与上游素材，不依赖主配置
    if let Some(Commands::Chart {
        dir: Some(dir),
        schema,
        title,
        unihan_dir,
        font,
        page_size,
        max_examples,
        top_chars,
        no_page,
        ..
    }) = &cli.command
    {
        let 参 = roots_chart::参数 {
            目录: dir.clone(),
            方案: schema.clone(),
            标题: title.clone(),
            unihan目录: unihan_dir.clone(),
            字体: font.clone(),
            每页字数: *page_size,
            例字数: *max_examples,
            高频字数: *top_chars,
            跳过页面: *no_page,
        };
        if let Err(e) = roots_chart::run(&参) {
            eprintln!("❌ {e}");
            std::process::exit(1);
        }
        return;
    }

    // shortcode 只读产物文件与字频表，不依赖主配置（可对历史输出目录补生成）
    if let Some(Commands::ShortCode {
        dir,
        freq,
        padding_space,
    }) = &cli.command
    {
        if let Err(e) = run_shortcode(dir, freq.as_deref(), *padding_space) {
            eprintln!("❌ {e}");
            std::process::exit(1);
        }
        return;
    }

    // clusters 只用 prepare 的数据文件，同样不依赖主配置
    if let Some(Commands::Clusters {
        schema,
        chaifen,
        roots,
        cluster,
        max_dups,
        per_row,
    }) = &cli.command
    {
        let 参 = clusters::参数 {
            方案: schema.clone(),
            拆分表: chaifen.clone(),
            字根声韵: roots.clone(),
            聚类: cluster.clone(),
            最多重数: *max_dups,
            逐行: *per_row,
        };
        if let Err(e) = clusters::run(&参) {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
        return;
    }

    // zigen 同理：只读 prepare 的产物与上游 csv，不依赖主配置
    if let Some(Commands::Zigen {
        schema,
        keymap,
        source,
        out,
        strict,
    }) = &cli.command
    {
        let 参 = zigen::参数 {
            方案: schema.clone(),
            键位文件: keymap.clone(),
            源目录: source.clone(),
            输出目录: out.clone(),
            严格: *strict,
        };
        if let Err(e) = zigen::run(&参) {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
        return;
    }

    // 配置**按需**加载（#1：解析失败直接报错退出，不再静默回退默认配置）。
    // 只有 encode / evaluate / optimize 需要主配置的缺省值；resume 读 `<dir>/inputs/` 里
    // 的备份配置，export-table / chart / assess 等只用命令行参数，若一律在分派前加载，
    // 在没放 config.ron 的目录里就会被一句「无法读取配置文件」直接拦掉。
    let 载入配置 = || -> Config {
        match Config::load_from_path(&cli.config) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
    };

    match cli.command {
        Some(Commands::Encode {
            division,
            keymap,
            output,
        }) => {
            let cfg = 载入配置();
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            run_encode(division_path, &keymap, &output);
        }
        Some(Commands::Evaluate {
            division,
            keymap,
            keydist,
            equiv,
            output,
        }) => {
            let cfg = 载入配置();
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            let keydist_path = keydist.as_deref().unwrap_or(&cfg.files.key_dist);
            let equiv_path = equiv.as_deref().unwrap_or(&cfg.files.pair_equiv);
            run_evaluate(
                &cfg,
                division_path,
                &keymap,
                keydist_path,
                equiv_path,
                &output,
            );
        }
        Some(Commands::Resume { dir }) => run_resume(&dir, cli.rng_seed),
        Some(Commands::ExportTable {
            keymap,
            chaifen,
            schema,
            max_code_len,
            output,
        }) => {
            let 参 = export_table::参数 {
                键位: &keymap,
                拆分: &chaifen,
                输出: &output,
                方案: &schema,
                最大码长: max_code_len,
            };
            if let Err(e) = export_table::run(&参) {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }
        Some(Commands::Chart {
            keymap,
            mapping,
            dir,
            schema,
            title,
            unihan_dir,
            font,
            page_size,
            max_examples,
            top_chars,
            no_page,
            output,
        }) => {
            if let Some(d) = dir {
                let 参 = roots_chart::参数 {
                    目录: d.clone(),
                    方案: schema.clone(),
                    标题: title.clone(),
                    unihan目录: unihan_dir.clone(),
                    字体: font.clone(),
                    每页字数: page_size,
                    例字数: max_examples,
                    高频字数: top_chars,
                    跳过页面: no_page,
                };
                if let Err(e) = roots_chart::run(&参) {
                    eprintln!("❌ {e}");
                    std::process::exit(1);
                }
            } else if let Some(k) = keymap {
                root_chart::run(&k, mapping.as_deref(), &output);
            } else {
                eprintln!("❌ chart 需要 `-k <键位表>`（简易图）或 `-d <输出目录>`（完整套件）");
                std::process::exit(2);
            }
        }
        Some(Commands::RimeConvert {
            base,
            output,
            keymap,
            chaifen,
            out,
            prefix,
        }) => {
            let 参 = rime_convert::参数 {
                底包目录: base,
                算码目录: output,
                键位文件: keymap,
                拆分文件: chaifen,
                输出目录: out,
                前缀: prefix,
            };
            if let Err(e) = rime_convert::run(&参) {
                eprintln!("❌ {}", e);
                std::process::exit(1);
            }
        }

        Some(Commands::Sentence {
            full_dict,
            words_dict,
            corpus,
            decoder,
            shared,
            user,
            schema,
            limit,
            max_len,
            dump,
            out,
            verbose,
        }) => sentence::run(&sentence::参数 {
            全码词典: full_dict.into(),
            词组词典: words_dict.map(Into::into),
            语料: corpus.into(),
            解码器: decoder.map(Into::into),
            共享目录: shared.into(),
            用户目录: user.into(),
            方案: schema,
            上限: limit,
            最长: max_len,
            导出: dump.map(Into::into),
            结果: out.map(Into::into),
            详细: verbose,
        }),

        Some(Commands::Pipeline {
            keymap,
            chaifen,
            fixed,
            chars,
            top,
            output,
        }) => pipeline::流水线(
            &keymap,
            &chaifen,
            fixed.as_deref(),
            &chars,
            top,
            output.as_deref(),
        ),

        Some(Commands::Merge {
            keymap,
            chaifen,
            rules,
            out,
            summary,
        }) => pipeline::归并(&keymap, &chaifen, &rules, &out, summary.as_deref()),

        Some(Commands::Diagnose {
            dir,
            data,
            file_name,
            summary,
        }) => diagnose::run(&dir, &data, &file_name, summary.as_deref()),

        Some(Commands::Assess {
            table,
            dir,
            file_name,
            scheme,
            yuhao_dir,
            keep,
            summary,
            profile,
            author,
            version,
            qualify,
        }) => assess::run(
            table.as_deref(),
            dir.as_deref(),
            &file_name,
            &scheme,
            &yuhao_dir,
            keep.as_deref(),
            summary.as_deref(),
            profile.as_deref(),
            &author,
            &version,
            qualify,
        ),
        Some(Commands::Optimize {
            output_dir,
            seed_dir,
            seed_keymap,
            scale,
        }) => {
            let cfg = 载入配置();
            run_optimize(
                &cfg,
                &cli.config,
                output_dir,
                seed_dir,
                seed_keymap,
                scale,
                &停止标志(),
                cli.rng_seed,
            )
        }
        // prepare 在加载配置之前就已返回，这里只是让 match 完备
        Some(Commands::Prepare { .. }) => unreachable!("prepare 已在加载配置前处理"),
        Some(Commands::Clusters { .. }) => unreachable!("clusters 已在加载配置前处理"),
        Some(Commands::Zigen { .. }) => unreachable!("zigen 已在加载配置前处理"),
        Some(Commands::Run { .. }) => unreachable!("run 已在加载配置前处理"),
        Some(Commands::ShortCode { .. }) => unreachable!("shortcode 已在加载配置前处理"),
        None => {
            let cfg = 载入配置();
            run_optimize(
                &cfg,
                &cli.config,
                None,
                None,
                None,
                None,
                &停止标志(),
                cli.rng_seed,
            )
        }
    }
}

// =========================================================================
// encode 子命令
// =========================================================================

/// 给已经算完的输出目录补生成三级前缀简码：不加载主配置、不重跑优化。
fn run_shortcode(dir: &str, freq: Option<&str>, 补空格: bool) -> Result<(), String> {
    let 全码路径 = format!("{dir}/完整码表.txt");
    let 全码文本 =
        std::fs::read_to_string(&全码路径).map_err(|e| format!("读 {全码路径} 失败: {e}"))?;
    let (字频, 来源) = 探测字频(dir, freq)?;
    let 条目 = shortcode::合成全码条目(&全码文本, &字频);
    if 条目.is_empty() {
        return Err(format!("{全码路径} 里没有可用的全码行"));
    }
    let 口径 = shortcode::码长口径::由开关(补空格);
    let 结果 = shortcode::生成_按口径(&条目, 口径);
    shortcode::打印统计(&结果.统计);
    shortcode::保存(dir, &结果, &来源);
    println!(
        "已写入 {dir}/简码表.txt [{}]，简码段已并入 合并码表.txt（重复运行会替换旧段）",
        口径.名称()
    );
    Ok(())
}

/// 字频来源优先级：显式 `--freq` > `inputs/` 下的拆分表（与退火同源）>
/// `全字频表.txt` > 内嵌 `assets/拆分全表.txt`（口径不同，只作兜底）。
fn 探测字频(dir: &str, 指定: Option<&str>) -> Result<(HashMap<char, u64>, String), String> {
    if let Some(p) = 指定 {
        let 文本 = std::fs::read_to_string(p).map_err(|e| format!("读字频表 {p} 失败: {e}"))?;
        let 表 = shortcode::读字频(&文本);
        if 表.is_empty() {
            return Err(format!("{p} 里没解析到字频"));
        }
        return Ok((表, p.to_string()));
    }

    let inputs = format!("{dir}/inputs");
    let mut 候选: Vec<String> = std::fs::read_dir(&inputs)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".txt") && n.contains("拆分"))
                .collect()
        })
        .unwrap_or_default();
    // 优化用的拆分表优先，名字里带「全表」的排后面
    候选.sort_by_key(|n| (n.contains("全表"), n.clone()));
    for 名 in &候选 {
        let p = format!("{inputs}/{名}");
        if let Ok(文本) = std::fs::read_to_string(&p) {
            let 表 = shortcode::读字频(&文本);
            if !表.is_empty() {
                return Ok((表, p));
            }
        }
    }

    let p = format!("{dir}/全字频表.txt");
    if let Ok(文本) = std::fs::read_to_string(&p) {
        let 表 = shortcode::读字频(&文本);
        if !表.is_empty() {
            return Ok((表, p));
        }
    }

    let 全量 = crate::export_table::加载拆分表内容(内嵌拆分全表);
    let 表: HashMap<char, u64> = 全量
        .iter()
        .filter_map(|(字, _, 频)| 字.chars().next().map(|c| (c, *频)))
        .collect();
    Ok((
        表,
        "内嵌 assets/拆分全表.txt（兜底：字频口径与优化不同）".to_string(),
    ))
}

fn run_encode(division_path: &str, keymap_path: &str, output_path: &str) {
    println!("=== CodeGenie 编码模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = must(loader::load_keymap(keymap_path, division_path));
    let splits = must(loader::load_splits(division_path));

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 为每个汉字编码
    let mut code_out = String::new();
    let mut missing_roots: HashMap<String, usize> = HashMap::new();
    let mut encoded_count = 0usize;
    let mut failed_count = 0usize;

    for (ch, roots, freq) in &splits {
        let mut code_parts = Vec::new();
        let mut all_found = true;

        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            } else {
                all_found = false;
                *missing_roots.entry(root.clone()).or_default() += 1;
            }
        }

        if all_found && !code_parts.is_empty() {
            let code_str: String = code_parts.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            encoded_count += 1;
        } else {
            // 即使有缺失字根，也输出已有部分（用 ? 标记缺失）
            let mut partial_code = Vec::new();
            for root in roots {
                if let Some(&key) = root_to_key.get(root) {
                    partial_code.push(key_to_char(key));
                } else {
                    partial_code.push('?');
                }
            }
            let code_str: String = partial_code.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            failed_count += 1;
        }
    }

    // 写入文件
    std::fs::write(output_path, &code_out).expect("无法写入编码输出文件");

    println!("\n✅ 编码完成:");
    println!("  成功编码: {} 字", encoded_count);
    if failed_count > 0 {
        println!("  ⚠️ 部分编码: {} 字（存在未映射字根）", failed_count);
    }
    if !missing_roots.is_empty() {
        println!("  ⚠️ 未找到映射的字根 ({} 种):", missing_roots.len());
        let mut sorted: Vec<_> = missing_roots.into_iter().collect();
        sorted.sort_by_key(|x| std::cmp::Reverse(x.1));
        for (root, count) in sorted.iter().take(20) {
            println!("    {} (出现 {} 次)", root, count);
        }
        if sorted.len() > 20 {
            println!("    ... 还有 {} 种", sorted.len() - 20);
        }
    }
    println!("  结果已保存至 {}", output_path);
}

// =========================================================================
// evaluate 子命令
// =========================================================================

fn run_evaluate(
    cfg: &Config,
    division_path: &str,
    keymap_path: &str,
    keydist_path: &str,
    equiv_path: &str,
    output_path: &str,
) {
    println!("=== CodeGenie 评估模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  键位分布: {}", keydist_path);
    println!("  当量数据: {}", equiv_path);
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = must(loader::load_keymap(keymap_path, division_path));
    let splits = must(loader::load_splits(division_path));
    let equiv_table = or_warn_default(loader::load_pair_equivalence(equiv_path));
    let key_dist_config = or_warn_default(loader::load_key_distribution(keydist_path));

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 将所有字根作为 fixed_roots，groups 为空
    let fixed_roots: HashMap<String, u8> = root_to_key;
    let groups: Vec<types::RootGroup> = vec![];
    let assignment: Vec<u8> = vec![];

    // 构建 OptContext
    let scale_config = types::ScaleConfig::default();
    let weights = cfg.get_weight_config();
    let mut ctx = OptContext::new_with_termination(
        &splits,
        &fixed_roots,
        &groups,
        equiv_table,
        key_dist_config,
        scale_config,
        weights,
        TargetsConfig::default(),
        &cfg.keys.termination_keys,
        cfg.constraints.clone().unwrap_or_default(),
    );
    ctx.display_order = cfg.keys.display_order.clone();

    println!("  编码基数: {}", ctx.code_base);
    println!("  编码空间: {}", ctx.code_space);
    println!("  最大码长: {}", ctx.max_parts);

    // 运行评估
    let evaluator = Evaluator::new(&ctx, &assignment);
    let metrics = evaluator.get_metrics(&ctx);

    // 打印评估结果
    println!("\n📊 评估结果:");
    println!("  ═══════════════════════════════════════");
    println!("  「全码」重码数:          {}", metrics.collision_count);
    println!(
        "  「全码」重码率:          {:.6}%",
        metrics.collision_rate * 100.0
    );
    println!("  「全码」加权键均当量:    {:.4}", metrics.equiv_mean);
    println!("  「全码」当量变异系数(CV): {:.4}", metrics.equiv_cv);
    println!("  「全码」用指分布偏差(L2): {:.4}", metrics.dist_deviation);

    println!("  ═══════════════════════════════════════");

    // 生成详细评估报告
    let report = build_evaluate_report(
        cfg,
        &ctx,
        &assignment,
        &evaluator,
        &metrics,
        &key_dist_config,
    );

    std::fs::write(output_path, &report).expect("无法写入评估输出文件");
    println!("\n✅ 详细评估报告已保存至 {}", output_path);
}

/// 构建详细评估报告
fn build_evaluate_report(
    cfg: &Config,
    ctx: &OptContext,
    assignment: &[u8],
    evaluator: &Evaluator,
    metrics: &types::Metrics,
    key_dist_config: &[KeyDistConfig; EQUIV_TABLE_SIZE],
) -> String {
    let mut out = String::new();

    // ===== 总览 =====
    out.push_str("# CodeGenie 编码方案评估报告\n");
    out.push_str("#\n");
    out.push_str(&format!("# 汉字数量: {}\n", ctx.char_infos.len()));
    out.push_str(&format!("# 总字频: {}\n", ctx.total_frequency));
    out.push_str(&format!("# 编码基数: {}\n", ctx.code_base));
    out.push_str(&format!("# 最大码长: {}\n", ctx.max_parts));
    out.push_str("#\n");

    // ===== 全码指标 =====
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 全码指标\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    out.push_str("#\n");

    // ===== 用指分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 用指分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    let inv_tkp = evaluator.inv_total_key_presses;
    for kc in cfg.keys.display_order.chars() {
        if let Some(ki) = types::char_to_key_index(kc) {
            if ki >= 31 {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * inv_tkp;
            let target = key_dist_config[ki].target_rate;
            let diff = actual - target;
            out.push_str(&format!(
                "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                kc,
                actual,
                target,
                diff,
                diff * diff
            ));
        }
    }

    // 特殊键位
    let order_set: std::collections::HashSet<char> = cfg.keys.display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * inv_tkp;
                let target = key_dist_config[*ki].target_rate;
                let diff = actual - target;
                out.push_str(&format!(
                    "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                    kc,
                    actual,
                    target,
                    diff,
                    diff * diff
                ));
            }
        }
    }

    // ===== 当量分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 当量分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", metrics.equiv_mean));
    out.push_str(&format!("# 变异系数(CV): {:.4}\n", metrics.equiv_cv));
    out.push_str(&format!(
        "# 标准差: {:.4}\n",
        metrics.equiv_cv * metrics.equiv_mean
    ));

    // 计算每个汉字的当量
    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        if let Some(equiv) = ctx.calc_equiv_from_parts(i, assignment) {
            char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
        }
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    out.push_str("#\n# 当量最高的20个高频字 (字频>1000000):\n# 汉字\t当量\t字频\n");
    let mut count = 0;
    for (ch, eq, freq) in &char_equivs {
        if *freq > 1_000_000 && count < 20 {
            out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
            count += 1;
        }
    }

    out.push_str("#\n# 当量最低的20个高频字 (字频>1000000):\n");
    let high: Vec<_> = char_equivs
        .iter()
        .filter(|(_, _, f)| *f > 1_000_000)
        .collect();
    let start = high.len().saturating_sub(20);
    for (ch, eq, freq) in high.iter().skip(start) {
        out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
    }

    // ===== 重码详情 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 重码详情\n");
    out.push_str("# ═══════════════════════════════════════\n");

    // 构建编码到汉字的映射
    let n = ctx.char_infos.len();
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }

    // 收集重码组
    let mut collision_groups: Vec<(usize, Vec<(char, u64)>)> = Vec::new();
    for (code, chars) in &code_to_chars {
        if chars.len() >= 2 {
            let mut group: Vec<(char, u64)> = chars
                .iter()
                .map(|&ci| (ctx.raw_splits[ci].0, ctx.char_infos[ci].frequency))
                .collect();
            group.sort_by_key(|x| std::cmp::Reverse(x.1));
            collision_groups.push((*code, group));
        }
    }

    // 按组内最高频率降序排序
    collision_groups.sort_by(|a, b| {
        let max_a = a.1.first().map(|x| x.1).unwrap_or(0);
        let max_b = b.1.first().map(|x| x.1).unwrap_or(0);
        max_b.cmp(&max_a)
    });

    out.push_str(&format!(
        "# 共 {} 组重码 (按最高频率降序)\n",
        collision_groups.len()
    ));
    out.push_str("# 编码\t重码字\t字频列表\n");

    for (_, group) in collision_groups.iter().take(200) {
        let chars_str: String = group.iter().map(|(ch, _)| *ch).collect();
        let freqs_str: String = group
            .iter()
            .map(|(_, f)| f.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // 获取编码字符串
        let first_ci = code_to_chars
            .values()
            .find(|v| v.len() >= 2 && ctx.raw_splits[v[0]].0 == group[0].0)
            .and_then(|v| v.first())
            .copied();

        let code_str = if let Some(ci) = first_ci {
            let info = &ctx.char_infos[ci];
            let keys: String = info
                .parts
                .iter()
                .map(|&p| key_to_char(ctx.resolve_key(p, assignment)))
                .collect();
            keys
        } else {
            "?".to_string()
        };

        out.push_str(&format!("{}\t{}\t{}\n", code_str, chars_str, freqs_str));
    }

    if collision_groups.len() > 200 {
        out.push_str(&format!(
            "# ... 还有 {} 组重码未列出\n",
            collision_groups.len() - 200
        ));
    }

    out
}

// =========================================================================
// optimize 子命令（原有优化流程）
// =========================================================================

/// 根据配置决定最终使用的 ScaleConfig 及其来源说明
/// - cfg.scale 为 None：完全依赖 calibrate，返回 calibrated 值
/// - cfg.scale 全码五项均为 Some：跳过 calibrate，直接使用手动配置
/// - cfg.scale 部分字段为 Some：先用 calibrated 值，再用 Some 字段覆盖
///
/// 注意：[scale] 中配置的值与 targets 中的指标同量纲（即"典型值"），
/// 读入时取倒数转换为 ScaleConfig 内部使用的缩放因子（scale = 1/典型值）
fn resolve_scale_config(cfg: &Config, calibrated: ScaleConfig) -> (ScaleConfig, &'static str) {
    match &cfg.scale {
        None => (calibrated, "自动校准"),
        Some(toml) => {
            let all_set = toml.collision_count.is_some()
                && toml.collision_rate.is_some()
                && toml.equivalence.is_some()
                && toml.equiv_cv.is_some()
                && toml.distribution.is_some();

            // 辅助闭包：将"典型值"转换为缩放因子（取倒数），0 值保护
            let to_scale = |v: f64| if v > 0.0 { 1.0 / v } else { 1.0 };

            if all_set {
                // all_set 已保证五项均 Some；用 if let 解构，避免 unwrap panic。
                if let (Some(cc), Some(cr), Some(eq), Some(cv), Some(di)) = (
                    toml.collision_count,
                    toml.collision_rate,
                    toml.equivalence,
                    toml.equiv_cv,
                    toml.distribution,
                ) {
                    let sc = ScaleConfig {
                        collision_count: to_scale(cc),
                        collision_rate: to_scale(cr),
                        equivalence: to_scale(eq),
                        equiv_cv: to_scale(cv),
                        distribution: to_scale(di),
                    };
                    (sc, "手动配置")
                } else {
                    // 理论不可达（all_set 已校验五项均 Some）；兜底回退校准值，不 panic。
                    (calibrated, "自动校准")
                }
            } else {
                // 部分覆盖：以 calibrated 为基础，用手动值（取倒数后）覆盖
                let mut sc = calibrated;
                if let Some(v) = toml.collision_count {
                    sc.collision_count = to_scale(v);
                }
                if let Some(v) = toml.collision_rate {
                    sc.collision_rate = to_scale(v);
                }
                if let Some(v) = toml.equivalence {
                    sc.equivalence = to_scale(v);
                }
                if let Some(v) = toml.equiv_cv {
                    sc.equiv_cv = to_scale(v);
                }
                if let Some(v) = toml.distribution {
                    sc.distribution = to_scale(v);
                }
                (sc, "部分手动覆盖")
            }
        }
    }
}

/// 线程→种子的轮询映射（需求 7）：线程 `thread_id` 使用第 `thread_id % k` 个种子。
/// `k` 为种子个数（调用方保证 `k >= 1`）。
fn seed_index(thread_id: usize, k: usize) -> usize {
    thread_id % k
}

/// 解析两个互斥的播种来源选项为「种子 keymap 文件路径」序列（需求 1/2/3）。
///
/// - 两者均为 `None`：返回空 `Vec`（不播种，保持基线行为）。
/// - 同时指定：返回 `Err`（互斥）。
/// - `--seed-keymap <FILE>`：校验文件存在 → 返回 `vec![FILE]`（K = 1）。
/// - `--seed-dir <DIR>`：枚举 `{DIR}/thread-{NN}/字根键位表.txt`（NN 为两位数字），
///   按 NN 升序收集；为空则 `Err`。仅触碰 thread-NN 子目录，不读 `inputs/`、`checkpoint/`。
fn resolve_seed_paths(
    seed_dir: Option<&str>,
    seed_keymap: Option<&str>,
) -> Result<Vec<String>, String> {
    match (seed_dir, seed_keymap) {
        (Some(_), Some(_)) => Err("--seed-dir 与 --seed-keymap 互斥，不能同时指定".to_string()),
        (None, None) => Ok(Vec::new()),
        (None, Some(file)) => {
            if !std::path::Path::new(file).is_file() {
                return Err(format!("--seed-keymap 指定的文件不存在: {}", file));
            }
            Ok(vec![file.to_string()])
        }
        (Some(dir), None) => {
            let dir_path = std::path::Path::new(dir);
            if !dir_path.is_dir() {
                return Err(format!("--seed-dir 指定的目录不存在: {}", dir));
            }
            // 收集 (NN, keymap_path)，按 NN 升序
            let mut found: Vec<(u32, String)> = Vec::new();
            let entries =
                std::fs::read_dir(dir_path).map_err(|e| format!("无法读取目录 {}: {}", dir, e))?;
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                // 仅匹配 thread-{NN}
                let nn = match name.strip_prefix("thread-") {
                    Some(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => {
                        match s.parse::<u32>() {
                            Ok(n) => n,
                            Err(_) => continue,
                        }
                    }
                    _ => continue,
                };
                let keymap = entry.path().join("字根键位表.txt");
                if keymap.is_file() {
                    found.push((nn, keymap.to_string_lossy().to_string()));
                }
            }
            if found.is_empty() {
                return Err(format!(
                    "--seed-dir {} 下未找到任何 thread-NN/字根键位表.txt",
                    dir
                ));
            }
            found.sort_by_key(|(nn, _)| *nn);
            Ok(found.into_iter().map(|(_, p)| p).collect())
        }
    }
}

/// clap 值解析器：只接受 ≥ 1 的整数。
///
/// 用于 `--page-size` 这类会做整除的参数：0 在运行时是除零 panic（release 下直接 abort）。
fn 正整数(s: &str) -> Result<usize, String> {
    match s.trim().parse::<usize>() {
        Ok(0) => Err("必须 ≥ 1".to_string()),
        Ok(n) => Ok(n),
        Err(e) => Err(format!("不是合法整数：{e}")),
    }
}

/// Ctrl-C 停止标志。
///
/// 处理器在进程内只能安装一次（`ctrlc::set_handler` 重复调用会返回 Err 且新标志永不
/// 置位），所以本进程只建一个标志，传给唯一那一次退火调用。
fn 停止标志() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&flag);
    let _ = ctrlc::set_handler(move || f.store(true, Ordering::Relaxed));
    flag
}

// 与 `simulated_annealing_resumable` 同口径：这里都是 CLI 直传的独立选项，
// 为一个新参数凭空引入一层 options struct 反而增加间接层。
#[allow(clippy::too_many_arguments)]
fn run_optimize(
    cfg: &Config,
    cli_config_path: &str,
    output_dir_opt: Option<String>,
    seed_dir: Option<String>,
    seed_keymap: Option<String>,
    scale_arg: Option<String>,
    stop_flag: &Arc<AtomicBool>,
    rng_seed: Option<u64>,
) {
    let start_time = Instant::now();
    println!("=== CodeGenie 码灵算法优化器 v10 ===");
    match rng_seed {
        Some(s) => println!("🎲 固定随机种子: {s}（同配置 + 同线程数可逐位复现）"),
        None => println!("🎲 随机源: 系统熵（可用 --rng-seed 固定）"),
    }

    // 验证配置（#11：权重和不为 1 直接报错退出，不再仅告警）
    if let Err(e) = cfg.validate_weights() {
        eprintln!("❌ {}", e);
        std::process::exit(1);
    }
    // 温度/舒适区参数校验：temp_start = 0 会让调度曲线出 NaN，退火静默退化成严格爬山。
    if let Err(e) = cfg.validate_annealing() {
        eprintln!("❌ {}", e);
        std::process::exit(1);
    }

    // 解析种子来源（互斥校验 + 路径收集，需求 1/2/3）：尽早失败，避免做完校准才报错。
    let seed_paths = match resolve_seed_paths(seed_dir.as_deref(), seed_keymap.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("❌ 种子参数无效: {}", e);
            std::process::exit(1);
        }
    };

    // 打印配置信息
    println!(
        "线程数: {}, 总步数: {}",
        cfg.annealing.threads, cfg.annealing.total_steps
    );
    println!(
        "温度: 取自配置 t_start={:.3e} / t_end={:.3e}（运行时不做标定）",
        cfg.annealing.temp_start, cfg.annealing.temp_end
    );
    println!("全局允许键位: {}", cfg.keys.allowed);
    println!(
        "全码权重: 重码数={:.2}, 重码率={:.2}, 当量={:.2}, CV={:.2}, 分布={:.2}",
        cfg.weights.full_code.collision_count,
        cfg.weights.full_code.collision_rate,
        cfg.weights.full_code.equivalence,
        cfg.weights.full_code.equiv_cv,
        cfg.weights.full_code.distribution,
    );
    println!("用指分布输出顺序: {}", cfg.keys.display_order);
    if let Some(c) = cfg.constraints.as_ref().filter(|c| c.enabled()) {
        println!(
            "编码种数约束: 上限 {}, 罚系数 {}（超出部分计入得分，达标为 0）",
            c.max_encodings, c.encoding_penalty
        );
    }

    // 创建/复用输出目录
    let output_dir = match output_dir_opt {
        // 指定 -d：不存在则创建，存在则复用（允许非空）；逐文件写入时防覆盖归档。
        Some(d) => d,
        // 缺省：output-{时间戳}
        None => {
            let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
            format!("output-{}", timestamp)
        }
    };
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);

    // 备份依赖输入文件到 {output_dir}/inputs/（需求 4），供 resume 用一致输入重建上下文。
    let _ = backup_inputs(cfg, cli_config_path, &output_dir);

    // ==================== 加载数据 ====================
    let (fixed_roots, constrained) = must(loader::load_fixed(&cfg.files.fixed));
    let splits = must(loader::load_splits(&cfg.files.splits));

    // 打印数据统计
    let max_parts_in_data = splits.iter().map(|(_, r, _)| r.len()).max().unwrap_or(0);
    let total_roots: usize = constrained.iter().map(|g| g.roots.len()).sum();
    let total_freq: u64 = splits.iter().map(|(_, _, f)| f).sum();

    println!("\n数据加载完毕:");
    println!("  - 固定字根(单键): {}", fixed_roots.len());
    println!(
        "  - 受限字根组(多键): {} 组 (共 {} 字根)",
        constrained.len(),
        total_roots
    );
    println!("  - 汉字数量: {}", splits.len());
    println!("  - 总字频: {}", total_freq);
    println!("  - 最大码长: {}", max_parts_in_data);

    // ==================== 校验 ====================
    if !validate::check_validation(&splits, &fixed_roots, &constrained) {
        std::process::exit(1);
    }

    // ==================== 初始校准 ====================
    // 判断是否需要 calibrate：全码 5 个字段必须全部设置
    let all_manual = cfg.scale.as_ref().is_some_and(|s| {
        s.collision_count.is_some()
            && s.collision_rate.is_some()
            && s.equivalence.is_some()
            && s.equiv_cv.is_some()
            && s.distribution.is_some()
    });
    let weights = cfg.get_weight_config();
    // 校准分支会构建 temp_ctx（含完整全码预计算）。为避免退火前重复构建，
    // 校准后复用 temp_ctx 作为正式 ctx，仅就地更新评分用的 scale/targets（二者不参与
    // 构建期预计算，仅评分路径读取）。all_manual（手动 scale 跳过校准）无 temp_ctx，
    // 正式 ctx 仍需单独构建一次。
    let (scale_config, scale_source, calib_ctx): (
        types::ScaleConfig,
        &'static str,
        Option<OptContext>,
    ) = if let Some(sp) = scale_arg.as_deref() {
        let text = std::fs::read_to_string(sp).unwrap_or_else(|e| {
            eprintln!("❌ 读不了尺度文件 {}: {}", sp, e);
            std::process::exit(1);
        });
        let sc: types::ScaleConfig = serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("❌ 尺度文件解析失败: {}", e);
            std::process::exit(1);
        });
        println!("\n📐 复用尺度文件 {}（跳过校准，多轮口径一致）", sp);
        (sc, "复用尺度文件", None)
    } else if all_manual {
        println!("\n📐 ScaleConfig 已全部手动配置，跳过自动校准...");
        let (sc, src) = resolve_scale_config(cfg, types::ScaleConfig::default());
        (sc, src, None)
    } else {
        println!("\n📐 正在进行初始尺度校准...");
        // C8：当量/分布仅校准路径需要，就地加载（all_manual / 复用尺度路径不再重复读）。
        let equiv_table = or_warn_default(loader::load_pair_equivalence(&cfg.files.pair_equiv));
        let key_dist_config = or_warn_default(loader::load_key_distribution(&cfg.files.key_dist));
        let temp_scale = types::ScaleConfig::default();
        let temp_ctx = OptContext::new_with_termination(
            &splits,
            &fixed_roots,
            &constrained,
            equiv_table,
            key_dist_config,
            temp_scale,
            weights,
            TargetsConfig::default(),
            &cfg.keys.termination_keys,
            cfg.constraints.clone().unwrap_or_default(),
        );

        let initial_assignment = annealing::smart_init(&temp_ctx, cfg);
        let initial_eval = Evaluator::new(&temp_ctx, &initial_assignment);
        let initial_metrics = initial_eval.get_metrics(&temp_ctx);

        println!("  初始状态观测:");
        println!(
            "    重码数: {},  重码率: {:.6}",
            initial_metrics.collision_count, initial_metrics.collision_rate
        );
        println!(
            "    当量: {:.4},  CV: {:.4},  分布偏差: {:.4}",
            initial_metrics.equiv_mean, initial_metrics.equiv_cv, initial_metrics.dist_deviation
        );
        let calibrated = calibrate_scales(&initial_metrics, &weights, &cfg.get_targets_config());
        let (sc, src) = resolve_scale_config(cfg, calibrated);
        // 复用校准期构建的 temp_ctx 作为正式 ctx（省去退火前第二次完整构建）。
        (sc, src, Some(temp_ctx))
    };

    println!("  ScaleConfig 来源: {}", scale_source);
    println!("  校准尺度 (Scale):");
    println!("    CollisionCount: {:.6}", scale_config.collision_count);
    println!("    CollisionRate:  {:.6}", scale_config.collision_rate);
    println!("    Equivalence:    {:.6}", scale_config.equivalence);
    println!("    EquivCV:        {:.6}", scale_config.equiv_cv);
    println!("    Distribution:   {:.6}", scale_config.distribution);
    // ==================== 正式优化 ====================
    let targets_config = cfg.get_targets_config();
    let ctx = match calib_ctx {
        // 复用校准期构建的上下文，仅就地更新评分用的 scale/targets。
        // 二者不参与 OptContext 构建期预计算、仅在评分路径读取，故就地替换等价于用最终
        // scale/targets 重新构建——消除退火前的重复构建。
        Some(mut c) => {
            c.scale_config = scale_config;
            c.targets_config = targets_config;
            c
        }
        // all_manual：跳过了校准、无可复用上下文，正式 ctx 单独构建一次。
        None => {
            let equiv_table_2 =
                or_warn_default(loader::load_pair_equivalence(&cfg.files.pair_equiv));
            let key_dist_config_2 =
                or_warn_default(loader::load_key_distribution(&cfg.files.key_dist));
            OptContext::new_with_termination(
                &splits,
                &fixed_roots,
                &constrained,
                equiv_table_2,
                key_dist_config_2,
                scale_config,
                weights,
                targets_config,
                &cfg.keys.termination_keys,
                cfg.constraints.clone().unwrap_or_default(),
            )
        }
    };

    println!("\n  - 编码基数: {}", ctx.code_base);
    println!("  - 编码空间: {}", ctx.code_space);

    // 桶存储后端选择日志（需求 12）：单线程、配置确认阶段输出一次。
    evaluator::log_bucket_backends(&ctx);

    // === 退火温度：直接用配置值（与上游 Dieken/code_genie 同模式）===
    // 不做运行时标定。温度只在评分尺度变化时才需要跟着变，而那件事由配置里的
    // temp_start / temp_end 显式表达；自动标定会让同配置的跨运行温度不可复现
    // （实测 t_start 摆动 5.6 倍），参数实验因此无法比较。
    println!(
        "🌡️ 退火温度: t_start={:.6e}, t_end={:.6e}, 舒适区={:.6e}（曲线位置 {:.2}）",
        cfg.annealing.temp_start,
        cfg.annealing.temp_end,
        annealing::舒适区温度(cfg),
        cfg.annealing.comfort_progress
    );
    let sa_cfg = cfg.clone();

    // 写出 checkpoint 元信息（需求 6）：校准已完成，scale_config 固定，运行期不再修改。
    let ckpt_dir = checkpoint::checkpoint_dir(&output_dir);
    std::fs::create_dir_all(&ckpt_dir).expect("无法创建 checkpoint 目录");
    {
        let meta = checkpoint::CheckpointMeta {
            version: checkpoint::CHECKPOINT_VERSION,
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            scale_config,
            targets_config: cfg.get_targets_config(),
            total_steps: cfg.annealing.total_steps,
            num_threads: cfg.annealing.threads,
            temp_start: sa_cfg.annealing.temp_start,
            temp_end: sa_cfg.annealing.temp_end,
            // 舒适区温度存**绝对值**：由进度换算而来，落盘后 resume 才能原样还原
            comfort_temp: annealing::舒适区温度(&sa_cfg),
        };
        if let Err(e) = checkpoint::save_meta(&meta, &ckpt_dir) {
            eprintln!("⚠️ 写 checkpoint 元信息失败: {}", e);
        }
    }

    // Ctrl-C 停止标志由调用方创建并只安装一次处理器：
    // `ctrlc::set_handler` 每个进程只能成功安装一次，重复调用会返回 Err 且新标志永不置位。
    println!(
        "💡 提示: 按 Ctrl-C 可暂停并保存检查点，之后用 `resume -d {}` 继续",
        output_dir
    );

    // 并行执行模拟退火（带 checkpoint 与可中断）
    println!("\n🚀 开始优化...");
    let num_threads = cfg.annealing.threads;

    // 从既有结果构造种子分配（ctx 就绪后；需求 4/5/7/8）。空 = 不播种，保持基线行为。
    let seeds: Vec<Vec<u8>> = if seed_paths.is_empty() {
        Vec::new()
    } else {
        // 播种文件未覆盖的组会随机补齐，这也是一次运行间的噪声源，必须同种子固定。
        let mut rng: rand::rngs::StdRng = match rng_seed {
            Some(s) => rand::rngs::StdRng::seed_from_u64(annealing::派生种子(
                s,
                0,
                annealing::用途_播种补全,
            )),
            None => rand::rngs::StdRng::from_rng(&mut rand::rng()),
        };
        let mut v = Vec::with_capacity(seed_paths.len());
        for p in &seed_paths {
            let map = must(loader::load_keymap(p, &cfg.files.splits));
            match loader::keymap_to_assignment(&ctx, &map, &mut rng) {
                Ok((asg, filled)) => {
                    // 种子未覆盖任何组（filled 等于全部组数）：视为无有效编码行或与当前方案
                    // 完全不匹配，报错退出（需求 2.3）。
                    if ctx.num_groups > 0 && filled == ctx.num_groups {
                        eprintln!(
                            "❌ 种子 {} 未覆盖任何字根组（无有效编码行或与当前方案不匹配）",
                            p
                        );
                        std::process::exit(1);
                    }
                    if filled > 0 {
                        println!("⚠️ 种子 {} 有 {} 个组未被覆盖，已随机合法填充", p, filled);
                    }
                    v.push(asg);
                }
                Err(e) => {
                    eprintln!("❌ 种子 {} 无效: {}", p, e);
                    std::process::exit(1);
                }
            }
        }
        println!(
            "🌱 播种来源: {} 个种子；{} 线程按 round-robin(i % K) 映射",
            v.len(),
            num_threads
        );
        v
    };

    let interval = annealing::checkpoint_interval(
        cfg.annealing.total_steps,
        cfg.annealing.checkpoint_interval_ratio,
    );
    let sa_results: Vec<SaResult> = (0..num_threads)
        .into_par_iter()
        .map(|i| {
            let seed_ref = if seeds.is_empty() {
                None
            } else {
                Some(seeds[seed_index(i, seeds.len())].as_slice())
            };
            simulated_annealing_resumable(
                &ctx,
                &sa_cfg,
                i,
                stop_flag,
                None,
                Some(&ckpt_dir),
                interval,
                seed_ref,
                rng_seed,
            )
        })
        .collect();

    let interrupted = stop_flag.load(Ordering::Relaxed) || sa_results.iter().any(|r| r.interrupted);
    finalize_and_save(cfg, &ctx, sa_results, &output_dir, start_time, interrupted);
}

/// 内嵌全量拆分表：`assets/拆分全表.txt`（12.3 万字，含全部字根）。
///
/// 优化为了速度只喂常用字（`files.splits`），而码表要覆盖全部字。
/// `files.full_splits` 留空时默认用这份内嵌表出「完整码表」；
/// 显式配置了路径则优先用该文件。
const 内嵌拆分全表: &str = include_str!("../assets/拆分全表.txt");

/// 全量拆分表的来源。
enum 全量来源 {
    /// 配置里 `files.full_splits` 显式指定的文件
    文件(String),
    /// 内嵌默认表（`assets/拆分全表.txt`）
    内嵌,
}

/// 取「全量拆分表」来源：`files.full_splits` 留空 → 内嵌默认表；否则用配置的文件。
fn 全量拆分路径(cfg: &Config) -> 全量来源 {
    let p = cfg.files.full_splits.trim();
    if p.is_empty() {
        全量来源::内嵌
    } else {
        全量来源::文件(p.to_string())
    }
}

/// 汇总各线程结果、打印最优、保存全部产物；optimize 与 resume 共用（需求 7.8）。
fn finalize_and_save(
    cfg: &Config,
    ctx: &OptContext,
    sa_results: Vec<SaResult>,
    output_dir: &str,
    start_time: Instant,
    interrupted: bool,
) {
    let root_usage = output::count_root_usage(ctx);
    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics)> = sa_results
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i, r.assignment, r.score, r.metrics))
        .collect();

    // 找出最优结果
    let (best_thread, best_assignment, best_score, best_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m)| (*tid, a.clone(), *s, *m))
        .unwrap();

    let elapsed = start_time.elapsed();

    // 打印最优结果
    let m = best_metrics;
    let best_eval = Evaluator::new(ctx, &best_assignment);
    let best_scores = best_eval.get_metric_scores(ctx);
    println!("\n=================================");
    if interrupted {
        println!("⏸️  优化已暂停（Ctrl-C），以下为当前最优；可继续续算");
    }
    println!("🏆 最优结果 (线程 {}):", best_thread);
    println!("   综合得分: {:.4}", best_score);
    println!(
        "   「全码」重码数: {}  (分: {:.4})",
        m.collision_count, best_scores.collision_count
    );
    println!(
        "   「全码」重码率: {:.6}%  (分: {:.4})",
        m.collision_rate * 100.0,
        best_scores.collision_rate
    );
    println!(
        "   「全码」加权键均当量: {:.4}  (分: {:.4})",
        m.equiv_mean, best_scores.equivalence
    );
    println!(
        "   「全码」当量变异系数(CV): {:.4}  (分: {:.4})",
        m.equiv_cv, best_scores.equiv_cv
    );
    println!(
        "   「全码」用指分布偏差(L2): {:.4}  (分: {:.4})",
        m.dist_deviation, best_scores.distribution
    );
    println!("   分项合计: {:.4}", best_scores.total_full);
    println!("⏱️ 总耗时: {:?}", elapsed);
    println!("=================================");

    // ==================== 保存结果 ====================
    println!("\n📁 保存所有线程结果...");
    for (tid, assignment, score, metrics) in &all_results {
        save_thread_results(
            ctx,
            assignment,
            *score,
            metrics,
            *tid,
            output_dir,
            &root_usage,
        );
    }

    save_results(
        ctx,
        &best_assignment,
        best_score,
        &best_metrics,
        output_dir,
        &root_usage,
    );

    // 全量码表：算完用「手动指定的全量拆分表」再编码一遍，覆盖全部字（如 12 万字）。
    // 只在 [files] full_splits 显式配置时生成，不做自动猜测。
    type 全量拆分条目 = (String, Vec<String>, u64);
    type 全量拆分及来源 = (Vec<全量拆分条目>, String);
    let 全量: Option<全量拆分及来源> = match 全量拆分路径(cfg) {
        全量来源::文件(p) => {
            if !std::path::Path::new(&p).is_file() {
                eprintln!("❌ files.full_splits 指向的文件不存在: {}，跳过完整码表", p);
                None
            } else {
                Some((crate::export_table::加载拆分表(&p), p))
            }
        }
        全量来源::内嵌 => Some((
            crate::export_table::加载拆分表内容(内嵌拆分全表),
            "内嵌 assets/拆分全表.txt".to_string(),
        )),
    };
    let 出了完整码表 = match 全量 {
        Some((全量, 来源)) => {
            if 全量.len() > ctx.raw_splits.len() {
                let 统计 = output::save_full_code_table(ctx, &best_assignment, &全量, output_dir);
                // 合并码表 = 全量全码段（与完整码表同 encoder、同覆盖范围）
                output::save_combined_from_full(ctx, &best_assignment, &全量, output_dir);
                println!(
                    "\n📄 完整码表: {} 条（缺字根 {}），来源 {}",
                    统计.全码条数, 统计.缺根条数, 来源
                );
                if 统计.缺根条数 > 0 {
                    let 样例: Vec<String> = 统计
                        .缺失字根
                        .iter()
                        .map(|(根, 数)| format!("{}({} 字)", 根, 数))
                        .collect();
                    if 样例.is_empty() {
                        println!(
                            "   ⚠️ 有 {} 个字因字根缺键位被跳过（键位表没覆盖全量拆分的字根）",
                            统计.缺根条数
                        );
                    } else {
                        println!(
                            "   ⚠️ 有 {} 个字因字根缺键位被跳过；缺失最多的字根: {}",
                            统计.缺根条数,
                            样例.join(", ")
                        );
                    }
                    // 过半缺失基本不可能是真缺根，而是两套表用了不同的根名体系。
                    if 统计.缺根条数 * 2 > 全量.len() {
                        println!(
                            "   💡 缺失超过一半：files.full_splits 与 files.splits 多半不是同一套根名体系"
                        );
                        println!(
                            "      （如一份用真实部件名、另一份用映射后的根名）。两者必须取自同一套字根，否则全量字无法编码。"
                        );
                    }
                }
                true
            } else {
                println!(
                    "\n📄 完整码表: 全量拆分表 {} 只有 {} 字，不大于当前拆分表（{} 字），跳过",
                    来源,
                    全量.len(),
                    ctx.raw_splits.len()
                );
                false
            }
        }
        None => false,
    };

    save_summary(cfg, &all_results, best_thread, output_dir, elapsed);

    // 全码已定稿：后处理生成三级前缀简码（效率优先），不参与退火、不改全码评分。
    let 简码统计 = if cfg.shortcode.enabled {
        let s = output::save_shortcode_outputs(
            ctx,
            &best_assignment,
            output_dir,
            cfg.shortcode.码长口径(),
        );
        shortcode::打印统计(&s);
        Some(s)
    } else {
        None
    };

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - 汇总.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    if 出了完整码表 {
        println!("  - 完整码表.txt           覆盖全部字的码表");
    }
    if 简码统计.is_some() {
        println!(
            "  - 简码表.txt             三级前缀简码 [{}]（已并入合并码表）",
            cfg.shortcode.码长口径().名称()
        );
    }
    println!("  - thread-XX/               各线程结果");
    if interrupted {
        println!("\n⏸️  已暂停。继续续算：");
        println!("   cargo run --release -- resume -d {}", output_dir);
    }
}

/// 备份依赖输入文件到 `{output_dir}/inputs/`（需求 4）。失败仅告警，不中止。
///
/// 返回成功备份的文件数。空路径（`pair_equiv` / `key_dist` / `full_splits` 省略时使用
/// 内嵌默认表）不参与备份：早期直接拼 `format!("{inputs_dir}/{base}")` 会拼出 `inputs/`
/// 这个目录，于是刷出「重命名旧文件 inputs/ 失败 (os error 32)」+「备份输入文件  失败
/// (os error 3)」两行误导性告警，且末尾仍无条件打印「已备份」。这里改为逐项统计、如实汇总。
fn backup_inputs(cfg: &Config, cli_config_path: &str, output_dir: &str) -> usize {
    let inputs_dir = format!("{}/inputs", output_dir);
    if let Err(e) = std::fs::create_dir_all(&inputs_dir) {
        eprintln!("⚠️ 创建 inputs 目录失败: {}（resume 将无法重建输入）", e);
        return 0;
    }

    // (标签, 源路径, 目标路径)；标签只用于日志，便于一眼看出漏了哪个输入。
    let mut 待备份: Vec<(&str, String, String)> = Vec::new();
    let mut 跳过: Vec<&str> = Vec::new();
    // 复制前就能判定的问题（如两个输入撞同一个 basename），同样计入失败汇总。
    let mut 预检失败: Vec<String> = Vec::new();

    // config 文件备份成 `config.<源扩展名>`（`.ron`/`.toml`），resume 按这两个名字找。
    // 保留扩展名是为了备份件能自己说明格式；内容仍以解析结果为准（加载器两种都认）。
    if cli_config_path.trim().is_empty() {
        跳过.push("配置文件");
    } else {
        let 源扩展 = std::path::Path::new(cli_config_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .filter(|e| e == "ron" || e == "toml")
            .unwrap_or_else(|| "ron".to_string());
        待备份.push((
            "配置文件",
            cli_config_path.to_string(),
            format!("{}/config.{}", inputs_dir, 源扩展),
        ));
    }

    for (标签, src) in [
        ("字根约束", cfg.files.fixed.as_str()),
        ("单字拆分", cfg.files.splits.as_str()),
        ("当量表", cfg.files.pair_equiv.as_str()),
        ("用指分布", cfg.files.key_dist.as_str()),
        // 全量拆分表只影响「完整码表」这份附属产物，但显式配置时同样要备份：
        // 否则 resume 重定向/读不到它，续算后会整份跳过完整码表。
        ("全量拆分", cfg.files.full_splits.as_str()),
    ] {
        if src.trim().is_empty() {
            跳过.push(标签);
            continue;
        }
        let base = std::path::Path::new(src)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if base.trim().is_empty() {
            eprintln!("⚠️ 跳过 {标签} 备份：路径 {} 取不到文件名", src);
            continue;
        }
        let dst = format!("{}/{}", inputs_dir, base);
        // full_splits 常与 splits 指向同一份全字集拆分表：同 (源,目标) 只备份一次，
        // 否则第二次会触发 archive_if_exists 把刚写好的备份改名，inputs/ 里凭空多出归档副本。
        if 待备份
            .iter()
            .any(|项| 项.1.as_str() == src && 项.2.as_str() == dst)
        {
            continue;
        }
        // 不同源却撞同一个 basename：备份件会互相覆盖，resume 只能读到后写的那份。
        // 计为失败并跳过，不让「✅ 已备份 N 个」把漏项盖过去。
        if let Some(已有) = 待备份.iter().find(|项| 项.2.as_str() == dst) {
            预检失败.push(format!(
                "{标签} {src} 与 {} {} 的备份同名（{base}），会互相覆盖",
                已有.0, 已有.1
            ));
            continue;
        }
        待备份.push((标签, src.to_string(), dst));
    }

    if !跳过.is_empty() {
        println!(
            "ℹ️ 无需备份（配置为空，运行时使用内嵌默认表）：{}",
            跳过.join("、")
        );
    }

    let 总数 = 待备份.len() + 预检失败.len();
    let mut 失败: Vec<String> = 预检失败;
    for (标签, src, dst) in &待备份 {
        if let Err(e) = fsutil::copy_with_backup(src, dst) {
            失败.push(format!("{标签} {src} → {dst}: {e}"));
        }
    }
    let 成功 = 总数 - 失败.len();
    if 失败.is_empty() {
        println!("✅ 已备份 {成功} 个输入文件至 {}/", inputs_dir);
    } else {
        eprintln!("⚠️ 输入备份不完整：成功 {成功}/{总数} 个，resume 可能无法用一致输入重建：");
        for 项 in &失败 {
            eprintln!("   · {项}");
        }
    }
    成功
}

/// 把配置里的输入路径重定向到 `{inputs_dir}/` 下的备份副本（按 basename）。
/// 空串（= 用内嵌默认表，如 pair_equiv / key_dist / full_splits）原样返回，
/// 否则会拼成 "inputs/" 这个目录去当文件读，反而刷出「读…失败」的假告警。
fn 重定向输入路径(p: &str, inputs_dir: &str) -> String {
    if p.trim().is_empty() {
        return p.to_string();
    }
    let base = std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string());
    format!("{}/{}", inputs_dir, base)
}

/// resume 子命令（需求 7）：从既有 output 目录的 checkpoint 续算，复用该目录。
/// 对 inputs/ 与 meta.json 只读；从 inputs/ 重建上下文、复用 meta.scale_config（不重校准）。
fn run_resume(dir: &str, rng_seed: Option<u64>) {
    let start_time = Instant::now();
    println!("=== CodeGenie 断点续算 ===");
    // checkpoint 不序列化 RNG 状态：同种子 + 同断点的续算彼此可复现，
    // 但不等价于「从零一次跑完」（RNG 流位置无法恢复）。
    match rng_seed {
        Some(s) => println!("🎲 固定随机种子: {s}（自断点重新初始化随机流）"),
        None => println!("🎲 随机源: 系统熵"),
    }
    println!("  输出目录: {}", dir);

    let ckpt_dir = checkpoint::checkpoint_dir(dir);

    // 1) 读元信息（只读 + 版本校验，需求 6.4/7.4/11.4）
    let meta = match checkpoint::load_meta(&ckpt_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ 无法加载 checkpoint 元信息: {}", e);
            std::process::exit(1);
        }
    };

    // 2) 从备份的 inputs/ 读取配置与输入（只读，需求 7.2/7.3）。
    let inputs_dir = format!("{}/inputs", dir);
    // 备份件按源扩展名命名（config.ron / config.toml），两个都认，先 ron 后 toml。
    let config_path = ["config.ron", "config.toml"]
        .iter()
        .map(|名| format!("{}/{}", inputs_dir, 名))
        .find(|p| std::path::Path::new(p).is_file())
        .unwrap_or_else(|| format!("{}/config.ron", inputs_dir));
    // #1：resume 同样不能静默回退默认配置，解析失败直接报错退出。
    let mut cfg = match Config::load_from_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ {}", e);
            std::process::exit(1);
        }
    };
    // 将 files.* 重定向到 inputs/ 下的备份副本（按 basename），确保用一致输入重建。
    cfg.files.fixed = 重定向输入路径(&cfg.files.fixed, &inputs_dir);
    cfg.files.splits = 重定向输入路径(&cfg.files.splits, &inputs_dir);
    cfg.files.pair_equiv = 重定向输入路径(&cfg.files.pair_equiv, &inputs_dir);
    cfg.files.key_dist = 重定向输入路径(&cfg.files.key_dist, &inputs_dir);
    let 原全量拆分 = cfg.files.full_splits.clone();
    cfg.files.full_splits = 重定向输入路径(&原全量拆分, &inputs_dir);
    // 全量拆分表只影响「完整码表」这份附属产物，不影响优化得分。修复前生成的输出目录
    // 没有它的备份，此时退回原路径并提示，避免续算后整份完整码表被静默跳过。
    if !原全量拆分.trim().is_empty()
        && !std::path::Path::new(&cfg.files.full_splits).is_file()
        && std::path::Path::new(&原全量拆分).is_file()
    {
        println!(
            "ℹ️ inputs/ 里没有全量拆分表备份（旧输出目录），沿用原路径: {}",
            原全量拆分
        );
        cfg.files.full_splits = 原全量拆分;
    }
    let cfg = cfg;

    // 3) 重建优化上下文（复用 meta.scale_config，跳过 calibrate，需求 6.3）。
    let (fixed_roots, constrained) = must(loader::load_fixed(&cfg.files.fixed));
    let splits = must(loader::load_splits(&cfg.files.splits));
    let equiv_table = or_warn_default(loader::load_pair_equivalence(&cfg.files.pair_equiv));
    let key_dist_config = or_warn_default(loader::load_key_distribution(&cfg.files.key_dist));
    if !validate::check_validation(&splits, &fixed_roots, &constrained) {
        std::process::exit(1);
    }
    let weights = cfg.get_weight_config();
    // #2：优先用首轮写入的 targets；旧检查点（无该字段 → 默认）退回备份配置，避免回归。
    let resume_targets = if meta.targets_config == TargetsConfig::default() {
        cfg.get_targets_config()
    } else {
        meta.targets_config.clone()
    };
    let mut ctx = OptContext::new_with_termination(
        &splits,
        &fixed_roots,
        &constrained,
        equiv_table,
        key_dist_config,
        meta.scale_config,
        weights,
        // #2：复用首轮写入的 targets，避免续算时因配置文件漂移导致评分口径“冻结”/不一致。
        resume_targets,
        &cfg.keys.termination_keys,
        cfg.constraints.clone().unwrap_or_default(),
    );
    ctx.display_order = cfg.keys.display_order.clone();
    evaluator::log_bucket_backends(&ctx);

    // 4) 加载各线程检查点（数量须与 meta.num_threads 一致，需求 11.4）。
    let num_threads = meta.num_threads;
    let mut tcs: Vec<ThreadCheckpoint> = Vec::with_capacity(num_threads);
    for i in 0..num_threads {
        let path = checkpoint::thread_path(&ckpt_dir, i);
        match checkpoint::load_thread_checkpoint(&path) {
            Ok(tc) => tcs.push(tc),
            Err(e) => {
                eprintln!("❌ 无法加载线程检查点 {}: {}", path.display(), e);
                std::process::exit(1);
            }
        }
    }
    println!(
        "已加载 {} 个线程检查点；总步数 {}，从各线程断点续算",
        num_threads, meta.total_steps
    );

    // 5) 安装 Ctrl-C，并行续算（resume=Some），收尾产出到同一目录（不新建、不改 inputs/meta）。
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let sf = Arc::clone(&stop_flag);
        let _ = ctrlc::set_handler(move || sf.store(true, Ordering::Relaxed));
    }
    println!("💡 提示: 按 Ctrl-C 可暂停并保存检查点");

    // 复用首轮的温度与舒适区：以 checkpoint meta 为权威，避免续算口径漂移。
    let mut sa_cfg = cfg.clone();
    sa_cfg.annealing.temp_start = meta.temp_start;
    sa_cfg.annealing.temp_end = meta.temp_end;
    // 舒适区同样以 meta 的绝对温度为权威：直接写回绝对值，避免「绝对→进度→绝对」往返误差。
    // 旧检查点若 comfort_temp 缺失/为 0，则沿用配置的 comfort_progress（或配置的绝对温度）。
    if meta.comfort_temp > 0.0 && meta.comfort_temp.is_finite() {
        sa_cfg.annealing.comfort_temp = Some(meta.comfort_temp);
    }

    let interval = annealing::checkpoint_interval(
        cfg.annealing.total_steps,
        cfg.annealing.checkpoint_interval_ratio,
    );
    let sa_results: Vec<SaResult> = (0..num_threads)
        .into_par_iter()
        .map(|i| {
            simulated_annealing_resumable(
                &ctx,
                &sa_cfg,
                i,
                &stop_flag,
                Some(&tcs[i]),
                Some(&ckpt_dir),
                interval,
                None,
                rng_seed,
            )
        })
        .collect();

    let interrupted = stop_flag.load(Ordering::Relaxed) || sa_results.iter().any(|r| r.interrupted);
    finalize_and_save(&cfg, &ctx, sa_results, dir, start_time, interrupted);
}

// =========================================================================
// 🧪 输入备份测试（annealing-checkpoint-resume, 需求 4）
// =========================================================================
#[cfg(test)]
mod backup_tests {
    use super::*;

    #[test]
    fn backup_inputs_copies_all_dependency_files() {
        // 准备临时源目录与 6 个依赖文件。
        let base = std::env::temp_dir().join(format!("cg_backup_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        let files = [
            ("config.ron", "config-content"),
            ("input-fixed.txt", "fixed-content"),
            ("input-division.txt", "division-content"),
            ("pair_equivalence.txt", "equiv-content"),
            ("key_distribution.txt", "keydist-content"),
        ];
        for (name, content) in files {
            std::fs::write(src.join(name), content).unwrap();
        }

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("input-fixed.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("input-division.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = src
            .join("pair_equivalence.txt")
            .to_string_lossy()
            .to_string();
        cfg.files.key_dist = src
            .join("key_distribution.txt")
            .to_string_lossy()
            .to_string();
        let config_path = src.join("config.ron").to_string_lossy().to_string();

        backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        // 断言 inputs/ 下 6 个文件均存在且内容一致（按 basename）。
        let inputs = out.join("inputs");
        for (name, content) in files {
            let dst = inputs.join(name);
            assert!(dst.exists(), "缺少备份文件: {}", dst.display());
            assert_eq!(
                std::fs::read_to_string(&dst).unwrap(),
                content,
                "{name} 内容不一致"
            );
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// config 文件名非默认名时，备份应统一成 `config.<扩展名>`，保证 resume 找得到；
    /// 扩展名要保留（`.ron`/`.toml`），否则备份件会自称 RON 而其实是 TOML。
    #[test]
    fn backup_inputs_renames_config_keeping_extension() {
        let base = std::env::temp_dir().join(format!("cg_backup_rename_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        // config 文件使用非默认名称
        let custom_config = "my-custom-config.ron";
        std::fs::write(src.join(custom_config), "custom-config-content").unwrap();
        std::fs::write(src.join("input-fixed.txt"), "fixed").unwrap();
        std::fs::write(src.join("input-division.txt"), "division").unwrap();
        std::fs::write(src.join("pair_equivalence.txt"), "equiv").unwrap();
        std::fs::write(src.join("key_distribution.txt"), "keydist").unwrap();

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("input-fixed.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("input-division.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = src
            .join("pair_equivalence.txt")
            .to_string_lossy()
            .to_string();
        cfg.files.key_dist = src
            .join("key_distribution.txt")
            .to_string_lossy()
            .to_string();
        let config_path = src.join(custom_config).to_string_lossy().to_string();

        backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        let inputs = out.join("inputs");
        // 统一命名为 config.ron（扩展名沿用源文件），内容与源文件一致
        let dst = inputs.join("config.ron");
        assert!(dst.exists(), "备份的 config 文件应命名为 config.ron");
        assert_eq!(
            std::fs::read_to_string(&dst).unwrap(),
            "custom-config-content"
        );
        // 原始文件名不应出现在 inputs/ 下
        assert!(
            !inputs.join(custom_config).exists(),
            "不应以原始文件名备份 config"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// TOML 配置备份成 `config.toml`（而不是硬改名为 config.ron），
    /// 这样备份件自带的扩展名与实际格式一致，resume 也能按名找到。
    #[test]
    fn backup_inputs_keeps_toml_extension() {
        let base = std::env::temp_dir().join(format!("cg_backup_toml_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test-toml");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        std::fs::write(src.join("cfg.toml"), "x = 1\n").unwrap();
        let cfg = Config::default();
        let config_path = src.join("cfg.toml").to_string_lossy().to_string();

        backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        let inputs = out.join("inputs");
        assert!(
            inputs.join("config.toml").exists(),
            "TOML 配置应备份成 config.toml"
        );
        assert!(
            !inputs.join("config.ron").exists(),
            "TOML 配置不应被改名为 config.ron"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 省略 `pair_equiv`（用内嵌当量表）时，不得把 `inputs/` 目录本身当备份目标。
    /// 对应日志里那两行「重命名旧文件 inputs/ 失败」+「备份输入文件  失败」。
    #[test]
    fn backup_inputs_skips_empty_optional_paths() {
        let base = std::env::temp_dir().join(format!("cg_backup_empty_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        for name in ["cfg.toml", "fixed.txt", "splits.txt", "keydist.txt"] {
            std::fs::write(src.join(name), name).unwrap();
        }

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("fixed.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("splits.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = String::new();
        cfg.files.key_dist = src.join("keydist.txt").to_string_lossy().to_string();
        cfg.files.full_splits = String::new();
        let config_path = src.join("cfg.toml").to_string_lossy().to_string();

        let 成功 = backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        assert_eq!(成功, 4, "config + fixed + splits + key_dist 均应成功");
        let inputs = out.join("inputs");
        assert!(inputs.is_dir(), "inputs/ 目录不应被改名或删除");
        let 文件: Vec<String> = std::fs::read_dir(&inputs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(文件.len(), 4, "只应有 4 个备份文件: {文件:?}");
        for 名 in ["config.toml", "fixed.txt", "splits.txt", "keydist.txt"] {
            assert!(inputs.join(名).is_file(), "缺少 {名}: {文件:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 复制失败必须计入失败，不能再被「已备份」一句话盖过去。
    #[test]
    fn backup_inputs_counts_missing_source_as_failure() {
        let base = std::env::temp_dir().join(format!("cg_backup_fail_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(src.join("cfg.toml"), "x = 1\n").unwrap();
        std::fs::write(src.join("splits.txt"), "splits").unwrap();

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("不存在.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("splits.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = String::new();
        cfg.files.key_dist = String::new();
        let config_path = src.join("cfg.toml").to_string_lossy().to_string();

        let 成功 = backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        assert_eq!(成功, 2, "config + splits 成功，缺失的 fixed 应计入失败");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `files.full_splits` 显式配置时必须一并备份，否则 resume 续算会读不到它、
    /// 整份跳过完整码表。
    #[test]
    fn backup_inputs_includes_full_splits() {
        let base = std::env::temp_dir().join(format!("cg_backup_full_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        for name in [
            "cfg.toml",
            "fixed.txt",
            "splits.txt",
            "full.txt",
            "keydist.txt",
        ] {
            std::fs::write(src.join(name), name).unwrap();
        }

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("fixed.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("splits.txt").to_string_lossy().to_string();
        cfg.files.key_dist = src.join("keydist.txt").to_string_lossy().to_string();
        cfg.files.full_splits = src.join("full.txt").to_string_lossy().to_string();
        let config_path = src.join("cfg.toml").to_string_lossy().to_string();

        let 成功 = backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        assert_eq!(成功, 5, "config + 4 个数据文件均应成功备份");
        let inputs = out.join("inputs");
        assert!(inputs.join("full.txt").is_file(), "全量拆分表应被备份");
        assert_eq!(
            std::fs::read_dir(&inputs).unwrap().count(),
            5,
            "inputs/ 里应恰好 5 个文件"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `full_splits` 与 `splits` 指向同一文件时只备份一次，不能触发归档产生多余副本。
    #[test]
    fn backup_inputs_dedups_same_full_splits_path() {
        let base = std::env::temp_dir().join(format!("cg_backup_dup_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        for name in ["cfg.toml", "fixed.txt", "splits.txt", "keydist.txt"] {
            std::fs::write(src.join(name), name).unwrap();
        }
        let 拆分表 = src.join("splits.txt").to_string_lossy().to_string();

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("fixed.txt").to_string_lossy().to_string();
        cfg.files.splits = 拆分表.clone();
        cfg.files.key_dist = src.join("keydist.txt").to_string_lossy().to_string();
        cfg.files.full_splits = 拆分表;
        let config_path = src.join("cfg.toml").to_string_lossy().to_string();

        let 成功 = backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        assert_eq!(成功, 4, "同一个文件不应被复制两次而重复计数");
        let inputs = out.join("inputs");
        let 文件: Vec<String> = std::fs::read_dir(&inputs)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(文件.len(), 4, "不应产生带时间戳的归档副本: {文件:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 不同源撞同一个 basename 会互相覆盖，必须计入失败而不是静默漏备份。
    #[test]
    fn backup_inputs_reports_basename_conflict() {
        let base = std::env::temp_dir().join(format!("cg_backup_conflict_{}", std::process::id()));
        let dir_a = base.join("a");
        let dir_b = base.join("b");
        let out = base.join("output-test");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(base.join("cfg.toml"), "x = 1\n").unwrap();
        std::fs::write(dir_a.join("拆分表.txt"), "A").unwrap();
        std::fs::write(dir_b.join("拆分表.txt"), "B").unwrap();

        let mut cfg = Config::default();
        cfg.files.splits = dir_a.join("拆分表.txt").to_string_lossy().to_string();
        cfg.files.full_splits = dir_b.join("拆分表.txt").to_string_lossy().to_string();
        let config_path = base.join("cfg.toml").to_string_lossy().to_string();

        let 成功 = backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        assert_eq!(
            成功, 2,
            "config + splits 成功，同名的 full_splits 应计为失败"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("inputs/拆分表.txt")).unwrap(),
            "A",
            "先备份的内容不应被后来的同名文件覆盖"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 重定向输入路径_keeps_empty_and_uses_basename() {
        assert_eq!(重定向输入路径("", "/out/inputs"), "");
        assert_eq!(重定向输入路径("  ", "/out/inputs"), "  ");
        assert_eq!(
            重定向输入路径("D:/data/拆分表.txt", "/out/inputs"),
            "/out/inputs/拆分表.txt"
        );
    }
}

// =========================================================================
// 🧪 播种来源解析与线程映射测试（seed-optimize-from-result, 需求 1/2/3/7）
// =========================================================================
#[cfg(test)]
mod seed_path_tests {
    use super::*;
    use proptest::prelude::*;

    fn tmp_base(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cg_seed_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn mutually_exclusive_errors() {
        // 同时指定 --seed-dir 与 --seed-keymap → Err（需求 3.1）
        assert!(resolve_seed_paths(Some("d"), Some("k")).is_err());
    }

    #[test]
    fn none_returns_empty() {
        // 均未指定 → 空 Vec（不播种，需求 3.2）
        assert_eq!(
            resolve_seed_paths(None, None).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn seed_keymap_single_file() {
        let base = tmp_base("kf");
        std::fs::create_dir_all(&base).unwrap();
        let f = base.join("km.txt");
        std::fs::write(&f, "口\tWko\t1\n").unwrap();
        let paths = resolve_seed_paths(None, Some(&f.to_string_lossy())).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], f.to_string_lossy());
        // 不存在的文件 → Err（需求 2.3）
        assert!(resolve_seed_paths(None, Some("/no/such/file.txt")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_dir_collects_thread_dirs_sorted() {
        let base = tmp_base("sd");
        // 故意乱序创建 thread-02, thread-00, thread-10，且各含 字根键位表.txt
        for nn in ["02", "00", "10"] {
            let d = base.join(format!("thread-{}", nn));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("字根键位表.txt"), "口\tWko\t1\n").unwrap();
        }
        // 干扰项：inputs/ 与 checkpoint/ 不应被收集（需求 1.5）
        std::fs::create_dir_all(base.join("inputs")).unwrap();
        std::fs::create_dir_all(base.join("checkpoint")).unwrap();
        // 干扰项：thread-03 无 字根键位表.txt → 不收集
        std::fs::create_dir_all(base.join("thread-03")).unwrap();

        let paths = resolve_seed_paths(Some(&base.to_string_lossy()), None).unwrap();
        assert_eq!(paths.len(), 3, "应只收集 3 个含 keymap 的 thread 目录");
        // 按 NN 升序：00, 02, 10
        // 用 Path::ends_with 按**路径组件**比较（Windows 分隔符是 `\`，字符串 ends_with 会失败）
        let 断言尾部 = |实际: &str, 期望: &str| {
            assert!(
                std::path::Path::new(实际).ends_with(std::path::Path::new(期望)),
                "路径 {实际} 应以 {期望} 结尾"
            );
        };
        断言尾部(&paths[0], "thread-00/字根键位表.txt");
        断言尾部(&paths[1], "thread-02/字根键位表.txt");
        断言尾部(&paths[2], "thread-10/字根键位表.txt");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_dir_empty_errors() {
        let base = tmp_base("empty");
        std::fs::create_dir_all(&base).unwrap();
        // 无任何 thread-NN/字根键位表.txt → Err（需求 1.4）
        assert!(resolve_seed_paths(Some(&base.to_string_lossy()), None).is_err());
        // 不存在的目录 → Err
        assert!(resolve_seed_paths(Some("/no/such/dir"), None).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: seed-optimize-from-result, Property 4: 线程→种子轮询映射
        #[test]
        fn prop_round_robin_mapping(k in 1usize..16, t in 1usize..64) {
            // 线程 i 映射到 i % k
            for i in 0..t {
                prop_assert_eq!(seed_index(i, k), i % k);
            }
            // 被使用的种子下标集合
            let used: std::collections::HashSet<usize> = (0..t).map(|i| seed_index(i, k)).collect();
            if t < k {
                // T<K：恰为 {0..T-1}
                let expected: std::collections::HashSet<usize> = (0..t).collect();
                prop_assert_eq!(used, expected);
            }
            // K==1：所有线程映射到 0
            if k == 1 {
                for i in 0..t {
                    prop_assert_eq!(seed_index(i, k), 0);
                }
            }
        }
    }
}

// =========================================================================
// 🧪 尺度覆盖语义 属性测试（target-deviation-optimization, Property 1）
// =========================================================================
#[cfg(test)]
mod scale_proptest {
    use super::*;
    use crate::config::ScaleConfigToml;
    use crate::types::ScaleConfig;
    use proptest::prelude::*;

    /// 与 `resolve_scale_config` 内部的 `to_scale` 同口径：取倒数，0 值保护
    fn 取倒数(v: f64) -> f64 {
        if v > 0.0 {
            1.0 / v
        } else {
            1.0
        }
    }

    fn 近似(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * b.abs().max(1.0)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        // Property 1: resolve_scale_config 覆盖语义
        // 任意手动配置 × 任意校准值：Some 字段 = 手动值取倒数，None 字段 = 校准值
        // Validates: Requirements 1.3, 1.4
        #[test]
        fn prop1_尺度覆盖语义(
            手动 in proptest::collection::vec(proptest::option::of(0.0f64..1e3), 5),
            校准 in proptest::collection::vec(1e-6f64..1e3, 5),
        ) {
            let toml = ScaleConfigToml {
                collision_count: 手动[0],
                collision_rate: 手动[1],
                equivalence: 手动[2],
                equiv_cv: 手动[3],
                distribution: 手动[4],
            };
            let calibrated = ScaleConfig {
                collision_count: 校准[0],
                collision_rate: 校准[1],
                equivalence: 校准[2],
                equiv_cv: 校准[3],
                distribution: 校准[4],
            };

            let cfg = Config {
                scale: Some(toml),
                ..Config::default()
            };

            let (got, 来源) = resolve_scale_config(&cfg, calibrated);
            // 三条路径：cfg.scale 为 None → 自动校准；五项齐 → 手动配置；
            // 部分字段为 None → 部分手动覆盖（以校准值为底、逐字段覆盖）。
            prop_assert!(
                来源 == "手动配置" || 来源 == "自动校准" || 来源 == "部分手动覆盖",
                "未知来源: {}", 来源
            );

            // 逐字段：Some → 取倒数(手动值)；None → 校准值
            let 期望 = |手动值: Option<f64>, 校准值: f64| 手动值.map(取倒数).unwrap_or(校准值);

            prop_assert!(近似(got.collision_count, 期望(手动[0], 校准[0])),
                "collision_count got={} want={}", got.collision_count, 期望(手动[0], 校准[0]));
            prop_assert!(近似(got.collision_rate, 期望(手动[1], 校准[1])),
                "collision_rate got={} want={}", got.collision_rate, 期望(手动[1], 校准[1]));
            prop_assert!(近似(got.equivalence, 期望(手动[2], 校准[2])),
                "equivalence got={} want={}", got.equivalence, 期望(手动[2], 校准[2]));
            prop_assert!(近似(got.equiv_cv, 期望(手动[3], 校准[3])),
                "equiv_cv got={} want={}", got.equiv_cv, 期望(手动[3], 校准[3]));
            prop_assert!(近似(got.distribution, 期望(手动[4], 校准[4])),
                "distribution got={} want={}", got.distribution, 期望(手动[4], 校准[4]));
        }

        // Property 1 的边界：完全不给 [scale] 段时必须原样返回校准值
        #[test]
        fn prop1b_无手动配置时原样返回校准值(
            校准 in proptest::collection::vec(1e-6f64..1e3, 5),
        ) {
            let calibrated = ScaleConfig {
                collision_count: 校准[0],
                collision_rate: 校准[1],
                equivalence: 校准[2],
                equiv_cv: 校准[3],
                distribution: 校准[4],
            };
            let cfg = Config {
                scale: None,
                ..Config::default()
            };

            let (got, 来源) = resolve_scale_config(&cfg, calibrated);
            prop_assert_eq!(来源, "自动校准");
            prop_assert!(近似(got.collision_count, 校准[0]));
            prop_assert!(近似(got.distribution, 校准[4]));
        }
    }
}
