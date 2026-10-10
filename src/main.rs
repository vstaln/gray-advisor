//! `gray-advisor` sidecar: JSON-lines on stdio. No args (or `sidecar`)
//! runs the wire loop; `advise|model|status` are the standalone CLI.

use gray_advisor as g;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn sidecar() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if req["method"] == "plugin/shutdown" {
            break;
        }
        let Some(id) = req.get("id").cloned() else {
            continue; // notifications need no reply
        };
        let result = match req["method"].as_str().unwrap_or("") {
            "plugin/manifest" => Ok(g::manifest()),
            "command/run" => {
                let argv: Vec<String> = req["params"]["argv"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let sid = req["params"]["session"]["id"].as_str().unwrap_or("");
                Ok(g::advise_command(&argv, sid))
            }
            _ => Err(format!("unsupported method {}", req["method"])),
        };
        let reply = match result {
            Ok(r) => json!({"id": id, "result": r}),
            Err(e) => json!({"id": id, "error": {"code": -32601, "message": e}}),
        };
        let mut o = stdout.lock();
        let _ = writeln!(o, "{reply}");
        let _ = o.flush();
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("advise") | Some("model") | Some("status") => {
            let v = g::advise_command(&args, "");
            if let Some(t) = v["text"].as_str() {
                println!("{t}");
            } else if let Some(p) = v["prompt"].as_str() {
                println!("{p}");
            }
        }
        _ => {
            if let Err(e) = sidecar() {
                eprintln!("gray-advisor: {e}");
            }
        }
    }
}
