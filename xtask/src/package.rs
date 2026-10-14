//! `cargo xtask package`: release packages from built binaries.
//!
//! - every platform: `TermCraft-<version>-<platform>-portable.zip`
//!   (the programs and an empty `termcraft.portable`);
//! - Linux: `termcraft_<version>_<arch>.deb` (programs in /usr/bin, a
//!   desktop entry), written directly (ar + tar.gz), no dpkg needed;
//! - macOS: `TermCraft-<version>-<platform>.app.zip`, an application bundle.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Package<'a> {
    pub bin_dir: &'a Path,
    /// `windows-x86_64`, `linux-x86_64`, `macos-aarch64` …
    pub platform: &'a str,
    pub version: &'a str,
    pub out: &'a Path,
}

const DESKTOP_ENTRY: &str = "[Desktop Entry]\nType=Application\nName=TermCraft\n\
GenericName=Terminal and remote sessions\nComment=SSH, SFTP, RDP, VNC, serial and more\n\
Exec=termcraft\nIcon=utilities-terminal\nTerminal=false\nCategories=Network;System;TerminalEmulator;\n";

fn exe(platform: &str, name: &str) -> String {
    if platform.starts_with("windows") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Build every package that fits the platform; returns their paths.
pub fn run(p: &Package<'_>) -> Result<Vec<PathBuf>> {
    let gui = p.bin_dir.join(exe(p.platform, "termcraft"));
    let cli = p.bin_dir.join(exe(p.platform, "termcraft-cli"));
    for f in [&gui, &cli] {
        if !f.is_file() {
            bail!("{} is missing (build with --release first)", f.display());
        }
    }
    std::fs::create_dir_all(p.out)?;
    let mut made = vec![portable_zip(p, &gui, &cli)?];
    if p.platform.starts_with("linux") {
        made.push(deb(p, &gui, &cli)?);
    }
    if p.platform.starts_with("macos") {
        made.push(app_bundle(p, &gui, &cli)?);
    }
    Ok(made)
}

fn zip_options(mode: u32) -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(mode)
}

fn portable_zip(p: &Package<'_>, gui: &Path, cli: &Path) -> Result<PathBuf> {
    let path = p.out.join(format!(
        "TermCraft-{}-{}-portable.zip",
        p.version, p.platform
    ));
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path)?);
    for f in [gui, cli] {
        let name = f.file_name().context("file name")?.to_string_lossy();
        zip.start_file(format!("TermCraft/{name}"), zip_options(0o755))?;
        zip.write_all(&std::fs::read(f)?)?;
    }
    zip.start_file("TermCraft/termcraft.portable", zip_options(0o644))?;
    zip.finish()?;
    Ok(path)
}

/// Debian architecture of a platform name.
fn deb_arch(platform: &str) -> &'static str {
    if platform.ends_with("aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

fn tar_gz(entries: &[(&str, &[u8], u32)]) -> Result<Vec<u8>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    for (path, data, mode) in entries {
        let mut header = tar::Header::new_gnu();
        // A path ending in `/` is a folder.
        if path.ends_with('/') {
            header.set_entry_type(tar::EntryType::Directory);
        }
        header.set_size(data.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        tar.append_data(&mut header, path, *data)?;
    }
    Ok(tar.into_inner()?.finish()?)
}

/// One member of an `ar` archive (the .deb container).
fn ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let header = format!(
        "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
        name,
        0,
        0,
        0,
        "100644",
        data.len()
    );
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(b'\n');
    }
}

pub fn deb_control(version: &str, arch: &str, installed_kb: u64) -> String {
    format!(
        "Package: termcraft\nVersion: {version}\nArchitecture: {arch}\n\
         Maintainer: TermCraft contributors\nInstalled-Size: {installed_kb}\n\
         Depends: libc6, libxkbcommon0, libgl1\nSection: net\nPriority: optional\n\
         Description: Terminal and remote session manager\n \
         SSH, SFTP, Telnet, RDP, VNC, serial, tunnels and more in one window.\n"
    )
}

