use chrono::Utc;
use ct::ctlog::{ct_home, logs_dir};
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const MAX_COMPACT_REFRESH_HZ: u64 = 20;
const TAIL_LINES: usize = 5;
const ADAPTIVE_COMPACT_AFTER_SECS: u64 = 2;
const DEFAULT_MAX_ERROR_LINES: usize = 5;
const ADAPTIVE_COMPACT_ENV: &str = "CT_ADAPTIVE_COMPACT";
const BUILTIN_FILTER_PROFILE_FILES: [(&str, &str); 16] = [
    ("cargo.toml", include_str!("../filters.d/cargo.toml")),
    ("maven.toml", include_str!("../filters.d/maven.toml")),
    ("gradle.toml", include_str!("../filters.d/gradle.toml")),
    ("npm.toml", include_str!("../filters.d/npm.toml")),
    ("pnpm.toml", include_str!("../filters.d/pnpm.toml")),
    ("yarn.toml", include_str!("../filters.d/yarn.toml")),
    ("gcc.toml", include_str!("../filters.d/gcc.toml")),
    ("clang.toml", include_str!("../filters.d/clang.toml")),
    ("cpp.toml", include_str!("../filters.d/cpp.toml")),
    ("make.toml", include_str!("../filters.d/make.toml")),
    ("cmake.toml", include_str!("../filters.d/cmake.toml")),
    ("go.toml", include_str!("../filters.d/go.toml")),
    ("pytest.toml", include_str!("../filters.d/pytest.toml")),
    ("dotnet.toml", include_str!("../filters.d/dotnet.toml")),
    ("jest.toml", include_str!("../filters.d/jest.toml")),
    ("vitest.toml", include_str!("../filters.d/vitest.toml")),
];

fn compact_refresh_interval() -> Duration {
    let ms = (1000 / MAX_COMPACT_REFRESH_HZ).max(1);
    Duration::from_millis(ms)
}

fn adaptive_compact_enabled() -> bool {
    match env::var(ADAPTIVE_COMPACT_ENV) {
        Ok(value) => {
            let lower = value.trim().to_ascii_lowercase();
            !matches!(lower.as_str(), "0" | "false" | "no" | "off")
        }
        Err(_) => true,
    }
}

#[derive(Clone, Default)]
struct CompactStats {
    line_count: u64,
    fatal_count: u64,
    warning_count: u64,
    error_count: u64,
    info_count: u64,
    trace_count: u64,
    unknown_count: u64,
    last_error: String,
    progress_percent: Option<i64>,
}

#[derive(Clone, Default)]
struct CommandSummary {
    warning_count: u64,
    error_count: u64,
    error_lines: VecDeque<String>,
}

#[derive(Clone, Deserialize)]
struct CtConfig {
    #[serde(default)]
    output: OutputConfig,
    #[serde(default)]
    filters: FiltersConfig,
    #[serde(default)]
    heuristics: HeuristicsConfig,
}

#[derive(Clone, Deserialize)]
struct OutputConfig {
    #[serde(default = "default_max_error_lines")]
    max_error_lines: usize,
}

#[derive(Clone, Deserialize)]
struct FiltersConfig {
    #[serde(flatten)]
    tools: HashMap<String, CommandFilterConfig>,
}

#[derive(Clone, Deserialize)]
struct CommandFilterConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "default_warning_patterns")]
    warning_patterns: Vec<String>,
    #[serde(default = "default_error_patterns")]
    error_patterns: Vec<String>,
    #[serde(default = "default_error_capture_patterns")]
    error_capture_patterns: Vec<String>,
    #[serde(default)]
    detection_regex: Vec<String>,
    #[serde(default)]
    level_patterns: LevelPatternsConfig,
}

#[derive(Clone, Deserialize, Default)]
struct LevelPatternsConfig {
    #[serde(default)]
    fatal: Vec<String>,
    #[serde(default)]
    error: Vec<String>,
    #[serde(default)]
    warning: Vec<String>,
    #[serde(default)]
    info: Vec<String>,
    #[serde(default)]
    trace: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct HeuristicsConfig {
    #[serde(default = "default_auto_detect_log_type")]
    auto_detect_log_type: bool,
}

#[derive(Deserialize, Default)]
struct CtConfigFile {
    #[serde(default)]
    output: OutputConfigFile,
    #[serde(default)]
    filters: HashMap<String, CommandFilterConfig>,
    #[serde(default)]
    heuristics: HeuristicsConfigFile,
}

#[derive(Deserialize, Default)]
struct OutputConfigFile {
    max_error_lines: Option<usize>,
}

#[derive(Deserialize, Default)]
struct HeuristicsConfigFile {
    auto_detect_log_type: Option<bool>,
}

#[derive(Deserialize)]
struct FilterProfileFile {
    tool: String,
    #[serde(flatten)]
    filter: CommandFilterConfig,
}

impl Default for CommandFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            aliases: Vec::new(),
            warning_patterns: default_warning_patterns(),
            error_patterns: default_error_patterns(),
            error_capture_patterns: default_error_capture_patterns(),
            detection_regex: Vec::new(),
            level_patterns: LevelPatternsConfig::default(),
        }
    }
}

impl Default for CtConfig {
    fn default() -> Self {
        Self {
            output: OutputConfig::default(),
            filters: FiltersConfig::default(),
            heuristics: HeuristicsConfig::default(),
        }
    }
}

impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }
}

impl Default for HeuristicsConfig {
    fn default() -> Self {
        Self {
            auto_detect_log_type: default_auto_detect_log_type(),
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            max_error_lines: default_max_error_lines(),
        }
    }
}

fn default_max_error_lines() -> usize {
    DEFAULT_MAX_ERROR_LINES
}

fn default_warning_patterns() -> Vec<String> {
    vec!["warning:".to_string(), "[warning]".to_string()]
}

fn default_error_patterns() -> Vec<String> {
    vec![
        "error:".to_string(),
        "error[".to_string(),
        "[error]".to_string(),
        "build failure".to_string(),
    ]
}

fn default_error_capture_patterns() -> Vec<String> {
    vec![
        "error:".to_string(),
        "error[".to_string(),
        "[error]".to_string(),
        "build failure".to_string(),
    ]
}

fn default_auto_detect_log_type() -> bool {
    true
}

#[derive(Clone)]
struct HeuristicCandidate {
    tool: String,
    filter: CommandFilterConfig,
    regexes: Vec<Regex>,
}

#[derive(Clone)]
struct HeuristicDetector {
    candidates: Arc<Vec<HeuristicCandidate>>,
    selected: Arc<Mutex<Option<(String, CommandFilterConfig)>>>,
}

impl HeuristicDetector {
    fn from_config(cfg: &CtConfig) -> Option<Self> {
        if !cfg.heuristics.auto_detect_log_type {
            return None;
        }

        let mut candidates = Vec::new();
        for (tool, filter) in &cfg.filters.tools {
            if !filter.enabled || filter.detection_regex.is_empty() {
                continue;
            }

            let regexes = filter
                .detection_regex
                .iter()
                .filter_map(|pattern| Regex::new(pattern).ok())
                .collect::<Vec<_>>();
            if regexes.is_empty() {
                continue;
            }

            candidates.push(HeuristicCandidate {
                tool: tool.to_string(),
                filter: filter.clone(),
                regexes,
            });
        }

        if candidates.is_empty() {
            return None;
        }

        Some(Self {
            candidates: Arc::new(candidates),
            selected: Arc::new(Mutex::new(None)),
        })
    }

