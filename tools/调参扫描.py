#!/usr/bin/env python3
"""用 Optuna / nevergrad / SALib 给退火参数做黑箱调参与敏感性分析。

设计边界（Rust 侧零改动）：
  - 本脚本是**外围离线工具**，不进 Rust crate，也不成为 code_genie 的运行时依赖。
  - 每个 trial 生成一份临时 TOML（覆写 [annealing] 参数 + 把 [files] 改成绝对路径），
    通过子进程调 release 版 code_genie.exe，从 stdout / 汇总.txt 解析综合得分。
  - 得分越低越好（save_summary 按得分升序排，第一行即最优线程）。
  - 一次 optimize 会写完整码表并复用输出目录，故每个 trial 用独立临时目录，跑完即删。

三个模式各管一件事：
  试跑       只跑一组默认参数，用来验证接口与估时
  optuna     TPE + 本地 SQLite，支持中断续跑（load_if_exists）
  nevergrad  离散/连续混合空间，默认 TwoPointsDE（NGOpt 在本环境会卡在内部 COBYLA）
  salib      Sobol 全局敏感性分析，回答「哪个参数真的影响得分」

用法::

    python tools/调参扫描.py 试跑 --配置 dlrk/config-星陈-V5繁混30-50k.toml
    python tools/调参扫描.py optuna --配置 dlrk/xxx.toml --试验数 12
    python tools/调参扫描.py nevergrad --配置 dlrk/xxx.toml --试验数 12
    python tools/调参扫描.py salib --配置 dlrk/xxx.toml --样本数 2 --重复 2

强烈建议加 ``--初始解 <某次 output/thread-00/字根键位表.txt>``：
退火主循环的 RNG 取自系统熵，不固定初始解的话，同一组参数两次运行也会有差异，
调参容易变成拟合噪声；SALib 对噪声尤其敏感，建议 ``--重复 2`` 取中位数。

依赖全部本地、零 API key / 零外部服务::

    python -m pip install optuna nevergrad SALib
"""
from __future__ import annotations

import argparse
import csv
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

项目根 = Path(__file__).resolve().parent.parent

# (参数名, 下界, 上界, 是否对数, 默认值)
# temp_end_ratio 是脚本内部派生量：temp_end = temp_start * ratio，天然满足
# config::validate_annealing 的「temp_end <= temp_start」约束。
参数空间 = [
    ("temp_start", 1e-4, 0.05, True, 0.002),
    ("temp_end_ratio", 1e-4, 0.5, True, 0.005),
    ("comfort_progress", 0.2, 0.8, False, 0.5),
    ("comfort_width", 0.05, 0.3, False, 0.15),
    ("comfort_slowdown", 0.2, 0.9, False, 0.6),
    ("swap_probability", 0.1, 0.6, False, 0.35),
]
参数名们 = [项[0] for 项 in 参数空间]
默认值组 = {项[0]: 项[4] for 项 in 参数空间}

# 简码功能已整体移除，这些段在 Rust 侧会按未知字段忽略；生成配置时顺手剥掉，
# 免得将来 config.rs 改成 deny_unknown_fields 时整批 trial 直接失败。
简码段 = {
    "weights.simple_code",
    "targets.simple_code",
    "fixed_simple_codes",
    "simple_levels",
    "simple_level",
}
文件键 = {"fixed", "splits", "key_dist", "pair_equiv"}


def 格式化值(值) -> str:
    if isinstance(值, bool):
        return "true" if 值 else "false"
    if isinstance(值, int):
        return str(值)
    return repr(float(值))


