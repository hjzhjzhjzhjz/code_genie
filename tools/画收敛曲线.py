# -*- coding: utf-8 -*-
"""把 `optimize` 日志里的收敛采样画成 SVG 曲线。

`annealing.rs` 每 1% 步数输出一行：

       [T0] 采样 step=5000 得分=0.987654 当前=1.234567 温度=0.001234

本脚本解析这些行，画出「历史最优得分」曲线。零依赖，只生成 SVG，
浏览器 / Word / Markdown 都能直接用。

为什么默认不画「当前得分」：退火高温期的当前得分远高于最优（动辄差 2~5 倍），
两条线放同一根 y 轴会把最优曲线压成一条贴底的直线。要看当前得分的震荡，
加 `--含当前`，脚本会改成左右双轴。

用法：
    python tools/画收敛曲线.py runs/xxx/optimize.log
    python tools/画收敛曲线.py runs/xxx/optimize.log -o 收敛.svg
    python tools/画收敛曲线.py runs/xxx/optimize.log --含当前
    python tools/画收敛曲线.py a.log b.log -o 对比.svg     # 多轮同图对比（只画最优）
"""

import argparse
import re
import sys
from pathlib import Path

采样行 = re.compile(
    r"采样\s+step=(\d+)\s+得分=([0-9.]+)\s+当前=([0-9.]+)\s+温度=([0-9.eE+-]+)"
)

宽, 高 = 960, 520
左, 右, 上, 下 = 84, 84, 58, 58


def 解析(路径: Path) -> list[tuple[int, float, float, float]]:
    点们 = []
    for 行 in 路径.read_text(encoding="utf-8", errors="replace").splitlines():
        m = 采样行.search(行)
        if not m:
            continue
        try:
            点们.append((int(m[1]), float(m[2]), float(m[3]), float(m[4])))
        except ValueError:
            continue
    return 点们


def 刻度(最小: float, 最大: float, 份数: int = 5) -> list[float]:
    if 最大 - 最小 < 1e-12:
        余量 = abs(最大) * 0.01 or 0.01
        最小 -= 余量
        最大 += 余量
    return [最小 + (最大 - 最小) * i / 份数 for i in range(份数 + 1)]


def 折线(点们, 映射x, 映射y) -> str:
    return " ".join(f"{映射x(x):.1f},{映射y(y):.1f}" for x, y in 点们)