    fn selected_filter(&self) -> Option<CommandFilterConfig> {
        self.selected
            .lock()
            .ok()
            .and_then(|v| v.as_ref().map(|(_, filter)| filter.clone()))
    }

    fn detect_line_if_needed(&self, line: &str) -> Option<(String, CommandFilterConfig)> {
        if let Ok(selected) = self.selected.lock()
            && selected.is_some()
        {
            return None;
        }

        let normalized = normalize_log_line(line);
        if normalized.is_empty() {
            return None;
        }

        for candidate in self.candidates.iter() {
            if candidate.regexes.iter().any(|re| re.is_match(&normalized)) {
                let detected = (candidate.tool.clone(), candidate.filter.clone());
                if let Ok(mut selected) = self.selected.lock()
                    && selected.is_none()
                {
                    *selected = Some(detected.clone());
                    return Some(detected);
                }
                return None;
            }
        }

        None
    }
}

#[derive(Clone)]
struct RunLogger {
    file: Arc<Mutex<File>>,
    sh_file: Arc<Mutex<Option<File>>>,
    log_path: PathBuf,
    start_ts_ms: i64,
    seen_hashes: Arc<Mutex<std::collections::HashSet<String>>>,
    hash_timestamps: Arc<Mutex<std::collections::HashMap<String, Vec<(i64, i64)>>>>,
    exit_logged: Arc<Mutex<bool>>,
}

#[derive(Clone)]
struct BufferedChunk {
    seq: u64,
    kind: String,
    data: Vec<u8>,
}

#[derive(Clone)]
struct CaptureOptions {
    passthrough: Arc<AtomicBool>,
    compact_enabled: Arc<AtomicBool>,
    compact_used: Arc<AtomicBool>,
    output_flag: Arc<AtomicBool>,
    tail_lines: Arc<Mutex<VecDeque<String>>>,
    stats: Arc<Mutex<CompactStats>>,
    command_summary: Option<Arc<Mutex<CommandSummary>>>,
    command_filter: Option<CommandFilterConfig>,
    heuristic_detector: Option<HeuristicDetector>,
    max_error_lines: usize,
    buffered_chunks: Option<Arc<Mutex<Vec<BufferedChunk>>>>,
    buffer_enabled: Option<Arc<AtomicBool>>,
    seq: Option<Arc<AtomicU64>>,
    historical_percents: Option<Arc<HashMap<String, i64>>>,
}

fn payload_hash(payload: &str) -> String {
    let normalized: String = payload.chars().filter(|c| !c.is_ascii_digit()).collect();
    format!("{:x}", md5::compute(normalized.as_bytes()))
}

fn line_hash_for_stats(line: &str) -> Option<String> {
    let payload = serde_json::to_string(line).ok()?;
    Some(payload_hash(&payload))
}

fn parse_stats_percent_map(content: &str) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    for line in content.lines() {
        if line.trim().is_empty() || line.starts_with("EXEC ") || line == "EXEC-END" {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(percent_raw) = parts.next() else {
            continue;
        };
        let Some(percent) = percent_raw.parse::<i64>().ok() else {
            continue;
        };
        out.insert(hash.to_string(), percent.clamp(0, 100));
    }
    out
}

fn load_stats_percent_map_for_command(command: &str) -> HashMap<String, i64> {
    let cmd_hash = format!("{:x}", md5::compute(command.as_bytes()));
    let cmd_hash_prefix = &cmd_hash[..8];
    let dir = match logs_dir() {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };
    let stats_path = dir.join(format!("{}.stats", cmd_hash_prefix));
    match fs::read_to_string(stats_path) {
        Ok(content) => parse_stats_percent_map(&content),
        Err(_) => HashMap::new(),
    }
}

fn parse_exec_header(line: &str) -> Option<(i64, i64)> {
    let rest = line.strip_prefix("EXEC ")?;
    let mut parts = rest.split_whitespace();
    let exec_ts = parts.next()?.parse().ok()?;
    let duration = parts.next()?.parse().ok()?;
    Some((exec_ts, duration))
}

fn parse_stats_hash_line(line: &str) -> Option<(String, Vec<(i64, i64)>)> {
    let mut parts = line.split_whitespace();
    let hash = parts.next()?.to_string();
    let percent = parts.next()?;
    if percent.parse::<i64>().is_err() {
        return None;
    }

    let mut entries = Vec::new();
    for token in parts {
        if !token.contains(':') {
            continue;
        }
        let mut ts = token.split(':');
        let exec_ts: i64 = ts.next()?.parse().ok()?;
        let rel_ts: i64 = ts.next()?.parse().ok()?;
        entries.push((exec_ts, rel_ts));
    }

    if entries.is_empty() {
        return None;
    }

    Some((hash, entries))
}

fn merge_stats_content(
    existing: &str,
    exec_ts: i64,
    duration: i64,
    current_run: &HashMap<String, Vec<(i64, i64)>>,
) -> String {
    let mut exec_runs: HashMap<i64, i64> = HashMap::new();
    let mut hash_history: HashMap<String, std::collections::HashSet<(i64, i64)>> = HashMap::new();

    for line in existing.lines() {
        if let Some((ts, dur)) = parse_exec_header(line) {
            exec_runs.insert(ts, dur);
            continue;
        }
        if line == "EXEC-END" || line.trim().is_empty() {
            continue;
        }
        if let Some((hash, entries)) = parse_stats_hash_line(line) {
            let set = hash_history.entry(hash).or_default();
            for pair in entries {
                set.insert(pair);
            }
        }
    }

    exec_runs.insert(exec_ts, duration);

    let mut ordered_exec_runs: Vec<(i64, i64)> = exec_runs.into_iter().collect();
    ordered_exec_runs.sort_by_key(|(ts, _)| *ts);
    if ordered_exec_runs.len() > 10 {
        ordered_exec_runs = ordered_exec_runs.split_off(ordered_exec_runs.len() - 10);
    }
    let kept_exec_ts: std::collections::HashSet<i64> =
        ordered_exec_runs.iter().map(|(ts, _)| *ts).collect();

    for (hash, entries) in current_run {
        let set = hash_history.entry(hash.clone()).or_default();
        for &(entry_exec_ts, rel_ts) in entries {
            if kept_exec_ts.contains(&entry_exec_ts) {
                set.insert((entry_exec_ts, rel_ts));
            }
        }
    }

    let mut ordered_hash_lines: Vec<(String, Vec<(i64, i64)>)> = Vec::new();
    for (hash, entries) in hash_history {
        let mut filtered: Vec<(i64, i64)> = entries
            .into_iter()
            .filter(|(entry_exec_ts, _)| kept_exec_ts.contains(entry_exec_ts))
            .collect();
        if filtered.is_empty() {
            continue;
        }
        filtered.sort_by_key(|(entry_exec_ts, rel_ts)| (*entry_exec_ts, *rel_ts));
        if filtered.len() > 10 {
            filtered = filtered.split_off(filtered.len() - 10);
        }
        ordered_hash_lines.push((hash, filtered));
    }
    ordered_hash_lines.sort_by(|a, b| a.0.cmp(&b.0));

    let run_duration_by_ts: HashMap<i64, i64> = ordered_exec_runs.iter().copied().collect();

    let mut out = String::new();
    for (run_exec_ts, run_duration) in ordered_exec_runs {
        out.push_str(&format!("EXEC {} {}\n", run_exec_ts, run_duration));
        out.push_str("EXEC-END\n");
    }

    for (hash, entries) in ordered_hash_lines {
        let mut pct_sum: i64 = 0;
        let mut pct_count: i64 = 0;
        for (entry_exec_ts, rel_ts) in &entries {
            if let Some(duration_for_run) = run_duration_by_ts.get(entry_exec_ts) {
                if *duration_for_run > 0 {
                    let pct = ((*rel_ts * 100) / *duration_for_run).clamp(0, 100);
                    pct_sum += pct;
                    pct_count += 1;
                }
            }
        }
        let percent = if pct_count > 0 { pct_sum / pct_count } else { 0 };
        let avg_rel = entries.iter().map(|(_, rel_ts)| *rel_ts).sum::<i64>() / entries.len() as i64;
        let joined = entries
            .iter()
            .map(|(entry_exec_ts, rel_ts)| format!("{}:{}", entry_exec_ts, rel_ts))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str(&format!("{} {} {} {}\n", hash, percent, avg_rel, joined));
    }
    out
}

