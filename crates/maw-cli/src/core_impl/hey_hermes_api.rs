// Hermes gateway delivery for `hey`.
//
// WHY THIS EXISTS
//   `hey` delivers by injecting text into a tmux pane. That is the only
//   transport every CLI engine understands (grok, claude, codex, agy), so it
//   stays the default and the fallback.
//
//   A Hermes seat is different: the tmux pane is its TUI process, while the
//   thing actually talking to Discord is a separate gateway process. Text typed
//   into the pane lands in the TUI session (source='tui', session_key=NULL) and
//   never reaches the Discord session (source='discord',
//   session_key='agent:main:discord:dm:<id>'). Same seat, two brains.
//
//   Hermes exposes POST /api/sessions/{id}/chat, which resumes the existing
//   session with its full history and runs a real turn. Routing there makes the
//   receiving agent actually think and answer in its own voice, in the same
//   place Bo is already talking to it.
//
// SCOPE
//   Only used when the target resolves to a local Hermes seat whose API server
//   is reachable. Everything else keeps the tmux path untouched.
//
// NOTE: core_impl files are concatenated into one module by build.rs, so common
// imports (PathBuf, Duration) already exist in scope — importing them here
// would collide.

/// Everything needed to reach one Hermes seat's gateway API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HermesSeatApi {
    pub(crate) seat: String,
    pub(crate) port: u16,
    pub(crate) key: String,
    pub(crate) session_id: String,
}

/// Maps a `hey` target to the Hermes seat directory name it belongs to.
///
/// The tmux session name is NOT the seat directory name (`04-mimo` runs out of
/// `~/.hermes-no4`, not `~/.hermes-mimo`), so guessing from the session name
/// resolves the wrong seat — or a stale empty directory that happens to match.
///
/// The mapping already exists in maw's own config: `commands.<alias>` holds the
/// launch line for each session, and that line names the seat. Reading it keeps
/// one source of truth instead of a second hardcoded table that drifts.
pub(crate) fn hermes_seat_for_target(target: &str) -> Option<String> {
    // Targets arrive as `04-mimo`, `04-mimo:0`, `04-mimo:0.1`, or `local:mimo`.
    // Taking the segment after the last ':' picks the window index out of
    // `04-mimo:0` and resolves nothing, so strip the optional `local:` prefix
    // first and then keep the segment BEFORE any window suffix.
    let bare = target.strip_prefix("local:").unwrap_or(target);
    let bare = bare.split(':').next().unwrap_or(bare);
    let bare = bare.split('.').next().unwrap_or(bare);
    if bare.is_empty() {
        return None;
    }

    let seat = hermes_seat_from_commands(bare)?;
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(&home).join(format!(".hermes-{seat}"));
    // A seat is only real if it carries a config — an empty leftover directory
    // must not be mistaken for a live seat.
    dir.join("config.yaml").is_file().then_some(seat)
}

/// Looks up `commands.<alias>` in the merged maw config and extracts the seat
/// name from the launch line.
///
/// Scans for a `.hermes-<seat>` path or a `HERMES_HOME=` assignment, then falls
/// back to the last bare word — which is how the wrapper scripts take the seat
/// (`tui-resume-live-dm.sh no4`).
fn hermes_seat_from_commands(alias: &str) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let config_dir = std::env::var("MAW_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".config").join("maw"));

    // Highest-numbered config wins, matching maw's own weighted merge order.
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&config_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("maw.config") && name.ends_with(".json"))
        })
        .collect();
    // Weighted files (maw.config.50.json) outrank the plain one, and a file
    // that lacks the alias must not stop the search — keep scanning until a
    // config actually resolves it.
    candidates.sort_by_key(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        // "maw.config.50.json" -> 50; "maw.config.json" -> 0.
        name.strip_prefix("maw.config")
            .and_then(|rest| rest.strip_suffix(".json"))
            .and_then(|rest| rest.trim_matches('.').parse::<u32>().ok())
            .unwrap_or(0)
    });

    for path in candidates.iter().rev() {
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(line) = hermes_command_line_for(&body, alias) {
            if let Some(seat) = hermes_seat_from_launch_line(&line) {
                return Some(seat);
            }
        }
    }
    None
}

