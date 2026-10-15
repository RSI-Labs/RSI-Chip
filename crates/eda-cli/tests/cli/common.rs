//! Shared CLI integration-test process/file helpers.

use std::fs;
use std::process::Command;

pub(super) struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    pub(super) fn new(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("eda-cli-{}-{}", std::process::id(), name));
        fs::write(&path, contents).expect("write temporary CLI input");
        Self { path }
    }

    pub(super) fn path(&self) -> &str {
        self.path.to_str().expect("temporary path is UTF-8")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn run_edu(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_eda"))
        .args(args)
        .output()
        .expect("run eda binary");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}
