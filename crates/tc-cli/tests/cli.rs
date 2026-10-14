//! End-to-end runs of the `termcraft-cli` binary. A copy of it is placed in
//! a temporary folder with the portable marker, so nothing touches the
//! user's real settings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Portable {
    dir: tempfile::TempDir,
    exe: PathBuf,
}

impl Portable {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new(env!("CARGO_BIN_EXE_termcraft-cli"));
        let exe = dir.path().join(src.file_name().unwrap());
        std::fs::copy(src, &exe).unwrap();
        std::fs::write(dir.path().join(tc_core::paths::PORTABLE_MARKER), "").unwrap();
        Self { dir, exe }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(&self.exe)
            .args(args)
            .current_dir(self.dir.path())
            .env_remove("TERMCRAFT_UPDATE_SECRET")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

#[test]
fn lists_every_catalog_command() {
    let cli = Portable::new();
    let out = cli.ok(&["commands"]);
    for spec in tc_commands::CATALOG {
        assert!(out.contains(spec.name), "{}", spec.name);
    }
}

#[test]
fn sessions_are_saved_in_the_portable_folder() {
    let cli = Portable::new();
    cli.ok(&[
        "run",
        "session_add",
        r#"{"session":{"name":"web","type":"ssh","host":"10.0.0.1","user":"root"}}"#,
    ]);
    let data = cli.dir.path().join("termcraft-data");
    assert!(data.is_dir());
    let list = cli.ok(&["sessions"]);
    assert!(list.contains("web") && list.contains("10.0.0.1"), "{list}");
}

#[test]
fn bad_input_fails_cleanly() {
    let cli = Portable::new();
    assert!(
        !cli.run(&["run", "session_add", "not json"])
            .status
            .success()
    );
    assert!(!cli.run(&["run", "no_such_command"]).status.success());
    assert!(!cli.run(&["run", "session_add", "{}"]).status.success());
}

#[test]
fn release_signing_round_trip() {
    let cli = Portable::new();
    let dir = cli.dir.path();
    let keys = cli.ok(&["update", "keygen"]);
    let value = |name: &str| {
        keys.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap()
            .trim()
            .to_string()
    };
    let (secret, public) = (
        value("TERMCRAFT_UPDATE_SECRET"),
        value("TERMCRAFT_UPDATE_PUBKEY"),
    );

    let asset = dir.join("termcraft-test.zip");
    std::fs::write(&asset, b"payload").unwrap();
    let hash = cli.ok(&["update", "hash", asset.to_str().unwrap()]);
    assert_eq!(hash.trim().len(), 64);

    let manifest = dir.join("stable.json");
    cli.ok(&[
        "update",
        "manifest",
        "--version",
        "9.9.9",
        "--base-url",
        "https://example.invalid/dl",
        "--out",
        manifest.to_str().unwrap(),
        &format!("windows-x86_64={}", asset.display()),
    ]);
    let text = std::fs::read_to_string(&manifest).unwrap();
    assert!(text.contains(hash.trim()), "{text}");

    // Without the secret, signing is refused.
    assert!(
        !cli.run(&["update", "sign", manifest.to_str().unwrap()])
            .status
            .success()
    );
    let out = Command::new(&cli.exe)
        .args(["update", "sign", manifest.to_str().unwrap()])
        .env("TERMCRAFT_UPDATE_SECRET", &secret)
        .output()
        .unwrap();
    assert!(out.status.success());

    let sig = std::fs::read_to_string(dir.join("stable.json.sig")).unwrap();
    let keys = tc_update::parse_keys(&public).unwrap();
    let bytes = std::fs::read(&manifest).unwrap();
    let parsed = tc_update::verify_manifest(&bytes, &sig, &keys, "stable").unwrap();
    assert_eq!(parsed.version, "9.9.9");
    // Any change to the manifest breaks the signature.
    let mut tampered = bytes.clone();
    tampered.extend_from_slice(b" ");
    assert!(tc_update::verify_manifest(&tampered, &sig, &keys, "stable").is_err());
}
