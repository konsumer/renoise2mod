//! Dev-workflow helper, invoked as `cargo xtask <command>` (aliased in `.cargo/config.toml`).
//! Cargo itself has no "run this after the binary is linked" hook -- a crate's own `build.rs`
//! runs *before* that crate is compiled, so it can't copy its own not-yet-built output. This is
//! the standard workaround (see <https://github.com/matklad/cargo-xtask>).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();

    let result = match command.as_str() {
        "tool" => build_tool(),
        _ => {
            eprintln!("usage: cargo xtask <command>");
            eprintln!();
            eprintln!("commands:");
            eprintln!("  tool    Build renoise2mod in release mode and copy it into tool/bin/,");
            eprintln!(
                "          so the tool/ directory is ready to drag onto Renoise for local testing."
            );
            std::process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn workspace_root() -> PathBuf {
    // xtask's own manifest dir is <workspace_root>/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is always a workspace member with a parent directory")
        .to_path_buf()
}

/// Platform-specific binary name used inside the .xrnx bundle -- must match what `tool/main.lua`
/// looks for and what `.github/workflows/release.yml` produces.
fn bundled_binary_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "renoise2mod-darwin",
        "windows" => "renoise2mod-windows.exe",
        _ => "renoise2mod-linux",
    }
}

fn build_tool() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();

    let status = Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["build", "--release", "-p", "renoise2mod"])
        .status()?;
    if !status.success() {
        return Err("cargo build failed".into());
    }

    let built_binary_name = if cfg!(windows) {
        "renoise2mod.exe"
    } else {
        "renoise2mod"
    };
    let built_path = root.join("target/release").join(built_binary_name);

    let bin_dir = root.join("tool/bin");
    std::fs::create_dir_all(&bin_dir)?;

    let dest_path = bin_dir.join(bundled_binary_name());
    std::fs::copy(&built_path, &dest_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest_path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        std::fs::set_permissions(&dest_path, perms)?;
    }

    println!("copied {} -> {}", built_path.display(), dest_path.display());
    println!(
        "tool/ is ready -- drag the tool/ directory onto Renoise to install it for local testing."
    );

    Ok(())
}