impl RunLogger {
    fn new(cmd: &str) -> io::Result<Self> {
        let dir = logs_dir()?;
        fs::create_dir_all(&dir)?;

        let cmd_hash = format!("{:x}", md5::compute(cmd.as_bytes()));
        let timestamp = Utc::now().timestamp_millis();
        let run_id = format!("{}-{}", &cmd_hash[..8], timestamp);
        let log_path = dir.join(format!("{}.log", run_id));
        let sh_path = dir.join(format!("{}.logsh", run_id));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let sh_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sh_path)
            .ok();

        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            sh_file: Arc::new(Mutex::new(sh_file)),
            log_path,
            start_ts_ms: timestamp,
            seen_hashes: Arc::new(Mutex::new(std::collections::HashSet::new())),
            hash_timestamps: Arc::new(Mutex::new(std::collections::HashMap::new())),
            exit_logged: Arc::new(Mutex::new(false)),
        })
    }

    fn log_command(&self, command: &str) {
        let line = format!("{} {command}\n", self.start_ts_ms);
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn log_json(&self, kind: &str, text: &str) {
        if let Ok(payload) = serde_json::to_string(text) {
            self.log_raw(kind, &payload);
        }
    }

    fn log_json_with_level(&self, level: char, kind: &str, text: &str) {
        if let Ok(payload) = serde_json::to_string(text) {
            self.log_raw_with_level(level, kind, &payload);
        }
    }

    fn log_exit(&self, code: i32) {
        let mut logged = self.exit_logged.lock().unwrap();
        if *logged {
            return;
        }
        *logged = true;
        let level = if code == 0 { 'I' } else { 'E' };
        self.log_raw_with_level(level, "EXIT", &code.to_string());
    }

    fn set_latest(&self) {
        if let Some(name) = self.log_path.file_name().and_then(|x| x.to_str()) {
            let latest_path = self.log_path.with_file_name(".latest");
            let _ = fs::write(latest_path, name);
        }
        self.write_stats();
    }

    fn write_stats(&self) {
        let cmd_hash = self
            .log_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('-').next())
            .unwrap_or("unknown");
        let stats_path = self
            .log_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(format!("{}.stats", cmd_hash));
        let exec_ts = self.start_ts_ms;
        let duration = Utc::now().timestamp_millis().saturating_sub(self.start_ts_ms);

        let existing = fs::read_to_string(&stats_path).unwrap_or_default();
        let current_run = self
            .hash_timestamps
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        let content = merge_stats_content(&existing, exec_ts, duration, &current_run);
        let _ = fs::write(&stats_path, content);
    }

    fn log_raw(&self, kind: &str, payload: &str) {
        self.log_raw_with_level(default_level_for_kind(kind), kind, payload);
    }

    fn log_raw_with_level(&self, level: char, kind: &str, payload: &str) {
        let rel_ms = Utc::now()
            .timestamp_millis()
            .saturating_sub(self.start_ts_ms);
        let line = format!("{rel_ms} {level} {kind} {payload}\n");
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
        }
        let hash = payload_hash(payload);

        let already_seen;
        {
            let mut seen = self.seen_hashes.lock().unwrap();
            if seen.contains(&hash) {
                already_seen = true;
            } else {
                seen.insert(hash.clone());
                already_seen = false;
            }
        }

        if !already_seen {
            let sh_line = format!("{} {}\n", rel_ms, hash);
            if let Ok(mut sh) = self.sh_file.lock() {
                if let Some(ref mut f) = *sh {
                    let _ = f.write_all(sh_line.as_bytes());
                }
            }
            if let Ok(mut ts_map) = self.hash_timestamps.lock() {
                let entry = ts_map.entry(hash).or_insert_with(Vec::new);
                entry.push((self.start_ts_ms, rel_ms));
            }
        }
    }

    fn from_path(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let start_ts_ms = fs::read_to_string(&path)
            .ok()
            .and_then(|v| {
                v.lines()
                    .next()
                    .and_then(|line| line.split_whitespace().next())
                    .and_then(|ts| ts.parse::<i64>().ok())
            })
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            sh_file: Arc::new(Mutex::new(None)),
            log_path: path,
            start_ts_ms,
            seen_hashes: Arc::new(Mutex::new(std::collections::HashSet::new())),
            hash_timestamps: Arc::new(Mutex::new(std::collections::HashMap::new())),
            exit_logged: Arc::new(Mutex::new(false)),
        })
    }

    fn log_path_string(&self) -> String {
        self.log_path.display().to_string()
    }
}

