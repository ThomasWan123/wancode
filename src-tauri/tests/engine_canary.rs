//! 引擎金丝雀（v0.13-4）：wancode 客户端建立在引擎源码的若干"考古结论"上，
//! 这些结论引擎升级时可能被悄悄推翻。本文件对固定 commit 的引擎源码做
//! 文本级断言——升级 vendor/grok-build.lock 后跑这里，红了的每一条都指向
//! 一个必须重新核实的客户端行为。
//!
//! 刻意用源码扫描而不是起引擎：这些坑（alias、回退、panic）大多在错误
//! 分支上，行为级测试反而难稳定复现；源码断言在 CI 里零成本且指向精确。

use std::path::{Path, PathBuf};

/// 引擎在仓库兄弟目录（见 vendor/grok-build.lock 头注）。
fn engine_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // wancode 仓库根
        .and_then(|p| p.parent()) // 兄弟层
        .map(|p| p.join("grok-build"))
        .expect("无法定位 ../../grok-build")
}

fn read(rel: &str) -> String {
    let p = engine_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到引擎文件 {}: {e}", p.display()))
}

fn ext_dir() -> PathBuf {
    engine_root().join("crates/codegen/xai-grok-shell/src/extensions")
}

/// 坑 1（ext_call 双注入的安全前提）：#[serde(alias)] 会把 sessionId 与
/// session_id 映射到同一字段，双注入直接 duplicate field。ext_call 目前
/// 只对 rewind/*、debug/* 做单键注入——如果引擎在其它 extension 里新增了
/// alias，这条会红，届时要扩充 agent.rs ext_call 的单键清单。
#[test]
fn alias_only_in_rewind_and_debug() {
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(ext_dir()).expect("读不到 extensions 目录") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("serde(alias") && !matches!(name.as_str(), "rewind" | "debug") {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "引擎新增了带 serde(alias) 的 extension：{offenders:?} —— \
         检查 agent.rs ext_call 的单键注入清单是否要扩"
    );
}

/// 坑 1b：rewind 以 snake_case 为主名（客户端只注入 session_id）、
/// debug 以 camelCase 为主名（只注入 sessionId）。主名反转会静默换毒。
#[test]
fn alias_canonical_names_unchanged() {
    let rewind = read("crates/codegen/xai-grok-shell/src/extensions/rewind.rs");
    assert!(
        rewind.contains(r#"#[serde(alias = "sessionId")]"#) && rewind.contains("session_id: String"),
        "rewind 参数主名不再是 snake_case session_id"
    );
    let debug = read("crates/codegen/xai-grok-shell/src/extensions/debug.rs");
    assert!(
        debug.contains(r#"#[serde(alias = "session_id")]"#),
        "debug 参数不再以 camelCase 为主名"
    );
}

/// 坑 2（#83 的根）：引擎 git 操作在 resolve_git_root 失败时静默回退
/// workspace 根——嵌入式场景那是宿主应用自己的仓库。客户端因此对
/// x.ai/git/*（worktree 除外）强制显式传 gitRoot。这条断言"回退仍存在"：
/// 若引擎哪天修掉了回退，这条红了 → 可以考虑简化客户端注入。
#[test]
fn git_root_fallback_still_exists() {
    let git = read("crates/codegen/xai-grok-shell/src/extensions/git.rs");
    let has_explicit_root = git.contains("gitRoot") || git.contains("git_root");
    assert!(
        has_explicit_root,
        "git.rs 不再接受显式 gitRoot 参数——客户端 #83 注入通道失效，必须重审"
    );
}

/// 坑 3（v0.12.1 的根）：零模型启动引擎 panic（emit_announcements 处
/// RefCell 双借 + capacity overflow）。客户端不变量="零模型绝不启动引擎"。
/// 这条守着 emit_announcements 仍在会话启动路径上；引擎重构掉这个函数时
/// 要重验零模型行为。
#[test]
fn zero_model_panic_site_still_present() {
    let agent_ops = engine_root()
        .join("crates/codegen/xai-grok-shell/src/agent/mvp_agent/agent_ops.rs");
    let text = std::fs::read_to_string(&agent_ops)
        .unwrap_or_else(|e| panic!("agent_ops.rs 读取失败: {e}"));
    assert!(
        text.contains("emit_announcements"),
        "emit_announcements 已不存在——零模型启动 panic 的假设需重验，\
         客户端启动门控（MODEL_REQUIRED）逻辑可能可以放宽"
    );
}

/// 坑 4：插话广播的去重键是 interjectionId（不是 id）。客户端
/// handle_acp_message 按这个键去重；键名变化会导致插话重复渲染。
#[test]
fn interjection_id_key_unchanged() {
    let interject = read("crates/codegen/xai-grok-shell/src/extensions/interject.rs");
    assert!(
        interject.contains("interjection_id") || interject.contains("interjectionId"),
        "interject.rs 里找不到 interjection_id —— 前端去重键需重审"
    );
}

/// 坑 5：模糊搜索流式协议以 searchId 关联结果批次。
#[test]
fn search_id_key_unchanged() {
    let search = read("crates/codegen/xai-grok-shell/src/extensions/search.rs");
    assert!(
        search.contains("search_id") || search.contains("searchId"),
        "search.rs 里找不到 search_id —— 前端结果关联键需重审"
    );
}

/// 引擎 commit 与 vendor/grok-build.lock 一致（防"本地悄悄升了引擎但
/// lock 没更新"的漂移；bootstrap 只警告，这里在 CI 里硬性把关）。
#[test]
fn engine_commit_matches_lock() {
    let lock = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/grok-build.lock"),
    )
    .expect("读不到 vendor/grok-build.lock");
    let pinned = lock
        .lines()
        .find_map(|l| l.strip_prefix("commit="))
        .expect("lock 缺 commit= 行")
        .trim();
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(engine_root())
        .output()
        .expect("git rev-parse 失败");
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(
        head, pinned,
        "../grok-build HEAD 与 vendor/grok-build.lock 不一致——升级引擎请同步 lock 并重跑全部金丝雀"
    );
}
