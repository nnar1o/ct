use crossterm::ExecutableCommand;
use crossterm::cursor::MoveTo;
use crossterm::terminal::{Clear, ClearType};
use ct::ctlog::{latest_log_path, parse_log_line, summarize_log};
use std::collections::VecDeque;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(args);
    process::exit(code);
}

fn run(args: Vec<String>) -> i32 {
    if args.is_empty() {
        print_usage();
        return 2;
    }

    match args[0].as_str() {
        "warnings" => print_latest_matches("warning"),
        "errors" => print_latest_matches("error"),
        "logs" => print_latest_log(&args[1..]),
        "status" => print_status(),
        "kill" => kill_latest(&args[1..]),
        "watch" => watch_latest(&args[1..]),
        _ => {
            eprintln!("ct-ctl: unknown command '{}'.", args[0]);
            print_usage();
            2
        }
    }
}

fn print_usage() {
    eprintln!("usage: ct-ctl warnings");
    eprintln!("       ct-ctl errors");
    eprintln!("       ct-ctl logs [--filter warning|error]");
    eprintln!("       ct-ctl status");
    eprintln!("       ct-ctl kill [--force]");
    eprintln!("       ct-ctl watch");
}

fn watch_latest(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("usage: ct-ctl watch");
        return 2;
    }

    let path = match latest_log_path() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("ct-ctl: {err}");
            return 1;
        }
    };

    let mut processed = 0usize;
    let mut pid: Option<u32> = None;
    let mut exit_code: Option<i32> = None;
    let mut tail = VecDeque::<String>::with_capacity(5);

    loop {
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("ct-ctl: failed to read log: {err}");
                return 1;
            }
        };

        let lines: Vec<&str> = contents.lines().collect();
        for line in lines.iter().skip(processed) {
            if let Some((kind, payload)) = parse_log_line(line) {
                match kind.as_str() {
                    "PID" => {
                        pid = payload.parse::<u32>().ok();
                    }
                    "EXIT" => {
                        exit_code = payload.parse::<i32>().ok();
                    }
                    "STDOUT" | "STDERR" => {
                        for chunk_line in payload.lines() {
                            if chunk_line.trim().is_empty() {
                                continue;
                            }
                            let entry = if kind == "STDERR" {
                                format!("[err] {chunk_line}")
                            } else {
                                chunk_line.to_string()
                            };
                            tail.push_back(entry);
                            while tail.len() > 5 {
                                let _ = tail.pop_front();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        processed = lines.len();

        render_watch(pid, exit_code, &tail);

        if exit_code.is_some() {
            return 0;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn render_watch(pid: Option<u32>, exit_code: Option<i32>, tail: &VecDeque<String>) {
    let mut out = io::stderr().lock();
    if io::stderr().is_terminal() {
        let _ = out.execute(MoveTo(0, 0));
        let _ = out.execute(Clear(ClearType::All));
    }

    let status = if let Some(code) = exit_code {
        format!("EXITED({code})")
    } else {
        "RUNNING".to_string()
    };
    let pid_text = pid
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".to_string());

    let _ = writeln!(out, "ct-ctl watch | pid={pid_text} | status={status}");
    for line in tail {
        let _ = writeln!(out, "{line}");
    }
    if exit_code.is_some() {
        let _ = writeln!(out, "(process finished, watcher exits)");
    } else {
        let _ = writeln!(out, "(Ctrl+C to close watcher without killing process)");
    }
    let _ = out.flush();
}

fn print_latest_log(args: &[String]) -> i32 {
    let mut filter: Option<&str> = None;
    if args.len() == 2 && args[0] == "--filter" {
        filter = Some(args[1].as_str());
    } else if !args.is_empty() {
        eprintln!("usage: ct-ctl logs [--filter warning|error]");
        return 2;
    }

    let contents = match read_latest_log() {
        Ok(c) => c,
        Err(code) => return code,
    };

    if let Some(f) = filter {
        let summary = summarize_log(&contents);
        let values = if f == "warning" {
            summary.warnings
        } else if f == "error" {
            summary.errors
        } else {
            eprintln!("ct-ctl: unknown filter '{f}', expected warning|error");
            return 2;
        };

        for line in values {
            println!("{line}");
        }
        return 0;
    }

    print!("{contents}");
    0
}

fn print_latest_matches(kind: &str) -> i32 {
    let contents = match read_latest_log() {
        Ok(c) => c,
        Err(code) => return code,
    };

    let summary = summarize_log(&contents);
    let values = if kind == "warning" {
        summary.warnings
    } else {
        summary.errors
    };

    for line in &values {
        println!("{line}");
    }

    if kind == "warning" {
        println!("TOTAL_WARNINGS={}", values.len());
    } else {
        println!("TOTAL_ERRORS={}", values.len());
    }
    0
}

fn print_status() -> i32 {
    let contents = match read_latest_log() {
        Ok(c) => c,
        Err(code) => return code,
    };

    let pid = extract_last_pid(&contents);
    let exit = extract_exit_code(&contents);

    match (pid, exit) {
        (Some(p), Some(code)) => {
            println!("STATUS=EXITED pid={p} exit={code}");
            0
        }
        (Some(p), None) => {
            if is_pid_running(p) {
                println!("STATUS=RUNNING pid={p}");
                0
            } else {
                println!("STATUS=UNKNOWN pid={p}");
                1
            }
        }
        (None, Some(code)) => {
            println!("STATUS=EXITED exit={code}");
            0
        }
        (None, None) => {
            println!("STATUS=UNKNOWN");
            1
        }
    }
}

fn kill_latest(args: &[String]) -> i32 {
    let force = if args.is_empty() {
        false
    } else if args.len() == 1 && args[0] == "--force" {
        true
    } else {
        eprintln!("usage: ct-ctl kill [--force]");
        return 2;
    };

    let contents = match read_latest_log() {
        Ok(c) => c,
        Err(code) => return code,
    };

    let pid = match extract_last_pid(&contents) {
        Some(p) => p,
        None => {
            eprintln!("ct-ctl: no PID found in latest log");
            return 1;
        }
    };

    if !is_pid_running(pid) {
        println!("NOT_RUNNING pid={pid}");
        return 0;
    }

    let signal = if force { "-KILL" } else { "-TERM" };
    let status = process::Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .status();

    match status {
        Ok(s) if s.success() => {
            if force {
                println!("KILLED pid={pid} signal=KILL");
            } else {
                println!("KILLED pid={pid} signal=TERM");
            }
            0
        }
        Ok(_) => {
            eprintln!("ct-ctl: failed to kill pid={pid}");
            1
        }
        Err(err) => {
            eprintln!("ct-ctl: failed to execute kill: {err}");
            1
        }
    }
}

fn read_latest_log() -> Result<String, i32> {
    let path = match latest_log_path() {
        Ok(p) => p,
        Err(err) => {
            eprintln!("ct-ctl: {err}");
            return Err(1);
        }
    };

    read_log_file(path)
}

fn read_log_file(path: PathBuf) -> Result<String, i32> {
    match fs::read_to_string(path) {
        Ok(c) => Ok(c),
        Err(err) => {
            eprintln!("ct-ctl: failed to read log: {err}");
            Err(1)
        }
    }
}

fn extract_last_pid(contents: &str) -> Option<u32> {
    for line in contents.lines().rev() {
        if let Some(pid) = parse_u32_record(line, "PID") {
            return Some(pid);
        }
    }
    for line in contents.lines().rev() {
        if let Some(pid) = parse_u32_record(line, "ASYNC") {
            return Some(pid);
        }
    }
    None
}

fn extract_exit_code(contents: &str) -> Option<i32> {
    for line in contents.lines().rev() {
        if let Some(code) = parse_i32_record(line, "EXIT") {
            return Some(code);
        }
    }
    None
}

fn parse_u32_record(line: &str, kind: &str) -> Option<u32> {
    let mut parts = line.splitn(3, ' ');
    let _ts = parts.next()?;
    let k = parts.next()?;
    if k != kind {
        return None;
    }
    let payload = parts.next()?.trim();
    payload.parse::<u32>().ok()
}

fn parse_i32_record(line: &str, kind: &str) -> Option<i32> {
    let mut parts = line.splitn(3, ' ');
    let _ts = parts.next()?;
    let k = parts.next()?;
    if k != kind {
        return None;
    }
    let payload = parts.next()?.trim();
    payload.parse::<i32>().ok()
}

fn is_pid_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