fn default_level_for_kind(kind: &str) -> char {
    match kind {
        "STDERR" => 'U',
        "STDOUT" => 'U',
        _ => 'U',
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = run(args);
    process::exit(code);
}

fn run(args: Vec<String>) -> i32 {
    if args.is_empty() {
        print_usage();
        return 2;
    }

    match args[0].as_str() {
        "--async-runner" => run_async_runner(&args[1..]),
        "--shell-bridge" => shell_bridge(&args[1..]),
        "--compact" | "-c" => {
            if args.len() < 2 {
                eprintln!("usage: ct --compact <command> [args...]");
                2
            } else {
                run_external(&args[1], &args[2..], true, false)
            }
        }
        "--stats" => {
            if args.len() < 2 {
                eprintln!("usage: ct --stats <command> [args...]");
                2
            } else {
                run_external(&args[1], &args[2..], false, true)
            }
        }
        "mcp" => {
            eprintln!("ct mcp: not implemented in this MVP yet");
            1
        }
        "cd" => {
            eprintln!("ct: 'cd' requires shell integration. Run ct-install and source ~/.bashrc");
            2
        }
        "-a" => run_async_mode(&args[1..]),
        _ => run_external(&args[0], &args[1..], false, false),
    }
}

fn print_usage() {
    eprintln!("usage: ct <command> [args...]");
    eprintln!("       ct --compact <command> [args...]");
    eprintln!("       ct --stats <command> [args...]");
    eprintln!("       ct -a <command> [args...]");
    eprintln!("       ct mcp");
    eprintln!("       ct --shell-bridge cd [args...]");
}

fn run_async_mode(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("usage: ct -a <command> [args...]");
        return 2;
    }

    let cmd = format_command(&args[0], &args[1..]);
    let logger = match RunLogger::new(&cmd) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("ct: failed to initialize logger: {err}");
            return 1;
        }
    };

    logger.log_command(&cmd);
    logger.log_raw("MODE", "\"async\"");
    logger.set_latest();

    let self_exe = match env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("ct: failed to resolve executable path: {err}");
            return 1;
        }
    };

    let mut cmd = Command::new(self_exe);
    cmd.arg("--async-runner")
        .arg(logger.log_path_string())
        .arg(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            logger.log_raw("ASYNC", &child.id().to_string());
            println!("STARTED pid={}", child.id());
            0
        }
        Err(err) => {
            let msg = format!("ct: failed to start async runner: {err}");
            eprintln!("{msg}");
            logger.log_json("STDERR", &msg);
            logger.log_exit(1);
            1
        }
    }
}

fn run_async_runner(args: &[String]) -> i32 {
    if args.len() < 2 {
        return 2;
    }

    let log_path = PathBuf::from(&args[0]);
    let cmd = &args[1];
    let cmd_args = &args[2..];

    let logger = match RunLogger::from_path(log_path) {
        Ok(l) => l,
        Err(_) => return 1,
    };

    let mut child = match Command::new(cmd)
        .args(cmd_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            let code = map_spawn_error_code(&err);
            logger.log_json("STDERR", &format_spawn_error(cmd, &err));
            logger.log_exit(code);
            return code;
        }
    };

    logger.log_raw("PID", &child.id().to_string());

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let out_logger = logger.clone();
    let out_thread = thread::spawn(move || {
        if let Some(mut reader) = stdout {
            let _ = capture_stream(&mut reader, "STDOUT", Some(&out_logger), None);
        }
    });

    let err_logger = logger.clone();
    let err_thread = thread::spawn(move || {
        if let Some(mut reader) = stderr {
            let _ = capture_stream(&mut reader, "STDERR", Some(&err_logger), None);
        }
    });

    let code = match child.wait() {
        Ok(status) => exit_code(status),
        Err(_) => 1,
    };

    let _ = out_thread.join();
    let _ = err_thread.join();
    logger.log_exit(code);
    code
}

fn shell_bridge(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("ct --shell-bridge: missing command");
        return 2;
    }

    if args[0] != "cd" {
        eprintln!("ct --shell-bridge: only 'cd' is supported in MVP");
        return 2;
    }

    let mut script = String::from("cd");
    for arg in &args[1..] {
        script.push(' ');
        script.push_str(&shell_escape(arg));
    }
    println!("__CT_BUILTIN__ {script}");
    0
}

fn run_external(cmd: &str, cmd_args: &[String], force_compact: bool, show_stats: bool) -> i32 {
    let config = load_global_config();
    let active_filter = command_filter_for(cmd, &config);
    let active_filter_cfg = active_filter.map(|(_, filter)| filter.clone());
    let heuristic_detector = if active_filter.is_none() {
        HeuristicDetector::from_config(&config)
    } else {
        None
    };
    let max_error_lines = config.output.max_error_lines;
    let filtered_mode = active_filter.is_some();
    let summary_mode = filtered_mode || heuristic_detector.is_some();
    let compact_mode = force_compact || filtered_mode;
    let interactive_tty = io::stderr().is_terminal();
    let adaptive_compact = interactive_tty && !compact_mode && adaptive_compact_enabled();
    let had_output = Arc::new(AtomicBool::new(false));
    let passthrough_enabled = Arc::new(AtomicBool::new(!adaptive_compact && !compact_mode));
    let compact_enabled = Arc::new(AtomicBool::new(compact_mode));
    let compact_used = Arc::new(AtomicBool::new(compact_mode));
    let process_running = Arc::new(AtomicBool::new(true));
    let buffer_enabled = Arc::new(AtomicBool::new(adaptive_compact));
    let buffered_chunks = Arc::new(Mutex::new(Vec::<BufferedChunk>::new()));
    let chunk_seq = Arc::new(AtomicU64::new(0));
    let tail_lines = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL_LINES)));
    let stats = Arc::new(Mutex::new(CompactStats::default()));
    let command_summary = if summary_mode {
        Some(Arc::new(Mutex::new(CommandSummary::default())))
    } else {
        None
    };
    let full_cmd = format_command(cmd, cmd_args);
    let historical_percents = load_stats_percent_map_for_command(&full_cmd);
    let historical_percents = if historical_percents.is_empty() {
        None
    } else {
        Some(Arc::new(historical_percents))
    };
    let logger = RunLogger::new(&full_cmd).ok();
    if let Some(l) = &logger {
        l.log_command(&full_cmd);
        if let Some((tool, _)) = active_filter {
            l.log_json("FILTER", tool);
        }
    }

    let mut child = match Command::new(cmd)
        .args(cmd_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            let code = map_spawn_error_code(&err);
            let msg = format_spawn_error(cmd, &err);
            eprintln!("{msg}");
            if let Some(l) = &logger {
                l.log_json("STDERR", &msg);
                l.log_exit(code);
                l.set_latest();
            }
            return code;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    if adaptive_compact {
        let running = Arc::clone(&process_running);
        let compact_switch = Arc::clone(&compact_enabled);
        let passthrough_switch = Arc::clone(&passthrough_enabled);
        let used_flag = Arc::clone(&compact_used);
        let buffering_switch = Arc::clone(&buffer_enabled);
        let buffered = Arc::clone(&buffered_chunks);
        let _ = thread::spawn(move || {
            thread::sleep(Duration::from_secs(ADAPTIVE_COMPACT_AFTER_SECS));
            if running.load(Ordering::Relaxed) {
                compact_switch.store(true, Ordering::Relaxed);
                passthrough_switch.store(false, Ordering::Relaxed);
                used_flag.store(true, Ordering::Relaxed);
                buffering_switch.store(false, Ordering::Relaxed);
                if let Ok(mut b) = buffered.lock() {
                    b.clear();
                }
            }
        });
    }

    let spinner_running = Arc::new(AtomicBool::new(true));
    let spinner_thread = if interactive_tty && !compact_mode {
        let running = Arc::clone(&spinner_running);
        let output_seen = Arc::clone(&had_output);
        let compact_on = Arc::clone(&compact_enabled);
        let spinner_label = format_command(cmd, cmd_args);
        Some(thread::spawn(move || {
            spin_wait(running, output_seen, compact_on, spinner_label);
        }))
    } else {
        None
    };

    let out_logger = logger.clone();
    let out_capture = CaptureOptions {
        passthrough: Arc::clone(&passthrough_enabled),
        compact_enabled: Arc::clone(&compact_enabled),
        compact_used: Arc::clone(&compact_used),
        output_flag: Arc::clone(&had_output),
        tail_lines: Arc::clone(&tail_lines),
        stats: Arc::clone(&stats),
        command_summary: command_summary.clone(),
        command_filter: active_filter_cfg.clone(),
        heuristic_detector: heuristic_detector.clone(),
        max_error_lines,
        buffered_chunks: Some(Arc::clone(&buffered_chunks)),
        buffer_enabled: Some(Arc::clone(&buffer_enabled)),
        seq: Some(Arc::clone(&chunk_seq)),
        historical_percents: historical_percents.clone(),
    };
    let out_thread = thread::spawn(move || {
        if let Some(mut reader) = stdout {
            let _ = capture_stream(
                &mut reader,
                "STDOUT",
                out_logger.as_ref(),
                Some(out_capture),
            );
        }
    });

    let err_logger = logger.clone();
    let err_capture = CaptureOptions {
        passthrough: Arc::clone(&passthrough_enabled),
        compact_enabled: Arc::clone(&compact_enabled),
        compact_used: Arc::clone(&compact_used),
        output_flag: Arc::clone(&had_output),
        tail_lines: Arc::clone(&tail_lines),
        stats: Arc::clone(&stats),
        command_summary: command_summary.clone(),
        command_filter: active_filter_cfg,
        heuristic_detector,
        max_error_lines,
        buffered_chunks: Some(Arc::clone(&buffered_chunks)),
        buffer_enabled: Some(Arc::clone(&buffer_enabled)),
        seq: Some(Arc::clone(&chunk_seq)),
        historical_percents,
    };
    let err_thread = thread::spawn(move || {
        if let Some(mut reader) = stderr {
            let _ = capture_stream(
                &mut reader,
                "STDERR",
                err_logger.as_ref(),
                Some(err_capture),
            );
        }
    });

    let compact_running = Arc::new(AtomicBool::new(true));
    let compact_renderer = if interactive_tty {
        let running = Arc::clone(&compact_running);
        let tail = Arc::clone(&tail_lines);
        let compact_on = Arc::clone(&compact_enabled);
        let compact_stats = Arc::clone(&stats);
        let compact_pid = child.id();
        let compact_cmd = format_command(cmd, cmd_args);
        Some(thread::spawn(move || {
            compact_tail_renderer(
                running,
                compact_on,
                tail,
                compact_stats,
                compact_pid,
                compact_cmd,
                TAIL_LINES,
            );
        }))
    } else {
        None
    };

    let status = match child.wait() {
        Ok(s) => s,
        Err(err) => {
            let msg = format!("ct: failed waiting for process: {err}");
            eprintln!("{msg}");
            if let Some(l) = &logger {
                l.log_json("STDERR", &msg);
                l.log_exit(1);
                l.set_latest();
            }
            return 1;
        }
    };

    let _ = out_thread.join();
    let _ = err_thread.join();
    process_running.store(false, Ordering::Relaxed);
    compact_running.store(false, Ordering::Relaxed);
    if let Some(handle) = compact_renderer {
        let _ = handle.join();
    }
    spinner_running.store(false, Ordering::Relaxed);
    if let Some(handle) = spinner_thread {
        let _ = handle.join();
    }

    let code = exit_code(status);
    if let Some(l) = &logger {
        l.log_exit(code);
        l.set_latest();
    }

    if compact_used.load(Ordering::Relaxed) {
        print_compact_result(code, command_summary.as_ref(), max_error_lines);
    } else if adaptive_compact {
        flush_buffered_output(&buffered_chunks);
    }
    if show_stats {
        let snapshot = match stats.lock() {
            Ok(s) => s.clone(),
            Err(_) => CompactStats::default(),
        };
        print_level_stats(&snapshot);
    }
    code
}

