use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dociler-{}-{}-{}",
            prefix,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parent of dociler crate")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

#[test]
fn test_install_sh_help() {
    let root = repo_root();
    let install_sh = root.join("install.sh");
    assert!(
        install_sh.exists(),
        "install.sh must exist in repository root"
    );

    let output = Command::new("sh")
        .arg(&install_sh)
        .arg("--help")
        .output()
        .expect("execute install.sh --help");

    assert!(output.status.success(), "install.sh --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dociler Installer for macOS and Linux"),
        "Help text must identify Dociler installer"
    );
    assert!(
        stdout.contains("Docker, Python, Ollama"),
        "Help text must contain the standalone safety note"
    );
    assert!(
        stdout.contains("--dir"),
        "Help text must document --dir option"
    );
    assert!(
        stdout.contains("--checksum"),
        "Help text must document --checksum option"
    );
}

#[test]
fn test_install_sh_dry_run() {
    let root = repo_root();
    let install_sh = root.join("install.sh");

    let output = Command::new("sh")
        .arg(&install_sh)
        .arg("--dry-run")
        .output()
        .expect("execute install.sh --dry-run");

    assert!(
        output.status.success(),
        "install.sh --dry-run should exit 0"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Dry run enabled"),
        "Output must confirm dry run enabled"
    );
    assert!(
        stdout.contains("Target Platform :"),
        "Output must display target platform"
    );
    assert!(
        stdout.contains("Install Dir     :"),
        "Output must display install directory"
    );
}

#[test]
fn test_install_sh_invalid_argument() {
    let root = repo_root();
    let install_sh = root.join("install.sh");

    let output = Command::new("sh")
        .arg(&install_sh)
        .arg("--invalid-flag")
        .output()
        .expect("execute install.sh with invalid flag");

    assert!(
        !output.status.success(),
        "install.sh with invalid flag should exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown argument"),
        "Error message must report unknown argument"
    );
}

