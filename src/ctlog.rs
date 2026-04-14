use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const BUILTIN_FILTER_PROFILE_FILES: [(&str, &str); 24] = [
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
    ("docker.toml", include_str!("../filters.d/docker.toml")),
    (
        "docker-compose.toml",
        include_str!("../filters.d/docker-compose.toml"),
    ),
    ("kubectl.toml", include_str!("../filters.d/kubectl.toml")),
    ("terraform.toml", include_str!("../filters.d/terraform.toml")),
    ("ansible.toml", include_str!("../filters.d/ansible.toml")),
    ("pip.toml", include_str!("../filters.d/pip.toml")),
    ("bazel.toml", include_str!("../filters.d/bazel.toml")),
    ("apt.toml", include_str!("../filters.d/apt.toml")),
];

#[derive(Default)]
pub struct LogSummary {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Deserialize, Default)]
struct LevelPatterns {
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
struct CommandFilterConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "default_warning_patterns")]
    warning_patterns: Vec<String>,
    #[serde(default = "default_error_patterns")]
    error_patterns: Vec<String>,
    #[serde(default)]
    detection_regex: Vec<String>,
    #[serde(default)]
    level_patterns: LevelPatterns,
}

#[derive(Deserialize)]
struct FilterProfileFile {
    tool: String,
    #[serde(flatten)]
    filter: CommandFilterConfig,
}

#[derive(Deserialize, Default)]
struct CtConfigFile {
    #[serde(default)]
    filters: HashMap<String, CommandFilterConfig>,
    #[serde(default)]
    heuristics: HeuristicsConfigFile,
}

#[derive(Deserialize, Default)]
struct HeuristicsConfigFile {
    auto_detect_log_type: Option<bool>,
}

impl Default for CommandFilterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            aliases: Vec::new(),
            warning_patterns: default_warning_patterns(),
            error_patterns: default_error_patterns(),
            detection_regex: Vec::new(),
            level_patterns: LevelPatterns::default(),
        }
    }
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

pub fn ct_home() -> io::Result<PathBuf> {
    let home =
        env::var("HOME").map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let base = Path::new(&home).join(".ct");
    fs::create_dir_all(&base)?;
    Ok(base)
}

pub fn logs_dir() -> io::Result<PathBuf> {
    Ok(ct_home()?.join("logs"))
}

pub fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn latest_log_path() -> io::Result<PathBuf> {
    let latest_ptr = logs_dir()?.join(".latest");
    if !latest_ptr.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no previous execution log found",
        ));
    }

    let file_name = fs::read_to_string(latest_ptr)?.trim().to_string();
    if file_name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "latest log pointer is empty",
        ));
    }

    let path = logs_dir()?.join(file_name);
    if !path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "latest log file not found",
        ));
    }

    Ok(path)
}

pub fn summarize_log(contents: &str) -> LogSummary {
    let mut summary = LogSummary::default();
    let active_filter = resolve_filter_for_log(contents);

    let level_patterns = active_filter.as_ref().map(|v| &v.level_patterns);

    for line in contents.lines() {
        let (kind, payload) = match parse_log_line(line) {
            Some(v) => v,
            None => continue,
        };

        if kind != "STDOUT" && kind != "STDERR" {
            continue;
        }

        for chunk_line in payload.lines() {
            let lower = chunk_line.to_ascii_lowercase();
            let has_level_config = level_patterns
                .map(|lp| !lp.warning.is_empty() || !lp.error.is_empty())
                .unwrap_or(false);

            let is_warning = if has_level_config {
                level_patterns
                    .map(|lp| contains_any_pattern(&lower, &lp.warning))
                    .unwrap_or(false)
            } else {
                active_filter
                    .as_ref()
                    .map(|f| contains_any_pattern(&lower, &f.warning_patterns))
                    .unwrap_or(false)
            };
            let is_error = if has_level_config {
                level_patterns
                    .map(|lp| contains_any_pattern(&lower, &lp.error))
                    .unwrap_or(false)
            } else {
                active_filter
                    .as_ref()
                    .map(|f| contains_any_pattern(&lower, &f.error_patterns))
                    .unwrap_or(false)
            };

            if is_warning {
                summary.warnings.push(chunk_line.to_string());
            }
            if is_error {
                summary.errors.push(chunk_line.to_string());
            }
        }
    }
    summary
}

