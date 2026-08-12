//! C1 Cowork 逃逸探针 spike —— 客户端路径限制谓词的真机边界测试。
//!
//! 设计 §2.1 的核心问题:worktree 只隔离 Git,不隔离文件系统。客户端在
//! G26 零引擎改动下唯一可能的隔离杠杆 = 在放行 shell 写之前,用一个路径
//! 限制谓词 `is_confined(root, target)` 判定目标是否在 worktree 内。
//!
//! 本 spike 在**真实文件系统**上测该谓词对**四个必需逃逸向量**的拦截力:
//!   ①绝对路径写宿主 ②`..` 相对路径逃逸 ③symlink/junction 指向宿主
//!   ④已存在的末段链接(codex R3-F1)
//! 外加一个 worktree 内合法写的**正对照**(证明谓词不是"全拒")。
//!
//! 判据(设计 §2.1):
//! 本 spike **不产出档位裁定**(codex R2-F1):它只测谓词能否识别逃逸,不驱动
//! 真实引擎会话、无真实 tool_call、不观测拒绝。C1/C2 档位由保留的 full-MvpAgent
//! 门决定(用户裁定 2026-08-12:保留门)。
//!
//! 诚实边界:这测的是"客户端谓词能不能识别逃逸",不是"引擎会不会在放行
//! 前调用它"。后者需完整 MvpAgent 会话(C1 第二阶段,provider_compliance 级)。
//! 本 spike 先钉死杠杆本身的能力上限 —— 这决定档 A 是否**理论可达**。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 客户端路径限制谓词:target 规范化后是否仍在 root 内。
/// **仅检测,不能关闭 check→write 的 TOCTOU 窗口**(codex R3)。
///
/// codex R3-F1 修正:若 target **本身已存在**(可能是指向宿主的文件符号链接),
/// 必须 canonicalize target 自身(解开末段链接);否则只解父目录会漏掉
/// "末段是文件 symlink"这一逃逸——父在 worktree 内,写入却顺链进宿主。
fn is_confined(root: &Path, target: &Path) -> Result<bool, String> {
    let root_c = fs::canonicalize(root).map_err(|e| format!("root canon: {e}"))?;
    if target.exists() {
        // 末段也解析(文件/目录 symlink 都被解开)
        let t_c = fs::canonicalize(target).map_err(|e| format!("target canon: {e}"))?;
        return Ok(t_c.starts_with(&root_c));
    }
    // 新建写:target 尚不存在。解析父目录(中间 symlink 被解开)再拼末段字面名。
    let parent = target.parent().unwrap_or(target);
    let name = target.file_name();
    let parent_c = fs::canonicalize(parent).map_err(|e| format!("parent canon: {e}"))?;
    let resolved = match name { Some(n) => parent_c.join(n), None => parent_c };
    Ok(resolved.starts_with(&root_c))
}