#[test]
fn test_install_sh_local_binary_install() {
    let root = repo_root();
    let install_sh = root.join("install.sh");
    let target_dir = TempDir::new("install-bin-dest");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_dociler"));

    assert!(binary_path.exists(), "Dociler binary must exist for test");

    let output = Command::new("sh")
        .arg(&install_sh)
        .arg("--dir")
        .arg(target_dir.path())
        .arg("--file")
        .arg(&binary_path)
        .output()
        .expect("execute install.sh with local binary");

    assert!(
        output.status.success(),
        "install.sh should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let installed_bin = target_dir.path().join("dociler");
    assert!(
        installed_bin.exists(),
        "Installed binary must exist at destination"
    );

    // Verify installed binary can execute --version
    let ver_output = Command::new(&installed_bin)
        .arg("--version")
        .output()
        .expect("execute installed binary");

    assert!(ver_output.status.success());
    let ver_str = String::from_utf8_lossy(&ver_output.stdout);
    assert!(ver_str.contains("dociler"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Installation complete!"),
        "Output must report completion"
    );
    assert!(
        stdout.contains("No Docker, Python, Ollama, model weights, or system services"),
        "Output must reinforce standalone privacy promise"
    );
}

#[test]
fn test_install_sh_checksum_verification_success_and_failure() {
    let root = repo_root();
    let install_sh = root.join("install.sh");
    let staging = TempDir::new("archive-staging");
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_dociler"));

    // Create a mock tarball containing dociler
    let archive_path = staging
        .path()
        .join("dociler-v0.1.0-dev-linux-x86_64.tar.gz");
    let tar_status = Command::new("tar")
        .arg("-czf")
        .arg(&archive_path)
        .arg("-C")
        .arg(binary_path.parent().expect("binary dir"))
        .arg(binary_path.file_name().expect("binary name"))
        .status()
        .expect("create tar archive");

    assert!(tar_status.success(), "tar creation should succeed");

    // Compute actual sha256
    let hash_output = Command::new("sha256sum")
        .arg(&archive_path)
        .output()
        .expect("compute sha256sum");
    assert!(hash_output.status.success());
    let actual_hash = String::from_utf8_lossy(&hash_output.stdout)
        .split_whitespace()
        .next()
        .expect("hash string")
        .to_string();

    // 1. Success case: correct checksum
    let dest_dir_ok = TempDir::new("install-dest-ok");
    let ok_output = Command::new("sh")
        .arg(&install_sh)
        .arg("--dir")
        .arg(dest_dir_ok.path())
        .arg("--file")
        .arg(&archive_path)
        .arg("--checksum")
        .arg(&actual_hash)
        .output()
        .expect("install.sh with valid checksum");

    assert!(
        ok_output.status.success(),
        "install.sh should succeed with valid checksum: {}",
        String::from_utf8_lossy(&ok_output.stderr)
    );
    assert!(dest_dir_ok.path().join("dociler").exists());

    // 2. Failure case: corrupted checksum
    let dest_dir_fail = TempDir::new("install-dest-fail");
    let bad_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let fail_output = Command::new("sh")
        .arg(&install_sh)
        .arg("--dir")
        .arg(dest_dir_fail.path())
        .arg("--file")
        .arg(&archive_path)
        .arg("--checksum")
        .arg(bad_hash)
        .output()
        .expect("install.sh with invalid checksum");

    assert!(
        !fail_output.status.success(),
        "install.sh must fail when checksum does not match"
    );
    let fail_err = String::from_utf8_lossy(&fail_output.stderr);
    assert!(
        fail_err.contains("Checksum verification failed"),
        "Error message must specify checksum verification failed: {}",
        fail_err
    );
    assert!(
        !dest_dir_fail.path().join("dociler").exists(),
        "Failed installation must not leave binary in target directory"
    );
}

#[test]
fn test_install_ps1_content_and_contract() {
    let root = repo_root();
    let install_ps1 = root.join("install.ps1");
    assert!(
        install_ps1.exists(),
        "install.ps1 must exist in repository root"
    );

    let content = fs::read_to_string(&install_ps1).expect("read install.ps1");

    // Verify key parameters
    assert!(content.contains("[string]$Version"));
    assert!(content.contains("[string]$InstallDir"));
    assert!(content.contains("[string]$Target"));
    assert!(content.contains("[string]$File"));
    assert!(content.contains("[string]$Checksum"));
    assert!(content.contains("[switch]$DryRun"));
    assert!(content.contains("[switch]$Help"));

    // Verify architecture check
    assert!(
        content.contains("$env:PROCESSOR_ARCHITECTURE"),
        "Must check processor architecture"
    );
    assert!(
        content.contains("AMD64"),
        "Must support 64-bit AMD/Intel architecture"
    );

    // Verify SHA-256 algorithm used
    assert!(
        content.contains("Get-FileHash") && content.contains("SHA256"),
        "Must verify SHA256 checksums"
    );

    // Verify zip expansion
    assert!(
        content.contains("Expand-Archive"),
        "Must safely extract zip package"
    );

    // Verify safety note
    assert!(
        content.contains("No Docker, Python, Ollama, model weights, or background services"),
        "Must state standalone safety promise"
    );
}

#[test]
fn test_package_release_script() {
    let root = repo_root();
    let package_sh = root.join("scripts").join("package_release.sh");
    assert!(package_sh.exists(), "scripts/package_release.sh must exist");

    let output = Command::new("bash")
        .arg(&package_sh)
        .arg("linux-x86_64")
        .current_dir(&root)
        .output()
        .expect("execute package_release.sh");

    assert!(
        output.status.success(),
        "package_release.sh should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dist_dir = root.join("target").join("dist");
    assert!(dist_dir.exists(), "target/dist must exist after packaging");

    let sha_manifest = dist_dir.join("SHA256SUMS");
    assert!(sha_manifest.exists(), "SHA256SUMS must exist in dist dir");

    let manifest_text = fs::read_to_string(&sha_manifest).expect("read SHA256SUMS");
    assert!(
        manifest_text.contains("dociler-v"),
        "SHA256SUMS must reference created package"
    );

    // Clean up test dist files
    let _ = fs::remove_dir_all(&dist_dir);
}

#[test]
fn test_notice_file_attribution() {
    let root = repo_root();
    let notice_path = root.join("NOTICE");
    assert!(
        notice_path.exists(),
        "NOTICE file must exist in repository root"
    );

    let content = fs::read_to_string(&notice_path).expect("read NOTICE");
    assert!(
        content.contains("Apache License, Version 2.0"),
        "NOTICE must declare Apache License, Version 2.0"
    );
    assert!(
        content.contains("dociler-lite") && content.contains("Qwen"),
        "NOTICE must attribute dociler-lite to Qwen"
    );
    assert!(
        content.contains("dociler-pro") && content.contains("Qwen"),
        "NOTICE must attribute dociler-pro to Qwen"
    );
    assert!(
        content.contains("llama.cpp") && content.contains("MIT License"),
        "NOTICE must attribute llama.cpp under MIT License"
    );
}
