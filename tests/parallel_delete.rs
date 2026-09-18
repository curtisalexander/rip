//! Stress the walker/deleter scheduling, not just small-tree correctness.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn wide_tree_completes_without_worker_starvation() {
    let work = Path::new(env!("CARGO_TARGET_TMPDIR")).join("parallel_delete");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();

    for threads in [4, 16, 4] {
        let root = work.join("victim");
        for d in 0..200 {
            let dir = root.join(format!("package-{d}"));
            fs::create_dir_all(&dir).unwrap();
            for f in 0..50 {
                fs::write(dir.join(format!("file-{f}")), b"payload").unwrap();
            }
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_rip"))
            .args(["-f", "-j", &threads.to_string()])
            .arg(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A deadlock guard, not a performance threshold for shared CI machines.
        let deadline = Instant::now() + Duration::from_secs(120);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("delete stalled with {threads} workers: {output:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let summary = String::from_utf8_lossy(&output.stderr);
        assert!(summary.contains("10000 files, 201 dirs"), "{summary}");
        assert!(summary.contains("(0 errors)"), "{summary}");
        assert!(!root.exists());
    }
}
