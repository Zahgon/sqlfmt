//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const SENTINEL: &str = ")))))__SQLFMT_OUTPUT__(((((";

/// The directory with the test data files.
pub fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("data")
}

/// Reads a test file and returns its unformatted source and expected output. A file
/// without the output sentinel is already formatted, so both are its contents.
pub fn read_test_data(relpath: &str) -> (String, String) {
    let text = fs::read_to_string(data_dir().join(relpath)).unwrap_or_else(|e| panic!("cannot read {relpath}: {e}"));
    let mut source = String::new();
    let mut formatted = String::new();
    let mut in_output = false;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == SENTINEL {
            in_output = true;
            continue;
        }
        if in_output {
            formatted.push_str(line);
        } else {
            source.push_str(line);
        }
    }
    if !in_output {
        formatted = source.clone();
    }
    (format!("{}\n", source.trim()), formatted)
}

/// A fresh, empty temporary directory, removed when dropped.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("sqlfmt-test-{}-{label}-{n}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove a stale test directory");
        }
        fs::create_dir_all(&path).expect("create a test directory");
        TempDir { path: path.canonicalize().expect("canonical test directory") }
    }

    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path.join(rel);
        fs::create_dir_all(path.parent().expect("a parent directory")).expect("create parent directories");
        fs::write(&path, contents).expect("write a test file");
        path
    }

    /// Writes the source query of every `.sql` file of a data directory (not its
    /// subdirectories) into `rel`.
    pub fn copy_data_dir(&self, data_rel: &str, rel: &str) -> PathBuf {
        let target = self.path.join(rel);
        fs::create_dir_all(&target).expect("create a test directory");
        for entry in fs::read_dir(data_dir().join(data_rel)).expect("read a data directory") {
            let entry = entry.expect("a directory entry");
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_file() && name.ends_with(".sql") {
                let (source, _) = read_test_data(&format!("{data_rel}/{name}"));
                fs::write(target.join(&name), source).expect("write a data file");
            }
        }
        target
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // a leftover temporary directory does not affect later tests (each run uses
        // fresh names), so a failed cleanup is reported but not fatal
        if let Err(e) = fs::remove_dir_all(&self.path) {
            eprintln!("could not remove {}: {e}", self.path.display());
        }
    }
}
