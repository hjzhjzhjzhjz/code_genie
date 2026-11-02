#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""检查红线.py —— 按 diff 范围执行 CONSTRAINTS.md 的底线规则

只检查「这次改动新增/删除的行」，不管存量。存量里的 23 处 allow、281 处
unwrap 一律不报，报的是「本次改动把门槛降低了」这类动作。

用法:
    python tools/检查红线.py                      # 与上游跟踪分支比较
    python tools/检查红线.py --base origin/moling
    python tools/检查红线.py --base HEAD          # 只看未提交改动
    python tools/检查红线.py --警告               # 只报告，不拦截（退出码恒 0）
    python tools/检查红线.py --豁免 测试减少:src/evaluator.rs

退出码: 0 干净 / 1 有违规 / 2 无法运行（绝不能把 2 当成 0）
"""

import argparse
import fnmatch
import os
import re
import subprocess
import sys
from pathlib import Path

退出_干净 = 0
退出_违规 = 1
退出_无法运行 = 2


# ---------------------------------------------------------------- git 基础

def git(*参数):
    """执行 git 命令，返回 CompletedProcess（统一 utf-8、关掉路径转义）"""
    return subprocess.run(
        ["git", "-c", "core.quotepath=false", *参数],
        capture_output=True, text=True, encoding="utf-8", errors="replace",
    )


def 失败(消息):
    print(f"检查红线: {消息}", file=sys.stderr)
    sys.exit(退出_无法运行)


def 去前缀(路径):
    """'b/src/foo.rs' -> 'src/foo.rs'，顺带剥掉 git 的引号和制表符时间戳"""
    路径 = 路径.strip()
    if 路径.startswith('"') and 路径.endswith('"'):
        路径 = 路径[1:-1]
    路径 = 路径.split("\t")[0]
    if 路径.startswith(("a/", "b/")):
        路径 = 路径[2:]
    return 路径


# ---------------------------------------------------------------- 差异采集

class 文件改动:
    def __init__(self):
        self.加 = []      # [(新行号, 文本)]
        self.删 = []      # [(旧行号, 文本)]
        self.已删除 = False


块首 = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def 解析差异(文本):
    """解析 unified diff，按文件归集增删行（带行号）"""
    结果 = {}
    当前 = None
    旧名 = ""
    旧号 = 新号 = 0
    在块内 = False  # --- / +++ 只在 diff 头和第一个 @@ 之间是文件头

    for 行 in 文本.splitlines():
        if 行.startswith("diff --git"):
            当前 = None
            在块内 = False
            continue
        if 行.startswith("--- "):
            旧名 = 去前缀(行[4:])
            continue
        if 行.startswith("+++ "):
            新名 = 去前缀(行[4:])
            名 = 旧名 if 新名 == "/dev/null" else 新名
            当前 = 结果.setdefault(名, 文件改动())
            if 新名 == "/dev/null":
                当前.已删除 = True
            continue
        m = 块首.match(行)
        if m:
            旧号, 新号 = int(m.group(1)), int(m.group(3))
            在块内 = True
            continue
        if not 在块内 or 当前 is None:
            continue
        if 行.startswith("+"):
            当前.加.append((新号, 行[1:]))
            新号 += 1
        elif 行.startswith("-"):
            当前.删.append((旧号, 行[1:]))
            旧号 += 1
        elif 行.startswith(" "):
            旧号 += 1
            新号 += 1
        elif 行.startswith("\\"):  # "\ No newline at end of file"
            continue
    return 结果


def 收集改动(基准提交):
    差异 = git("diff", "--unified=0", "--no-color", 基准提交, "--")
    if 差异.returncode != 0:
        失败(f"无法与 {基准提交} 做 diff：{差异.stderr.strip()[:200]}")
    结果 = 解析差异(差异.stdout)

    # 新增文件：git diff 看不见未跟踪文件，直接读盘，整份算「新增行」
    清单 = git("ls-files", "--others", "--exclude-standard")
    if 清单.returncode != 0:
        失败(f"无法列出未跟踪文件：{清单.stderr.strip()[:200]}")
    for 名 in [x for x in 清单.stdout.splitlines() if x.strip()]:
        路径 = Path(名)
        try:
            if 路径.stat().st_size > 4 * 1024 * 1024:
                continue
            文本 = 路径.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue  # 二进制或读不到，跳过
        项 = 结果.setdefault(名, 文件改动())
        项.加 = [(i, 行) for i, 行 in enumerate(文本.splitlines(), start=1)]
    return 结果


# ---------------------------------------------------------------- 规则定义

# 新增行里出现的「降低门槛」动作
匹配_测试跳过 = re.compile(r"#\s*\[\s*ignore\b|#\s*\[\s*cfg_attr\([^)]*\bignore\b")
匹配_crate抑制 = re.compile(r"#!\[\s*(allow|expect)\b")
匹配_局部抑制 = re.compile(r"#\[(allow|expect)\(([^\]]*)\)")
匹配_待办 = re.compile(r"\btodo!\(|\bunimplemented!\(")
匹配_待办注释 = re.compile(r"\b(TODO|FIXME|XXX)\b")
匹配_断言 = re.compile(
    r"\b(assert!|assert_eq!|assert_ne!|debug_assert!|debug_assert_eq!|"
    r"debug_assert_ne!|prop_assert\w*!|assert_matches!)")
匹配_测试属性 = re.compile(r"#\s*\[\s*(test|rstest|test_case)\b")
匹配_豁免行 = re.compile(r"^\|\s*(W|E)\d+\s*\|")

# CONSTRAINTS.md 里的阈值方向词：最小值被调小 / 最大值被调大 都算放宽
最小前 = re.compile(r"(>=|>|≥|不少于|至少|不低于|不得低于|不得下降|不得减少|不得变慢)\s*$")
最大前 = re.compile(r"(<=|<|≤|不超过|至多|不高于|不得高于|不得增长|不得增加)\s*$")
最小后 = re.compile(r"^\s*\S*\s*(或更多|以上|起|不得下降|不得减少)")
最大后 = re.compile(r"^\s*\S*\s*(或更少|以内|以下|不得增长|不得增加)")


def 是测试文件(路径):
    return (路径.startswith("tests/") or "/tests/" in 路径
            or bool(re.search(r"(^|/)test_[^/]*\.rs$", 路径))
            or 路径.endswith("_test.rs"))


def 阈值(文本):
    """抽出文本里的数字及其方向（min/max/None）"""
    结果 = []
    for m in re.finditer(r"\d+(?:\.\d+)?", 文本):
        前 = 文本[max(0, m.start() - 24):m.start()].lower()
        后 = 文本[m.end():m.end() + 40].lower()
        if 最小前.search(前) or 最小后.search(后):
            方向 = "min"
        elif 最大前.search(前) or 最大后.search(后):
            方向 = "max"
        else:
            方向 = None
        结果.append((float(m.group()), 方向))
    return 结果


def 规则键(文本):
    """表格行取第一格；列表项取冒号前的文字。标题/散句不算规则"""
    s = 文本.strip()
    if s.startswith("|"):
        格 = [x.strip() for x in s.split("|") if x.strip()]
        return 格[0] if 格 else None
    if s.startswith(("- ", "* ")):
        return s[2:].split(":")[0].split("：")[0].strip()
    return None


# ---------------------------------------------------------------- 主流程

已知规则 = (
    "测试跳过", "crate级抑制", "无理由抑制", "未完成代码", "断言减少", "测试减少",
    "测试文件删除", "测试随文件删除", "规则被删", "阈值放宽", "阈值变动", "阈值被删",
    "新增豁免",
)


def 载入豁免清单():
    """`.constraintsignore`：一行一条，行尾 `#` 后写原因。

        tools/检查红线.py            # 只豁免「新增行」类规则（抑制/待办/跳过）
        测试减少:src/zigen.rs        # 精确豁免某条规则

    这份清单本身进 diff，评审时看得见——这就是它和「悄悄关掉检查」的区别。
    """
    路径 = Path(".constraintsignore")
    if not 路径.exists():
        return {}
    表 = {}
    for 行 in 路径.read_text(encoding="utf-8").splitlines():
        行 = 行.split("#", 1)[0].strip()
        if not 行:
            continue
        规, 通配 = "*", 行
        if ":" in 行 and 行.split(":", 1)[0].strip() in 已知规则:
            规, 通配 = 行.split(":", 1)
            规, 通配 = 规.strip(), 通配.strip()
        if 通配:
            表.setdefault(规, set()).add(通配)
    return 表


def 在豁免清单(路径, 规则, 清单):
    通配们 = 清单.get("*", set()) | 清单.get(规则, set())
    return any(fnmatch.fnmatch(路径, 模式) or fnmatch.fnmatch("/" + 路径, 模式)
               for 模式 in 通配们)


def 主函数():
    ap = argparse.ArgumentParser(description="按 diff 范围执行 CONSTRAINTS.md 的底线规则")
    ap.add_argument("--base", help="比较基准（分支、标签或提交）")
    ap.add_argument("--警告", action="store_true", help="只报告不拦截，退出码恒 0")
    ap.add_argument("--豁免", action="append", default=[], metavar="规则:路径",
                    help="一次性豁免，可重复：--豁免 测试减少:src/foo.rs")
    args = ap.parse_args()

    顶 = git("rev-parse", "--show-toplevel")
    if 顶.returncode != 0:
        失败("不在 git 工作区内")
    根 = Path(顶.stdout.strip())
    参考基准 = args.base
    if 参考基准 is None:
        上游 = git("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}")
        参考基准 = 上游.stdout.strip() if 上游.returncode == 0 and 上游.stdout.strip() else "HEAD"

    基准 = git("rev-parse", "--verify", "--quiet", 参考基准 + "^{commit}")
    if 基准.returncode != 0:
        失败(f"基准 {参考基准} 不是有效提交")
    基准提交 = 基准.stdout.strip()

    基点 = git("merge-base", 基准提交, "HEAD")
    if 基点.returncode != 0 or not 基点.stdout.strip():
        失败(f"与 {参考基准} 没有共同祖先（浅克隆？换一个 --base）")
    基点提交 = 基点.stdout.strip()

    os.chdir(根)
    改动表 = 收集改动(基点提交)
    豁免清单 = 载入豁免清单()
    一次性 = {}
    for 项 in args.豁免:
        if ":" in 项:
            规, 路 = 项.split(":", 1)
            一次性.setdefault(规.strip(), set()).add(路.strip())

    发现 = []

    def 记(规则, 路径, 行号, 文本, 说明=""):
        if 路径 in 一次性.get(规则, set()):
            return
        if 在豁免清单(路径, 规则, 豁免清单):
            return
        发现.append((规则, 路径, 行号, 文本.strip()[:100], 说明))

    文件内容缓存 = {}

    def 取行(路径):
        if 路径 not in 文件内容缓存:
            try:
                文件内容缓存[路径] = (根 / 路径).read_text(encoding="utf-8").splitlines()
            except (OSError, UnicodeDecodeError):
                文件内容缓存[路径] = []
        return 文件内容缓存[路径]

    for 路径, 项 in sorted(改动表.items()):
        扫新增 = not 路径.endswith((".md", ".txt", ".ron", ".yml", ".yaml", ".lock"))

        if 扫新增:
            for 行号, 文本 in 项.加:
                if not 文本.strip():
                    continue
                if 匹配_测试跳过.search(文本):
                    记("测试跳过", 路径, 行号, 文本,
                        "新增 #[ignore] 等于把测试关掉；要么修，要么在 CONSTRAINTS.md 开豁免")
                if 匹配_crate抑制.search(文本):
                    记("crate级抑制", 路径, 行号, 文本,
                        "#![allow] 会关掉整个 crate 的该 lint，不接受新增")
                for m in 匹配_局部抑制.finditer(文本):
                    同行 = 文本[m.end():]
                    if "//" in 同行 and len(同行.split("//", 1)[1].strip()) >= 4:
                        continue
                    上文 = 取行(路径)
                    if 0 <= 行号 - 2 < len(上文) and 上文[行号 - 2].strip().startswith("//"):
                        continue
                    记("无理由抑制", 路径, 行号, 文本,
                        "#[allow]/#[expect] 必须在同行或上一行写明为什么，否则等于静默关检查")
                if 匹配_待办.search(文本):
                    记("未完成代码", 路径, 行号, 文本, "todo!/unimplemented! 不能进主干")
                if 匹配_待办注释.search(文本) and 文本.lstrip().startswith("//"):
                    记("未完成代码", 路径, 行号, 文本, "TODO/FIXME 注释不能进主干，写进提交信息")

        # 断言与测试数量：删掉的比新增的多才算「测试被削弱」
        if 路径.endswith(".rs"):
            删断言 = sum(1 for _, t in 项.删 if 匹配_断言.search(t))
            加断言 = sum(1 for _, t in 项.加 if 匹配_断言.search(t))
            if 删断言 > 加断言:
                记("断言减少", 路径, 0, f"删除 {删断言} 行断言，新增 {加断言} 行",
                    "测试里拿掉断言等于让测试永远通过")
            删测试 = sum(1 for _, t in 项.删 if 匹配_测试属性.search(t))
            加测试 = sum(1 for _, t in 项.加 if 匹配_测试属性.search(t))
            if 删测试 > 加测试:
                记("测试减少", 路径, 0, f"删除 {删测试} 个 #[test]，新增 {加测试} 个",
                    "删测试要么补回来，要么在提交信息里说明理由并显式豁免")

        # 整份文件被删：先看里面有没有测试
        if 项.已删除 and 是测试文件(路径):
            记("测试文件删除", 路径, 0, "整份测试文件被删除", "")
        elif 项.已删除 and 路径.endswith(".rs"):
            旧 = git("show", f"{基点提交}:{路径}")
            if 旧.returncode == 0:
                个数 = len(匹配_测试属性.findall(旧.stdout))
                if 个数:
                    记("测试随文件删除", 路径, 0, f"删除的文件里原有 {个数} 个测试", "")

        # CONSTRAINTS.md：只查「放宽」，收紧保持安静
        if 路径.endswith("CONSTRAINTS.md"):
            旧规则 = [(规则键(t), t) for _, t in 项.删 if 规则键(t)]
            新规则 = [(规则键(t), t) for _, t in 项.加 if 规则键(t)]
            for 键, 旧文 in 旧规则:
                新文 = next((t for k, t in 新规则 if k == 键), None)
                if 新文 is None:
                    if not 匹配_豁免行.search(旧文.strip()):
                        记("规则被删", 路径, 0, 旧文, "删规则等于永久降低门槛；改用豁免表")
                    continue
                旧阈, 新阈 = 阈值(旧文), 阈值(新文)
                裁决 = None
                for 方向 in ("min", "max", None):
                    旧组 = [n for n, d in 旧阈 if d == 方向]
                    新组 = [n for n, d in 新阈 if d == 方向]
                    for i, 前值 in enumerate(旧组):
                        if 裁决:
                            break
                        if i >= len(新组):
                            裁决 = "阈值被删"
                        elif 新组[i] == 前值:
                            continue
                        elif 方向 == "min" and 新组[i] < 前值:
                            裁决 = "阈值放宽"
                        elif 方向 == "max" and 新组[i] > 前值:
                            裁决 = "阈值放宽"
                        elif 方向 is None:
                            裁决 = "阈值变动"
                if 裁决:
                    记(裁决, 路径, 0, f"{旧文.strip()}  →  {新文.strip()}", "")
            for _, 文本 in 项.加:
                if 匹配_豁免行.search(文本.strip()):
                    记("新增豁免", 路径, 0, 文本, "豁免要有负责人和到期日，并在提交信息里说明")

    扫描 = len(改动表)
    print(f"检查红线: 基线 {参考基准} ({基准提交[:8]})，涉及 {扫描} 个文件")

    if not 发现:
        print("检查红线: 干净")
        sys.exit(退出_干净)

    在线 = bool(os.environ.get("GITHUB_ACTIONS"))
    print(f"检查红线: {len(发现)} 处违规", file=sys.stderr)
    按规则 = {}
    for 规则, 路径, 行号, 文本, 说明 in 发现:
        按规则.setdefault(规则, []).append((路径, 行号, 文本, 说明))
        if 在线:
            位置 = f" file={路径}" + (f",line={行号}" if 行号 else "")
            print(f"::error{位置}::[{规则}] {文本}", file=sys.stderr)

    for 规则, 条目 in 按规则.items():
        print(f"\n[{规则}] {len(条目)} 处", file=sys.stderr)
        for 路径, 行号, 文本, 说明 in 条目[:20]:
            位置 = f"{路径}:{行号}" if 行号 else 路径
            print(f"  {位置}  {文本}", file=sys.stderr)
            if 说明:
                print(f"      → {说明}", file=sys.stderr)
        if len(条目) > 20:
            print(f"  …… 另有 {len(条目) - 20} 处", file=sys.stderr)

    print("\n这些都是「把门槛降低」的动作。正解是改代码；确实要放行，走 CONSTRAINTS.md 的"
          "豁免表（要有负责人和到期日），或用 --豁免 规则:路径 做一次性放行。", file=sys.stderr)
    sys.exit(退出_干净 if args.警告 else 退出_违规)


if __name__ == "__main__":
    主函数()