fn command_filter_for<'a>(cmd: &str, cfg: &'a CtConfig) -> Option<(&'a str, &'a CommandFilterConfig)> {
    let exec_name = std::path::Path::new(cmd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cmd);

    for (tool, filter) in &cfg.filters.tools {
        if !filter.enabled {
            continue;
        }
        if tool == exec_name || filter.aliases.iter().any(|alias| alias == exec_name) {
            return Some((tool.as_str(), filter));
        }
    }
    None
}

fn capture_stream<R: Read>(
    reader: &mut R,
    kind: &str,
    logger: Option<&RunLogger>,
    options: Option<CaptureOptions>,
) -> io::Result<()> {
    let mut buf = [0_u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }

        if let Some(opts) = &options {
            opts.output_flag.store(true, Ordering::Relaxed);
        }

        let should_passthrough = options
            .as_ref()
            .map(|opts| opts.passthrough.load(Ordering::Relaxed))
            .unwrap_or(true);

        if should_passthrough {
            if kind == "STDERR" {
                let mut w = io::stderr().lock();
                w.write_all(&buf[..n])?;
                w.flush()?;
            } else {
                let mut w = io::stdout().lock();
                w.write_all(&buf[..n])?;
                w.flush()?;
            }
        } else if options
            .as_ref()
            .and_then(|opts| opts.buffer_enabled.as_ref())
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
        {
            let chunk_id = options
                .as_ref()
                .and_then(|opts| opts.seq.as_ref())
                .map(|seq| seq.fetch_add(1, Ordering::Relaxed))
                .unwrap_or(0);
            if let Some(storage) = options
                .as_ref()
                .and_then(|opts| opts.buffered_chunks.as_ref())
            {
                if let Ok(mut all) = storage.lock() {
                    all.push(BufferedChunk {
                        seq: chunk_id,
                        kind: kind.to_string(),
                        data: buf[..n].to_vec(),
                    });
                }
            }
        }

        let text = String::from_utf8_lossy(&buf[..n]).to_string();
        let mut logged_any_line = false;

        if let Some(opts) = &options {
            for line in text.lines() {
                let mut selected_filter = opts.command_filter.clone();
                if selected_filter.is_none() && let Some(detector) = &opts.heuristic_detector {
                    if let Some((tool, filter)) = detector.detect_line_if_needed(line) {
                        if let Some(l) = logger {
                            l.log_json("FILTER", &tool);
                        }
                        opts.compact_enabled.store(true, Ordering::Relaxed);
                        opts.passthrough.store(false, Ordering::Relaxed);
                        opts.compact_used.store(true, Ordering::Relaxed);
                        if let Some(flag) = &opts.buffer_enabled {
                            flag.store(false, Ordering::Relaxed);
                        }
                        if let Some(storage) = &opts.buffered_chunks
                            && let Ok(mut all) = storage.lock()
                        {
                            all.clear();
                        }
                        selected_filter = Some(filter);
                    } else {
                        selected_filter = detector.selected_filter();
                    }
                }

                let level = classify_log_level(kind, line, selected_filter.as_ref());

                if let Some(l) = logger {
                    l.log_json_with_level(level, kind, line);
                    logged_any_line = true;
                }

                push_tail_line(&opts.tail_lines, kind, line, TAIL_LINES);
                update_progress_from_history(
                    &opts.stats,
                    line,
                    opts.historical_percents.as_deref(),
                );
                update_stats(&opts.stats, level, line);
                if let Some(summary) = &opts.command_summary {
                    update_command_summary(
                        summary,
                        line,
                        level,
                        selected_filter.as_ref(),
                        opts.max_error_lines,
                    );
                }
            }
        } else if let Some(l) = logger {
            for line in text.lines() {
                let level = classify_log_level(kind, line, None);
                l.log_json_with_level(level, kind, line);
                logged_any_line = true;
            }
        }

        if !logged_any_line && !text.is_empty() {
            if let Some(l) = logger {
                let level = classify_log_level(kind, &text, None);
                l.log_json_with_level(level, kind, &text);
            }
        }
    }
    Ok(())
}

