// =========================================================================
// 🏃 `run` 子命令：一轮完整优化（prepare → optimize/resume）的外层事务
// =========================================================================
//
// 这是原 `moling/optimize.sh` 的 Rust 移植版（脚本已删除）。它不做优化，只负责
// 一圈「外围事务」，这些在 `optimize` 子命令里都没有对应实现：
//
//   1. 定输出目录：没给 `-d` 就用 `<方案>-output/output-<时间戳>`；
//   2. 写 `COMMENT.txt`：问一句备注，**追加**写入（resume 复用时保留历史）；
//   3. 备份输入到 `<OUT>/source/`（仅首次）：配置 + 产物 + 人工维护的输入 +
//      `拆分*.txt` + 素材里的字频 JSON。注意 `generate-root-chart.sh` 就把
//      `<OUT>/source` 当数据源（`SOURCE="$1/source"`），所以这份备份是活的依赖，
//      不是纯粹留档；
//   4. 落日志 `<OUT>/optimize-<时间戳>.log`：环境变量、git 版本、两条子命令的
//      完整输出；
//   5. 收尾把 `汇总.txt` 打出来（`optimize` 只写文件、不打印表格）。
//
// 与原脚本的差异：
//
// 1. 子命令用 `current_exe()` 自调用，不再靠 `$CODE_GENIE` 环境变量指路径，也
//    就不需要 `cargo build`——跑的就是当前这个二进制。
// 2. 子进程输出**同时**进日志和屏幕。脚本是全量重定向到日志、跑完才 `tail -n 32`，
//    长时间退火中途什么都看不见，失败时也只能看到尾巴。
// 3. 父进程装一个空转的 Ctrl-C 处理器：中断只交给子进程走「存 checkpoint 再退出」。
//    否则父进程先死、管道读端关闭，子进程下一次 `println!` 会因 EPIPE panic，
//    checkpoint 可能只写一半。
// 4. 备份挪到 `prepare` **之后**：脚本在之前备份，等于把上一轮的旧产物当成这一轮的
//    输入留档（数据源改过就对不上），而且全新检出的仓库里还没有 `拆分*.txt`，
//    bash 的 `failglob` 会直接把脚本打断。
// 5. `prepare` 显式带 `--schema`：脚本漏了这一项，`SCHEMA` 不是 moling 时会按默认
//    方案准备输入。
// 6. macOS 上自动套 `caffeinate -imsu` 防休眠（与脚本一致），`--no-caffeinate` 可关；
//    剖析（脚本里 `PROFILE=profiling` 套 `samply record`）不在此处支持——包在
//    `run` 外面采样到的是父进程，直接 `samply record code_genie optimize ...` 才有用。
// =========================================================================

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::prepare::{names, schema};

/// 命令行参数
pub struct 参数 {
    /// 工作目录：`prepare`/`optimize` 的 cwd，`config*.ron` 与产物都在这里
    pub 工作目录: String,
    /// 方案名，见 [`schema::取`]
    pub 方案: String,
    /// 上游素材目录；`None` 时交给 `prepare` 自己按 `yuhao-data/` 探测
    pub 素材目录: Option<String>,
    /// 输出目录；`None` 时用 `<方案>-output/output-<时间戳>`（相对工作目录）
    pub 输出目录: Option<String>,
    /// 断点续算（走 `resume` 而不是 `optimize`）
    pub 续算: bool,
    /// 备注；`None` 时交互式提问
    pub 备注: Option<String>,
    /// 跳过 `prepare`
    pub 跳过准备: bool,
    /// 只打印计划，不落盘也不跑
    pub 试运行: bool,
    /// 不套 `caffeinate`（macOS 默认套）
    pub 不防休眠: bool,
    /// 转给 `optimize --seed-dir`
    pub 播种目录: Option<String>,
    /// 转给 `optimize --seed-keymap`
    pub 播种键位: Option<String>,
    /// 转给 `optimize --scale`
    pub 尺度: Option<String>,
}

// =========================================================================
// 入口
// =========================================================================

