//! Bug-2 feedback loop: `clean_temp_files` must return within a bounded time
//! and must not produce an unbounded result message.
//!
//! Real-world symptom (user report): clicking 「清理临时文件」 spins forever.
//!
//! `clean_temp_files()` reads `TEMP` / `WINDIR` from the environment, so this
//! test points those at a fixture tree that mirrors a real Windows TEMP
//! directory: many small deletable files, files held open exclusively (the
//! normal case for in-use temp files), and nested directories.
//!
//! This drives the real production code path (`cleanup::clean_temp_files`) —
//! no reimplementation of the logic under test.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod common;

/// `std::env::set_var` is process-global; serialise the two tests that use it.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Exclusively-held file handles that must outlive the cleanup call.
/// Returning the handles (rather than dropping them) is what keeps the files
/// undeletable for the duration of the operation under test.
type LockedFiles = Vec<fs::File>;

/// Files that are deletable. Kept modest so the loop stays fast: this is a
/// *behavioural* probe (does it terminate / does the message blow up), and the
/// real-world scale is reached by the multiplication of locked files.
fn clean_files() -> usize {
    env_count("SECM_BUG2_CLEAN_FILES", 400)
}

/// Files held open with `share_mode(0)`, which Windows refuses to delete.
/// These exercise the `mark_delete_on_reboot` fallback and the error path.
///
/// Overridable via `SECM_BUG2_LOCKED_FILES` for bisection: the whole point of
/// this loop is that per-locked-file cost is what blows the budget up.
fn locked_files() -> usize {
    env_count("SECM_BUG2_LOCKED_FILES", 120)
}

fn env_count(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
/// Nested directory depth, mirroring `Temp\<guid>\...\` real layouts.
const NEST_DEPTH: usize = 6;

fn make_fixture(root: &Path) -> LockedFiles {
    let temp = root.join("TEMP");
    fs::create_dir_all(&temp).expect("create fixture TEMP");

    let clean_files = clean_files();
    let locked_files = locked_files();

    // Deletable files spread across a few subdirectories.
    for bucket in 0..8 {
        let dir = temp.join(format!("bucket-{bucket}"));
        fs::create_dir_all(&dir).expect("create bucket");
        for i in 0..(clean_files / 8) {
            let mut f = fs::File::create(dir.join(format!("f{i}.tmp"))).expect("create file");
            let _ = f.write_all(b"0123456789abcdef");
        }
    }

    // Deeply nested tree, mirroring node_modules-style layouts in real TEMP.
    let mut nested = temp.join("deep");
    for d in 0..NEST_DEPTH {
        nested = nested.join(format!("level-{d}"));
    }
    fs::create_dir_all(&nested).expect("create nested");
    fs::File::create(nested.join("leaf.tmp"))
        .expect("create leaf")
        .write_all(b"x")
        .ok();

    // Exclusively-held files: the program under test cannot delete these.
    let mut handles = Vec::new();
    for i in 0..locked_files {
        let path = temp.join(format!("locked-{i}.tmp"));
        let mut f = fs::File::create(&path).expect("create locked file");
        let _ = f.write_all(b"locked-data");
        f.flush().ok();
        drop(f);

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let handle = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0) // no sharing -> Windows blocks deletion
                .open(&path)
                .expect("exclusive open");
            handles.push(handle);
        }
    }

    // WINDIR fixture: Temp + Prefetch must exist so those code paths run.
    let windir = root.join("Win");
    fs::create_dir_all(windir.join("Temp")).expect("create Win/Temp");
    fs::create_dir_all(windir.join("Prefetch")).expect("create Prefetch");
    for i in 0..40 {
        let mut f = fs::File::create(windir.join("Temp").join(format!("w{i}.tmp")))
            .expect("create win temp file");
        let _ = f.write_all(b"windows-temp");
    }

    handles
}
/// Restores `TEMP`/`WINDIR` to their original values on drop.
struct EnvGuard {
    temp: Option<String>,
    windir: Option<String>,
}

impl EnvGuard {
    fn redirect_to(root: &Path) -> Self {
        let guard = Self {
            temp: std::env::var("TEMP").ok(),
            windir: std::env::var("WINDIR").ok(),
        };
        std::env::set_var("TEMP", root.join("TEMP"));
        std::env::set_var("WINDIR", root.join("Win"));
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.temp {
            Some(v) => std::env::set_var("TEMP", v),
            None => std::env::remove_var("TEMP"),
        }
        match &self.windir {
            Some(v) => std::env::set_var("WINDIR", v),
            None => std::env::remove_var("WINDIR"),
        }
    }
}

