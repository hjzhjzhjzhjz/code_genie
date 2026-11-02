# -*- coding: utf-8 -*-
"""对比两版码表（`output-keymap.txt`）的字根→键位差异。

码表产物格式是 `字根<TAB>键位[<TAB>频次]`，前面还有若干 `#` 开头的统计注释；
csvdiff 要求带表头的标准 CSV，所以这里先剥注释、补表头、写临时文件，再交给它比较。

用法：
    python tools/对比码表.py 旧/output-keymap.txt 新/output-keymap.txt
    python tools/对比码表.py runs/s4-export runs/0.0220-0.1113   # 传目录则找 output-keymap.txt
    python tools/对比码表.py 旧.txt 新.txt --全部                  # 不截断明细
    python tools/对比码表.py 旧.txt 新.txt --仅摘要                # 只看计数

退出码：0 = 两版完全相同，1 = 有差异（可直接用于 CI 判定）。
"""

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

表头 = "字根\t键位"


def 定位码表(参数: str) -> Path:
    """参数既可以是码表文件本身，也可以是含 output-keymap.txt 的目录。"""
    路径 = Path(参数)
    if 路径.is_dir():
        候选 = 路径 / "output-keymap.txt"
        if not 候选.is_file():
            sys.exit(f"❌ 目录 {路径} 下没有 output-keymap.txt")
        return 候选
    if not 路径.is_file():
        sys.exit(f"❌ 找不到 {路径}")
    return 路径


def 读码表(路径: Path) -> list[tuple[str, str]]:
    """剥掉 # 注释行，取前两列。频次列不参与比较（键位变了频次本来也不变）。"""
    行们 = []
    for 行 in 路径.read_text(encoding="utf-8").splitlines():
        行 = 行.rstrip("\r\n")
        if not 行 or 行.startswith("#"):
            continue
        列 = 行.split("\t")
        if len(列) < 2:
            continue
        行们.append((列[0], 列[1]))
    if not 行们:
        sys.exit(f"❌ {路径} 里没有可比较的数据行")
    return 行们


def 写临时表(行们: list[tuple[str, str]], 目录: Path, 名字: str) -> Path:
    路径 = 目录 / 名字
    文本 = 表头 + "\n" + "".join(f"{根}\t{键}\n" for 根, 键 in 行们)
    路径.write_text(文本, encoding="utf-8", newline="\n")
    return 路径


def 调csvdiff(旧表: Path, 新表: Path) -> dict:
    """csvdiff 的 --sep 必须传真正的制表符，写成 "\\t" 会报
    `delimiter must be a 1-character string`（文档里那条命令就是这么错的）。"""
    结果 = subprocess.run(
        ["csvdiff", "--sep", "\t", "字根", str(旧表), str(新表)],
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if 结果.returncode not in (0, 1):
        sys.exit(f"❌ csvdiff 失败（{结果.returncode}）：\n{结果.stderr.strip()}")
    if not 结果.stdout.strip():
        return {"added": [], "removed": [], "changed": []}
    return json.loads(结果.stdout)


def 打印报告(差异: dict, 旧名: str, 新名: str, 上限: int, 仅摘要: bool) -> None:
    新增 = 差异.get("added", [])
    删除 = 差异.get("removed", [])
    改动 = 差异.get("changed", [])

    print(f"旧：{旧名}")
    print(f"新：{新名}")
    print()
    print(f"换键 {len(改动)} 个    新增 {len(新增)} 个    删除 {len(删除)} 个")
    print()

    if 仅摘要:
        return

    if 改动:
        打印 = 改动 if 上限 <= 0 else 改动[:上限]
        print(f"── 换键（{len(改动)} 个）──")
        for 项 in 打印:
            根 = " ".join(项.get("key", []))
            字段 = 项.get("fields", {})
            旧键 = 字段.get("键位", {}).get("from", "?")
            新键 = 字段.get("键位", {}).get("to", "?")
            print(f"  {根:<8} {旧键} → {新键}")
        if len(打印) < len(改动):
            print(f"  … 另有 {len(改动) - len(打印)} 个，用 --全部 查看")
        print()

    if 新增:
        打印 = 新增 if 上限 <= 0 else 新增[:上限]
        print(f"── 新增（{len(新增)} 个）──")
        for 项 in 打印:
            print(f"  {项.get('字根', '?'):<8} {项.get('键位', '?')}")
        if len(打印) < len(新增):
            print(f"  … 另有 {len(新增) - len(打印)} 个")
        print()

    if 删除:
        打印 = 删除 if 上限 <= 0 else 删除[:上限]
        print(f"── 删除（{len(删除)} 个）──")
        for 项 in 打印:
            print(f"  {项.get('字根', '?'):<8} {项.get('键位', '?')}")
        if len(打印) < len(删除):
            print(f"  … 另有 {len(删除) - len(打印)} 个")


def main() -> int:
    解析 = argparse.ArgumentParser(description="对比两版码表的字根键位差异")
    解析.add_argument("旧", help="旧码表文件，或含 output-keymap.txt 的目录")
    解析.add_argument("新", help="新码表文件，或含 output-keymap.txt 的目录")
    解析.add_argument("--全部", action="store_true", help="明细不截断")
    解析.add_argument("--仅摘要", action="store_true", help="只输出计数")
    解析.add_argument("--明细上限", type=int, default=30, help="每类最多列多少条（默认 30）")
    参数 = 解析.parse_args()

    旧路径 = 定位码表(参数.旧)
    新路径 = 定位码表(参数.新)
    旧行 = 读码表(旧路径)
    新行 = 读码表(新路径)

    with tempfile.TemporaryDirectory(prefix="码表对比-") as 临时:
        临时目录 = Path(临时)
        旧表 = 写临时表(旧行, 临时目录, "旧.tsv")
        新表 = 写临时表(新行, 临时目录, "新.tsv")
        差异 = 调csvdiff(旧表, 新表)

    上限 = 0 if 参数.全部 else 参数.明细上限
    打印报告(差异, str(旧路径), str(新路径), 上限, 参数.仅摘要)

    有无差异 = any(差异.get(k) for k in ("added", "removed", "changed"))
    return 1 if 有无差异 else 0


if __name__ == "__main__":
    sys.exit(main())
