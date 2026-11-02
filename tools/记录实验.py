#!/usr/bin/env python3
"""把一次 optimize 运行归档成一份实验档案。

每次调参跑完 200 万 / 4000 万步，参数、得分、曲线散落在日志和 config 里，
过几天就记不清「那次 6.58 是初始解还是优化出来的」。这个脚本把一次运行压成
一份 markdown：参数、结果、收敛曲线，加上留给人填的「假设」和「结论」。

用法::

    python tools/记录实验.py runs/xxx/optimize.log --标签 降初温
    python tools/记录实验.py runs/xxx/optimize.log -c moling/config.ron --标签 权重调整 --假设 "把重码权重提到 0.6 能否压到 6.5 以下"

产物：`experiments/<日期>-<时间>-<标签>.md` 与同名 `.png`（曲线）。
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from datetime import datetime
from pathlib import Path

根目录 = Path(__file__).resolve().parent.parent

# `[T0] 采样 step=5000 得分=6.589900 当前=12.345678 温度=0.001234`
采样行 = re.compile(
    r"采样\s+step=(\d+)\s+得分=([-\d.]+)\s+当前=([-\d.]+)\s+温度=([-\d.]+)"
)
# RON 里的 `键: 值,`（值是括号时只当层级，不是数据）
配置行 = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.*?),?\s*$")


def 读采样(文本: str) -> list[tuple[int, float, float, float]]:
    点 = []
    for 匹配 in 采样行.finditer(文本):
        步, 得分, 当前, 温度 = 匹配.groups()
        点.append((int(步), float(得分), float(当前), float(温度)))
    return 点


def 解析配置(路径: Path) -> dict[str, str]:
    """把嵌套的 RON 展平成 `a.b.c -> 值`。

    只认 `键: 值` 与本项目的 RON 写法，够用即可——这里不做通用 RON 解析，
    拿不到的参数留空，不影响档案可读。
    """
    结果: dict[str, str] = {}
    if not 路径.exists():
        return 结果
    层级: list[str] = []
    for 行 in 路径.read_text(encoding="utf-8").splitlines():
        内容 = 行.split("//")[0].rstrip()
        if not 内容.strip():
            continue
        去缩进 = 内容.strip()
        if 去缩进.startswith(")"):
            if 层级:
                层级.pop()
            continue
        匹配 = 配置行.match(内容)
        if not 匹配:
            continue
        键, 值 = 匹配.group(1), 匹配.group(2).strip()
        if 值 == "(" or 值.startswith("("):
            层级.append(键)
            continue
        值 = 值.rstrip(",").strip().strip('"')
        if 值 and 值 != ")" and not 值.startswith("//"):
            结果[".".join(层级 + [键])] = 值
    return 结果


def 取(配置: dict[str, str], 键: str, 缺省: str = "—") -> str:
    return 配置.get(键, 缺省)


def 找二进制() -> Path | None:
    for 候选 in (
        "target/x86_64-pc-windows-gnu/release/code_genie.exe",
        "target/release/code_genie.exe",
        "target/release/code_genie",
    ):
        路径 = 根目录 / 候选
        if 路径.exists():
            return 路径
    return None


def 出图(日志: Path, 输出: Path) -> tuple[bool, str]:
    程序 = 找二进制()
    if 程序 is None:
        return False, "找不到 code_genie 可执行文件，先 cargo build --release --features chart"
    结果 = subprocess.run(
        [str(程序), "plot", str(日志), "-o", str(输出), "--含当前"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if 结果.returncode != 0:
        return False, (结果.stderr or 结果.stdout).strip()
    return True, ""


def 相对项目(路径: Path) -> str:
    try:
        return str(路径.resolve().relative_to(根目录)).replace("\\", "/")
    except ValueError:
        return str(路径)


def 主() -> int:
    解析器 = argparse.ArgumentParser(
        description="把一次 optimize 运行归档成实验档案（参数 + 得分 + 收敛曲线）"
    )
    解析器.add_argument("日志", help="optimize 的日志文件")
    解析器.add_argument("-c", "--配置", default="moling/config.ron", help="配置文件（默认 moling/config.ron）")
    解析器.add_argument("--标签", help="档案标签；缺省取日志文件名")
    解析器.add_argument("--假设", default="", help="跑之前想验证什么，写进档案开头")
    解析器.add_argument("--输出", default="experiments", help="档案目录（默认 experiments）")
    解析器.add_argument("--no-png", action="store_true", help="不出曲线图")
    参数 = 解析器.parse_args()

    日志 = Path(参数.日志)
    if not 日志.exists():
        print(f"日志不存在：{日志}", file=sys.stderr)
        return 2
    文本 = 日志.read_text(encoding="utf-8", errors="replace")
    点 = 读采样(文本)
    if not 点:
        print(
            f"{日志} 里没有「采样 step=」行：日志需要来自带收敛采样输出的 optimize 运行",
            file=sys.stderr,
        )
        return 2

    标签 = 参数.标签 or 日志.stem
    标签 = re.sub(r"[\\/:*?\"<>|\s]+", "-", 标签).strip("-") or "实验"
    现在 = datetime.now()
    目录 = 根目录 / 参数.输出
    目录.mkdir(parents=True, exist_ok=True)
    基名 = f"{现在:%Y-%m-%d-%H%M}-{标签}"
    档案 = 目录 / f"{基名}.md"
    图片 = 目录 / f"{基名}.png"

    配置 = 解析配置(Path(参数.配置))

    首, 末 = 点[0], 点[-1]
    初始最优, 最终最优 = 首[1], 末[1]
    改进次数 = sum(1 for 前, 后 in zip(点, 点[1:]) if 后[1] < 前[1] - 1e-12)
    相对 = (初始最优 - 最终最优) / 初始最优 * 100 if 初始最优 else 0.0
    温度区间 = f"{首[3]:.4f} → {末[3]:.6f}"

    画好, 错误 = (False, "已跳过（--no-png）")
    if not 参数.no_png:
        画好, 错误 = 出图(日志, 图片)
        if not 画好:
            图片 = None  # type: ignore[assignment]

    行 = [
        f"# 实验 {现在:%Y-%m-%d %H:%M} · {标签}",
        "",
        "## 假设",
        "",
        参数.假设 if 参数.假设 else "<!-- 跑之前想验证什么，一句话 -->",
        "",
        "## 结论",
        "",
        "<!-- 跑完补一句：有效 / 无效 / 意外发现，下一步试什么 -->",
        "",
        "## 结果",
        "",
        "| 指标 | 值 |",
        "|---|---|",
        f"| 初始最优得分 | {初始最优:.4f} |",
        f"| 最终最优得分 | {最终最优:.4f} |",
        f"| 相对改进 | {相对:+.2f}% |",
        f"| 最优改进次数 | {改进次数} |",
        f"| 结束时当前得分 | {末[2]:.4f} |",
        f"| 温度 | {温度区间} |",
        f"| 采样点 | {len(点)} |",
        "",
        "## 参数",
        "",
        "| 项 | 值 |",
        "|---|---|",
        f"| 总步数 | {取(配置, 'annealing.total_steps')} |",
        f"| 线程数 | {取(配置, 'annealing.threads')} |",
        f"| 初温系数 | {取(配置, 'annealing.temp_start')} |",
        f"| 交换概率 | {取(配置, 'annealing.swap_probability')} |",
        f"| 舒适区 | 进度 {取(配置, 'annealing.comfort_progress')} / 宽度 {取(配置, 'annealing.comfort_width')} / 降速 {取(配置, 'annealing.comfort_slowdown')} |",
        f"| 权重·重码数 | {取(配置, 'weights.full_code.collision_count')} |",
        f"| 权重·重码率 | {取(配置, 'weights.full_code.collision_rate')} |",
        f"| 权重·当量 | {取(配置, 'weights.full_code.equivalence')} |",
        f"| 权重·CV | {取(配置, 'weights.full_code.equiv_cv')} |",
        f"| 权重·分布 | {取(配置, 'weights.full_code.distribution')} |",
        f"| 简码 | {'启用' if 取(配置, 'weights.simple_code.enabled') == 'true' else '关闭'} |",
        "",
        "## 曲线",
        "",
    ]
    if 画好 and 图片 is not None:
        行.append(f"![收敛曲线]({图片.name})")
    else:
        行.append(f"<!-- 未出图：{错误} -->")
    行 += [
        "",
        "## 出处",
        "",
        f"- 日志：`{相对项目(日志)}`",
        f"- 配置：`{相对项目(Path(参数.配置))}`",
        "",
    ]

    档案.write_text("\n".join(行), encoding="utf-8", newline="\n")

    print(f"✅ 已写出 {相对项目(档案)}")
    print(f"   最优 {初始最优:.4f} → {最终最优:.4f}（改进了 {改进次数} 次）")
    if 画好:
        print(f"   曲线 {相对项目(图片)}")
    else:
        print(f"   ⚠️ 没出图：{错误}")
    return 0


if __name__ == "__main__":
    sys.exit(主())