/// **The red/green loop.** Runs the real `clean_temp_files()` against a fixture
/// TEMP containing in-use files and asserts the call terminates within budget
/// and does not emit an unbounded message.
#[test]
fn clean_temp_files_terminates_within_budget_on_locked_files() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root: PathBuf = common::fixture_dir("bug2_fixture_timing");

    let _handles = make_fixture(&root);
    let _guard = EnvGuard::redirect_to(&root);

    let start = Instant::now();
    let result = secm_lib::cleanup::clean_temp_files();
    let elapsed = start.elapsed();

    let _ = fs::remove_dir_all(&root);

    // BUDGET: this fixture is ~570 small files — two orders of magnitude smaller
    // than a real TEMP (measured: 24,711 files / 16,111 dirs / 1.48 GB on the
    // reporting machine). Even so it must finish in seconds, not spin.
    //
    // Overridable via `SECM_BUG2_BUDGET_SECS` for diagnosis: with a generous
    // budget the run continues so the message/failure assertions below report
    // independently of the timeout.
    let budget = Duration::from_secs(env_count("SECM_BUG2_BUDGET_SECS", 10) as u64);
    assert!(
        elapsed < budget,
        "clean_temp_files took {:?} on a {} file fixture (budget {:?})",
        elapsed,
        clean_files() + locked_files(),
        budget
    );

    // 失败语义：被占用文件**登记成功**时不算失败（提权进程）——否则每次清理只要
    // 有一个占用文件就报 success=false。登记失败时（非提权进程，或文件无法入队）
    // 必须如实报 success=false，不能静默吞掉。
    //
    // 这里刻意**不**断言 `result.success == true`：该值取决于测试进程是否提权
    // （`MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` 要求 Administrators 组），
    // 在普通用户 / CI runner 上必然为 false。断言"必须是 true"会让本用例在
    // 非提权环境因与修复无关的原因失败。
    //
    // 断言两种环境都成立的**不变量**：若所有被占用文件都成功入重启队列，
    // 则操作必须报告成功（这正是本次修复要修的"必然报失败"）；若入队失败，
    // 则必须出现在错误里（不能静默）。
    #[cfg(windows)]
    {
        let queued = result.message.contains("重启后自动删除");
        if queued && locked_files() > 0 {
            assert!(
                result.success,
                "被占用文件已全部登记重启删除，却仍报告 success=false：message=\n{}",
                result.message
            );
        } else if locked_files() > 0 {
            // 未能入队（非提权）→ 必须如实报错，且消息里要有可诊断信息
            assert!(
                !result.success,
                "未能登记重启删除却不报错：message=\n{}",
                result.message
            );
            assert!(
                result.message.contains("错误"),
                "入队失败必须留下错误明细：message=\n{}",
                result.message
            );
        }
    }

    // 消息长度必须有界（IPC + 日志安全）
    const MAX_MESSAGE_BYTES: usize = 64 * 1024;
    assert!(
        result.message.len() < MAX_MESSAGE_BYTES,
        "result message is {} bytes for {} locked files — unbounded IPC/log payload",
        result.message.len(),
        locked_files()
    );
}

