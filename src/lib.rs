//! gray-advisor — port of pi-advisor-flow. `/advise <q>` ships the question
//! plus a digest of the current session to a second, separately configured
//! "advisor" model and returns its take so the executor continues with both.
//!
//! Advisor call choice (documented per the port spec): a `gray -p --bare`
//! subprocess, not the wire's `host/run`. `host/run` has no model-selection
//! parameter — it always runs the session's own model — and caps at ~28s;
//! the whole point is a *different* configured model. The subprocess takes
//! `--model`, needs no `host.turn` capability grant, and keeps this plugin
//! self-contained.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PLUGIN_NAME: &str = "advisor";
pub const PROTOCOL: &str = "1.1";
/// command/run answers under the host's 30s outer TTL.
pub const ADVISOR_TIMEOUT: Duration = Duration::from_secs(24);
const DIGEST_MESSAGES: usize = 8;
const DIGEST_CHARS: usize = 160;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `provider/model-id` the advisor runs, e.g. "openrouter/openai/gpt-5".
    #[serde(default)]
    pub model: String,
    /// gray binary to spawn.
    #[serde(default = "default_bin")]
    pub bin: String,
}

fn default_bin() -> String {
    "gray".to_string()
}

pub fn gray_home() -> PathBuf {
    if let Some(p) = std::env::var_os("GRAY_HOME") {
        return PathBuf::from(p);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".gray")
}

pub fn config_path() -> PathBuf {
    gray_home().join("advisor").join("config.json")
}

pub fn load_config() -> Config {
    let mut c: Config = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if c.bin.is_empty() {
        c.bin = default_bin();
    }
    if let Ok(m) = std::env::var("GRAY_ADVISOR_MODEL") {
        if !m.trim().is_empty() {
            c.model = m;
        }
    }
    if let Ok(b) = std::env::var("GRAY_ADVISOR_BIN") {
        if !b.trim().is_empty() {
            c.bin = b;
        }
    }
    c
}

pub fn save_config(c: &Config) -> std::io::Result<()> {
    let p = config_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, serde_json::to_string_pretty(c)?)
}