fn deb(p: &Package<'_>, gui: &Path, cli: &Path) -> Result<PathBuf> {
    let arch = deb_arch(p.platform);
    let (gui_bytes, cli_bytes) = (std::fs::read(gui)?, std::fs::read(cli)?);
    let installed = (gui_bytes.len() + cli_bytes.len()).div_ceil(1024) as u64;
    let control = deb_control(p.version, arch, installed);
    let control_tar = tar_gz(&[("./control", control.as_bytes(), 0o644)])?;
    let data_tar = tar_gz(&[
        ("./usr/", b"", 0o755),
        ("./usr/bin/", b"", 0o755),
        ("./usr/share/", b"", 0o755),
        ("./usr/share/applications/", b"", 0o755),
        ("./usr/bin/termcraft", &gui_bytes, 0o755),
        ("./usr/bin/termcraft-cli", &cli_bytes, 0o755),
        (
            "./usr/share/applications/termcraft.desktop",
            DESKTOP_ENTRY.as_bytes(),
            0o644,
        ),
    ])?;
    let mut deb = b"!<arch>\n".to_vec();
    ar_member(&mut deb, "debian-binary", b"2.0\n");
    ar_member(&mut deb, "control.tar.gz", &control_tar);
    ar_member(&mut deb, "data.tar.gz", &data_tar);
    let path = p.out.join(format!("termcraft_{}_{arch}.deb", p.version));
    std::fs::write(&path, deb)?;
    Ok(path)
}

pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>TermCraft</string>
  <key>CFBundleDisplayName</key><string>TermCraft</string>
  <key>CFBundleIdentifier</key><string>io.github.termcraft</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleExecutable</key><string>termcraft</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>TermCraft links</string>
      <key>CFBundleURLSchemes</key><array><string>termcraft</string></array>
    </dict>
  </array>
</dict>
</plist>
"#
    )
}

fn app_bundle(p: &Package<'_>, gui: &Path, cli: &Path) -> Result<PathBuf> {
    let path = p
        .out
        .join(format!("TermCraft-{}-{}.app.zip", p.version, p.platform));
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path)?);
    zip.start_file("TermCraft.app/Contents/Info.plist", zip_options(0o644))?;
    zip.write_all(info_plist(p.version).as_bytes())?;
    for (f, name) in [(gui, "termcraft"), (cli, "termcraft-cli")] {
        zip.start_file(
            format!("TermCraft.app/Contents/MacOS/{name}"),
            zip_options(0o755),
        )?;
        zip.write_all(&std::fs::read(f)?)?;
    }
    zip.finish()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bins(dir: &Path, platform: &str) {
        for n in ["termcraft", "termcraft-cli"] {
            std::fs::write(dir.join(exe(platform, n)), b"binary").unwrap();
        }
    }

    #[test]
    fn packages_per_platform() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        for platform in ["windows-x86_64", "linux-x86_64", "macos-aarch64"] {
            fake_bins(dir.path(), platform);
            let made = run(&Package {
                bin_dir: dir.path(),
                platform,
                version: "1.2.3",
                out: &out,
            })
            .unwrap();
            let names: Vec<String> = made
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            match platform {
                "linux-x86_64" => assert_eq!(
                    names,
                    [
                        "TermCraft-1.2.3-linux-x86_64-portable.zip",
                        "termcraft_1.2.3_amd64.deb"
                    ]
                ),
                "macos-aarch64" => assert_eq!(
                    names,
                    [
                        "TermCraft-1.2.3-macos-aarch64-portable.zip",
                        "TermCraft-1.2.3-macos-aarch64.app.zip"
                    ]
                ),
                _ => assert_eq!(names, ["TermCraft-1.2.3-windows-x86_64-portable.zip"]),
            }
        }
        let deb = std::fs::read(out.join("termcraft_1.2.3_amd64.deb")).unwrap();
        assert!(deb.starts_with(b"!<arch>\ndebian-binary   "));
        let zip = zip::ZipArchive::new(
            std::fs::File::open(out.join("TermCraft-1.2.3-macos-aarch64.app.zip")).unwrap(),
        )
        .unwrap();
        let names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        assert!(
            names
                .iter()
                .any(|n| n == "TermCraft.app/Contents/MacOS/termcraft")
        );
        assert!(
            names
                .iter()
                .any(|n| n == "TermCraft.app/Contents/Info.plist")
        );
        assert!(
            run(&Package {
                bin_dir: &dir.path().join("nothing"),
                platform: "linux-x86_64",
                version: "1",
                out: &out,
            })
            .is_err()
        );
    }

    #[test]
    fn metadata() {
        assert!(deb_control("1.0", "amd64", 10).contains("Architecture: amd64\n"));
        assert!(info_plist("2.0").contains("<string>2.0</string>"));
        assert_eq!(deb_arch("linux-aarch64"), "arm64");
    }
}
