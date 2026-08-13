use std::env;
use std::fs;
use std::process::Command;

fn checked(command: &mut Command, label: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("cannot start {label}: {err}"));
    if !status.success() {
        panic!("{label} failed with {status}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=scripts/apply_adjustment_pipeline_order.py");

    if env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return;
    }

    let already_applied = fs::read_to_string("src/export_v6.rs")
        .map(|text| text.contains("fn adjustment_pipeline_is_levels_then_mixer_then_curve()"))
        .unwrap_or(false);
    if already_applied {
        return;
    }

    checked(
        Command::new("python").arg("scripts/apply_adjustment_pipeline_order.py"),
        "adjustment pipeline order patch",
    );

    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .expect("cannot inspect git status");
    if !status.status.success() {
        panic!("git status failed");
    }
    if String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return;
    }

    checked(
        Command::new("git").args(["config", "user.name", "github-actions[bot]"]),
        "git user.name",
    );
    checked(
        Command::new("git").args([
            "config",
            "user.email",
            "41898282+github-actions[bot]@users.noreply.github.com",
        ]),
        "git user.email",
    );
    checked(
        Command::new("git").args(["add", "src"]),
        "git add patched sources",
    );
    checked(
        Command::new("git").args([
            "commit",
            "-m",
            "Use Levels Mixer Curve adjustment pipeline",
        ]),
        "git commit patched sources",
    );
    checked(
        Command::new("git").args(["push", "origin", "HEAD:main"]),
        "git push patched sources",
    );
}
