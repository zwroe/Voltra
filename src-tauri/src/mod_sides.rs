use std::{fs::File, io::Read, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSide {
    Client,
    Server,
    Both,
    Singleplayer,
    #[default]
    Unknown,
}

impl ModSide {
    pub fn from_modrinth(
        environment: Option<&str>,
        client: Option<&str>,
        server: Option<&str>,
    ) -> Self {
        match environment {
            Some("client_only") => return Self::Client,
            Some("singleplayer_only") => return Self::Singleplayer,
            Some("server_only" | "dedicated_server_only") => return Self::Server,
            Some(
                "client_and_server"
                | "client_or_server"
                | "client_or_server_prefers_both"
                | "client_only_server_optional"
                | "server_only_client_optional",
            ) => return Self::Both,
            _ => (),
        }
        match (client, server) {
            (Some("required" | "optional"), Some("unsupported")) => Self::Client,
            (Some("unsupported"), Some("required" | "optional")) => Self::Server,
            (Some("required" | "optional"), Some("required" | "optional")) => Self::Both,
            _ => Self::Unknown,
        }
    }

    pub fn from_jar(path: &Path) -> Self {
        jar_side(path).unwrap_or_default()
    }

    pub fn folders(self) -> &'static [&'static str] {
        match self {
            Self::Client | Self::Singleplayer => &["client"],
            Self::Server => &["server"],
            Self::Both => &["server", "client"],
            Self::Unknown => &["unclassified"],
        }
    }
}

fn jar_side(path: &Path) -> Result<ModSide> {
    let manifest = fabric_manifest(path)?;
    Ok(
        match manifest.as_ref().and_then(|info| info.get("environment")) {
            Some(Value::String(side)) if side == "client" => ModSide::Client,
            Some(Value::String(side)) if side == "server" => ModSide::Server,
            Some(Value::String(side)) if side == "*" => ModSide::Both,
            _ => ModSide::Unknown,
        },
    )
}

pub fn fabric_manifest(path: &Path) -> Result<Option<Value>> {
    crate::storage::regular_file(path)?;
    let mut zip = zip::ZipArchive::new(File::open(path)?)?;
    let Ok(metadata) = zip.by_name("fabric.mod.json") else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_reader(metadata.take(1024 * 1024))?))
}

#[cfg(test)]
mod tests {
    use super::ModSide;

    #[test]
    fn singleplayer_is_distinct_from_client_only_and_exports_to_client() {
        let singleplayer = ModSide::from_modrinth(
            Some("singleplayer_only"),
            Some("required"),
            Some("unsupported"),
        );
        assert_eq!(singleplayer, ModSide::Singleplayer);
        assert_eq!(singleplayer.folders(), ["client"]);
        assert_eq!(
            ModSide::from_modrinth(None, Some("required"), Some("unsupported")),
            ModSide::Client
        );
    }
}