fn update_command_summary(
    summary: &Arc<Mutex<CommandSummary>>,
    line: &str,
    level: char,
    command_filter: Option<&CommandFilterConfig>,
    max_error_lines: usize,
) {
    let cleaned = normalize_log_line(line);
    if cleaned.is_empty() {
        return;
    }
    let lower = cleaned.to_ascii_lowercase();
    let (is_warning, is_error, should_capture_error) = if let Some(filter) = command_filter {
        let is_warning = contains_any_pattern(&lower, &filter.warning_patterns);
        let is_error = contains_any_pattern(&lower, &filter.error_patterns);
        let should_capture_error = contains_any_pattern(&lower, &filter.error_capture_patterns);
        (is_warning, is_error, should_capture_error)
    } else {
        let is_warning = level == 'W';
        let is_error = matches!(level, 'F' | 'E');
        (is_warning, is_error, is_error)
    };

    if let Ok(mut s) = summary.lock() {
        if is_warning {
            s.warning_count += 1;
        }
        if is_error {
            s.error_count += 1;
            if should_capture_error {
                if max_error_lines > 0 {
                    s.error_lines.push_back(cleaned.to_string());
                    while s.error_lines.len() > max_error_lines {
                        let _ = s.error_lines.pop_front();
                    }
                }
            }
        }
    }
}

fn contains_any_pattern(lower_line: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .map(|pattern| pattern.trim().to_ascii_lowercase())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| lower_line.contains(&pattern))
}

fn classify_log_level(kind: &str, line: &str, command_filter: Option<&CommandFilterConfig>) -> char {
    let cleaned = normalize_log_line(line);
    if cleaned.is_empty() {
        return 'T';
    }

    let lower = cleaned.to_ascii_lowercase();
    if let Some(filter) = command_filter {
        if contains_any_pattern(&lower, &filter.level_patterns.fatal) {
            return 'F';
        }
        if contains_any_pattern(&lower, &filter.level_patterns.error) {
            return 'E';
        }
        if contains_any_pattern(&lower, &filter.level_patterns.warning) {
            return 'W';
        }
        if contains_any_pattern(&lower, &filter.level_patterns.info) {
            return 'I';
        }
        if contains_any_pattern(&lower, &filter.level_patterns.trace) {
            return 'T';
        }
        if contains_any_pattern(&lower, &filter.error_patterns) {
            return 'E';
        }
        if contains_any_pattern(&lower, &filter.warning_patterns) {
            return 'W';
        }
    }

    match kind {
        "STDERR" => 'U',
        "STDOUT" => 'U',
        _ => 'U',
    }
}

fn print_compact_result(
    code: i32,
    command_summary: Option<&Arc<Mutex<CommandSummary>>>,
    max_error_lines: usize,
) {
    let data = command_summary
        .map(|summary| match summary.lock() {
            Ok(s) => s.clone(),
            Err(_) => CommandSummary::default(),
        })
        .unwrap_or_default();

    if code == 0 {
        if data.warning_count > 0 {
            println!("SUCCESS ({} warnings)", data.warning_count);
        } else {
            println!("SUCCESS");
        }
        return;
    }

    println!("FAILED");
    if !data.error_lines.is_empty() {
        for line in data.error_lines.into_iter().take(max_error_lines) {
            println!("{line}");
        }
    }
}

fn print_level_stats(stats: &CompactStats) {
    println!(
        "STATS lines={} fatal={} error={} warning={} info={} trace={} unknown={}",
        stats.line_count,
        stats.fatal_count,
        stats.error_count,
        stats.warning_count,
        stats.info_count,
        stats.trace_count,
        stats.unknown_count
    );
}

fn load_global_config() -> CtConfig {
    let mut cfg = CtConfig::default();

    for (tool, profile) in load_embedded_filter_profiles() {
        cfg.filters.tools.insert(tool, profile);
    }

    let home = match ct_home() {
        Ok(h) => h,
        Err(_) => return cfg,
    };

    for (tool, profile) in load_filter_profiles(&home) {
        cfg.filters.tools.insert(tool, profile);
    }

    let path = home.join("config.toml");
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return cfg,
    };

    let user_cfg = match toml::from_str::<CtConfigFile>(&raw) {
        Ok(v) => v,
        Err(_) => return cfg,
    };

    if let Some(max_error_lines) = user_cfg.output.max_error_lines {
        cfg.output.max_error_lines = max_error_lines;
    }
    if let Some(auto_detect_log_type) = user_cfg.heuristics.auto_detect_log_type {
        cfg.heuristics.auto_detect_log_type = auto_detect_log_type;
    }
    for (tool, filter) in user_cfg.filters {
        cfg.filters.tools.insert(tool, filter);
    }

    cfg
}

fn load_filter_profiles(home: &Path) -> HashMap<String, CommandFilterConfig> {
    let mut out = HashMap::new();
    let dir = home.join("filters.d");
    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return out,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("toml") {
            continue;
        }

        let raw = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let profile = match toml::from_str::<FilterProfileFile>(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let tool = profile.tool.trim();
        if tool.is_empty() {
            continue;
        }

        out.insert(
            tool.to_string(),
            profile.filter,
        );
    }

    out
}

