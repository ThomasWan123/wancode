//! C1 Cowork 逃逸探针 spike —— 客户端路径限制谓词的真机边界测试。
//!
//! 设计 §2.1 的核心问题:worktree 只隔离 Git,不隔离文件系统。客户端在
//! G26 零引擎改动下唯一可能的隔离杠杆 = 在放行 shell 写之前,用一个路径
//! 限制谓词 `is_confined(root, target)` 判定目标是否在 worktree 内。
//!
//! 本 spike 在**真实文件系统**上测该谓词对三个逃逸向量的拦截力:
//!   ①绝对路径写宿主  ②`..` 相对路径逃逸  ③symlink/junction 指向宿主
//! 外加一个 worktree 内合法写的**正对照**(证明谓词不是"全拒")。
//!
//! 判据(设计 §2.1):
//!   - 谓词对 ①②③ 全部判"越界"(would-block)+ 正对照判"允许" → 档 A 候选
//!   - 任一逃逸被谓词误判为"在内" → **档 B**(该杠杆不足以强制隔离)
//!
//! 诚实边界:这测的是"客户端谓词能不能识别逃逸",不是"引擎会不会在放行
//! 前调用它"。后者需完整 MvpAgent 会话(C1 第二阶段,provider_compliance 级)。
//! 本 spike 先钉死杠杆本身的能力上限 —— 这决定档 A 是否**理论可达**。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 客户端路径限制谓词:target 规范化后是否仍在 root 内。
/// 这是 canonicalize + starts_with 的标准写法 —— 客户端能做到的最强形态。
fn is_confined(root: &Path, target: &Path) -> Result<bool, String> {
    let root_c = fs::canonicalize(root).map_err(|e| format!("root canon: {e}"))?;
    // target 可能尚不存在(新建写),规范化其**父目录**再拼文件名。
    let parent = target.parent().unwrap_or(target);
    let name = target.file_name();
    let parent_c = fs::canonicalize(parent).map_err(|e| format!("parent canon: {e}"))?;
    let resolved = match name {
        Some(n) => parent_c.join(n),
        None => parent_c,
    };
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
    // 谓词若只按字面路径判(不解 link)会误判"在内" → 档 B 的决定性证据。
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

    finish(&results, &sentinel, &sentinel_before, &base);
}

fn finish(results: &[(String, String, String)], sentinel: &Path,
          before: &[u8], base: &Path) {
    // 哨兵完整性:谓词只是判定,本 spike 从不真写宿主 —— 哨兵必须不变
    let sentinel_ok = fs::read(sentinel).map(|b| b == before).unwrap_or(false)
        && sentinel.exists();

    let items: Vec<String> = results.iter().map(|(v, p, d)| {
        format!("{{\"vector\":\"{}\",\"predicate\":\"{}\",\"detail\":\"{}\"}}",
            v, p, d.replace('\\', "/").replace('"', "'"))
    }).collect();

    // 谓词能力面(必要非充分):三逃逸向量各自是否被谓词识别。
    let escapes: Vec<&(String, String, String)> = results.iter()
        .filter(|(v, _, _)| v != "in_worktree_control").collect();
    let predicate_catches_all = escapes.iter()
        .all(|(_, p, _)| p == "WOULD_BLOCK");
    let control_ok = results.iter().any(|(v, p, _)|
        v == "in_worktree_control" && p == "ALLOW");

    // 档位裁定(设计 §2.1 + PR #39 已确证的执行点缺口):
    // 档 A 需要**写前强制拦截执行点**。PR #39 确证:引擎在放行 shell 写前
    // 无调用点会咨询客户端谓词(工具/MCP 策略只在 NewSession 构造期设,
    // PromptRequest 无逐操作钩子)。因此**无论谓词多强,档位都是 B**——
    // 谓词能识别逃逸 ≠ 有地方在写发生前拦下它。这正是探针要钉死的结论。
    let tier = "B"; // 由执行点缺失决定,非由谓词能力决定
    let tier_reason = if predicate_catches_all {
        "谓词可识别全部受测逃逸向量,但客户端无写前执行点调用它(PR #39 G26 缺口)→ 档 B"
    } else {
        "谓词未能识别全部逃逸,且无写前执行点 → 档 B(更强的理由)"
    };

    println!("C1_EVIDENCE {{\"lever\":\"canonicalize+starts_with predicate\",\"sentinel_intact\":{},\"predicate_catches_all_tested\":{},\"control_allows\":{},\"enforcement_point_exists\":false,\"tier_ruling\":\"{}\",\"tier_reason\":\"{}\",\"vectors\":[{}]}}",
        sentinel_ok, predicate_catches_all, control_ok, tier, tier_reason, items.join(","));

    let _ = fs::remove_dir_all(base);
    // 成功 = 哨兵完好 + 正对照通过(证明测试装置有效);档位恒为 B 是预期产出。
    std::process::exit(if sentinel_ok && control_ok { 0 } else { 1 });
}

#[cfg(windows)]
fn make_dir_link(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

#[cfg(not(windows))]
fn make_dir_link(target: &Path, link: &Path) -> bool {
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
