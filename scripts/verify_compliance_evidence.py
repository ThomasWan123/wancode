# -*- coding: utf-8 -*-
"""#127-5 合规证据校验器（复核定案：生成时断言必须进仓库和 CI）。

默认（verify 模式，CI 运行）：只校验**已提交**的证据文件，不下载
即将过期的 artifact——
  1. 逐 suite：名称与情景数量（4a=7 / 4b=4）；
  2. 全部情景 pass；
  3. ci_sha == tested_main_sha；
  4. 冻结的内层摘要文件哈希 == 记录的 summary_file_sha256；
  5. artifact_id / artifact_name / archive_digest_sha256 三字段在册且
     digest 形如 64 位十六进制。

regenerate 模式（人工审计用，需 gh CLI 登录）：
  python scripts/verify_compliance_evidence.py regenerate <run_id>
  重新下载 artifact、比对官方 archive digest 与冻结文件哈希。
"""
import hashlib
import io
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EVIDENCE = os.path.join(ROOT, "docs", "evidence", "provider-compliance-v0.18.9.json")
EXPECTED_SCENARIOS = {"4a-transport-errors": 7, "4b-tools-multimodal": 4}


def fail(msg):
    print(f"EVIDENCE VERIFY FAIL: {msg}")
    sys.exit(1)


def sha256_file(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def verify():
    doc = json.load(io.open(EVIDENCE, encoding="utf-8"))
    main_sha = doc["tested_main_sha"]
    if not re.fullmatch(r"[0-9a-f]{40}", main_sha):
        fail(f"tested_main_sha 形状异常: {main_sha}")
    suites = doc["suites"]
    if set(suites) != set(EXPECTED_SCENARIOS):
        fail(f"suite 集合不符: {sorted(suites)}")
    for name, meta in suites.items():
        s = meta["summary"]
        if s["suite"] != name:
            fail(f"{name}: summary.suite 不符 ({s['suite']})")
        n = EXPECTED_SCENARIOS[name]
        if s["total"] != n or len(s["scenarios"]) != n:
            fail(f"{name}: 情景数量不符 total={s['total']} len={len(s['scenarios'])}")
        if not all(x["pass"] for x in s["scenarios"]):
            fail(f"{name}: 存在未 pass 情景")
        if s["ci_sha"] != main_sha:
            fail(f"{name}: ci_sha({s['ci_sha']}) != tested_main_sha")
        frozen = os.path.join(ROOT, "docs", "evidence", meta["summary_file"])
        got = sha256_file(frozen)
        if got != meta["summary_file_sha256"]:
            fail(f"{name}: 冻结文件哈希不符 got={got}")
        # 冻结文件内容与内嵌 summary 必须一致
        if json.load(io.open(frozen, encoding="utf-8")) != s:
            fail(f"{name}: 冻结文件内容与内嵌 summary 不一致")
        for field in ("artifact_id", "artifact_name", "archive_digest_sha256"):
            if not meta.get(field):
                fail(f"{name}: 缺 {field}")
        if not re.fullmatch(r"[0-9a-f]{64}", meta["archive_digest_sha256"]):
            fail(f"{name}: archive digest 形状异常")
        if meta["artifact_name"] != f"compliance-summary-{name.split('-')[0]}":
            fail(f"{name}: artifact_name 不符 ({meta['artifact_name']})")
    print("EVIDENCE VERIFY OK:", ", ".join(sorted(suites)))


def regenerate(run_id):
    import subprocess
    import tempfile

    api = subprocess.run(
        ["gh", "api", f"repos/ThomasWan123/wancode/actions/runs/{run_id}/artifacts"],
        capture_output=True, text=True, check=True,
    )
    arts = {a["name"]: a for a in json.loads(api.stdout)["artifacts"]}
    doc = json.load(io.open(EVIDENCE, encoding="utf-8"))
    with tempfile.TemporaryDirectory() as tmp:
        for name, meta in doc["suites"].items():
            art = arts[meta["artifact_name"]]
            official = art["digest"].removeprefix("sha256:")
            if official != meta["archive_digest_sha256"]:
                fail(f"{name}: 官方 archive digest 不符 ({official})")
            if art["id"] != meta["artifact_id"]:
                fail(f"{name}: artifact_id 不符 ({art['id']})")
            d = os.path.join(tmp, name)
            subprocess.run(
                ["gh", "run", "download", str(run_id), "-n", meta["artifact_name"], "-D", d],
                check=True,
            )
            inner = os.path.join(d, meta["summary_file"])
            if sha256_file(inner) != meta["summary_file_sha256"]:
                fail(f"{name}: 重下载内层文件哈希不符")
    print("EVIDENCE REGENERATE-CHECK OK")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "regenerate":
        regenerate(sys.argv[2])
    else:
        verify()