fn load_embedded_filter_profiles() -> HashMap<String, CommandFilterConfig> {
    let mut out = HashMap::new();
    for (_name, raw) in BUILTIN_FILTER_PROFILE_FILES {
        let profile = match toml::from_str::<FilterProfileFile>(raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let tool = profile.tool.trim();
        if tool.is_empty() {
            continue;
        }
        out.insert(tool.to_string(), profile.filter);
    }
    out
}

fn flush_buffered_output(buffered_chunks: &Arc<Mutex<Vec<BufferedChunk>>>) {
    let mut chunks = match buffered_chunks.lock() {
        Ok(c) => c.clone(),
        Err(_) => Vec::new(),
    };
    chunks.sort_by_key(|c| c.seq);

    for chunk in chunks {
        if chunk.kind == "STDERR" {
            let mut w = io::stderr().lock();
            let _ = w.write_all(&chunk.data);
            let _ = w.flush();
        } else {
            let mut w = io::stdout().lock();
            let _ = w.write_all(&chunk.data);
            let _ = w.flush();
        }
    }
}

fn push_tail_line(tail: &Arc<Mutex<VecDeque<String>>>, kind: &str, line: &str, max: usize) {
    let cleaned = normalize_log_line(line);
    if cleaned.is_empty() {
        return;
    }

    if let Ok(mut q) = tail.lock() {
        let entry = if kind == "STDERR" {
            format!("[err] {cleaned}")
        } else {
            cleaned.clone()
        };
        q.push_back(entry);
        while q.len() > max {
            let _ = q.pop_front();
        }
    }
}

fn compact_tail_renderer(
    running: Arc<AtomicBool>,
    compact_enabled: Arc<AtomicBool>,
    _tail_lines: Arc<Mutex<VecDeque<String>>>,
    stats: Arc<Mutex<CompactStats>>,
    pid: u32,
    command: String,
    _max: usize,
) {
    let mut spinner_idx = 0usize;
    let start = Instant::now();
    let spinner_frames = ['|', '/', '-', '\\'];
    let mut rendered_any = false;

    let refresh_interval = compact_refresh_interval();

    while running.load(Ordering::Relaxed) {
        if !compact_enabled.load(Ordering::Relaxed) {
            thread::sleep(refresh_interval);
            continue;
        }

        let state = match stats.lock() {
            Ok(s) => s.clone(),
            Err(_) => CompactStats::default(),
        };

        let elapsed = format_elapsed(start.elapsed());
        let header = format_compact_header(
            spinner_frames[spinner_idx % spinner_frames.len()],
            pid,
            &command,
            &elapsed,
            &state,
        );
        spinner_idx += 1;

        eprint!(
            "\r\x1b[2K{} RUNNING pid:{} t:{} cmd:{} lines:{} warn:{} err:{}{} last:{}",
            header.spinner,
            header.pid,
            header.elapsed,
            header.cmd_short,
            header.line_count,
            header.warning_count,
            header.error_count,
            header.progress,
            header.last_error
        );
        let _ = io::stderr().flush();
        rendered_any = true;

        thread::sleep(refresh_interval);
    }

    if rendered_any {
        eprint!("\r\x1b[2K");
        let _ = io::stderr().flush();
    }
}

fn update_progress_from_history(
    stats: &Arc<Mutex<CompactStats>>,
    line: &str,
    historical_percents: Option<&HashMap<String, i64>>,
) {
    let Some(history) = historical_percents else {
        return;
    };
    let Some(hash) = line_hash_for_stats(line) else {
        return;
    };
    let Some(percent) = history.get(&hash) else {
        return;
    };

    if let Ok(mut s) = stats.lock() {
        let next = (*percent).clamp(0, 100);
        s.progress_percent = Some(match s.progress_percent {
            Some(current) => current.max(next),
            None => next,
        });
    }
}

fn update_stats(stats: &Arc<Mutex<CompactStats>>, level: char, line: &str) {
    let cleaned = normalize_log_line(line);
    if let Ok(mut s) = stats.lock() {
        s.line_count += 1;
        match level {
            'F' => {
                s.fatal_count += 1;
                s.error_count += 1;
                s.last_error = cleaned.to_string();
            }
            'E' => {
                s.error_count += 1;
                s.last_error = cleaned.to_string();
            }
            'W' => s.warning_count += 1,
            'I' => s.info_count += 1,
            'T' => s.trace_count += 1,
            _ => s.unknown_count += 1,
        }
    }
}

fn strip_leading_timestamp(line: &str) -> &str {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end_idx) = rest.find(']')
        && timestamp_prefix_regex().is_match(rest[..end_idx].trim())
    {
        return rest[end_idx + 1..].trim_start();
    }
    trimmed
}

fn timestamp_prefix_regex() -> &'static Regex {
    static TS_RE: OnceLock<Regex> = OnceLock::new();
    TS_RE.get_or_init(|| {
        Regex::new(
            r"^(?:\d{4}-\d{2}-\d{2}(?:[T ][0-9:.+\-Z]+)?|\d{2}:\d{2}:\d{2}(?:[.,]\d+)?)$",
        )
        .expect("valid timestamp regex")
    })
}

fn ansi_regex() -> &'static Regex {
    static ANSI_RE: OnceLock<Regex> = OnceLock::new();
    ANSI_RE.get_or_init(|| {
        Regex::new(r"\x1B(?:\[[0-?]*[ -/]*[@-~]|\][^\x07\x1B]*(?:\x07|\x1B\\))")
            .expect("valid ANSI regex")
    })
}

fn normalize_log_line(line: &str) -> String {
    let without_ansi = ansi_regex().replace_all(line, "");
    strip_leading_timestamp(without_ansi.as_ref()).to_string()
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

struct CompactHeader {
    spinner: char,
    pid: u32,
    elapsed: String,
    cmd_short: String,
    line_count: u64,
    warning_count: u64,
    error_count: u64,
    progress: String,
    last_error: String,
}

fn format_compact_header(
    spinner: char,
    pid: u32,
    command: &str,
    elapsed: &str,
    stats: &CompactStats,
) -> CompactHeader {
    let cmd_short = truncate(command, 36);
    let last_error = if stats.last_error.is_empty() {
        "-".to_string()
    } else {
        truncate(&stats.last_error, 48)
    };
    let progress = stats
        .progress_percent
        .map(|value| format!(" pct:{}%", value.clamp(0, 100)))
        .unwrap_or_default();

    CompactHeader {
        spinner,
        pid,
        elapsed: elapsed.to_string(),
        cmd_short,
        line_count: stats.line_count,
        warning_count: stats.warning_count,
        error_count: stats.error_count,
        progress,
        last_error,
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let candidate: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{candidate}...")
    } else {
        candidate
    }
}

fn spin_wait(
    running: Arc<AtomicBool>,
    output_seen: Arc<AtomicBool>,
    compact_enabled: Arc<AtomicBool>,
    label: String,
) {
    let frames = ['|', '/', '-', '\\'];
    let mut idx = 0usize;
    while running.load(Ordering::Relaxed)
        && !output_seen.load(Ordering::Relaxed)
        && !compact_enabled.load(Ordering::Relaxed)
    {
        eprint!("\r{} Running {}", frames[idx % frames.len()], label);
        let _ = io::stderr().flush();
        idx += 1;
        thread::sleep(Duration::from_millis(120));
    }

    eprint!("\r\x1b[2K");
    let _ = io::stderr().flush();
}

fn map_spawn_error_code(err: &io::Error) -> i32 {
    match err.kind() {
        io::ErrorKind::NotFound => 127,
        io::ErrorKind::PermissionDenied => 126,
        _ => 1,
    }
}

fn format_spawn_error(cmd: &str, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => format!("{cmd}: command not found"),
        io::ErrorKind::PermissionDenied => format!("{cmd}: permission denied"),
        _ => format!("ct: failed to run '{cmd}': {err}"),
    }
}

fn exit_code(status: process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }

    1
}

