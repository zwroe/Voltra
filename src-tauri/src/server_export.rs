use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    mods,
    storage::{check_directory, check_filename, copy_tree, regular_file, Store},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub path: PathBuf,
    pub server: usize,
    pub client: usize,
    pub unclassified: usize,
    pub disabled: usize,
}

pub fn copy(store: &Store, id: Uuid, destination: &Path) -> Result<Export> {
    check_directory(destination)?;
    let game = store.game_dir(id)?;
    ensure!(
        !destination
            .canonicalize()?
            .starts_with(game.canonicalize()?),
        "Choose an export folder outside this instance's game folder."
    );

    let stage = tempfile::tempdir_in(destination)?;
    let root = stage.path().join("export");
    for name in ["server", "client"] {
        fs::create_dir_all(root.join(name).join("mods"))?;
    }
    let code = Uuid::new_v4().simple().to_string();
    let path = destination.join(format!("Voltra server export-{}", &code[..8]));
    let mut result = Export {
        path,
        server: 0,
        client: 0,
        unclassified: 0,
        disabled: 0,
    };
    let directory = game.join("mods");
    for item in mods::list(store, id)? {
        if !item.enabled {
            result.disabled += 1;
            continue;
        }
        check_filename(&item.file)?;
        let source = directory.join(&item.file);
        regular_file(&source)?;
        for side in item.side.folders() {
            let target = root.join(side).join("mods");
            fs::create_dir_all(&target)?;
            fs::copy(&source, target.join(&item.file))?;
            match *side {
                "server" => result.server += 1,
                "client" => result.client += 1,
                _ => result.unclassified += 1,
            }
        }
    }
    for name in ["config", "defaultconfigs"] {
        let source = game.join(name);
        match fs::symlink_metadata(&source) {
            Ok(_) => copy_tree(&source, &root.join(name))?,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                if name == "config" {
                    fs::create_dir(root.join(name))?;
                }
            }
            Err(cause) => return Err(cause.into()),
        }
    }
    let instance = store.instance(id)?;
    let loader = instance.options.loader.as_str();
    let version = instance.options.minecraft_version;
    fs::write(root.join("README.txt"), format!(
        "Minecraft {version} ({loader})\n\nUpload server/mods and the shared config folder to your server. Copy defaultconfigs too if present.\n\nUse client/mods and the same configs for the client. Mods that run on both sides are copied into both folders.\n\nReview any JARs in unclassified/mods before using them. Disabled mods are omitted.\n"
    ))?;
    fs::rename(root, &result.path).context("Finishing the server export")?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_sides::ModSide;
    use std::{fs::File, io::Write};
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn write_mod(path: &Path, manifest: &[u8]) -> Result<()> {
        let mut archive = ZipWriter::new(File::create(path)?);
        archive.start_file("fabric.mod.json", SimpleFileOptions::default())?;
        archive.write_all(manifest)?;
        archive.finish()?;
        Ok(())
    }

    #[test]
    fn export_copies_enabled_mods_and_configs_by_side() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let store = Store::new(workspace.path().join("store"))?;
        let id = store.create(serde_json::from_value(serde_json::json!({
            "name": "Server export test", "minecraftVersion": "1.21.1", "loader": "fabric", "ramMib": 4096
        }))?)?.instance.id;
        let game = store.game_dir(id)?;
        let mod_dir = game.join("mods");
        fs::create_dir_all(&mod_dir)?;
        for (name, manifest) in [
            ("client.jar", br#"{"environment":"client"}"#.as_slice()),
            ("server.jar", br#"{"environment":"server"}"#),
            ("shared.jar", br#"{"environment":"*"}"#),
            ("unknown.jar", br#"{}"#),
            ("disabled.jar.disabled", br#"{"environment":"server"}"#),
        ] {
            write_mod(&mod_dir.join(name), manifest)?;
        }
        fs::create_dir_all(game.join("config/subdirectory"))?;
        fs::write(
            game.join("config/subdirectory/sample.toml"),
            "setting = true",
        )?;
        fs::create_dir_all(game.join("defaultconfigs"))?;
        fs::write(game.join("defaultconfigs/world.toml"), "world = true")?;
        assert!(copy(&store, id, &game).is_err());
        let export = copy(&store, id, workspace.path())?;
        assert_eq!(
            (
                export.server,
                export.client,
                export.unclassified,
                export.disabled
            ),
            (2, 2, 1, 1)
        );
        let files = |name: &str| -> Result<Vec<String>> {
            let mut files = crate::storage::list_files(&export.path.join(name))?;
            files.sort();
            Ok(files)
        };
        assert_eq!(files("server/mods")?, ["server.jar", "shared.jar"]);
        assert_eq!(files("client/mods")?, ["client.jar", "shared.jar"]);
        assert_eq!(files("unclassified/mods")?, ["unknown.jar"]);
        assert_eq!(
            fs::read(export.path.join("server/mods/shared.jar"))?,
            fs::read(mod_dir.join("shared.jar"))?
        );
        assert_eq!(
            fs::read_to_string(export.path.join("config/subdirectory/sample.toml"))?,
            "setting = true"
        );
        assert_eq!(
            fs::read_to_string(export.path.join("defaultconfigs/world.toml"))?,
            "world = true"
        );
        assert!(mod_dir.join("disabled.jar.disabled").is_file());

        mods::set_side(&store, id, "unknown.jar", Some(ModSide::Server))?;
        let duplicate = store.duplicate(id)?;
        let export = copy(&store, duplicate.instance.id, workspace.path())?;
        assert!(export.path.join("server/mods/unknown.jar").is_file());
        assert!(!export.path.join("unclassified").exists());
        Ok(())
    }
}