def 生成配置(基础文本: str, 参数: dict, 步数: int, 线程数: int, 配置目录: Path) -> str:
    """在基础 TOML 上覆写退火参数，其余段落逐行原样保留（含注释）。"""
    覆写 = dict(参数)
    覆写["temp_start"] = 参数["temp_start"]
    覆写["temp_end"] = 参数["temp_start"] * 参数["temp_end_ratio"]
    覆写["total_steps"] = 步数
    覆写["threads"] = 线程数
    覆写.pop("temp_end_ratio", None)

    结果: list[str] = []
    当前段 = ""
    跳过 = False
    已写入: set[str] = set()
    退火段末: int | None = None

    for 行 in 基础文本.splitlines():
        去空白 = 行.strip()
        if 去空白.startswith("["):
            段匹配 = re.match(r"\[+\s*([^\]]+)\]", 去空白)
            当前段 = 段匹配.group(1).strip().lower() if 段匹配 else ""
            跳过 = 当前段 in 简码段 or 当前段.startswith("simple")
            if 跳过:
                continue
            结果.append(行)
            continue
        if 跳过:
            continue
        键匹配 = re.match(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$", 行)
        if 键匹配:
            键, 原值 = 键匹配.group(1), 键匹配.group(2)
            if 当前段 == "files" and 键 in 文件键:
                文本值 = 原值.split("#")[0].strip().strip('"').strip("'")
                if 文本值:
                    候选 = 配置目录 / 文本值
                    if not 候选.exists() and (项目根 / 文本值).exists():
                        候选 = 项目根 / 文本值
                    结果.append(f'{键} = "{候选.resolve().as_posix()}"')
                else:
                    结果.append(行)
                continue
            if 当前段 == "annealing":
                if 键 in 覆写:
                    结果.append(f"{键} = {格式化值(覆写[键])}")
                    已写入.add(键)
                    continue
                if 键.startswith("simple_"):
                    continue
        结果.append(行)
        # 记录 [annealing] 段最后一个非注释、非空行的位置，供追加缺失键用
        if 当前段 == "annealing" and 去空白 and not 去空白.startswith("#"):
            退火段末 = len(结果) - 1

    缺失 = [键 for 键 in 覆写 if 键 not in 已写入]
    if 缺失:
        新增 = [f"{键} = {格式化值(覆写[键])}" for 键 in 缺失]
        if 退火段末 is None:
            结果.append("[annealing]")
            结果.extend(新增)
        else:
            结果[退火段末 + 1 : 退火段末 + 1] = 新增
    return "\n".join(结果) + "\n"


def 解析得分(日志文本: str, 汇总路径: Path) -> float | None:
    """优先取 stdout 的「综合得分」，失败再退回汇总.txt 表格首行。"""
    命中 = re.findall(r"综合得分[:：]\s*([0-9]*\.?[0-9]+(?:[eE][-+]?\d+)?)", 日志文本)
    if 命中:
        return float(命中[-1])
    try:
        for 行 in 汇总路径.read_text(encoding="utf-8").splitlines():
            if re.match(r"^T\d+\s", 行):
                return float(行.split()[1])
    except (OSError, ValueError, IndexError):
        pass
    return None


def 找二进制(指定: str | None) -> Path:
    if 指定:
        路径 = Path(指定).resolve()
        if not 路径.exists():
            raise SystemExit(f"找不到二进制：{路径}")
        return 路径
    根 = 项目根 / "target"
    候选 = sorted(根.glob("*/release/code_genie.exe"))
    候选 += sorted(根.glob("*/release/code_genie"))
    候选 += sorted(根.glob("release/code_genie.exe"))
    候选 += sorted(根.glob("release/code_genie"))
    if not 候选:
        raise SystemExit("未找到 release 二进制：先跑 cargo build --release，或用 --二进制 指定路径")
    return 候选[0].resolve()


def 可优化得分(值: float) -> float:
    """nevergrad / SALib 不接受 inf，失败样本统一给一个足够大的罚分。"""
    return 1e6 if 值 == float("inf") else 值


class 运行环境:
    """一次扫描的公共上下文：配置模板、二进制、CSV 记录与子进程调用。"""

    def __init__(self, 参):
        self.基础配置 = Path(参.配置).resolve()
        if not self.基础配置.exists():
            raise SystemExit(f"找不到基础配置：{self.基础配置}")
        self.基础文本 = self.基础配置.read_text(encoding="utf-8")
        self.配置目录 = self.基础配置.parent
        self.二进制 = 找二进制(参.二进制)
        self.结果目录 = Path(参.输出).resolve()
        self.结果目录.mkdir(parents=True, exist_ok=True)
        self.日志目录 = self.结果目录 / "日志"
        self.日志目录.mkdir(exist_ok=True)
        self.初始解 = Path(参.初始解).resolve() if 参.初始解 else None
        if self.初始解 and not self.初始解.exists():
            raise SystemExit(f"找不到初始解文件：{self.初始解}")
        self.步数 = 参.步数
        self.线程 = 参.线程
        self.重复 = 参.重复
        self.超时 = 参.超时
        self.序号 = 0
        self.csv文件 = open(
            self.结果目录 / "trials.csv", "a", newline="", encoding="utf-8-sig"
        )
        self.写手 = csv.DictWriter(
            self.csv文件,
            fieldnames=["序号", "标签", "重复", "得分", "耗时秒"] + 参数名们 + ["返回码"],
        )
        if self.csv文件.tell() == 0:
            self.写手.writeheader()
            self.csv文件.flush()

    def 关闭(self):
        self.csv文件.close()

    def 评估(self, 参数: dict, 标签: str) -> float:
        """同一组参数重复跑若干次取中位数，压一压退火 RNG 带来的噪声。"""
        return statistics.median(
            self.跑一次(参数, 标签, 次) for 次 in range(self.重复)
        )

    def 跑一次(self, 参数: dict, 标签: str, 次: int) -> float:
        self.序号 += 1
        序号 = self.序号
        配置文本 = 生成配置(
            self.基础文本, 参数, self.步数, self.线程, self.配置目录
        )
        with tempfile.TemporaryDirectory(prefix="调参_") as 临时:
            临时目录 = Path(临时)
            配置路径 = 临时目录 / "config.toml"
            配置路径.write_text(配置文本, encoding="utf-8")
            输出目录 = 临时目录 / "out"
            命令 = [
                str(self.二进制),
                "-c",
                str(配置路径),
                "optimize",
                "-d",
                str(输出目录),
            ]
            if self.初始解:
                命令 += ["--seed-keymap", str(self.初始解)]
            开始 = time.monotonic()
            try:
                完成 = subprocess.run(
                    命令,
                    cwd=str(self.配置目录),
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    timeout=self.超时,
                )
                日志 = (完成.stdout or "") + "\n" + (完成.stderr or "")
                返回码 = 完成.returncode
            except subprocess.TimeoutExpired:
                日志 = f"⏱️ 单次运行超过 {self.超时} 秒，已终止"
                返回码 = -1
            耗时 = time.monotonic() - 开始
            (self.日志目录 / f"{序号:03d}-{标签}-{次}.log").write_text(
                日志, encoding="utf-8"
            )
            得分 = 解析得分(日志, 输出目录 / "汇总.txt")
            if 得分 is None:
                print(f"  [{序号:03d}] 失败（返回码 {返回码}），日志末 8 行：", file=sys.stderr)
                for 行 in 日志.strip().splitlines()[-8:]:
                    print("    " + 行, file=sys.stderr)
                print(f"  [{序号:03d}] 生成的配置已存至 {self.日志目录 / f'{序号:03d}-config.toml'}", file=sys.stderr)
                (self.日志目录 / f"{序号:03d}-config.toml").write_text(
                    配置文本, encoding="utf-8"
                )
                得分 = float("inf")
            print(f"  [{序号:03d}] {标签} 得分={得分:.4f} 耗时={耗时:.1f}s", flush=True)
            行数据 = {
                "序号": 序号,
                "标签": 标签,
                "重复": 次,
                "得分": "inf" if 得分 == float("inf") else f"{得分:.6f}",
                "耗时秒": f"{耗时:.2f}",
                "返回码": 返回码,
            }
            行数据.update({名: 参数[名] for 名 in 参数名们})
            self.写手.writerow(行数据)
            self.csv文件.flush()
            return 得分


def 跑_试跑(环境: 运行环境):
    得分 = 环境.评估(默认值组, "试跑")
    print(f"\n✅ 试跑得分 {得分:.4f}（越低越好）")
    print(f"   明细见 {环境.结果目录 / 'trials.csv'}")


def 跑_optuna(环境: 运行环境, 参):
    import optuna

    optuna.logging.set_verbosity(optuna.logging.WARNING)
    数据库 = (环境.结果目录 / "optuna.db").as_posix()
    study = optuna.create_study(
        study_name=参.研究名,
        storage=f"sqlite:///{数据库}",
        load_if_exists=True,
        direction="minimize",
        sampler=optuna.samplers.TPESampler(seed=参.随机种子),
    )

    def 目标(trial):
        参数 = {
            名: trial.suggest_float(名, 下, 上, log=对数)
            for 名, 下, 上, 对数, _ in 参数空间
        }
        return 可优化得分(环境.评估(参数, f"optuna-{trial.number}"))

    study.optimize(目标, n_trials=参.试验数, n_jobs=1)
    print(f"\n✅ Optuna 完成：共 {len(study.trials)} 个 trial，最优得分 {study.best_value:.4f}")
    for 名, 值 in study.best_params.items():
        print(f"   {名:<20} = {值:.6g}")
    print(f"   断点续跑：重复同一条命令即可（study 在 {数据库}）")


def 跑_nevergrad(环境: 运行环境, 参):
    import nevergrad as ng

    参数化 = ng.p.Dict(
        **{
            名: (ng.p.Log(lower=下, upper=上) if 对数 else ng.p.Scalar(lower=下, upper=上))
            for 名, 下, 上, 对数, _ in 参数空间
        }
    )
    if 参.优化器 not in ng.optimizers.registry:
        raise SystemExit(f"未知优化器：{参.优化器}（可用 TwoPointsDE / DE / RandomSearch 等）")
    优化器 = ng.optimizers.registry[参.优化器](
        parametrization=参数化, budget=参.试验数
    )
    for i in range(参.试验数):
        候选 = 优化器.ask()
        参数 = dict(候选.value)
        优化器.tell(候选, 可优化得分(环境.评估(参数, f"nevergrad-{i}")))
    推荐 = 优化器.provide_recommendation()
    print(f"\n✅ nevergrad ({参.优化器}) 完成：{参.试验数} 次评估")
    for 名, 值 in 推荐.value.items():
        print(f"   {名:<20} = {值:.6g}")


def 跑_salib(环境: 运行环境, 参):
    import numpy as np
    from SALib.analyze import sobol as 分析
    from SALib.sample import sobol as 采样

    # 在 [0,1] 的归一参数上采样，再单调映射到实际区间：不依赖 SALib 的 dists
    # 名称差异，且单调变换不改变 Sobol 指数的相对排序。
    问题 = {
        "num_vars": len(参数空间),
        "names": 参数名们,
        "bounds": [[0.0, 1.0] for _ in 参数空间],
    }
    X = 采样.sample(
        问题, 参.样本数, calc_second_order=False, scramble=True, seed=参.随机种子
    )
    print(f"Sobol 采样 {len(X)} 点，每点重复 {环境.重复} 次")
    Y = []
    for i, 单位点 in enumerate(X):
        参数 = {}
        for (名, 下, 上, 对数, _), u in zip(参数空间, 单位点):
            参数[名] = 下 * (上 / 下) ** u if 对数 else 下 + u * (上 - 下)
        Y.append(可优化得分(环境.评估(参数, f"salib-{i}")))
    Si = 分析.analyze(问题, np.asarray(Y, dtype=float), calc_second_order=False)
    排序 = sorted(
        zip(参数名们, Si["S1"], Si["ST"], Si["S1_conf"], Si["ST_conf"]),
        key=lambda 项: -abs(项[2]),
    )
    print("\n参数                 一阶 S1          总效应 ST   （按 |ST| 降序）")
    for 名, s1, st, c1, ct in 排序:
        print(f"  {名:<20} {s1:+.3f} ± {c1:.3f}    {st:+.3f} ± {ct:.3f}")
    不可靠 = [名 for 名, _, _, c1, ct in 排序 if c1 > 0.5 or ct > 0.5]
    if 不可靠:
        print("\n⚠️  本次样本量不足，结论不可解读：置信区间过宽的参数 —— " + "、".join(不可靠))
        print("   Sobol 指数理论上 S1 ∈ [0,1]、ST ≥ 0；出现负值或大于 1 都是估计噪声。")
        print(f"   建议 --样本数 ≥ 16（当前 {参.样本数}）、--重复 ≥ 2（当前 {环境.重复}）；")
        print("   总评估次数 = 样本数 × (参数数 + 2) × 重复数。")
    print("\n退火 RNG 取自系统熵，噪声会混进交互项；即使给了 --初始解 也只固定了起点。")
    print("正式跑之前先用「试跑」估单次耗时，再按上面的公式算总成本。")


def 建解析器():
    解析 = argparse.ArgumentParser(
        description="用 Optuna / nevergrad / SALib 给退火参数做黑箱调参与敏感性分析",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    子们 = 解析.add_subparsers(dest="模式", required=True)

    def 加公共(子):
        子.add_argument("--配置", required=True, help="基础 TOML 配置（脚本只覆写 [annealing] 参数）")
        子.add_argument("--步数", type=int, default=2000, help="每次 trial 的 total_steps（默认 2000）")
        子.add_argument("--线程", type=int, default=1, help="每次 trial 的线程数（默认 1，控制变量）")
        子.add_argument("--重复", type=int, default=1, help="每组参数重复次数，取中位数（默认 1）")
        子.add_argument("--超时", type=int, default=600, help="单次运行超时秒数（默认 600）")
        子.add_argument("--初始解", help="固定起点的字根键位表.txt，强烈建议提供")
        子.add_argument("--输出", default=str(项目根 / "调参结果"), help="结果目录")
        子.add_argument("--二进制", help="code_genie.exe 路径；缺省自动在 target 下找 release 版")
        子.add_argument("--随机种子", type=int, default=20261107)

    for 名字 in ("试跑", "optuna", "nevergrad", "salib"):
        子 = 子们.add_parser(名字)
        加公共(子)
        if 名字 in ("optuna", "nevergrad"):
            子.add_argument("--试验数", type=int, default=12)
        if 名字 == "optuna":
            子.add_argument("--研究名", default="退火调参", help="SQLite study 名，同名字可续跑")
        if 名字 == "nevergrad":
            子.add_argument("--优化器", default="TwoPointsDE", help="默认 TwoPointsDE（NGOpt 在本环境会卡住）")
        if 名字 == "salib":
            子.add_argument("--样本数", type=int, default=2, help="Sobol 基数 N，总评估数 N×(参数数+2)")
    return 解析


def main():
    参 = 建解析器().parse_args()
    环境 = 运行环境(参)
    print(f"二进制: {环境.二进制}")
    print(f"配置:   {环境.基础配置}")
    print(f"步数={环境.步数}  线程={环境.线程}  重复={环境.重复}  初始解={环境.初始解 or '随机'}")
    print(f"结果:   {环境.结果目录}")
    try:
        if 参.模式 == "试跑":
            跑_试跑(环境)
        elif 参.模式 == "optuna":
            跑_optuna(环境, 参)
        elif 参.模式 == "nevergrad":
            跑_nevergrad(环境, 参)
        else:
            跑_salib(环境, 参)
    finally:
        环境.关闭()


if __name__ == "__main__":
    main()