fn resolve_filter_for_log(contents: &str) -> Option<CommandFilterConfig> {
    let mut filter_name: Option<String> = None;
    let mut cmd_exec: Option<String> = None;

    for line in contents.lines() {
        let (kind, payload) = match parse_log_line(line) {
            Some(v) => v,
            None => continue,
        };
        if kind == "FILTER" {
            filter_name = Some(payload);
            break;
        }
        if kind == "CMD" && cmd_exec.is_none() {
            cmd_exec = extract_exec_name(&payload);
        }
    }

    let loaded = load_all_filter_profiles();
    let profiles = loaded.tools;

    if let Some(tool) = filter_name {
        if let Some(profile) = profiles.get(&tool) {
            if profile.enabled {
                return Some(profile.clone());
            }
        }
    }

    if let Some(exec_name) = cmd_exec {
        for (tool, profile) in &profiles {
            if !profile.enabled {
                continue;
            }
            if tool == &exec_name || profile.aliases.iter().any(|alias| alias == &exec_name) {
                return Some(profile.clone());
            }
        }
    }

    if loaded.auto_detect_log_type {
        if let Some(profile) = detect_filter_by_log_contents(contents, &profiles) {
            return Some(profile);
        }
    }

    None
}

#[derive(Default)]
struct LoadedProfiles {
    tools: HashMap<String, CommandFilterConfig>,
    auto_detect_log_type: bool,
}

fn extract_exec_name(command_line: &str) -> Option<String> {
    let first = command_line.split_whitespace().next()?;
    let token = first.trim_matches(|c| c == '\'' || c == '"');
    let name = Path::new(token).file_name()?.to_str()?;
    Some(name.to_string())
}

fn load_all_filter_profiles() -> LoadedProfiles {
    let mut loaded = LoadedProfiles {
        tools: load_embedded_filter_profiles(),
        auto_detect_log_type: true,
    };

    let home = match ct_home() {
        Ok(v) => v,
        Err(_) => return loaded,
    };

    for (tool, filter) in load_filter_profiles_dir(&home.join("filters.d")) {
        loaded.tools.insert(tool, filter);
    }

    let config_path = home.join("config.toml");
    if let Ok(raw) = fs::read_to_string(config_path)
        && let Ok(cfg) = toml::from_str::<CtConfigFile>(&raw)
    {
        if let Some(auto_detect_log_type) = cfg.heuristics.auto_detect_log_type {
            loaded.auto_detect_log_type = auto_detect_log_type;
        }
        for (tool, filter) in cfg.filters {
            loaded.tools.insert(tool, filter);
        }
    }

    loaded
}

fn detect_filter_by_log_contents(
    contents: &str,
    profiles: &HashMap<String, CommandFilterConfig>,
) -> Option<CommandFilterConfig> {
    let mut match_counts: HashMap<String, usize> = HashMap::new();
    let mut compiled: Vec<(String, CommandFilterConfig, Vec<Regex>)> = Vec::new();

    for (tool, filter) in profiles {
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
        compiled.push((tool.clone(), filter.clone(), regexes));
    }

    if compiled.is_empty() {
        return None;
    }

    for line in contents.lines() {
        let (kind, payload) = match parse_log_line(line) {
            Some(v) => v,
            None => continue,
        };
        if kind != "STDOUT" && kind != "STDERR" {
            continue;
        }

        for chunk_line in payload.lines() {
            for (tool, _filter, regexes) in &compiled {
                if regexes.iter().any(|re| re.is_match(chunk_line)) {
                    let entry = match_counts.entry(tool.clone()).or_insert(0);
                    *entry += 1;
                }
            }
        }
    }

    let best_tool = match_counts
        .into_iter()
        .max_by(|(tool_a, count_a), (tool_b, count_b)| {
            count_a.cmp(count_b).then_with(|| tool_b.cmp(tool_a))
        })
        .map(|(tool, _)| tool)?;

    profiles.get(&best_tool).cloned()
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

fn load_filter_profiles_dir(dir: &Path) -> HashMap<String, CommandFilterConfig> {
    let mut out = HashMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return out,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("toml") {
            continue;
        }

        let raw = match fs::read_to_string(path) {
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
        out.insert(tool.to_string(), profile.filter);
    }

    out
}

fn contains_any_pattern(lower_line: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .map(|pattern| pattern.trim().to_ascii_lowercase())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| lower_line.contains(&pattern))
}

