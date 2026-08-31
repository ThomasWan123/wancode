#!/usr/bin/env python3
"""Check the committed timetable PDF is landscape A4 and contains every cell."""

from __future__ import annotations

import sys
from pathlib import Path

from pypdf import PdfReader

PDF = Path(__file__).resolve().parent / "2026学年第一学期-01五(2)班课表.pdf"

REQUIRED = [
    "2026学年第一学期课程表01 五(2)班 课表",
    "星期一",
    "星期二",
    "星期三",
    "星期四",
    "星期五",
    "音乐",
    "顾丽玲:音乐室",
    "外语",
    "徐静",
    "语文",
    "陈悦安",
    "美术",
    "胡炜",
    "体育与健康",
    "王杰:操场",
    "数学",
    "陈惠娜",
    "科学",
    "顾思艺:科学室",
    "体育与健康2",
    "潘嘉巍",
    "悦读时光",
    "信息科技",
    "俞欣辰:电脑房",
    "静小宝@大未来",
    "道德与法治",
    "吴一君",
    "劳动",
    "陈文渊",
    "一",
    "二",
    "三",
    "四",
    "五",
    "六",
    "七",
    "八",
]


def main() -> int:
    if not PDF.is_file():
        print(f"missing {PDF}", file=sys.stderr)
        return 1
    reader = PdfReader(str(PDF))
    if len(reader.pages) != 1:
        print(f"expected 1 page, got {len(reader.pages)}", file=sys.stderr)
        return 1
    box = reader.pages[0].mediabox
    width, height = float(box.width), float(box.height)
    # A4 landscape is 841.89 x 595.28 pt; Chrome rounds slightly.
    if width < 830 or height < 580 or width <= height:
        print(f"expected A4 landscape, got {width} x {height}", file=sys.stderr)
        return 1
    text = reader.pages[0].extract_text() or ""
    missing = [item for item in REQUIRED if item not in text]
    if missing:
        print("missing strings:", missing, file=sys.stderr)
        return 1
    print(f"ok pages=1 size={width:.1f}x{height:.1f}pt strings={len(REQUIRED)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