fn main() {
    let base = std::env::temp_dir().join(format!("c1-escape-{}", std::process::id()));
    let worktree = base.join("worktree");
    let host = base.join("host"); // worktree 之外的"宿主"
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&host).unwrap();

    // 宿主哨兵:探针后其字节/存在性必须不变(抓改写与删除)
    let sentinel = host.join("sentinel.txt");
    fs::write(&sentinel, b"ORIGINAL").unwrap();
    let sentinel_before = fs::read(&sentinel).unwrap();

    let mut results: Vec<(String, String, String)> = Vec::new(); // (vector, predicate, detail)

    // ---- 正对照:worktree 内合法写 ----
    {
        let target = worktree.join("ok.txt");
        match is_confined(&worktree, &target) {
            Ok(true) => results.push(("in_worktree_control".into(), "ALLOW".into(),
                "worktree 内路径判定为在内(正对照通过)".into())),
            Ok(false) => results.push(("in_worktree_control".into(), "WOULD_BLOCK".into(),
                "误拦合法写 —— 谓词过严".into())),
            Err(e) => results.push(("in_worktree_control".into(), "ERROR".into(), e)),
        }
    }

    // ---- 向量①:绝对路径写宿主 ----
    {
        let target = host.join("via_absolute.txt");
        let verdict = match is_confined(&worktree, &target) {
            Ok(false) => "WOULD_BLOCK", Ok(true) => "ESCAPED", Err(_) => "ERROR",
        };
        results.push(("absolute_path".into(), verdict.into(),
            format!("目标={}", target.display())));
    }

    // ---- 向量②:`..` 相对路径逃逸 ----
    {
        // worktree/subdir/../../host/via_dotdot.txt
        let sub = worktree.join("subdir");
        fs::create_dir_all(&sub).unwrap();
        let target = sub.join("..").join("..").join("host").join("via_dotdot.txt");
        let verdict = match is_confined(&worktree, &target) {
            Ok(false) => "WOULD_BLOCK", Ok(true) => "ESCAPED", Err(_) => "ERROR",
        };
        results.push(("dotdot_relative".into(), verdict.into(),
            format!("目标={}", target.display())));
    }

    // ---- 向量③:symlink/junction 指向宿主 ----
    // 在 worktree 内造一个链接指向宿主目录,再经该链接写文件。
    // 谓词若只按字面路径判(不解 link)会误判"在内";canonicalize 解链接后应识别越界。
    {
        let link = worktree.join("escape_link");
        let link_made = make_dir_link(&host, &link);
        if link_made {
            // 经链接写:worktree/escape_link/via_symlink.txt 实际落在 host/
            let target = link.join("via_symlink.txt");
            let verdict = match is_confined(&worktree, &target) {
                // canonicalize 会解 link → 若实现正确应判 ESCAPED(越界)
                Ok(false) => "WOULD_BLOCK",
                Ok(true) => "ESCAPED",
                Err(e) => {
                    // 部分环境 canonicalize 对不存在的 target 父级(链接)行为不同
                    results.push(("symlink_junction".into(), "ERROR".into(), e));
                    finish(&results, &sentinel, &sentinel_before, &base);
                    return;
                }
            };
            results.push(("symlink_junction".into(), verdict.into(),
                format!("经 worktree 内链接写宿主,目标={}", target.display())));
        } else {
            // symlink 需权限;改试 junction(mklink /J,Windows 免管理员)
            let jlink = worktree.join("escape_junction");
            if make_junction(&host, &jlink) {
                let target = jlink.join("via_junction.txt");
                let verdict = match is_confined(&worktree, &target) {
                    Ok(false) => "WOULD_BLOCK", Ok(true) => "ESCAPED", Err(_) => "ERROR",
                };
                results.push(("symlink_junction".into(), verdict.into(),
                    format!("经 junction 写宿主(symlink 无权限,改用 junction),目标={}",
                        target.display())));
            } else {
                results.push(("symlink_junction".into(), "SKIPPED".into(),
                    "本环境无权创建 symlink 且 junction 创建失败;生产环境须假设两者可用".into()));
            }
        }
    }

    // ---- 向量④:**已存在的末段文件符号链接** 指向宿主(codex R3-F1)----
    // worktree/link.txt 是指向 host/sentinel.txt 的文件 symlink。旧谓词只解
    // 父目录 → 父在 worktree 内 → 误判 ALLOW,写入却顺链改宿主。修正后须 ESCAPED。
    {
        let flink = worktree.join("link.txt");
        if make_file_link(&sentinel, &flink) {
            let verdict = match is_confined(&worktree, &flink) {
                Ok(false) => "WOULD_BLOCK", // 修正后:解析末段链接 → 越界识别
                Ok(true) => "ESCAPED",      // 旧实现会走到这里(漏判)
                Err(_) => "ERROR",
            };
            results.push(("existing_final_link".into(), verdict.into(),
                "worktree/link.txt 文件 symlink 指向宿主哨兵;解末段识别越界".into()));
        } else {
            // 文件 symlink 需权限:改用**已存在的 junction 本身**作 target,
            // 同样走 target.exists()→canonicalize 全路径的修正路径,本机可实证。
            let jlink = worktree.join("existing_junction_target");
            if make_junction(&host, &jlink) {
                let verdict = match is_confined(&worktree, &jlink) {
                    Ok(false) => "WOULD_BLOCK", // 修正后:末段 junction 被解析 → 越界
                    Ok(true) => "ESCAPED",      // 旧实现只解父,漏判为 ALLOW
                    Err(_) => "ERROR",
                };
                results.push(("existing_final_link".into(), verdict.into(),
                    "已存在的末段 junction 作 target;修正后解全路径识别越界(文件 symlink 无权限,junction 等价验证)".into()));
            } else {
                results.push(("existing_final_link".into(), "SKIPPED".into(),
                    "本环境无权创建文件 symlink 或 junction;生产须假设可用".into()));
            }
        }
    }

    finish(&results, &sentinel, &sentinel_before, &base);
}

