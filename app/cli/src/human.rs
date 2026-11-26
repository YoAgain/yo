//! Text for a person on a terminal: what each command's JSON says.

use serde_json::Value;

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// A bar's name as the app shows it: in capitals, `/BOT` after a bot, `#` before a group.
pub fn bar_name(item: &Value) -> String {
    let name = s(&item["name"]).to_uppercase();
    match (s(&item["kind"]), item["bot"].as_bool().unwrap_or(false)) {
        ("group", _) => format!("#{name}"),
        (_, true) => format!("{name} /BOT"),
        _ => name,
    }
}

fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

pub fn render(kind: &str, value: &Value) -> String {
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    match kind {
        "whoami" | "status" | "signup" | "signin" => {
            let mut line =
                format!("{}{}", s(&value["username"]).to_uppercase(), if value["bot"] == true { " /BOT" } else { "" });
            line += &format!(
                " · device {} · hub {}",
                device(&value["device"]),
                s(&value["hub"]).trim_start_matches("https://")
            );
            if let Some(listening) = value["listening"].as_bool() {
                line += if listening { " · listener running" } else { " · no listener" };
                line += &format!(
                    "\n  {} friends · {} groups · {} waiting to send",
                    value["friends"], value["groups"], value["waiting"]
                );
            }
            if kind == "signup" {
                line = format!("Signed up: {line}");
                if value["bot"] == true {
                    line += "\n  Marked as a bot (people see /BOT); a bot cannot become a person later.";
                }
            }
            if kind == "signin" {
                line = format!("Signed in: {line}");
            }
            if let Some(key) = value["recovery_key"].as_str() {
                line += &format!("\n  Recovery key  {}\n  Store it now; it is shown once (yo recovery show reads it from the state dir).", group(key));
            }
            if let Some(error) = value["recovery_error"].as_str() {
                line += &format!("\n  No recovery key yet ({error}): yo recovery rotate");
            }
            line
        }
        "friends-view" => {
            let items = list(value);
            if items.is_empty() {
                return "No friends yet: yo friends add NAME".into();
            }
            items
                .iter()
                .map(|i| {
                    let mut line = format!("  {:<24}", bar_name(i));
                    if i["kind"] == "group" {
                        line += &format!(" group, {} members", i["members"]);
                    }
                    if i["blocked"] == true {
                        line += " blocked";
                    }
                    if i["gone"] == true {
                        line += " gone (yo friends remove)";
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "friends" | "contacts" => list(value)
            .iter()
            .map(|f| {
                let name = f["handles"].get(0).or(f.get("name")).map(s).unwrap_or("").to_uppercase();
                format!("  {name:<24} {}", s(&f["user"]))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "requests" if value.get("requests").is_some() => {
            let mut out = format!(
                "Requests from strangers: {}",
                if value["requests_open"] == true { "on" } else { "off (yo requests on)" }
            );
            for r in list(&value["requests"]) {
                let name = s(&r["name"]);
                out += &format!(
                    "\n  {:<20} [yo requests accept {name} | decline {name} | block {name}]",
                    name.to_uppercase()
                );
            }
            out
        }
        "requests" if value.get("requests_open").is_some() => {
            format!("Requests from strangers: {}", if value["requests_open"] == true { "on" } else { "off" })
        }
        "groups" => list(value)
            .iter()
            .filter(|g| g["left"] != true)
            .map(|g| {
                let pending = if g["accepted"] == false { "  (invited: yo groups accept ID)" } else { "" };
                format!(
                    "  #{:<20} {} members  group:{}{pending}",
                    s(&g["name"]).to_uppercase(),
                    list(&g["members"]).len(),
                    s(&g["id"])
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "devices" => list(value)
            .iter()
            .map(|d| {
                let this = if d["this"] == true { "  (this one)" } else { "" };
                format!(
                    "  {}  {}  last seen {}{this}",
                    d["number"],
                    s(&d["app"]),
                    ago(d["last_seen_at"].as_u64().unwrap_or(0))
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "connections" => list(value)
            .iter()
            .map(|c| {
                format!("  {:<20} {}", s(&c["name"]), if c["connected"] == true { "connected" } else { "waiting" })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "recovery" => format!("Recovery key  {}", group(s(&value["recovery_key"]))),
        "whois" => format!(
            "{} {}{}",
            s(&value["username"]).to_uppercase(),
            s(&value["user"]),
            if value["bot"] == true { " /BOT" } else { "" }
        ),
        "add" => format!("Added {}", s(&value["username"]).to_uppercase()),
        "remove" => "Removed".into(),
        "version" => format!("yo {}", s(&value["version"])),
        "probe" => format!("PING answered in {} ms", value["ms"]),
        "doctor" => list(&value["checks"])
            .iter()
            .map(|c| {
                format!("  {} {:<14} {}", if c["ok"] == true { "ok " } else { "!! " }, s(&c["check"]), s(&c["detail"]))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        k if k.starts_with("send ") => format!("Yo → {} ✓", k[5..].trim_start_matches('@').to_uppercase()),
        _ => match value {
            Value::Object(o) if o.len() == 1 && o.values().all(|v| v == &Value::Bool(true)) => "done".into(),
            Value::Array(a) if a.is_empty() => "nothing".into(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        },
    }
}

/// A device id as a short hex.
fn device(v: &Value) -> String {
    let bytes = v.as_array().cloned().unwrap_or_default();
    bytes.iter().take(4).filter_map(Value::as_u64).map(|b| format!("{b:02x}")).collect()
}

/// The recovery key in groups of four.
fn group(key: &str) -> String {
    key.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).to_uppercase()).collect::<Vec<_>>().join("-")
}

/// Completions of the first word.
pub fn completion(shell: &str, verbs: &[&str]) -> String {
    let words: Vec<&str> = verbs.iter().copied().filter(|v| !v.contains('-') || *v == "echo-probe").collect();
    let words = words.join(" ");
    match shell {
        "bash" => format!("complete -W \"{words}\" yo"),
        "zsh" => format!("#compdef yo\n_arguments '1:command:({words})'"),
        "fish" => format!("complete -c yo -f -n __fish_use_subcommand -a \"{words}\""),
        _ => "completion bash|zsh|fish".into(),
    }
}
