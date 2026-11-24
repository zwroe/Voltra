use crate::storage::{
    atomic_json, check_directory, check_filename, list_files, read_default, regular_file, Store,
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    instance_id: Uuid,
    instance: String,
    name: String,
    taken_at: u64,
}

pub fn screenshots(store: &Store, selected: Option<Uuid>) -> Result<Vec<Screenshot>> {
    let mut result = Vec::new();
    for item in store.instances()? {
        let instance = item.instance;
        if selected.is_some_and(|id| instance.id != id) {
            continue;
        }
        let dir = store.game_dir(instance.id)?.join("screenshots");
        for name in list_files(&dir)?
            .into_iter()
            .filter(|name| name.to_ascii_lowercase().ends_with(".png"))
        {
            let path = dir.join(&name);
            regular_file(&path)?;
            let taken_at = fs::metadata(path)?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            result.push(Screenshot {
                instance_id: instance.id,
                instance: instance.options.name.clone(),
                name,
                taken_at,
            });
        }
    }
    result.sort_unstable_by_key(|shot| std::cmp::Reverse(shot.taken_at));
    Ok(result)
}

pub fn screenshot_path(store: &Store, id: Uuid, name: &str) -> Result<PathBuf> {
    check_filename(name)?;
    ensure!(
        name.to_ascii_lowercase().ends_with(".png"),
        "Choose a Minecraft screenshot."
    );
    let dir = store.game_dir(id)?.join("screenshots");
    check_directory(&dir)?;
    let path = dir.join(name);
    regular_file(&path)?;
    Ok(path)
}

pub fn screenshot_bytes(store: &Store, id: Uuid, name: &str) -> Result<Vec<u8>> {
    let file = screenshot_path(store, id, name)?;
    read_limited(&file, 64 * 1024 * 1024)
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Classic,
    Slim,
}

impl Variant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Slim => "slim",
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    pub id: String,
    pub name: String,
    pub variant: Variant,
    pub height: u32,
    pub builtin: bool,
}

const BUILTINS: [(&str, &str, Variant, &[u8]); 3] = [
    (
        "rift",
        "Rift Scout",
        Variant::Classic,
        include_bytes!("../../public/skins/rift.png"),
    ),
    (
        "ember",
        "Ember Runner",
        Variant::Slim,
        include_bytes!("../../public/skins/ember.png"),
    ),
    (
        "frost",
        "Frost Surveyor",
        Variant::Classic,
        include_bytes!("../../public/skins/frost.png"),
    ),
];

pub fn skins(store: &Store) -> Result<Vec<Skin>> {
    let mut skins = saved_skins(store)?;
    skins.extend(BUILTINS.iter().map(|(id, name, variant, _)| Skin {
        id: (*id).into(),
        name: (*name).into(),
        variant: *variant,
        height: 64,
        builtin: true,
    }));
    Ok(skins)
}

fn saved_skins(store: &Store) -> Result<Vec<Skin>> {
    read_default(&store.root.join("config/skins.json"))
}

static SKIN_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn import_skin(store: &Store, path: &Path, variant: Variant) -> Result<Skin> {
    regular_file(path)?;
    let bytes = read_limited(path, 1024 * 1024)?;
    let height = skin_height(&bytes)?;
    ensure!(
        height == 64 || !matches!(variant, Variant::Slim),
        "Legacy 64×32 skins require classic arms."
    );
    let id = Uuid::new_v4().to_string();
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Imported skin")
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect::<String>();
    let skin = Skin {
        id: id.clone(),
        name,
        variant,
        height,
        builtin: false,
    };
    let directory = store.root.join("skins");
    fs::create_dir_all(&directory)?;
    check_directory(&directory)?;
    let _guard = SKIN_WRITES.lock().expect("skin writes lock");
    let mut saved = saved_skins(store)?;
    let file = directory.join(format!("{id}.png"));
    fs::write(&file, bytes)?;
    saved.insert(0, skin.clone());
    if let Err(error) = atomic_json(&store.root.join("config/skins.json"), &saved) {
        let _ = fs::remove_file(file);
        return Err(error);
    }
    Ok(skin)
}

pub fn skin_bytes(store: &Store, id: &str) -> Result<(Skin, Vec<u8>)> {
    let skin = skins(store)?
        .into_iter()
        .find(|skin| skin.id == id)
        .context("Skin not found.")?;
    let bytes = if let Some((_, _, _, image)) = BUILTINS.iter().find(|(name, _, _, _)| *name == id)
    {
        image.to_vec()
    } else {
        let id = Uuid::parse_str(id).context("Invalid skin ID.")?;
        let directory = store.root.join("skins");
        check_directory(&directory)?;
        let file = directory.join(format!("{id}.png"));
        regular_file(&file)?;
        read_limited(&file, 1024 * 1024)?
    };
    skin_height(&bytes)?;
    Ok((skin, bytes))
}

pub fn set_skin_variant(store: &Store, id: &str, variant: Variant) -> Result<Skin> {
    let _guard = SKIN_WRITES.lock().expect("skin writes lock");
    let mut saved = saved_skins(store)?;
    let skin = saved.iter_mut().find(|skin| skin.id == id).context("Imported skin not found.")?;
    ensure!(skin.height == 64 || !matches!(variant, Variant::Slim), "Legacy 64x32 skins require classic arms.");
    skin.variant = variant;
    let updated = skin.clone();
    atomic_json(&store.root.join("config/skins.json"), &saved)?;
    Ok(updated)
}

pub fn worlds(store: &Store, id: Uuid) -> Result<Vec<String>> {
    let saves = store.game_dir(id)?.join("saves");
    if !saves.exists() {
        return Ok(vec![]);
    }
    check_directory(&saves)?;
    let mut worlds = fs::read_dir(&saves)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            check_directory(&entry.path()).ok()?;
            regular_file(&entry.path().join("level.dat")).ok()?;
            entry.file_name().into_string().ok()
        })
        .collect::<Vec<_>>();
    worlds.sort_by_key(|world| world.to_lowercase());
    Ok(worlds)
}

pub fn world_dir(store: &Store, id: Uuid, world: &str) -> Result<PathBuf> {
    check_filename(world)?;
    let saves = store.game_dir(id)?.join("saves");
    check_directory(&saves)?;
    let world = saves.join(world);
    check_directory(&world)?;
    regular_file(&world.join("level.dat"))?;
    Ok(world)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut data = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= limit, "This image is too large.");
    Ok(data)
}

pub(crate) fn skin_height(bytes: &[u8]) -> Result<u32> {
    let image = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .context("Choose a valid PNG skin.")?;
    let (width, height) = (image.info().width, image.info().height);
    ensure!(
        width == 64 && matches!(height, 32 | 64),
        "Minecraft skins must be 64×64 or 64×32 PNG images."
    );
    Ok(height)
}