pub fn parse_log_line(line: &str) -> Option<(String, String)> {
    let (timestamp, rest) = line.split_once(' ')?;

    if timestamp.parse::<i64>().is_ok() {
        let (token, after_token) = match rest.split_once(' ') {
            Some(v) => v,
            None => return Some(("CMD".to_string(), rest.to_string())),
        };

        if is_level_code(token) {
            let (kind, payload) = after_token.split_once(' ')?;
            return decode_log_payload(kind, payload);
        }

        if is_known_kind(token) {
            return decode_log_payload(token, after_token);
        }

        return Some(("CMD".to_string(), rest.to_string()));
    }

    let (kind, payload) = rest.split_once(' ')?;
    decode_log_payload(kind, payload)
}

fn is_level_code(token: &str) -> bool {
    matches!(token, "F" | "E" | "W" | "I" | "T" | "U")
}

fn is_known_kind(token: &str) -> bool {
    matches!(
        token,
        "CMD" | "STDOUT" | "STDERR" | "FILTER" | "MODE" | "EXIT" | "PID" | "ASYNC"
    )
}

fn decode_log_payload(kind: &str, payload: &str) -> Option<(String, String)> {
    let kind = kind.to_string();
    let payload = payload.trim();
    if kind == "CMD" || kind == "STDOUT" || kind == "STDERR" || kind == "FILTER" {
        let decoded = serde_json::from_str::<String>(payload).ok()?;
        return Some((kind, decoded));
    }
    Some((kind, payload.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    const NPM_WARNING_LOG: &str = include_str!("../unit_test_example_logs/npm_warning.log");
    const NPM_ERROR_LOG: &str = include_str!("../unit_test_example_logs/npm_error.log");
    const NPM_SUCCESS_LOG: &str = include_str!("../unit_test_example_logs/npm_success.log");
    const MAVEN_SUCCESS_LOG: &str = include_str!("../unit_test_example_logs/maven_success.log");
    const MAVEN_WARNING_LOG: &str = include_str!("../unit_test_example_logs/maven_warning.log");
    const MAVEN_ERROR_LOG: &str = include_str!("../unit_test_example_logs/maven_error.log");
    const CPP_WARNING_LOG: &str = include_str!("../unit_test_example_logs/cpp_warning.log");
    const CPP_ERROR_LOG: &str = include_str!("../unit_test_example_logs/cpp_error.log");

    struct TempHomeGuard {
        original_home: Option<std::ffi::OsString>,
        temp_home: PathBuf,
    }

    impl TempHomeGuard {
        fn new() -> Self {
            let original_home = env::var_os("HOME");
            let temp_home = unique_temp_home();
            fs::create_dir_all(&temp_home).expect("create temp HOME directory");
            unsafe {
                env::set_var("HOME", &temp_home);
            }
            Self {
                original_home,
                temp_home,
            }
        }
    }

    impl Drop for TempHomeGuard {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => unsafe {
                    env::set_var("HOME", value);
                },
                None => unsafe {
                    env::remove_var("HOME");
                },
            }
            let _ = fs::remove_dir_all(&self.temp_home);
        }
    }

    fn unique_temp_home() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        env::temp_dir().join(format!("ct-test-home-{}-{}", process::id(), nanos))
    }

    fn home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_temp_home<F>(test: F)
    where
        F: FnOnce(&Path),
    {
        let _lock = home_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let guard = TempHomeGuard::new();
        test(&guard.temp_home);
    }

    fn encode_ct_line(kind: &str, payload: &str) -> String {
        format!(
            "2026-01-01T00:00:00.000000Z {kind} {}",
            serde_json::to_string(payload).expect("encode payload")
        )
    }

    #[test]
    fn parse_log_line_supports_new_cmd_header_format() {
        let parsed = parse_log_line("1712345678901 mvn clean install")
            .expect("new command header should parse");
        assert_eq!(parsed.0, "CMD");
        assert_eq!(parsed.1, "mvn clean install");
    }

    #[test]
    fn parse_log_line_supports_level_and_kind_format() {
        let parsed = parse_log_line("42 E STDOUT \"[ERROR] build failed\"")
            .expect("level+kind format should parse");
        assert_eq!(parsed.0, "STDOUT");
        assert_eq!(parsed.1, "[ERROR] build failed");
    }

    fn build_stdout_only_log(raw_log: &str) -> String {
        format!("{}\n", encode_ct_line("STDOUT", raw_log))
    }

    #[test]
    fn embedded_profiles_include_new_popular_tools() {
        let filters = load_embedded_filter_profiles();
        for tool in [
            "docker",
            "docker-compose",
            "kubectl",
            "terraform",
            "ansible",
            "pip",
            "bazel",
            "apt",
        ] {
            let filter = filters
                .get(tool)
                .unwrap_or_else(|| panic!("{tool} filter missing"));
            assert!(filter.enabled, "{tool} should be enabled");
            assert!(
                !filter.detection_regex.is_empty(),
                "{tool} should provide detection regexes"
            );
            assert!(
                !filter.error_patterns.is_empty(),
                "{tool} should provide error patterns"
            );
        }
    }

    #[test]
    fn heuristic_detects_npm_warning_patterns_without_cmd() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(NPM_WARNING_LOG));
            assert!(!summary.warnings.is_empty());
            assert!(
                summary
                    .warnings
                    .iter()
                    .any(|line| line.contains("npm WARN deprecated"))
            );
        });
    }

    #[test]
    fn heuristic_detects_npm_error_extractor_patterns() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(NPM_ERROR_LOG));
            assert!(!summary.errors.is_empty());
            assert!(
                summary
                    .errors
                    .iter()
                    .any(|line| line.contains("command failed"))
            );
        });
    }

    #[test]
    fn heuristic_detects_cpp_linker_error_pattern() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(CPP_ERROR_LOG));
            assert!(!summary.errors.is_empty());
            assert!(
                summary
                    .errors
                    .iter()
                    .any(|line| line.contains("undefined reference to"))
            );
        });
    }

    #[test]
    fn cpp_warning_patterns_extract_warning_line() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(CPP_WARNING_LOG));
            assert!(
                summary
                    .warnings
                    .iter()
                    .any(|line| line.contains(": warning:"))
            );
        });
    }

    #[test]
    fn maven_success_log_has_no_extracted_errors_or_warnings() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(MAVEN_SUCCESS_LOG));
            assert!(summary.errors.is_empty());
            assert!(summary.warnings.is_empty());
        });
    }

    #[test]
    fn npm_success_log_has_no_extracted_errors_or_warnings() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(NPM_SUCCESS_LOG));
            assert!(summary.errors.is_empty());
            assert!(summary.warnings.is_empty());
        });
    }

    #[test]
    fn maven_warning_patterns_extract_warning_line() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(MAVEN_WARNING_LOG));
            assert!(
                summary
                    .warnings
                    .iter()
                    .any(|line| line.contains("[WARNING]"))
            );
        });
    }

    #[test]
    fn filter_marker_takes_priority_over_cmd_and_heuristics() {
        with_temp_home(|_| {
            let mut ct_log = String::new();
            ct_log.push_str(&encode_ct_line("FILTER", "maven"));
            ct_log.push('\n');
            ct_log.push_str(&encode_ct_line("CMD", "npm install"));
            ct_log.push('\n');
            ct_log.push_str(&encode_ct_line("STDOUT", NPM_WARNING_LOG));
            ct_log.push('\n');

            let summary = summarize_log(&ct_log);
            assert!(summary.warnings.is_empty());
            assert!(summary.errors.is_empty());
        });
    }

    #[test]
    fn cmd_alias_resolution_precedes_heuristics() {
        with_temp_home(|_| {
            let mut ct_log = String::new();
            ct_log.push_str(&encode_ct_line("CMD", "mvn test"));
            ct_log.push('\n');
            ct_log.push_str(&encode_ct_line("STDOUT", NPM_WARNING_LOG));
            ct_log.push('\n');

            let summary = summarize_log(&ct_log);
            assert!(summary.warnings.is_empty());
        });
    }

    #[test]
    fn heuristics_can_be_disabled_by_user_config() {
        with_temp_home(|home| {
            let ct_dir = home.join(".ct");
            fs::create_dir_all(&ct_dir).expect("create .ct directory");
            fs::write(
                ct_dir.join("config.toml"),
                "[heuristics]\nauto_detect_log_type = false\n",
            )
            .expect("write config.toml");

            let summary = summarize_log(&build_stdout_only_log(NPM_WARNING_LOG));
            assert!(summary.warnings.is_empty());
            assert!(summary.errors.is_empty());
        });
    }

    #[test]
    fn maven_error_patterns_extract_failed_goal_line() {
        with_temp_home(|_| {
            let summary = summarize_log(&build_stdout_only_log(MAVEN_ERROR_LOG));
            assert!(
                summary
                    .errors
                    .iter()
                    .any(|line| line.contains("Failed to execute goal"))
            );
        });
    }
}