fn format_command(cmd: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(shell_escape(cmd));
    for arg in args {
        parts.push(shell_escape(arg));
    }
    parts.join(" ")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_embedded_filters() -> CtConfig {
        let mut cfg = CtConfig::default();
        for (tool, profile) in load_embedded_filter_profiles() {
            cfg.filters.tools.insert(tool, profile);
        }
        cfg
    }

    #[test]
    fn heuristic_detector_detects_maven_lines() {
        let cfg = config_with_embedded_filters();
        let maven = cfg.filters.tools.get("maven").expect("maven filter missing");
        assert!(maven.enabled);
        assert!(!maven.detection_regex.is_empty());
        let detector = HeuristicDetector::from_config(&cfg).expect("heuristics should be enabled");
        let detected = detector
            .detect_line_if_needed("[INFO] Scanning for projects...")
            .expect("maven line should be detected");
        assert_eq!(detected.0, "maven");
    }

    #[test]
    fn classify_log_level_uses_maven_level_patterns() {
        let cfg = config_with_embedded_filters();
        let maven = cfg.filters.tools.get("maven").expect("maven filter missing");

        assert_eq!(classify_log_level("STDOUT", "[INFO] Building", Some(maven)), 'I');
        assert_eq!(
            classify_log_level("STDOUT", "[WARNING] Deprecated API", Some(maven)),
            'W'
        );
        assert_eq!(
            classify_log_level("STDOUT", "[ERROR] Build failure", Some(maven)),
            'E'
        );
    }

    #[test]
    fn stats_file_name_is_per_command_hash() {
        let log_path = PathBuf::from("/tmp/c5792326-1776109764924.log");
        let cmd_hash = log_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('-').next())
            .unwrap();
        let stats_path = log_path
            .parent()
            .unwrap()
            .join(format!("{}.stats", cmd_hash));
        assert_eq!(stats_path.file_name().unwrap().to_str().unwrap(), "c5792326.stats");
    }

    #[test]
    fn merge_stats_accumulates_hash_history_across_runs() {
        let hash = "00e85f9db78e94b8d9b5d64b07d15534";
        let existing = format!("EXEC 1000 111\nEXEC-END\n{} 0 1000:10\n", hash);

        let mut current: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
        current.insert(hash.to_string(), vec![(2000, 20)]);

        let merged = merge_stats_content(&existing, 2000, 222, &current);
        assert_eq!(merged.matches("EXEC ").count(), 2);
        assert_eq!(merged.matches("EXEC-END").count(), 2);

        let hash_line = merged
            .lines()
            .find(|l| l.starts_with(hash))
            .expect("hash line missing");
        let tokens: Vec<&str> = hash_line.split_whitespace().collect();
        assert_eq!(tokens[0], hash);
        assert_eq!(tokens[1], "9");
        assert_eq!(tokens[2], "15");
        assert_eq!(&tokens[3..], &["1000:10", "2000:20"]);
    }

    #[test]
    fn merge_stats_deduplicates_identical_timestamps() {
        let existing = "EXEC 1000 111\nEXEC-END\nabc 0 1000:5 1000:5\n";

        let mut current: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
        current.insert("abc".to_string(), vec![(1000, 5), (1000, 5)]);

        let merged = merge_stats_content(existing, 1000, 111, &current);
        let hash_line = merged
            .lines()
            .find(|l| l.starts_with("abc "))
            .expect("abc line missing");
        let tokens: Vec<&str> = hash_line.split_whitespace().collect();
        assert_eq!(tokens, vec!["abc", "4", "5", "1000:5"]);
    }

    #[test]
    fn merge_stats_keeps_only_last_10_runs() {
        let mut existing = String::new();
        existing.push_str("abc 0");
        for i in 1..=12 {
            let ts = i * 1000;
            existing.push_str(&format!(" {}:{}", ts, i));
        }
        existing.push('\n');
        for i in 1..=12 {
            let ts = i * 1000;
            existing.push_str(&format!("EXEC {} {}\nEXEC-END\n", ts, i));
        }

        let current: HashMap<String, Vec<(i64, i64)>> = HashMap::new();
        let merged = merge_stats_content(&existing, 12000, 12, &current);

        assert_eq!(merged.matches("EXEC ").count(), 10);
        assert_eq!(merged.matches("EXEC-END").count(), 10);

        let hash_line = merged
            .lines()
            .find(|l| l.starts_with("abc "))
            .expect("abc line missing");
        let tokens: Vec<&str> = hash_line.split_whitespace().collect();
        assert_eq!(tokens[0], "abc");
        assert_eq!(tokens[1], "100");
        assert_eq!(tokens[2], "7");
        assert_eq!(tokens.len(), 13);
        assert_eq!(tokens[3], "3000:3");
        assert_eq!(tokens[12], "12000:12");
    }

    #[test]
    fn merge_stats_reads_previous_format_without_average_field() {
        let existing = "EXEC 1000 111\nEXEC-END\nabc 0 1000:10 2000:20\n";
        let current: HashMap<String, Vec<(i64, i64)>> = HashMap::new();

        let merged = merge_stats_content(existing, 2000, 222, &current);
        let hash_line = merged
            .lines()
            .find(|l| l.starts_with("abc "))
            .expect("abc line missing");
        let tokens: Vec<&str> = hash_line.split_whitespace().collect();
        assert_eq!(tokens[0], "abc");
        assert_eq!(tokens[1], "9");
        assert_eq!(tokens[2], "15");
        assert_eq!(&tokens[3..], &["1000:10", "2000:20"]);
    }

    #[test]
    fn merge_stats_computes_percent_from_relative_position() {
        let existing =
            "EXEC 1000 111\nEXEC-END\nEXEC 2000 222\nEXEC-END\nabc 0 1000:10\ndef 0 1000:20 2000:30\n";
        let current: HashMap<String, Vec<(i64, i64)>> = HashMap::new();

        let merged = merge_stats_content(existing, 2000, 222, &current);
        let abc_line = merged
            .lines()
            .find(|l| l.starts_with("abc "))
            .expect("abc line missing");
        let abc_tokens: Vec<&str> = abc_line.split_whitespace().collect();
        assert_eq!(abc_tokens, vec!["abc", "9", "10", "1000:10"]);

        let def_line = merged
            .lines()
            .find(|l| l.starts_with("def "))
            .expect("def line missing");
        let def_tokens: Vec<&str> = def_line.split_whitespace().collect();
        assert_eq!(def_tokens, vec!["def", "15", "25", "1000:20", "2000:30"]);
    }

    #[test]
    fn parse_stats_percent_map_reads_hash_percent_pairs() {
        let content = "EXEC 1000 111\nEXEC-END\nabc 35 120 1000:120\ndef 90 200 1000:200\n";
        let map = parse_stats_percent_map(content);

        assert_eq!(map.get("abc"), Some(&35));
        assert_eq!(map.get("def"), Some(&90));
    }

    #[test]
    fn compact_header_hides_progress_without_historical_stats() {
        let stats = CompactStats::default();
        let header = format_compact_header('|', 123, "cargo test", "00:05", &stats);
        assert!(header.progress.is_empty());
    }

    #[test]
    fn update_progress_from_history_sets_max_progress() {
        let stats = Arc::new(Mutex::new(CompactStats::default()));
        let mut history = HashMap::new();

        let hash_a = line_hash_for_stats("ALPHA").expect("hash for ALPHA");
        let hash_b = line_hash_for_stats("BETA").expect("hash for BETA");
        history.insert(hash_a, 22);
        history.insert(hash_b, 61);

        update_progress_from_history(&stats, "ALPHA", Some(&history));
        update_progress_from_history(&stats, "BETA", Some(&history));

        let snapshot = stats.lock().expect("lock stats").clone();
        assert_eq!(snapshot.progress_percent, Some(61));
    }
}