fn finish(results: &[(String, String, String)], sentinel: &Path,
          before: &[u8], base: &Path) {
    // 哨兵完整性:谓词只是判定,本 spike 从不真写宿主 —— 哨兵必须不变
    let sentinel_ok = fs::read(sentinel).map(|b| b == before).unwrap_or(false)
        && sentinel.exists();

    let control_ok = results.iter().any(|(v, p, _)|
        v == "in_worktree_control" && p == "ALLOW");

    // 本 spike **不产出档位裁定**(codex R1-F3/R2-F1):只测谓词能否识别逃逸,
    // 不驱动真实引擎会话、不观测拒绝,也不宣称执行点是否存在。C1/C2 档位由
    // 保留的 full-MvpAgent 门决定(用户裁定 2026-08-12)。
    //
    // codex R4-F1:必需向量集**精确命名**,四个全部必须**运行且 WOULD_BLOCK**
    // 才算通过。任一 SKIPPED/ESCAPED/ERROR/缺失 → 不作通过证据(该次运行整体
    // 视为 NOT_RUN,exit 非零)。不再过滤掉 SKIPPED 后只看剩余子集。
    const REQUIRED: [&str; 4] = ["absolute_path", "dotdot_relative",
        "symlink_junction", "existing_final_link"];
    let all_vectors_blocked = REQUIRED.iter().all(|req| {
        results.iter().any(|(v, p, _)| v == req && p == "WOULD_BLOCK")
    });
    // 谓词能力面(供 JSON 记录):同上口径。
    let predicate_catches_all = all_vectors_blocked;

    // 合法 JSON 产物(F4):写入文件,控制台标记分离
    let json = build_json(&results, sentinel_ok, predicate_catches_all,
        control_ok, all_vectors_blocked);
    std::fs::write("c1-evidence.json", &json).ok();

    println!("C1 predicate-spike: vectors_blocked={} control_ok={} sentinel_intact={} (NOT full C1 evidence)",
        all_vectors_blocked, control_ok, sentinel_ok);
    let _ = fs::remove_dir_all(base);
    // 成功 = 谓词识别全部四个必需向量 + 正对照 + 哨兵完好。缺一即失败。
    std::process::exit(if sentinel_ok && control_ok && all_vectors_blocked { 0 } else { 1 });
}

#[cfg(windows)]
fn make_dir_link(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(not(windows))]
fn make_dir_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn make_file_link(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(not(windows))]
fn make_file_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[allow(dead_code)]
fn write_probe(target: &Path) -> std::io::Result<()> {
    let mut f = fs::File::create(target)?;
    f.write_all(b"probe")
}

#[allow(dead_code)]
fn as_pathbuf(s: &str) -> PathBuf { PathBuf::from(s) }

#[cfg(windows)]
fn make_junction(target: &Path, link: &Path) -> bool {
    // mklink /J 创建目录 junction,Windows 上免管理员权限。
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J",
            &link.to_string_lossy(), &target.to_string_lossy()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn make_junction(_t: &Path, _l: &Path) -> bool { false }

/// 合法 JSON 产物(codex R2-F3):用 serde_json,根除手写转义的控制符 bug。
fn build_json(results: &[(String, String, String)], sentinel_ok: bool,
    predicate_catches_all: bool, control_ok: bool, all_blocked: bool) -> String {
    let vectors: Vec<serde_json::Value> = results.iter()
        .map(|(v, p, d)| serde_json::json!({"vector": v, "predicate": p, "detail": d}))
        .collect();
    let doc = serde_json::json!({
        "artifact": "c1-escape-predicate-spike",
        "scope": "client predicate capability ONLY — not full C1 evidence \
(no engine session, no real tool_calls, no observed rejection); produces NO tier ruling. \
C1/C2 remain gated by the preserved full-MvpAgent requirement (user ruling 2026-08-12).",
        "sentinel_intact": sentinel_ok,
        "predicate_catches_all_tested": predicate_catches_all,
        "control_allows": control_ok,
        "all_required_vectors_blocked": all_blocked,
        "vectors": vectors,
    });
    let s = serde_json::to_string_pretty(&doc).unwrap();
    // parse-back:序列化产物必须可被重新解析
    assert!(serde_json::from_str::<serde_json::Value>(&s).is_ok(), "parse-back failed");
    s
}