/// Short tail-of-transcript digest from `~/.gray/sessions/<id>.jsonl`:
/// last N entries, `role: first 160 chars` of text/tool names.
pub fn session_digest(session_id: &str) -> String {
    if session_id.is_empty() {
        return String::new();
    }
    let path = gray_home()
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut rows: Vec<String> = Vec::new();
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(m) = v.get("message") else { continue };
        let role = m["role"].as_str().unwrap_or("?");
        let mut text = String::new();
        if let Some(parts) = m["content"].as_array() {
            for p in parts {
                match p["type"].as_str().unwrap_or("") {
                    "text" => {
                        if let Some(t) = p["text"].as_str() {
                            if !text.is_empty() {
                                text.push(' ');
                            }
                            text.push_str(t);
                        }
                    }
                    "tool_use" => {
                        let n = p["name"].as_str().unwrap_or("tool");
                        text.push_str(&format!("[{n}]"));
                    }
                    _ => {}
                }
            }
        }
        let text: String = text.chars().take(DIGEST_CHARS).collect();
        if !text.trim().is_empty() {
            rows.push(format!("{role}: {}", text.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
    }
    rows.iter()
        .rev()
        .take(DIGEST_MESSAGES)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prompt handed to the advisor model.
pub fn advisor_prompt(question: &str, digest: &str) -> String {
    let mut p = format!(
        "You are an advisor model giving a second opinion to a primary coding \
         agent that is already working. Be concise (under 200 words): name the \
         biggest risk, the approach you'd take, and one thing to verify.\n\n\
         Question under consideration:\n{question}\n"
    );
    if !digest.trim().is_empty() {
        p.push_str(&format!("\nCurrent session digest (tail):\n{digest}\n"));
    }
    p
}

/// Run `gray -p --bare --model <m> <prompt>`; returns the advisor's text.
/// Kills the child after [`ADVISOR_TIMEOUT`] so `command/run` stays under
/// the host's outer TTL.
pub fn ask_advisor(cfg: &Config, prompt: &str) -> Result<String, String> {
    if cfg.model.trim().is_empty() {
        return Err(
            "no advisor model configured — `/advise model <provider/model-id>` \
             or GRAY_ADVISOR_MODEL"
                .to_string(),
        );
    }
    let mut child = Command::new(&cfg.bin)
        .args([
            "-p",
            prompt,
            "--model",
            &cfg.model,
            "--bare",
            "--max-requests",
            "2",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", cfg.bin))?;
    let deadline = Instant::now() + ADVISOR_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    use std::io::Read;
                    let _ = so.read_to_string(&mut out);
                }
                let take = out.trim().to_string();
                return if take.is_empty() {
                    Err("advisor returned empty output".to_string())
                } else {
                    Ok(take)
                };
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "advisor timed out after {}s",
                    ADVISOR_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("advisor wait failed: {e}")),
        }
    }
}

/// `command/run` handler for `/advise`.
pub fn advise_command(argv: &[String], session_id: &str) -> Value {
    match argv.first().map(|s| s.as_str()) {
        Some("model") => {
            let model = argv.get(1).cloned().unwrap_or_default();
            if model.trim().is_empty() {
                return json!({"text": "usage: /advise model <provider/model-id>"});
            }
            let mut c = load_config();
            c.model = model.clone();
            match save_config(&c) {
                Ok(()) => json!({"text": format!("advisor model set to {model}")}),
                Err(e) => json!({"text": format!("failed to save config: {e}")}),
            }
        }
        Some("status") | None => {
            let c = load_config();
            let model = if c.model.is_empty() {
                "(unset)".to_string()
            } else {
                c.model.clone()
            };
            json!({"text": format!(
                "advisor: model={model} bin={}\nusage: /advise <question> | /advise model <provider/model-id>",
                c.bin
            )})
        }
        _ => {
            let question = argv.join(" ");
            let cfg = load_config();
            let digest = session_digest(session_id);
            match ask_advisor(&cfg, &advisor_prompt(&question, &digest)) {
                // `{prompt}` continues the executor with both the question
                // and the advisor's take — the pi-advisor-flow handoff.
                Ok(take) => json!({"prompt": format!(
                    "Advisor ({}) take:\n{take}\n\nQuestion: {question}\n\nWeigh the advisor's take and proceed.",
                    cfg.model
                )}),
                Err(e) => json!({"text": format!("advise failed: {e}")}),
            }
        }
    }
}

pub fn manifest() -> Value {
    json!({
        "name": PLUGIN_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "sidecar_args": ["sidecar"],
        "tools": [],
        "commands": ["/advise"],
        "completion": ["model", "status"],
        "hooks": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_reads_last_entries() {
        let dir = std::env::temp_dir().join(format!("adv-test-{}", std::process::id()));
        let sess = dir.join("sessions");
        std::fs::create_dir_all(&sess).unwrap();
        std::fs::write(
            sess.join("s1.jsonl"),
            concat!(
                "{\"version\":1,\"id\":\"s1\"}\n",
                "{\"entry_id\":0,\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello world\"}]}}\n",
                "{\"entry_id\":1,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"bash\"},{\"type\":\"text\",\"text\":\"ran it\"}]}}\n"
            ),
        )
        .unwrap();
        std::env::set_var("GRAY_HOME", &dir);
        let d = session_digest("s1");
        assert!(d.contains("user: hello world"));
        assert!(d.contains("assistant: [bash] ran it"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_includes_question_and_digest() {
        let p = advisor_prompt("why?", "user: hi");
        assert!(p.contains("why?") && p.contains("user: hi") && p.contains("advisor"));
    }

    #[test]
    fn unconfigured_advisor_errors() {
        let e = ask_advisor(&Config { model: String::new(), bin: "gray".into() }, "q")
            .unwrap_err();
        assert!(e.contains("no advisor model"));
    }

    #[test]
    fn status_shape() {
        let v = advise_command(&["status".to_string()], "");
        assert!(v["text"].as_str().unwrap().contains("usage:"));
    }
}