def 生成svg(
    序列们: list[tuple[str, list[tuple[int, float, float, float]]]],
    含当前: bool,
) -> str:
    全部x = [p[0] for _, 点们 in 序列们 for p in 点们]
    x最小, x最大 = min(全部x), max(全部x)
    if x最大 == x最小:
        x最大 = x最小 + 1

    best最小 = min(p[1] for _, 点们 in 序列们 for p in 点们)
    best最大 = max(p[1] for _, 点们 in 序列们 for p in 点们)
    best余 = (best最大 - best最小) * 0.08 or abs(best最大) * 0.02 or 0.01
    best最小 -= best余
    best最大 += best余

    图宽 = 宽 - 左 - 右
    图高 = 高 - 上 - 下

    def 映射x(v: float) -> float:
        return 左 + (v - x最小) / (x最大 - x最小) * 图宽

    def 映射y(v: float) -> float:
        return 上 + 图高 - (v - best最小) / (best最大 - best最小) * 图高

    def 映射y右(v: float) -> float:
        return 上 + 图高 - (v - 当前最小) / (当前最大 - 当前最小) * 图高

    片段 = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{宽}" height="{高}" '
        f'viewBox="0 0 {宽} {高}" font-family="Microsoft YaHei, Segoe UI, sans-serif">',
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{左}" y="30" font-size="15" font-weight="600" fill="#1f2328">'
        f"退火收敛曲线</text>",
        f'<text x="{左}" y="48" font-size="11" fill="#6e7781">'
        f"{len(全部x)} 个采样点 · 横轴步数 · 纵轴目标得分（越小越好）</text>",
    ]

    # 左轴（最优得分）网格与刻度
    for 值 in 刻度(best最小, best最大):
        y = 映射y(值)
        片段.append(
            f'<line x1="{左}" y1="{y:.1f}" x2="{左 + 图宽}" y2="{y:.1f}" '
            f'stroke="#eaeef2" stroke-width="1"/>'
        )
        片段.append(
            f'<text x="{左 - 10}" y="{y + 4:.1f}" font-size="11" fill="#6e7781" '
            f'text-anchor="end">{值:.4f}</text>'
        )

    # x 刻度
    for i in range(6):
        值 = x最小 + (x最大 - x最小) * i / 5
        x = 映射x(值)
        片段.append(
            f'<line x1="{x:.1f}" y1="{上 + 图高}" x2="{x:.1f}" y2="{上 + 图高 + 4}" '
            f'stroke="#d0d7de"/>'
        )
        片段.append(
            f'<text x="{x:.1f}" y="{上 + 图高 + 20}" font-size="11" fill="#6e7781" '
            f'text-anchor="middle">{int(值)}</text>'
        )

    片段.append(f'<line x1="{左}" y1="{上}" x2="{左}" y2="{上 + 图高}" stroke="#d0d7de"/>')
    片段.append(
        f'<line x1="{左}" y1="{上 + 图高}" x2="{左 + 图宽}" y2="{上 + 图高}" stroke="#d0d7de"/>'
    )

    配色 = ["#0969da", "#bf3989", "#1a7f37", "#9a6700", "#8250df"]
    图例 = []
    for i, (名字, 点们) in enumerate(序列们):
        颜色 = 配色[i % len(配色)]
        片段.append(
            f'<polyline fill="none" stroke="{颜色}" stroke-width="2" '
            f'stroke-linejoin="round" points="{折线([(p[0], p[1]) for p in 点们], 映射x, 映射y)}"/>'
        )
        图例.append(f'<tspan fill="{颜色}">━</tspan> {名字}')

    # 右轴（当前得分）——仅单日志且开启时画，多轮对比画当前会糊成一团
    if 含当前 and len(序列们) == 1:
        当前们 = [(p[0], p[2]) for p in 序列们[0][1]]
        当前最小 = min(v for _, v in 当前们)
        当前最大 = max(v for _, v in 当前们)
        余 = (当前最大 - 当前最小) * 0.08 or abs(当前最大) * 0.02 or 0.01
        当前最小 -= 余
        当前最大 += 余
        片段.append(
            f'<polyline fill="none" stroke="#8c959f" stroke-width="1" opacity="0.55" '
            f'points="{折线(当前们, 映射x, 映射y右)}"/>'
        )
        片段.append(
            f'<line x1="{左 + 图宽}" y1="{上}" x2="{左 + 图宽}" y2="{上 + 图高}" stroke="#d0d7de"/>'
        )
        for 值 in 刻度(当前最小, 当前最大):
            y = 映射y右(值)
            片段.append(
                f'<text x="{左 + 图宽 + 10}" y="{y + 4:.1f}" font-size="11" fill="#8c959f">{值:.2f}</text>'
            )
        图例.append('<tspan fill="#8c959f">━</tspan> 当前得分（右轴）')

    片段.append(
        f'<text x="{左 + 图宽}" y="30" font-size="11" fill="#1f2328" '
        f'text-anchor="end">{"　".join(图例)}</text>'
    )
    片段.append("</svg>")
    return "\n".join(片段)


def main() -> int:
    解析器 = argparse.ArgumentParser(description="把 optimize 日志的收敛采样画成 SVG")
    解析器.add_argument("日志", nargs="+", help="一个或多个 optimize 日志")
    解析器.add_argument("-o", "--输出", default="", help="输出 SVG 路径（默认与首个日志同目录）")
    解析器.add_argument(
        "--含当前", action="store_true", help="叠加「当前得分」细线（右轴，仅支持单条日志）"
    )
    参数 = 解析器.parse_args()

    序列们 = []
    for 名字 in 参数.日志:
        路径 = Path(名字)
        if not 路径.is_file():
            sys.exit(f"❌ 找不到 {路径}")
        点们 = 解析(路径)
        if not 点们:
            sys.exit(
                f"❌ {路径} 里没有「采样 step=… 得分=…」行。\n"
                "   旧版二进制不输出该行，请用当前代码重跑一次 optimize。"
            )
        序列们.append((路径.stem, 点们))

    if 参数.含当前 and len(序列们) > 1:
        print("⚠️  多日志对比时忽略 --含当前（当前得分曲线会互相干扰）")

    svg = 生成svg(序列们, 参数.含当前)

    输出 = Path(参数.输出) if 参数.输出 else Path(参数.日志[0]).with_name(
        f"{Path(参数.日志[0]).stem}-收敛.svg"
    )
    输出.write_text(svg, encoding="utf-8", newline="\n")

    for 名字, 点们 in 序列们:
        print(
            f"{名字}: {len(点们)} 点 · 步数 {点们[0][0]} → {点们[-1][0]} · "
            f"最优 {点们[0][1]:.4f} → {点们[-1][1]:.4f}"
        )
    print(f"✅ 已写出 {输出}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