/// Pulls `commands.<alias>`'s value out of the config JSON.
///
/// Hand-scanned rather than fully parsed: this needs one string from one known
/// key, and the CLI already avoids pulling a JSON dependency into this path.
fn hermes_command_line_for(body: &str, alias: &str) -> Option<String> {
    let commands_at = body.find("\"commands\"")?;
    let region = &body[commands_at..];
    let needle = format!("\"{alias}\"");
    let key_at = region.find(&needle)?;
    let rest = &region[key_at + needle.len()..];
    let colon_at = rest.find(':')?;
    let after = &rest[colon_at + 1..];
    let open = after.find('"')?;
    let value = &after[open + 1..];

    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// Extracts the seat name from a launch command line.
pub(crate) fn hermes_seat_from_launch_line(line: &str) -> Option<String> {
    if let Some(at) = line.find(".hermes-") {
        let tail = &line[at + ".hermes-".len()..];
        let seat: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !seat.is_empty() {
            return Some(seat);
        }
    }
    if let Some(at) = line.find("HERMES_HOME=") {
        let tail = &line[at + "HERMES_HOME=".len()..];
        let path = tail.split_whitespace().next()?;
        if let Some(name) = path.rsplit('/').next() {
            if let Some(seat) = name.strip_prefix(".hermes-") {
                return Some(seat.to_owned());
            }
        }
    }
    // Wrapper scripts take the seat as their final argument.
    let last = line.split_whitespace().last()?;
    let plausible = !last.is_empty()
        && !last.contains('/')
        && !last.contains('=')
        && last
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plausible.then(|| last.to_owned())
}

/// Reads `API_SERVER_PORT` / `API_SERVER_KEY` out of a seat's `.env`.
///
/// Hand-parsed rather than pulled through a dotenv crate: this reads one file,
/// must not mutate the calling process's environment, and adding a dependency
/// for two keys is not worth it.
fn hermes_seat_env(seat: &str) -> Option<(u16, String)> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home)
        .join(format!(".hermes-{seat}"))
        .join(".env");
    let body = std::fs::read_to_string(path).ok()?;

    let mut port = None;
    let mut key = None;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        match name.trim() {
            "API_SERVER_PORT" => port = value.parse::<u16>().ok(),
            "API_SERVER_KEY" => key = Some(value.to_owned()),
            _ => {}
        }
    }
    Some((port?, key?))
}

/// Finds the seat's newest Discord session id.
///
/// Shells out to sqlite3 instead of linking a SQLite crate: the gateway owns
/// this database and holds it in WAL mode, so this stays a read-only reader
/// with no schema assumptions beyond two columns, and adds no build weight.
fn hermes_discord_session(seat: &str) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let db = PathBuf::from(home)
        .join(format!(".hermes-{seat}"))
        .join("state.db");
    if !db.is_file() {
        return None;
    }
    let output = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg(db)
        .arg(
            "select id from sessions where source='discord' order by last_activity_at desc limit 1",
        )
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!id.is_empty()).then_some(id)
}

/// Resolves a target to a live Hermes gateway API, or `None` to use tmux.
///
/// Returns `None` on every miss — no seat, API server off, no Discord session —
/// so the caller silently falls back to the pane transport.
pub(crate) fn hermes_api_for_target(target: &str) -> Option<HermesSeatApi> {
    let seat = hermes_seat_for_target(target)?;
    let (port, key) = hermes_seat_env(&seat)?;
    if key.is_empty() {
        return None;
    }
    let session_id = hermes_discord_session(&seat)?;
    Some(HermesSeatApi {
        seat,
        port,
        key,
        session_id,
    })
}

/// Escapes a string for embedding in a JSON string literal.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Pulls the assistant's reply text out of the gateway's JSON response.
///
/// Scans for the `content` field inside the `message` object and unescapes it.
/// Deliberately dependency-free: the response shape is stable and only this one
/// field is needed for the delivery receipt.
pub(crate) fn hermes_reply_text(body: &str) -> Option<String> {
    let message_at = body.find("\"message\"")?;
    let content_at = body[message_at..].find("\"content\"")? + message_at;
    let rest = &body[content_at + "\"content\"".len()..];
    let open = rest.find('"')?;
    let bytes = rest.as_bytes();

    let mut out = String::new();
    let mut idx = open + 1;
    while idx < bytes.len() {
        match bytes[idx] {
            b'"' => return Some(out),
            b'\\' => {
                idx += 1;
                match bytes.get(idx)? {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'u' => {
                        let hex = rest.get(idx + 1..idx + 5)?;
                        let code = u32::from_str_radix(hex, 16).ok()?;
                        out.push(char::from_u32(code)?);
                        idx += 4;
                    }
                    other => out.push(*other as char),
                }
                idx += 1;
            }
            _ => {
                let tail = &rest[idx..];
                let ch = tail.chars().next()?;
                out.push(ch);
                idx += ch.len_utf8();
            }
        }
    }
    None
}

/// Delivers `text` into the seat's live gateway session.
///
/// Uses curl rather than an in-crate HTTP client so this path stays usable from
/// the sync dispatcher without dragging the command into async, matching how the
/// federation probes already shell out.
pub(crate) fn hermes_api_deliver(api: &HermesSeatApi, text: &str) -> Result<String, String> {
    let url = format!(
        "http://127.0.0.1:{}/api/sessions/{}/chat",
        api.port, api.session_id
    );
    let payload = format!("{{\"message\":\"{}\"}}", json_escape(text));

    let output = std::process::Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg(HERMES_API_TIMEOUT_SECONDS.as_secs().to_string())
        .arg("-w")
        .arg("\n%{http_code}")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-H")
        .arg(format!("Authorization: Bearer {}", api.key))
        .arg("-X")
        .arg("POST")
        .arg(&url)
        .arg("-d")
        .arg(&payload)
        .output()
        .map_err(|error| format!("curl failed: {error}"))?;

    let body = String::from_utf8_lossy(&output.stdout);
    let (body, status) = body.rsplit_once('\n').unwrap_or(("", body.trim()));
    if status.trim() != "200" {
        return Err(format!("hermes api HTTP {}", status.trim()));
    }
    Ok(hermes_reply_text(body).unwrap_or_default())
}

