//! The services as a list for the apps and the web, from the folders' TOMLs and logos, read once at the start.

use crate::gatekeeper::Header;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

const MAX_LOGO: u64 = 256 * 1024;

struct Service {
    folder: String,
    group: String,
    header: Header,
    logo: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct List {
    /// `[{ name, group, about, bridge, logo }]`: `group` is the folder above, `bridge` `{ return_url, help_url }` or null,
    /// `logo` a path at the API's address or null.
    pub json: String,
    /// PNGs, by service.
    pub logos: HashMap<String, Vec<u8>>,
}

impl List {
    /// Of every `GROUP/SERVICE` folder in `root`; one that does not read is left out.
    pub fn read(root: &Path) -> List {
        let mut found: Vec<Service> =
            folders(root).iter().flat_map(|group| folders(group)).filter_map(|f| service(&f)).collect();
        found.sort_by(|a, b| a.folder.cmp(&b.folder));
        let mut list = List::default();
        let mut values = Vec::new();
        for Service { folder, group, header, logo } in found {
            let name = &header.names[0];
            let path = logo.as_ref().map(|png| {
                // A new logo is another address: no cache shows the old one.
                let hash = Sha256::digest(png);
                format!("/services/{folder}.png?v={:02x}{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2], hash[3])
            });
            let bridge = header.bridge.as_ref();
            let bridge = bridge.map(|b| json!({ "return_url": b.return_url, "help_url": b.help_url }));
            values.push(json!({
                "name": name, "group": group, "about": header.about, "bridge": bridge, "logo": path,
            }));
            if let Some(logo) = logo {
                list.logos.insert(folder, logo);
            }
        }
        list.json = Value::Array(values).to_string();
        list
    }
}

fn folders(of: &Path) -> Vec<std::path::PathBuf> {
    let entries = std::fs::read_dir(of).into_iter().flatten().flatten();
    entries.filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
}

fn service(folder: &Path) -> Option<Service> {
    let name = folder.file_name()?.to_str()?;
    let text = std::fs::read_to_string(folder.join(format!("{name}.toml"))).ok()?;
    let header = Header::parse(&text, name).map_err(|why| eprintln!("not listed: {name}: {why}")).ok()?;
    let logo = folder.join("logo.png");
    let small = logo.symlink_metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_LOGO);
    let logo = small.then(|| std::fs::read(logo).ok()).flatten().filter(|png| png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let group = folder.parent()?.file_name()?.to_str()?.to_owned();
    Some(Service { folder: name.to_owned(), group, header, logo })
}
