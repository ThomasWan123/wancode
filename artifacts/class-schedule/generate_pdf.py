#!/usr/bin/env python3
"""Render artifacts/class-schedule/schedule.html to a landscape A4 PDF."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_HTML = HERE / "schedule.html"
DEFAULT_PDF = HERE / "2026学年第一学期-01五(2)班课表.pdf"


def chrome_bin() -> str:
    for name in (
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ):
        found = shutil.which(name)
        if found:
            return found
    raise FileNotFoundError("Chrome/Chromium is required to print the schedule PDF")


def generate(html: Path, pdf: Path) -> Path:
    html = html.resolve()
    pdf = pdf.resolve()
    if not html.is_file():
        raise FileNotFoundError(html)
    pdf.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        chrome_bin(),
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--no-pdf-header-footer",
        "--no-first-run",
        "--no-default-browser-check",
        f"--print-to-pdf={pdf}",
        html.as_uri(),
    ]
    subprocess.run(cmd, check=True, capture_output=True, text=True, timeout=90)
    if not pdf.is_file() or pdf.stat().st_size < 1024:
        raise RuntimeError(f"PDF was not written: {pdf}")
    return pdf


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--html", type=Path, default=DEFAULT_HTML)
    parser.add_argument("--pdf", type=Path, default=DEFAULT_PDF)
    args = parser.parse_args()
    out = generate(args.html, args.pdf)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
