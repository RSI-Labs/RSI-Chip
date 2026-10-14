use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::available_parallelism;

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to run {:?}: {error}", command));
    if !status.success() {
        panic!("command failed with {status}: {:?}", command);
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("vendor/kissat");
    let build_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("kissat");

    println!("cargo:rerun-if-changed=vendor/kissat/configure");
    println!("cargo:rerun-if-changed=vendor/kissat/makefile.in");
    println!("cargo:rerun-if-changed=vendor/kissat/src");

    if build_dir.exists() {
        fs::remove_dir_all(&build_dir).expect("remove stale Kissat build tree");
    }
    copy_tree(&vendor, &build_dir).expect("copy vendored Kissat source");

    run(Command::new("./configure")
        .arg("--quiet")
        .current_dir(&build_dir));
    let jobs = available_parallelism()
        .map(|jobs| jobs.get().to_string())
        .unwrap_or_else(|_| "2".to_string());
    run(Command::new("make")
        .arg("-j")
        .arg(jobs)
        .current_dir(&build_dir));

    let library_dir = build_dir.join("build");
    println!("cargo:rustc-link-search=native={}", library_dir.display());
    println!("cargo:rustc-link-lib=static=kissat");
    println!("cargo:rustc-link-lib=dylib=m");
}
