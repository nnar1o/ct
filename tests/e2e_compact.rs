#![cfg(unix)]

use std::process::Command;

fn shell_escape(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }

    if input.bytes().all(|b| {
        matches!(
            b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/' | b':' | b'='
        )
    }) {
        return input.to_string();
    }

    let escaped = input.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn run_ct_in_pty(args: &[&str]) -> (String, bool) {
    let bin = env!("CARGO_BIN_EXE_ct");
    let mut command_line = shell_escape(bin);
    for arg in args {
        command_line.push(' ');
        command_line.push_str(&shell_escape(arg));
    }

    let output = Command::new("script")
        .args(["-qec", &command_line, "/dev/null"])
        .output()
        .expect("run command under pseudo-tty");

    let mut merged = String::new();
    merged.push_str(&String::from_utf8_lossy(&output.stdout));
    merged.push_str(&String::from_utf8_lossy(&output.stderr));
    (merged, output.status.success())
}

#[test]
fn quick_process_in_default_mode_keeps_plain_output_without_compact_summary() {
    let (output, ok) = run_ct_in_pty(&["bash", "-lc", "printf 'fast-e2e\\n'"]);
    assert!(ok, "ct quick command should exit successfully: {output}");
    assert!(
        output.contains("fast-e2e"),
        "expected passthrough output: {output}"
    );
    assert!(
        !output.contains("SUCCESS") && !output.contains("FAILED"),
        "quick process should not switch to compact summary: {output}"
    );
}

#[test]
fn slow_process_in_default_mode_switches_to_compact_summary() {
    let (output, ok) = run_ct_in_pty(&["bash", "-lc", "sleep 4; printf 'slow-e2e\\n'"]);
    assert!(ok, "ct slow command should exit successfully: {output}");
    assert!(
        output.contains("SUCCESS"),
        "slow process should switch to compact summary: {output}"
    );
}

#[test]
fn compact_flag_forces_summary_even_for_quick_process() {
    let (output, ok) = run_ct_in_pty(&["--compact", "bash", "-lc", "printf 'forced-e2e\\n'"]);
    assert!(
        ok,
        "ct --compact command should exit successfully: {output}"
    );
    assert!(
        output.contains("SUCCESS"),
        "forced compact mode should print summary: {output}"
    );
}
