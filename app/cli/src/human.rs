//! Text for a person on a terminal: what each command's JSON says.

use serde_json::Value;

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// A bar's name as the app shows it: in capitals, `/` before a bot, `#` before a group.
pub fn bar_name(item: &Value) -> String {
    let name = s(&item["name"]).to_uppercase();
    yo_ui::line::bar_title(&name, s(&item["kind"]) == "group", item["bot"].as_bool().unwrap_or(false))
}

pub fn ago(ms: u64) -> String {
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
            let mut out = match kind {
                "signup" => format!("{} ", paint(DIM, "Signed up")),
                "signin" => format!("{} ", paint(DIM, "Signed in")),
                _ => String::new(),
            };
            out += &paint(
                BOLD,
                &yo_ui::line::bar_title(&s(&value["username"]).to_uppercase(), false, value["bot"] == true),
            );
            let mut rows = vec![
                ("Device", device(&value["device"])),
                ("Hub", s(&value["hub"]).trim_start_matches("https://").to_owned()),
            ];
            if let Some(listening) = value["listening"].as_bool() {
                rows.push(("Listener", if listening { "running" } else { "stopped (yo listen starts it)" }.into()));
                rows.push((
                    "Friends",
                    format!("{} · Groups {} · Unsent {}", value["friends"], value["groups"], value["waiting"]),
                ));
            }
            for (label, text) in rows {
                out += &format!("\n  {}{text}", paint(DIM, &format!("{label:<10}")));
            }
            if kind == "signup" && value["bot"] == true {
                out += &format!("\n  {}", paint(DIM, "Bot account: people see / before the name. Can't be undone."));
            }
            if let Some(key) = value["recovery_key"].as_str() {
                out += &format!("\n\n  {}\n", paint(BOLD, "Recovery Key"));
                for row in group(key).split('-').collect::<Vec<_>>().chunks(4) {
                    out += &format!("\n    {}", paint(ACCENT, &row.join("  ")));
                }
                out += &format!(
                    "\n\n  {}",
                    paint(DIM, "Save it now, e.g. in a password manager. yo recovery show shows it again.")
                );
            }
            if let Some(error) = value["recovery_error"].as_str() {
                out += &format!("\n  No Recovery Key yet ({error}): yo recovery rotate");
            }
            out
        }
        "friends-view" => {
            let items = list(value);
            if items.is_empty() {
                return format!("No friends yet: {} (try PING)", paint(BOLD, "yo friends add NAME"));
            }
            table(
                items
                    .iter()
                    .map(|i| {
                        let mut what = match (s(&i["kind"]), i["bot"] == true) {
                            ("group", _) => format!("group · {} members", i["members"]),
                            (_, true) => "bot".into(),
                            _ => "friend".into(),
                        };
                        if i["accepted"] == false {
                            what += " · invited (yo groups accept ID)";
                        }
                        if i["blocked"] == true {
                            what += " · blocked";
                        }
                        if i["gone"] == true {
                            what += " · account deleted (yo friends remove)";
                        }
                        vec![bar_name(i), what]
                    })
                    .collect(),
            )
        }
        "friends" | "contacts" => table(
            list(value)
                .iter()
                .map(|f| {
                    let name = f["handles"].get(0).or(f.get("name")).map(s).unwrap_or("").to_uppercase();
                    vec![name, s(&f["user"]).to_owned()]
                })
                .collect(),
        ),
        "requests" if value.get("requests").is_some() => {
            let mut out = format!(
                "Requests from Strangers: {}",
                if value["requests_open"] == true {
                    paint(BOLD, "on")
                } else {
                    paint(BOLD, "off") + &paint(DIM, " (yo requests on)")
                }
            );
            let rows: Vec<Vec<String>> = list(&value["requests"])
                .iter()
                .map(|r| {
                    let name = s(&r["name"]);
                    vec![bar_name(r), format!("yo requests accept|decline|block {name}")]
                })
                .collect();
            if rows.is_empty() {
                out += &format!("\n  {}", paint(DIM, "None"));
            } else {
                out += &format!("\n{}", table(rows));
            }
            out
        }
        "requests" if value.get("requests_open").is_some() => {
            format!("Requests from Strangers: {}", if value["requests_open"] == true { "on" } else { "off" })
        }
        "groups" if list(value).iter().all(|g| g["left"] == true) => {
            "No groups yet: yo groups create NAME MEMBER…".into()
        }
        "connections" if list(value).is_empty() => "No services".into(),
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
        "devices" => table(
            list(value)
                .iter()
                .map(|d| {
                    let this =
                        if d["this"] == true { "  ".to_owned() + &paint(ACCENT, "This device") } else { String::new() };
                    vec![
                        d["number"].to_string(),
                        s(&d["app"]).to_owned(),
                        format!("Last seen {}", ago(d["last_seen_at"].as_u64().unwrap_or(0))) + &this,
                    ]
                })
                .collect(),
        ),
        "connections" => list(value)
            .iter()
            .map(|c| {
                format!("  {:<20} {}", s(&c["name"]), if c["connected"] == true { "connected" } else { "waiting" })
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "recovery" => format!("Recovery Key  {}", group(s(&value["recovery_key"]))),
        "whois" => format!(
            "{}  {}",
            yo_ui::line::bar_title(&s(&value["username"]).to_uppercase(), false, value["bot"] == true),
            s(&value["user"])
        ),
        "add" => format!("Added {}", s(&value["username"]).to_uppercase()),
        "remove" => "Removed".into(),
        "signout" => if value["signed_out"] == true { "Signed out" } else { "Cancelled" }.into(),
        "profiles" => list(value)
            .iter()
            .map(|p| format!("{:<16} {}", p["profile"].as_str().unwrap_or("(default)"), s(&p["state"])))
            .collect::<Vec<_>>()
            .join("\n"),
        "version" => format!(
            "yo {} (core {}, protocol {})",
            s(&value["version"]),
            s(&value["core_version"]),
            value["protocol_version"]
        ),
        "probe" => format!("PING answered in {} ms", value["ms"]),
        "doctor" => list(&value["checks"])
            .iter()
            .map(|c| {
                format!("  {} {:<14} {}", if c["ok"] == true { "ok " } else { "!! " }, s(&c["check"]), s(&c["detail"]))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        k if k.starts_with("send ") => format!("Sent Yo! → {}", k[5..].trim_start_matches('@').to_uppercase()),
        _ => match value {
            Value::Object(o) if o.len() == 1 && o.values().all(|v| v == &Value::Bool(true)) => "Done".into(),
            Value::Array(a) if a.is_empty() => "Nothing".into(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        },
    }
}

const BOLD: &str = "1";
const DIM: &str = "2";
const ACCENT: &str = "1;35";

/// Text in an SGR style, on a terminal with colour; as it is otherwise.
pub fn paint(code: &str, text: &str) -> String {
    use std::io::IsTerminal;
    if crate::mode().color && !crate::mode().json && std::io::stdout().is_terminal() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Rows in aligned columns: the first bold, the last dim, the ones between plain.
fn table(rows: Vec<Vec<String>>) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|t| t.chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(c, text)| {
                    let last = c + 1 == row.len();
                    let padded = if last { text.clone() } else { format!("{text:<w$}", w = widths[c]) };
                    match c {
                        0 => paint(BOLD, &padded),
                        _ if last && columns > 1 => paint(DIM, &padded),
                        _ => padded,
                    }
                })
                .collect();
            format!("  {}", cells.join("   "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A device id as a short hex.
pub fn device(v: &Value) -> String {
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
    let words = words.join(" ") + " --profile --state --hub --json";
    match shell {
        "bash" => format!("complete -W \"{words}\" yo"),
        "zsh" => format!("#compdef yo\n_arguments '1:command:({words})'"),
        "fish" => format!("complete -c yo -f -n __fish_use_subcommand -a \"{words}\""),
        _ => "usage: yo completion bash|zsh|fish".into(),
    }
}