/// **Regression test for the unbounded result message.**
///
/// The original code emitted one line per undeletable file, carrying the full
/// path — either `标记重启后删除: <path>` when the reboot-delete registration
/// succeeded, or `删除 <path> 失败（无法标记重启删除）` when it failed. Message
/// length therefore grew linearly with the number of in-use files.
///
/// Both branches are covered by this test's design: on an elevated process the
/// files are queued (the `标记重启后删除` branch); on a non-elevated process the
/// registration fails and they take the error branch. Either way the message
/// must stay bounded, so this assertion is meaningful regardless of the
/// elevation of the test runner.
///
/// With 400 in-use files, one line per file would be ~44 KB; the cap is 16 KB.
#[test]
fn clean_temp_files_keeps_result_message_bounded() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root: PathBuf = common::fixture_dir("bug2_fixture_msgbound");

    // Enough in-use files that an unbounded per-file message cannot fit the cap.
    let locked = 400usize;
    let temp = root.join("TEMP");
    fs::create_dir_all(&temp).expect("create TEMP");
    let mut handles = Vec::new();
    for i in 0..locked {
        let path = temp.join(format!("locked-{i}.tmp"));
        fs::write(&path, b"locked-data").expect("write locked");
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            handles.push(
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .share_mode(0)
                    .open(&path)
                    .expect("exclusive open"),
            );
        }
    }
    let windir = root.join("Win");
    fs::create_dir_all(windir.join("Temp")).expect("create Win/Temp");
    fs::create_dir_all(windir.join("Prefetch")).expect("create Prefetch");

    let _guard = EnvGuard::redirect_to(&root);

    let start = Instant::now();
    let result = secm_lib::cleanup::clean_temp_files();
    let elapsed = start.elapsed();

    // Prove the in-use files really are undeletable: they must survive the call.
    let surviving = fs::read_dir(&temp)
        .map(|d| d.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0);
    let _ = handles;

    let _ = fs::remove_dir_all(&root);

    // Must terminate promptly even in this pathological branch.
    let budget = Duration::from_secs(env_count("SECM_BUG2_BUDGET_SECS", 10) as u64);
    assert!(
        elapsed < budget,
        "took {:?} with {} locked files (budget {:?})",
        elapsed,
        locked,
        budget
    );

    #[cfg(windows)]
    assert!(
        surviving > 0,
        "fixture is vacuous: no file survived, so nothing was actually undeletable"
    );

    // THE regression assertion: message length must not scale with file count.
    const MAX_MESSAGE_BYTES: usize = 16 * 1024;
    assert!(
        result.message.len() < MAX_MESSAGE_BYTES,
        "message is {} bytes for {} undeletable files — the per-file message is still unbounded",
        result.message.len(),
        locked
    );

    // The outcome must still be truthful — we must not silently swallow failures.
    // `messages` always contains the unconditional "清理 TEMP: N 字节" line, so
    // asserting `!message.is_empty()` would be vacuous. Assert the real promise
    // instead: if nothing could be deleted and nothing was queued for reboot,
    // the operation must NOT claim success.
    #[cfg(windows)]
    if result.bytes_freed == 0 && !result.message.contains("重启后自动删除") {
        assert!(
            !result.success,
            "nothing deleted and nothing queued, yet success=true: {:?}",
            result.message
        );
    }
}

/// **Deterministic test for the message-length cap** (`MAX_REPORTED_ERRORS`).
///
/// The cap only fires in the "could not delete AND could not schedule for
/// reboot-delete" branch, which an elevated process never reaches — so
/// `clean_temp_files()` cannot exercise it end-to-end on such a machine, and
/// `cargo test --lib` is unavailable here (pre-existing comctl32
/// `STATUS_ENTRYPOINT_NOT_FOUND`). Driving `CleanOutcome` directly makes the
/// cap testable regardless of elevation.
///
/// Without the cap this appends one line per file (the original bug), so this
/// test fails if the cap is removed.
#[test]
fn clean_outcome_caps_reported_errors() {
    let total = 600usize;
    let mut out = secm_lib::cleanup::CleanOutcome::new();
    for i in 0..total {
        out.push_error(format!(
            "删除 C:\\Users\\somebody\\AppData\\Local\\Temp\\file-{}.tmp 失败（无法标记重启删除）",
            i
        ));
    }

    // Cap holds for the stored detail lines...
    assert_eq!(
        out.errors.len(),
        secm_lib::cleanup::MAX_REPORTED_ERRORS,
        "detail lines must be capped at MAX_REPORTED_ERRORS"
    );
    // ...and the surplus is counted, not silently dropped.
    assert_eq!(
        out.suppressed_errors,
        (total - secm_lib::cleanup::MAX_REPORTED_ERRORS) as u64,
        "every capped line must be counted so it can be surfaced in the summary"
    );

    let mut messages = Vec::new();
    let mut has_errors = false;
    out.extend_messages(&mut messages, &mut has_errors);

    // The cap must actually bound the rendered message.
    let rendered = messages.join("\n");
    assert!(
        rendered.len() < 8 * 1024,
        "rendered message is {} bytes for {} failures — cap is not effective",
        rendered.len(),
        total
    );
    // Suppressed count must be visible to the user, not swallowed.
    let expected = total - secm_lib::cleanup::MAX_REPORTED_ERRORS;
    assert!(
        rendered.contains(&format!("另有 {} 个文件", expected)),
        "suppressed count must be surfaced; got: {}",
        rendered
    );
    assert!(
        has_errors,
        "capped failures must still mark the operation as failed"
    );
}

