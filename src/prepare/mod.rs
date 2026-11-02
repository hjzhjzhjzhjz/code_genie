// =========================================================================
// 🏗️ `prepare` 子命令：把上游数据加工成优化器要的输入文件
// =========================================================================
//
// 这是 `moling/prepare-inputs.sh`（710 行 bash + 内嵌 Perl，13 个步骤）的 Rust
// 移植版。步骤编号与脚本里的 `echo '(N) ...'` 一一对应，行为按「逐字节等价」
// 移植，包括排序键、浮点格式串、行尾悬空 TAB 这类细节。
//
// 与原脚本的三点差异：
//
// 1. 所有路径都相对 `--dir`（默认 `moling`）解析，不再要求先 `cd moling`。
// 2. 行尾统一按 `str::lines()` 处理，CRLF 工作区与 LF 检出结果一致。
// 3. 第 0 步需要的两份字频 JSON 若缺失，会先尝试本机 `curl` 拉取（与 bash 版
//    同样的外部依赖），失败则给出「手动放到 --dir 下」的明确提示。
//
// 上游素材（`yustar_chaifen*.dict.yaml`、`yuhao_charsets.lua`、`yuhao-zigens.csv`、
// `beiyu-char-freq.txt`、`charAbsoluteFrequency*.json`、`chars.dict.yaml`）跟方案
// 无关，统一放在仓库根的 `yuhao-data/`（见 [`crate::data_dir`]）。读取时先看
// `--dir` 再看素材目录，所以「素材平铺在 `--dir` 下」的老布局照样能跑；
// 产物一律只写 `--dir`。用 `--data-dir` 可以手工指定素材目录。
//
// `--from-step/--to-step` 用于分段调试（例如本机没有那份 JSON 时从第 1 步开始，
// 复用已有的 `全字频表.txt`）；`--baseline-dir` 用于把产物与一份基线逐字节对照。
// =========================================================================

pub(crate) mod inputs;
pub mod names;
mod roots;
pub(crate) mod schema;
mod tables;
pub(crate) mod text;

use std::path::{Path, PathBuf};

use crate::prepare::schema::方案;

/// 一份准备任务的上下文：工作目录 + 方案 + 解析后的四个可被方案覆盖的文件名
pub struct 上下文 {
    pub 目录: PathBuf,
    /// 上游素材目录（`yuhao-data/`），读素材时回落到这里
    pub 素材目录: PathBuf,
    pub 方案: &'static 方案,
    /// `ROOTS_TXT`：字根声码韵码表
    pub 字根表: String,
    /// `ROOTS_CLUSTER_TXT`：字根聚类表
    pub 聚类表: String,
    /// `FULL_FREQ_TXT`：全字集字频表
    pub 全字频表: String,
    /// `CHARS_TXT`：算码字符集
    pub 算码字集: String,
}

impl 上下文 {
    fn 新建(目录: &str, 方案: &'static 方案, 素材: Option<&str>) -> Self {
        let 目录 = PathBuf::from(目录);
        let 素材目录 = match 素材 {
            Some(p) => PathBuf::from(p),
            None => crate::data_dir::定位(&目录),
        };
        // 对应 init.sh 末尾的 `for s in full-freq chars roots roots-cluster` 循环：
        // 方案自己放了 `全字频表-<schema>.txt` 这类文件就用它，否则用默认名
        // （`roots-cluster` 是人工维护的输入，不随产物一起改名）。
        let 解析 = |基名: &str, 默认: &str| {
            let 专用 = 目录.join(format!("{基名}-{}.txt", 方案.名));
            if 专用.exists() {
                专用
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| 默认.to_string())
            } else {
                默认.to_string()
            }
        };
        Self {
            字根表: 方案
                .字根表
                .map(str::to_string)
                .unwrap_or_else(|| 解析(names::字根声韵基名, names::字根声韵)),
            聚类表: 解析("roots-cluster", "roots-cluster.txt"),
            全字频表: 解析(names::全字频表基名, names::全字频表),
            算码字集: 解析(names::算码字集基名, names::算码字集),
            方案,
            目录,
            素材目录,
        }
    }

    pub fn 路径(&self, 名: &str) -> PathBuf {
        self.目录.join(名)
    }

    /// 源文件的实际位置：先看产物目录，再看素材目录
    ///
    /// 上游素材（`yustar_chaifen*.dict.yaml` 这类）只在素材目录里，产物只在
    /// `--dir` 里，两者同名的情况以 `--dir` 为准。
    pub fn 源路径(&self, 名: &str) -> Option<PathBuf> {
        let 本地 = self.路径(名);
        if 本地.is_file() {
            return Some(本地);
        }
        let 素材 = self.素材目录.join(名);
        素材.is_file().then_some(素材)
    }

    pub fn 读(&self, 名: &str) -> Result<Vec<String>, String> {
        // 回落必须用 `self.素材目录`（可能是 `--data-dir` 显式指定的），不能重新
        // `data_dir::定位`——那样会无视手工指定的目录。
        match self.源路径(名) {
            Some(p) => text::读行(&p),
            None => text::读行(&self.路径(名)),
        }
    }

    pub fn 写(&self, 名: &str, 内容: &str) -> Result<(), String> {
        text::写文本(&self.路径(名), 内容)
    }

    pub fn 追加(&self, 名: &str, 内容: &str) -> Result<(), String> {
        text::追加文本(&self.路径(名), 内容)
    }
}