pub fn run(参: &参数) -> Result<(), String> {
    let 方案 = schema::取(&参.方案)?;

    let 工作 = 绝对化(Path::new(&参.工作目录));
    if !工作.is_dir() {
        return Err(format!("工作目录不存在：{}", 工作.display()));
    }
    let 素材 = match &参.素材目录 {
        Some(p) => 绝对化(Path::new(p)),
        None => crate::data_dir::定位(&工作),
    };

    if 参.续算 && 参.输出目录.is_none() {
        return Err(
            "resume 必须用 -d 指定既有的 output 目录（不会自动新建一个空目录再续算）".to_string(),
        );
    }

    let 配置 = 选配置(&工作, 方案.名)?;
    let 时间戳 = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let 输出 = 参
        .输出目录
        .clone()
        .unwrap_or_else(|| 默认输出目录(方案.名, &时间戳));
    let 输出路径 = 绝对化(&工作.join(&输出));

    let 自身 = std::env::current_exe().map_err(|e| format!("找不到自身可执行文件：{e}"))?;
    let 准备任务 = 准备任务(&自身, 方案.名, &素材, 参.素材目录.is_some());
    let 优化任务 = 优化任务(&自身, &配置, &输出, 参);
    let 防休眠 =
        !参.不防休眠 && cfg!(target_os = "macos") && Path::new("/usr/bin/caffeinate").exists();

    // Ctrl-C 由子进程处理，父进程只维持管道（原因见文件头第 3 条）。
    let _ = ctrlc::set_handler(|| {});

    if 参.试运行 {
        打印计划(方案, &工作, &配置, &输出路径, &准备任务, &优化任务, 参);
        return Ok(());
    }

    std::fs::create_dir_all(&输出路径)
        .map_err(|e| format!("建不了输出目录 {}：{e}", 输出路径.display()))?;

    // 备注：脚本在备份与日志之前就要写完，中断了也能看到「这是想干什么」
    let 备注 = 取备注(参.备注.as_deref())?;
    追加备注(
        &输出路径.join("COMMENT.txt"),
        &format!("{} : {备注}", 优化任务.渲染()),
    )?;

    let 日志文件名 = format!("optimize-{时间戳}.log");
    let 日志 = 日志::新建(&输出路径.join(&日志文件名))?;

    日志.记(&format!(
        "=== 一轮完整优化（{} / {}）===",
        方案.名称, 方案.名
    ));
    日志.记(&format!(
        "  开始时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    日志.记(&format!("  工作目录: {}", 工作.display()));
    日志.记(&format!("  配置文件: {配置}"));
    日志.记(&format!("  输出目录: {}", 输出路径.display()));
    日志.记(&format!("  备注:     {备注}"));
    日志.记("");
    日志.记("检查优化相关环境变量：---->");
    for 行 in 环境快照() {
        日志.记(&行);
    }
    日志.记("<--------------------------");
    for 行 in git快照(&工作) {
        日志.记(&行);
    }
    日志.记("");

    if 参.跳过准备 {
        日志.记("跳过 prepare（--no-prepare）");
    } else {
        执行(&准备任务, &工作, &日志, false)?;
        日志.记("");
    }

    // 备份放在 prepare 之后：留的就是这一轮真正吃掉的那批输入（见文件头第 4 条）
    let 备份目录 = 输出路径.join("source");
    if 备份目录.is_dir() {
        日志.记(&format!(
            "备份目录 {} 已存在，跳过输入文件备份",
            备份目录.display()
        ));
    } else {
        let 清单 = 待备份清单(&工作, &素材, &配置, 方案.名);
        let 数 = 备份(&清单, &备份目录)?;
        日志.记(&format!("已备份 {数} 个输入文件到 {}/", 备份目录.display()));
    }
    日志.记("");

    执行(&优化任务, &工作, &日志, 防休眠)?;

    日志.记("");
    日志.记(&format!(
        "结束时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));

    // `optimize` 只把汇总写进 汇总.txt，不打印表格，这里替它打出来
    let 汇总 = 输出路径.join("汇总.txt");
    if 汇总.is_file() {
        let 文本 = std::fs::read_to_string(&汇总)
            .map_err(|e| format!("读 {} 失败：{e}", 汇总.display()))?;
        println!("\n=== {} ===", 汇总.display());
        print!("{文本}");
    }
    println!("\n日志: {}", 日志.路径.display());
    Ok(())
}

/// 试运行时把整份计划打印出来（不建目录、不写备注、不备份）
fn 打印计划(
    方案: &schema::方案,
    工作: &Path,
    配置: &str,
    输出: &Path,
    准备任务: &任务,
    优化任务: &任务,
    参: &参数,
) {
    println!(
        "=== 一轮完整优化（{} / {}）· 试运行 ===",
        方案.名称, 方案.名
    );
    println!("  工作目录: {}", 工作.display());
    println!("  配置文件: {配置}");
    println!("  输出目录: {}", 输出.display());
    if 参.跳过准备 {
        println!("  将跳过 prepare（--no-prepare）");
    } else {
        println!("  将运行: {}", 准备任务.渲染());
    }
    println!("  将运行: {}", 优化任务.渲染());
    println!("  将备份到: {}", 输出.join("source").display());
    println!("  备注将追加到: {}", 输出.join("COMMENT.txt").display());
}

// =========================================================================
// 目录与文件
// =========================================================================

/// 相对路径按工作目录解析；绝对路径原样返回
fn 绝对化(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `config-<方案>.ron` → `config-<方案>.toml` → `config.ron` → `config.toml` 的顺序找
fn 选配置(工作: &Path, 方案名: &str) -> Result<String, String> {
    // 同名优先 ron 再说 toml：老目录里 ron 是主格式，新机器上可能只剩 toml。
    let 候选 = [
        format!("config-{方案名}.ron"),
        format!("config-{方案名}.toml"),
        "config.ron".to_string(),
        "config.toml".to_string(),
    ];
    for 名 in &候选 {
        if 工作.join(名).is_file() {
            return Ok(名.clone());
        }
    }
    Err(format!(
        "{} 下找不到任何配置文件（试过 {:?}）",
        工作.display(),
        候选
    ))
}

/// 缺省输出目录：`<方案>-output/output-<时间戳>`
fn 默认输出目录(方案名: &str, 时间戳: &str) -> String {
    format!("{方案名}-output/output-{时间戳}")
}

/// 备注追加写入（脚本用 `>>`，resume 复用时保留历史；换行压成空格保持一行一条）
fn 追加备注(路径: &Path, 行: &str) -> Result<(), String> {
    let 单行: String = 行
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(路径)
        .map_err(|e| format!("写 {} 失败：{e}", 路径.display()))?;
    writeln!(f, "{}", 单行).map_err(|e| format!("写 {} 失败：{e}", 路径.display()))
}

/// 交互式取一句话备注；stdin 已关（非交互场景）时按空备注继续
fn 取备注(给定: Option<&str>) -> Result<String, String> {
    if let Some(s) = 给定 {
        return Ok(s.trim().to_string());
    }
    print!("一句话备注： ");
    let _ = std::io::stdout().flush();
    let mut 行 = String::new();
    match std::io::stdin().read_line(&mut 行) {
        Ok(0) => Ok(String::new()),
        Ok(_) => Ok(行.trim().to_string()),
        Err(e) => Err(format!("读备注失败：{e}")),
    }
}

/// 去掉 `.txt` 得到「方案可覆盖」的基名：`单字拆分.txt` → `单字拆分`
fn 基名(产物: &str) -> &str {
    产物.strip_suffix(".txt").unwrap_or(产物)
}

/// 有 `<基名>-<方案>.txt` 就用它，否则用 `<基名>.txt`
fn 方案文件(工作: &Path, 基: &str, 方案名: &str) -> PathBuf {
    let 专用 = 工作.join(format!("{基}-{方案名}.txt"));
    if 专用.is_file() {
        专用
    } else {
        工作.join(format!("{基}.txt"))
    }
}

/// 按「前缀 + 后缀」扫目录（脚本里的 `拆分*.txt`、`charAbsoluteFrequency*.json`）
fn 按模式扫(目录: &Path, 前缀: &str, 后缀: &str) -> Vec<PathBuf> {
    let Ok(条目) = std::fs::read_dir(目录) else {
        return Vec::new();
    };
    条目
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(前缀) && n.ends_with(后缀))
        })
        .collect()
}

/// 这次运行要留档的输入清单（只收真实存在的文件）
///
/// 对应脚本里那条 `cp 拆分*.txt … $files "$BAK/"`：产物按「方案覆盖优先」展开，
/// 人工维护的输入保持原名，`拆分*.txt` 与字频 JSON 按通配扫。
fn 待备份清单(工作: &Path, 素材: &Path, 配置: &str, 方案名: &str) -> Vec<PathBuf> {
    let mut 名单: Vec<PathBuf> = vec![工作.join(配置)];

    // 产物 + 人工维护的输入：后两项名字固定，前五项可能带 `-<方案>` 后缀
    // （聚类表已内嵌进二进制，不再需要留档）
    for 基 in [
        names::算码字集基名,
        names::全字频表基名,
        names::字根声韵基名,
        基名(names::单字拆分),
        基名(names::字根约束),
        "key_distribution",
        "pair_equivalence",
    ] {
        名单.push(方案文件(工作, 基, 方案名));
    }

    // 方案覆盖版聚类表（`roots-cluster-<方案>.ron` / `roots-cluster.ron`）若存在也留档
    for 名 in [
        format!("roots-cluster-{方案名}.ron"),
        "roots-cluster.ron".to_string(),
    ] {
        名单.push(工作.join(名));
    }

    // 脚本里写死在 `cp` 行上的几项
    for 名 in [names::算码字频, names::字根频率, names::字根读音] {
        名单.push(工作.join(名));
    }

    // 两条通配：拆分表（含 `拆分全表-台.txt`）与素材目录里的字频 JSON
    名单.extend(按模式扫(工作, "拆分", ".txt"));
    名单.extend(按模式扫(素材, "charAbsoluteFrequency", ".json"));

    名单.retain(|p| p.is_file());
    名单.sort();
    名单.dedup();
    名单
}

/// 把清单平铺拷进备份目录，返回成功条数
fn 备份(清单: &[PathBuf], 备份目录: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(备份目录)
        .map_err(|e| format!("建不了备份目录 {}：{e}", 备份目录.display()))?;
    let mut 成功 = 0usize;
    let mut 失败: Vec<String> = Vec::new();
    for 源 in 清单 {
        let Some(名) = 源.file_name() else {
            continue;
        };
        match std::fs::copy(源, 备份目录.join(名)) {
            Ok(_) => 成功 += 1,
            Err(e) => 失败.push(format!("{}（{e}）", 源.display())),
        }
    }
    if 失败.is_empty() {
        Ok(成功)
    } else {
        Err(format!(
            "{} 个文件备份失败：{}",
            失败.len(),
            失败.join("、")
        ))
    }
}

// =========================================================================
// 子命令任务
// =========================================================================

/// 一条待执行的子命令：程序 + 参数（防休眠的前缀留到真正 spawn 时再套）
struct 任务 {
    说明: &'static str,
    程序: PathBuf,
    参数: Vec<String>,
}

impl 任务 {
    /// 拼出真实命令；macOS 上按需在前面套 `caffeinate -imsu`
    fn 命令(&self, 工作: &Path, 防休眠: bool) -> Command {
        let mut 命令 = if 防休眠 {
            let mut c = Command::new("caffeinate");
            c.arg("-imsu").arg(&self.程序);
            c
        } else {
            Command::new(&self.程序)
        };
        命令.args(&self.参数).current_dir(工作);
        命令
    }

    /// 打印用的命令文本（不含量 `caffeinate`，便于人工复制）
    fn 渲染(&self) -> String {
        let 名 = self
            .程序
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "code_genie".to_string());
        let mut 文 = 名;
        for 参 in &self.参数 {
            文.push(' ');
            if 参.contains(' ') {
                文.push('"');
                文.push_str(参);
                文.push('"');
            } else {
                文.push_str(参);
            }
        }
        文
    }
}

/// `prepare --dir . --schema <方案>`（工作目录就是 cwd）
fn 准备任务(自身: &Path, 方案名: &str, 素材: &Path, 显式素材: bool) -> 任务 {
    let mut 参数 = vec![
        "prepare".to_string(),
        "--dir".to_string(),
        ".".to_string(),
        "--schema".to_string(),
        方案名.to_string(),
    ];
    if 显式素材 {
        参数.push("--data-dir".to_string());
        参数.push(素材.display().to_string());
    }
    任务 {
        说明: "准备输入",
        程序: 自身.to_path_buf(),
        参数,
    }
}

/// `-c <配置> (optimize|resume) -d <输出> [种子参数]`
fn 优化任务(自身: &Path, 配置: &str, 输出: &str, 参: &参数) -> 任务 {
    let mut 参数 = vec!["-c".to_string(), 配置.to_string()];
    参数.push(if 参.续算 { "resume" } else { "optimize" }.to_string());
    参数.push("-d".to_string());
    参数.push(输出.to_string());

    // resume 不接受种子/尺度参数，给了也只会被忽略，索性不传
    if !参.续算 {
        for (旗, 值) in [
            ("--seed-dir", &参.播种目录),
            ("--seed-keymap", &参.播种键位),
            ("--scale", &参.尺度),
        ] {
            if let Some(v) = 值 {
                参数.push(旗.to_string());
                参数.push(v.clone());
            }
        }
    }

    任务 {
        说明: if 参.续算 { "断点续算" } else { "优化" },
        程序: 自身.to_path_buf(),
        参数,
    }
}

// =========================================================================
// 执行 + 日志
// =========================================================================

/// 日志文件；`记` 同时写屏幕与文件，`写` 只写文件
struct 日志 {
    路径: PathBuf,
    文件: Arc<Mutex<File>>,
}

impl 日志 {
    fn 新建(路径: &Path) -> Result<Self, String> {
        if let Some(父) = 路径.parent() {
            std::fs::create_dir_all(父)
                .map_err(|e| format!("建不了 {} 的上级目录：{e}", 路径.display()))?;
        }
        let 文件 = OpenOptions::new()
            .create(true)
            .append(true)
            .open(路径)
            .map_err(|e| format!("建不了日志 {}：{e}", 路径.display()))?;
        Ok(Self {
            路径: 路径.to_path_buf(),
            文件: Arc::new(Mutex::new(文件)),
        })
    }

    fn 记(&self, 行: &str) {
        println!("{行}");
        self.写(行);
    }

    fn 写(&self, 行: &str) {
        if let Ok(mut f) = self.文件.lock() {
            let _ = writeln!(f, "{行}");
        }
    }
}

/// 跑一条子命令：stdout/stderr 同时转发到屏幕与日志
fn 执行(任务: &任务, 工作: &Path, 日志: &日志, 防休眠: bool) -> Result<(), String> {
    日志.记(&format!("运行: {}", 任务.渲染()));

    let mut 子 = 任务
        .命令(工作, 防休眠)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动{}失败：{e}", 任务.说明))?;

    let 出口 = 子.stdout.take().expect("stdout 已 piped");
    let 错口 = 子.stderr.take().expect("stderr 已 piped");

    let 出线程 = {
        let 文件 = Arc::clone(&日志.文件);
        std::thread::spawn(move || 转发(出口, false, 文件))
    };
    let 错线程 = {
        let 文件 = Arc::clone(&日志.文件);
        std::thread::spawn(move || 转发(错口, true, 文件))
    };

    let 状态 = 子
        .wait()
        .map_err(|e| format!("等待{}结束失败：{e}", 任务.说明))?;
    let _ = 出线程.join();
    let _ = 错线程.join();

    if 状态.success() {
        Ok(())
    } else {
        let 码 = 状态
            .code()
            .map_or_else(|| "未知".to_string(), |c| c.to_string());
        Err(format!(
            "{}失败（退出码 {码}），完整日志: {}",
            任务.说明,
            日志.路径.display()
        ))
    }
}

/// 按块转发：保留 `\r` 进度、不按行切（避免半截行卡住）
fn 转发<R: Read>(mut 源: R, 错误流: bool, 日志: Arc<Mutex<File>>) {
    let mut 块 = [0u8; 8192];
    loop {
        let n = match 源.read(&mut 块) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let 片 = &块[..n];
        let 写结果 = if 错误流 {
            let mut 出 = std::io::stderr().lock();
            出.write_all(片).and_then(|_| 出.flush())
        } else {
            let mut 出 = std::io::stdout().lock();
            出.write_all(片).and_then(|_| 出.flush())
        };
        if 写结果.is_err() {
            // 屏幕那头没了（比如被 `| head` 截断）也继续写日志，别丢记录
        }
        if let Ok(mut f) = 日志.lock() {
            let _ = f.write_all(片);
            let _ = f.flush();
        }
    }
}

/// 脚本里 `env | grep -E 'SCHEM|USE_|OPTIMIZE_|ENABLE_|DISABLE_|NO_'` 的等价物
fn 环境快照() -> Vec<String> {
    const 前缀: [&str; 6] = ["SCHEM", "USE_", "OPTIMIZE_", "ENABLE_", "DISABLE_", "NO_"];
    let mut 行: Vec<String> = std::env::vars()
        .filter(|(k, _)| 前缀.iter().any(|p| k.starts_with(p)))
        .map(|(k, v)| format!("  {k}={v}"))
        .collect();
    行.sort();
    行
}

/// 仓库里才记 git 版本与工作区状态（脚本里是 `[ -e ../.git ]`）
fn git快照(工作: &Path) -> Vec<String> {
    let 有git =
        工作.join(".git").exists() || 工作.parent().is_some_and(|父| 父.join(".git").exists());
    if !有git {
        return Vec::new();
    }
    let mut 行 = Vec::new();
    if let Some(描述) = 跑命令(工作, "git", &["describe", "--always", "--dirty", "--tags"]) {
        行.push(format!("GIT version: {描述}"));
    }
    if let Some(状态) = 跑命令(工作, "git", &["status", "--short", "--branch"]) {
        行.push(状态);
    }
    行
}

fn 跑命令(工作: &Path, 程序: &str, 参数: &[&str]) -> Option<String> {
    let 出 = Command::new(程序)
        .current_dir(工作)
        .args(参数)
        .output()
        .ok()?;
    if !出.status.success() {
        return None;
    }
    let 文 = String::from_utf8_lossy(&出.stdout).trim_end().to_string();
    Some(
        文.lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// =========================================================================
// 🧪 测试
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn 临时目录(标签: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cg-run-{标签}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn 参数样例() -> 参数 {
        参数 {
            工作目录: "moling".to_string(),
            方案: "moling".to_string(),
            素材目录: None,
            输出目录: Some("out".to_string()),
            续算: false,
            备注: Some("测一下".to_string()),
            跳过准备: false,
            试运行: false,
            不防休眠: true,
            播种目录: None,
            播种键位: None,
            尺度: None,
        }
    }

    #[test]
    fn 缺省输出目录带方案前缀与时间戳() {
        assert_eq!(
            默认输出目录("moling", "20261102-121000"),
            "moling-output/output-20261102-121000"
        );
    }

    #[test]
    fn 配置优先方案专用文件() {
        let 目录 = 临时目录("cfg");
        std::fs::write(目录.join("config.ron"), "a").unwrap();
        assert_eq!(选配置(&目录, "yustar").unwrap(), "config.ron");

        std::fs::write(目录.join("config-yustar.ron"), "b").unwrap();
        assert_eq!(选配置(&目录, "yustar").unwrap(), "config-yustar.ron");
        std::fs::remove_dir_all(&目录).ok();
    }

    #[test]
    fn 配置都不存在时报错() {
        let 目录 = 临时目录("nocfg");
        let 错 = 选配置(&目录, "moling").unwrap_err();
        assert!(错.contains("config.ron"), "{错}");
        std::fs::remove_dir_all(&目录).ok();
    }

    /// 备注是 `>>`：多次调用逐行累积，resume 复用同一目录时不会冲掉历史
    #[test]
    fn 备注追加不覆盖且保持单行() {
        let 目录 = 临时目录("comment");
        let 路径 = 目录.join("COMMENT.txt");
        追加备注(&路径, "第一条 : 基线").unwrap();
        追加备注(&路径, "第二条 : 换行\n要压平").unwrap();
        let 文本 = std::fs::read_to_string(&路径).unwrap();
        let 行: Vec<&str> = 文本.lines().collect();
        assert_eq!(行, vec!["第一条 : 基线", "第二条 : 换行 要压平"]);
        std::fs::remove_dir_all(&目录).ok();
    }

    /// 清单：`-<方案>` 后缀优先、`拆分*.txt` 通配、素材目录的字频 JSON，且只收存在的
    #[test]
    fn 备份清单按方案覆盖并过滤缺失() {
        let 根 = 临时目录("list");
        let 工作 = 根.join("moling");
        let 素材 = 根.join("yuhao-data");
        std::fs::create_dir_all(&工作).unwrap();
        std::fs::create_dir_all(&素材).unwrap();

        std::fs::write(工作.join("config.ron"), "").unwrap();
        std::fs::write(工作.join("字根声韵.txt"), "默认").unwrap();
        std::fs::write(工作.join("字根声韵-yustar.txt"), "专用").unwrap();
        for 名 in ["拆分表.txt", "拆分全表.txt", "拆分全表-台.txt"] {
            std::fs::write(工作.join(名), "").unwrap();
        }
        // 聚类表默认内嵌，只有方案覆盖版才需要留档
        std::fs::write(工作.join("roots-cluster-yustar.ron"), "").unwrap();
        std::fs::write(素材.join("charAbsoluteFrequencySC.json"), "{}").unwrap();

        let 清单 = 待备份清单(&工作, &素材, "config.ron", "yustar");
        let 名们: Vec<String> = 清单
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(名们.contains(&"字根声韵-yustar.txt".to_string()));
        assert!(
            !名们.contains(&"字根声韵.txt".to_string()),
            "专用文件应顶掉默认名"
        );
        for 名 in ["拆分表.txt", "拆分全表.txt", "拆分全表-台.txt"] {
            assert!(名们.contains(&名.to_string()), "缺 {名}: {名们:?}");
        }
        assert!(名们.contains(&"charAbsoluteFrequencySC.json".to_string()));
        assert!(
            名们.contains(&"roots-cluster-yustar.ron".to_string()),
            "方案覆盖版聚类表应留档: {名们:?}"
        );
        assert!(
            !名们.contains(&"算码字频.txt".to_string()),
            "不存在的文件不该进清单"
        );
        std::fs::remove_dir_all(&根).ok();
    }

    #[test]
    fn 备份把清单平铺拷进目标目录() {
        let 目录 = 临时目录("copy");
        let 源目录 = 目录.join("src");
        std::fs::create_dir_all(&源目录).unwrap();
        let a = 源目录.join("a.txt");
        let b = 源目录.join("b.txt");
        std::fs::write(&a, "甲").unwrap();
        std::fs::write(&b, "乙").unwrap();

        let 目标 = 目录.join("out/source");
        let 数 = 备份(&[a, b], &目标).unwrap();
        assert_eq!(数, 2);
        assert_eq!(std::fs::read_to_string(目标.join("a.txt")).unwrap(), "甲");
        assert_eq!(std::fs::read_to_string(目标.join("b.txt")).unwrap(), "乙");
        std::fs::remove_dir_all(&目录).ok();
    }

    #[test]
    fn 优化命令带上配置输出与种子() {
        let 自身 = Path::new("/tmp/code_genie");
        let mut 参 = 参数样例();
        参.播种目录 = Some("old".to_string());
        let 文 = 优化任务(自身, "config.ron", "out-1", &参).渲染();
        assert_eq!(
            文,
            "code_genie -c config.ron optimize -d out-1 --seed-dir old"
        );
    }

    /// resume 不吃种子/尺度，拼进去只会让参数看着像生效了
    #[test]
    fn 续算命令不带种子与尺度() {
        let 自身 = Path::new("/tmp/code_genie");
        let mut 参 = 参数样例();
        参.续算 = true;
        参.播种目录 = Some("old".to_string());
        参.尺度 = Some("尺度.json".to_string());
        let 文 = 优化任务(自身, "config.ron", "out-1", &参).渲染();
        assert_eq!(文, "code_genie -c config.ron resume -d out-1");
    }

    #[test]
    fn 准备命令显式带方案() {
        let 自身 = Path::new("/tmp/code_genie");
        let 准 = 准备任务(自身, "yuling", Path::new("/d/yuhao-data"), false);
        assert_eq!(准.渲染(), "code_genie prepare --dir . --schema yuling");

        let 准 = 准备任务(自身, "yuling", Path::new("/d/yuhao-data"), true);
        assert_eq!(
            准.渲染(),
            "code_genie prepare --dir . --schema yuling --data-dir /d/yuhao-data"
        );
    }
}