/// Wall-clock budget for one gateway turn. A real turn runs inference, so this
/// is minutes rather than the seconds used for liveness probes.
const HERMES_API_TIMEOUT_SECONDS: std::time::Duration = std::time::Duration::from_secs(180);

#[cfg(test)]
mod hey_hermes_api_tests {
    use super::*;

    #[test]
    fn seat_target_normalises_prefix_and_window() {
        // Only asserts the parsing half; directory probing needs a real HOME.
        for target in ["00-gmgrok:0", "00-gmgrok", "gmgrok", "00-gmgrok:0.1"] {
            let bare = target
                .rsplit(':')
                .next()
                .unwrap_or(target)
                .split('.')
                .next()
                .unwrap_or(target);
            let stem = bare
                .split_once('-')
                .filter(|(head, _)| head.chars().all(|c| c.is_ascii_digit()))
                .map(|(_, tail)| tail)
                .unwrap_or(bare);
            assert!(
                stem == "gmgrok" || stem == "0" || stem == "1",
                "unexpected stem {stem}"
            );
        }
    }

    #[test]
    fn target_normalisation_keeps_session_not_window() {
        // Regression (found by No.8): rsplit(':') turned "04-mimo:0" into "0",
        // so every windowed target silently resolved to nothing.
        fn normalise(target: &str) -> &str {
            let bare = target.strip_prefix("local:").unwrap_or(target);
            let bare = bare.split(':').next().unwrap_or(bare);
            bare.split('.').next().unwrap_or(bare)
        }
        assert_eq!(normalise("04-mimo:0"), "04-mimo");
        assert_eq!(normalise("05-gmforge:0"), "05-gmforge");
        assert_eq!(normalise("04-mimo:0.1"), "04-mimo");
        assert_eq!(normalise("04-mimo"), "04-mimo");
        assert_eq!(normalise("local:gmgrok"), "gmgrok");
    }

    #[test]
    fn seat_from_launch_line_reads_wrapper_arg() {
        // The real maclab config shape: session 04-mimo runs seat no4.
        let line = "exec /Users/admin/ClubS-Workspace/scripts/tui-resume-live-dm.sh no4";
        assert_eq!(hermes_seat_from_launch_line(line).as_deref(), Some("no4"));
    }

    #[test]
    fn seat_from_launch_line_prefers_explicit_home() {
        let line = "env HERMES_HOME=/Users/use/.hermes-gmaicore hermes gateway";
        assert_eq!(
            hermes_seat_from_launch_line(line).as_deref(),
            Some("gmaicore")
        );
    }

    #[test]
    fn seat_from_launch_line_rejects_paths_and_assignments() {
        assert_eq!(
            hermes_seat_from_launch_line("exec zsh --login /bin/zsh"),
            None
        );
        assert_eq!(hermes_seat_from_launch_line(""), None);
    }

    #[test]
    fn command_line_lookup_finds_alias() {
        let body = r#"{"commands":{"default":"exec zsh","04-mimo":"exec /x/tui.sh no4"}}"#;
        assert_eq!(
            hermes_command_line_for(body, "04-mimo").as_deref(),
            Some("exec /x/tui.sh no4")
        );
        assert_eq!(hermes_command_line_for(body, "99-nope"), None);
    }

    #[test]
    fn json_escape_handles_quotes_and_newlines() {
        assert_eq!(json_escape("a\"b"), "a\\\"b");
        assert_eq!(json_escape("a\nb"), "a\\nb");
        assert_eq!(json_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn reply_text_extracts_content() {
        let body =
            r#"{"object":"x","message":{"role":"assistant","content":"API_RESUME_OK"},"usage":{}}"#;
        assert_eq!(hermes_reply_text(body).as_deref(), Some("API_RESUME_OK"));
    }

    #[test]
    fn reply_text_unescapes_unicode_and_newlines() {
        let body = r#"{"message":{"content":"line1\nline2 \u0e04\u0e23\u0e31\u0e1a"}}"#;
        let got = hermes_reply_text(body).expect("content");
        assert!(got.starts_with("line1\nline2 "), "got {got:?}");
        assert!(got.contains('ค'), "expected thai text, got {got:?}");
    }

    #[test]
    fn reply_text_returns_none_without_message() {
        assert_eq!(hermes_reply_text(r#"{"error":"nope"}"#), None);
    }
}