/// 命令行参数
pub struct 参数 {
    /// 工作目录，脚本里的 cwd（默认 `moling`）
    pub 目录: String,
    /// 上游素材目录；缺省自动探测 `--dir` 下的 `yuhao-data/` 或上一级的 `yuhao-data/`
    pub 素材目录: Option<String>,
    /// 方案名，见 [`schema::取`]
    pub 方案: String,
    /// 起始步骤（含）
    pub 从步骤: usize,
    /// 结束步骤（含）
    pub 到步骤: usize,
    /// 基线目录：给了就逐字节对照产物
    pub 基线目录: Option<String>,
    /// 读 `[weights.simple_code].enabled` 的配置文件；缺省在 `--dir` 下按
    /// `config-<schema>.toml` → `config.toml` 的顺序找
    pub 配置: Option<String>,
}

/// 一次产出的文件清单，用于 `--baseline-dir` 对照
const 产物清单: [&str; 13] = [
    names::全字频表,
    names::算码字集,
    names::算码字频,
    names::拆分表,
    names::拆分全表,
    names::拆分全表台,
    names::字根频率,
    names::字根读音,
    names::字根声韵,
    names::飞键候选,
    names::字根约束,
    names::动态字根,
    names::单字拆分,
];

/// 入口
pub fn run(参: &参数) -> Result<(), String> {
    let 方案 = schema::取(&参.方案)?;
    let 上 = 上下文::新建(&参.目录, 方案, 参.素材目录.as_deref());

    println!("=== 准备优化输入（{} / {}）===", 方案.名, 方案.名称);
    println!("  工作目录: {}", 上.目录.display());
    println!("  素材目录: {}", 上.素材目录.display());
    println!("  字频表:   {}", 上.全字频表);
    println!("  算码字集: {}", 上.算码字集);
    println!("  字根表:   {}", 上.字根表);
    println!("  聚类表:   {}", 上.聚类表);

    for 步骤 in 参.从步骤..=参.到步骤 {
        match 步骤 {
            0 => tables::步骤0_全字频表(&上)?,
            1 => tables::步骤1_字符集(&上)?,
            2 => tables::步骤2_算码字频(&上)?,
            3 => tables::步骤3_拆分表(&上)?,
            4 => tables::步骤4_字根频率(&上)?,
            5 => roots::步骤5_字根读音(&上)?,
            6 => roots::步骤6_字根声韵码(&上)?,
            7 => roots::步骤7_飞键候选(&上)?,
            8 => inputs::步骤8_大码约束(&上)?,
            9 => inputs::步骤9_声韵约束(&上)?,
            10 => inputs::步骤10_飞键约束(&上)?,
            11 => inputs::步骤11_空字根表(&上)?,
            12 => inputs::步骤12_编码展开(&上)?,
            13 => inputs::步骤13_简码码位(&上, 解析配置(参, &上).as_deref())?,
            other => return Err(format!("步骤号 {other} 超出 0~13")),
        }
    }

    println!();
    println!("数据准备工作成功完成！");

    if let Some(基线) = &参.基线目录 {
        对照(&上, Path::new(基线), 参.从步骤, 参.到步骤)?;
    }
    Ok(())
}

/// 定位配置文件（`config-<schema>.toml` 优先，其次 `config.toml`）
fn 解析配置(参: &参数, 上: &上下文) -> Option<PathBuf> {
    if let Some(p) = &参.配置 {
        return Some(PathBuf::from(p));
    }
    let 专用 = 上.目录.join(format!("config-{}.toml", 上.方案.名));
    if 专用.exists() {
        return Some(专用);
    }
    let 默认 = 上.目录.join("config.toml");
    默认.exists().then_some(默认)
}

/// 逐字节对照产物与基线目录，打印首个差异段落
fn 对照(
    上: &上下文, 基线目录: &Path, 起始步骤: usize, 结束步骤: usize
) -> Result<(), String> {
    println!();
    println!("=== 与基线 {} 逐字节对照 ===", 基线目录.display());
    let mut 不一致 = 0usize;
    for 名 in 产物清单 {
        let 基线路径 = 基线目录.join(名);
        if !基线路径.exists() {
            continue;
        }
        let 实际路径 = 上.路径(名);
        let 基线文本 = text::读行(&基线路径)?;
        let 实际文本 = match text::读行(&实际路径) {
            Ok(v) => v,
            Err(_) => {
                println!("  ✗ {名}: 产物缺失");
                不一致 += 1;
                continue;
            }
        };
        if 基线文本 == 实际文本 {
            println!("  ✓ {名}: {} 行一致", 基线文本.len());
            continue;
        }
        不一致 += 1;
        println!(
            "  ✗ {名}: 基线 {} 行 / 产物 {} 行",
            基线文本.len(),
            实际文本.len()
        );
        let 首个 = 基线文本
            .iter()
            .zip(实际文本.iter())
            .position(|(a, b)| a != b);
        match 首个 {
            Some(行号) => {
                println!("    首个差异在第 {} 行：", 行号 + 1);
                println!("      基线: {:?}", 基线文本[行号]);
                println!("      产物: {:?}", 实际文本[行号]);
            }
            None => println!("    前缀相同，仅长度不同"),
        }
        let 差异数 = 基线文本
            .iter()
            .zip(实际文本.iter())
            .filter(|(a, b)| a != b)
            .count();
        println!("    逐行差异 {差异数} 处");
    }
    if 不一致 == 0 {
        println!("  全部一致（步骤 {起始步骤}~{结束步骤}）");
        Ok(())
    } else {
        Err(format!("有 {不一致} 个产物与基线不一致"))
    }
}
