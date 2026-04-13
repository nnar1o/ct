use ct::ctlog::ct_home;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const START_MARKER: &str = "# >>> ct >>>";
const END_MARKER: &str = "# <<< ct <<<";
const DEFAULT_CONFIG_TOML: &str = include_str!("../../config.toml");

fn main() {
    if let Err(err) = run() {
        eprintln!("ct-install: {err}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let home =
        env::var("HOME").map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let bashrc_path = Path::new(&home).join(".bashrc");

    let current = if bashrc_path.exists() {
        fs::read_to_string(&bashrc_path)?
    } else {
        String::new()
    };

    let block = ct_block();
    let updated = upsert_block(&current, &block);
    let bashrc_changed = updated != current;

    if bashrc_changed {
        backup_if_needed(&bashrc_path)?;
        atomic_write(&bashrc_path, &updated)?;
        println!("ct-install: added ct bash integration to ~/.bashrc");
    } else {
        println!("ct-install: ~/.bashrc already configured");
    }

    if ensure_default_config()? {
        println!("ct-install: created ~/.ct/config.toml");
    } else {
        println!("ct-install: ~/.ct/config.toml already exists");
    }

    println!("Run: source ~/.bashrc");
    Ok(())
}

fn ensure_default_config() -> io::Result<bool> {
    let path = ct_home()?.join("config.toml");
    if path.exists() {
        return Ok(false);
    }

    atomic_write(&path, DEFAULT_CONFIG_TOML)?;
    Ok(true)
}

fn upsert_block(current: &str, block: &str) -> String {
    if let Some(start) = current.find(START_MARKER) {
        if let Some(end_rel) = current[start..].find(END_MARKER) {
            let end = start + end_rel + END_MARKER.len();
            let mut out = String::new();
            out.push_str(&current[..start]);
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            out.push_str(block);
            if end < current.len() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&current[end..]);
            }
            return normalize_newlines(out);
        }
    }

    let mut out = String::new();
    out.push_str(current);
    if !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
    out.push('\n');
    normalize_newlines(out)
}

fn ct_block() -> String {
    let block = r#"# >>> ct >>>
ct() {
  if [ "$1" = "cd" ]; then
    local __ct_out
    __ct_out="$(command ct --shell-bridge "$@")"
    local __ct_rc=$?

    if [ $__ct_rc -ne 0 ]; then
      [ -n "$__ct_out" ] && printf '%s\n' "$__ct_out" >&2
      return $__ct_rc
    fi

    case "$__ct_out" in
      __CT_BUILTIN__\ *)
        eval "${__ct_out#__CT_BUILTIN__ }"
        return $?
        ;;
      *)
        [ -n "$__ct_out" ] && printf '%s\n' "$__ct_out"
        return 0
        ;;
    esac
  fi

  command ct "$@"
}
# <<< ct <<<"#;

    block.to_string()
}

fn backup_if_needed(target: &Path) -> io::Result<()> {
    if !target.exists() {
        return Ok(());
    }

    let backup = backup_path(target);
    if backup.exists() {
        return Ok(());
    }

    fs::copy(target, backup)?;
    Ok(())
}

fn backup_path(target: &Path) -> PathBuf {
    let mut p = target.as_os_str().to_owned();
    p.push(".ct.bak");
    PathBuf::from(p)
}

fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let tmp = path.with_extension("tmp.ct");
    fs::write(&tmp, content)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn normalize_newlines(mut text: String) -> String {
    text = text.replace("\r\n", "\n");
    text
}
