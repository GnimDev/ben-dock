//! Puts Bến's logo into `ben.exe` as its icon: what Explorer, Task Manager and a shortcut
//! show for the program, and what the settings window wears.
//!
//! An icon inside a program is a resource. `assets/ben.rc` names `assets/ben.ico`; the
//! resource compiler of the toolchain in use turns that into something the linker takes
//! (`rc.exe` of the Windows SDK with MSVC, `windres` of MinGW with GNU), and the linker is
//! told to take it. If no resource compiler is found, or it fails, the build goes on and the
//! program just has no icon of its own.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    for file in ["build.rs", "assets/ben.rc", "assets/ben.ico"] {
        println!("cargo:rerun-if-changed={file}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    match if msvc { with_rc(&out) } else { with_windres(&out) } {
        Some(made) => println!("cargo:rustc-link-arg-bins={}", made.display()),
        None => println!("cargo:warning=no working resource compiler ({}): ben.exe is built without its icon", if msvc { "rc.exe of the Windows SDK" } else { "windres of MinGW" }),
    }
}

/// With MSVC: `rc.exe`, from the PATH (a developer prompt has it there) or else the newest
/// one in the Windows SDK's own folder.
fn with_rc(out: &Path) -> Option<PathBuf> {
    let made = out.join("ben.res");
    let mut compilers = vec![PathBuf::from("rc.exe")];
    let arch = match std::env::var("HOST").unwrap_or_default().split('-').next() {
        Some("aarch64") => "arm64",
        Some("i686") | Some("i586") => "x86",
        _ => "x64",
    };
    for root in ["ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(|name| std::env::var_os(name)) {
        let bin = Path::new(&root).join("Windows Kits").join("10").join("bin");
        let mut versions: Vec<PathBuf> = std::fs::read_dir(&bin).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.join(arch).join("rc.exe").is_file()).collect();
        // The newest first: 10.0.26100.0 before 10.0.9200.0, by its numbers and not its letters.
        versions.sort_by_key(|path| std::cmp::Reverse(path.file_name().map(|name| name.to_string_lossy().split('.').map(|part| part.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())));
        compilers.extend(versions.into_iter().map(|path| path.join(arch).join("rc.exe")));
    }
    compilers.into_iter().any(|rc| ran(Command::new(rc).args(["/nologo", "/i", "assets", "/fo"]).arg(&made).arg("assets\\ben.rc"))).then_some(made)
}

/// With GNU: `windres`, which stands beside the `dlltool` and `as` that toolchain needs anyway.
fn with_windres(out: &Path) -> Option<PathBuf> {
    let made = out.join("ben_icon.o");
    ran(Command::new("windres").args(["--include-dir", "assets", "--output-format=coff", "--input", "assets/ben.rc", "--output"]).arg(&made)).then_some(made)
}

/// Runs a compiler in the project's folder; true if it was there and had no complaint.
fn ran(command: &mut Command) -> bool {
    command.current_dir(std::env::var("CARGO_MANIFEST_DIR").unwrap()).output().is_ok_and(|done| done.status.success())
}
