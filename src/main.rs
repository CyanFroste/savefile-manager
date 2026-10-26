use anyhow::{Result, anyhow};
use chrono::{DateTime, Local};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::{env, fs, io};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const TIMESTAMP_FORMAT: &str = "%Y-%m-%d_%H-%M";

fn main() -> Result<()> {
    let args = Cli::parse();
    let config = Config::find("config.json", 5).and_then(Config::new)?;

    match &args.command {
        Command::List => {
            println!("Supported games:");

            for (index, it) in [ProjectZomboid::ID].iter().enumerate() {
                let aliases = config
                    .games
                    .get(*it)
                    .map(|x| x.aliases.clone())
                    .unwrap_or_default();

                println!("[{}] {it} (aliases: {aliases:?})", index + 1);
            }

            Ok(())
        }

        Command::Backup {
            id: id_or_alias,
            name: output_name,
        } => {
            let games = [ProjectZomboid];

            let game = games
                .iter()
                .find(|x| x.id() == id_or_alias)
                .or_else(|| {
                    for (id, c) in config.games.iter() {
                        if c.aliases.contains(&id_or_alias) {
                            return games.iter().find(|x| x.id() == id);
                        }
                    }
                    None
                })
                .ok_or_else(|| anyhow!("Game with id or alias \"{id_or_alias}\" not found"))?;

            let files = game.list_relevant_files(config.games.get(game.id()))?;

            println!("Relevant files for \"{}\":", game.id());

            for (index, it) in files.iter().enumerate() {
                println!(
                    "[{}] [{:?}] {} (path: {})",
                    index + 1,
                    it.r#type,
                    it.name,
                    it.original_path.display(),
                );
            }

            println!("\nSelect files to backup (white space separated, type \"*\" for all):");

            io::stdout().flush()?;
            let mut input = String::new();

            io::stdin().read_line(&mut input)?;
            let choice = input.trim();

            if choice.is_empty() {
                println!("No files selected");
                return Ok(());
            }

            let mut selected = Vec::new();

            if choice == "*" {
                selected = files;
            } else {
                for c in choice.split_whitespace() {
                    let index = c.parse::<usize>()? - 1;
                    selected.push(files[index].clone());
                }
            }

            game.backup(&selected, &config.output_path, output_name.as_deref())
        }
    }
}

// start game

pub struct ProjectZomboid;

impl ProjectZomboid {
    pub const ID: &str = "Project Zomboid";
}

impl Game for ProjectZomboid {
    fn id(&self) -> &str {
        Self::ID
    }

    fn list_relevant_files(&self, config: Option<&ConfigGameEntry>) -> Result<Vec<BackupFile>> {
        let mut files = Vec::new();

        let data_path = config
            .and_then(|x| x.data_path.clone())
            .or_else(|| {
                let default_data_path: PathBuf =
                    shellexpand::tilde("~/Zomboid").into_owned().into();
                Some(default_data_path)
            })
            .ok_or_else(|| anyhow!("Data path for {} not found", Self::ID))?;

        for it in ["Saves/Sandbox", "Saves/Multiplayer"] {
            for entry in fs::read_dir(data_path.join(it))? {
                if let Ok(entry) = entry {
                    let mut created = None;
                    let mut modified = None;

                    if let Ok(metadata) = entry.metadata() {
                        created = metadata.created().ok().map(get_timestamp);
                        modified = metadata.modified().ok().map(get_timestamp);
                    }

                    files.push(BackupFile {
                        name: entry.file_name().to_string_lossy().to_string(),
                        original_path: entry.path().to_string_lossy().replace("\\", "/").into(),
                        r#type: BackupFileType::Save,
                        created,
                        modified,
                    });
                }
            }
        }

        let file_name = "Sandbox Presets";
        let file_path: PathBuf = data_path
            .join(file_name)
            .to_string_lossy()
            .replace("\\", "/")
            .into();
        let mut created = None;
        let mut modified = None;

        if let Ok(metadata) = fs::metadata(&file_path) {
            created = metadata.created().ok().map(get_timestamp);
            modified = metadata.modified().ok().map(get_timestamp);
        }

        if file_path.exists() {
            files.push(BackupFile {
                name: file_name.to_string(),
                original_path: file_path,
                r#type: BackupFileType::Config,
                created,
                modified,
            });
        }

        Ok(files)
    }
}

// end game

pub trait Game {
    fn id(&self) -> &str;

    fn list_relevant_files(&self, config: Option<&ConfigGameEntry>) -> Result<Vec<BackupFile>>;

    fn backup(
        &self,
        files: &[BackupFile],
        output_path: &Path,
        output_name: Option<&str>,
    ) -> Result<()> {
        let backup_path = output_path.join(self.id());
        fs::create_dir_all(&backup_path)?;

        let file_path = if let Some(name) = output_name {
            backup_path.join(name)
        } else {
            backup_path.join(format!(
                "backup_{}.zip",
                Local::now().format(TIMESTAMP_FORMAT)
            ))
        };

        println!("\nCreating backup {}", file_path.display());
        let file = File::create(file_path)?;
        let mut zip = ZipWriter::new(file);

        fn on_progress(current: usize, total: usize) {
            if total == 0 {
                return;
            }

            // this should be safe?
            let percent = (current as f64 / total as f64) * 100.0;
            // \r returns cursor to line start
            print!("\r- Compressing: {percent:.2}% [{current}/{total}]");
            _ = io::stdout().flush();
        }

        for it in files {
            println!("- Adding {} to backup", it.name);
            zip.add_all_files_from_dir(&it.original_path, None, Some(on_progress))?;
            println!("\n- Added {} to backup", it.name);
        }

        zip.add_text_file(
            "metadata.json",
            &serde_json::to_string_pretty(&files)?,
            None,
        )?;
        println!("- Added metadata to backup");

        zip.finish()?;
        Ok(())
    }
}

// ASSUMPTIONS:
// - everything is a directory (so zip it)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub name: String,
    pub original_path: PathBuf,
    pub r#type: BackupFileType,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum BackupFileType {
    Save,
    Config,
    Mod,
    Other,
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "A simple command line utility for managing game data"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List supported games
    List,
    /// Initiate a backup for a supported game
    Backup {
        /// Id (name or alias) of the game
        id: String,
        /// Output name (default: backup_<timestamp>.zip)
        #[arg(short, long)]
        name: Option<String>,
    },
}

fn get_timestamp(source: impl Into<DateTime<Local>>) -> String {
    let local_datetime: DateTime<Local> = source.into();
    local_datetime.format(TIMESTAMP_FORMAT).to_string()
}

impl<W: Write + Seek> ZipExt for ZipWriter<W> {
    fn add_all_files_from_dir(
        &mut self,
        dir_path: &Path,
        compression_method: Option<CompressionMethod>,
        on_progress: Option<impl Fn(usize, usize)>,
    ) -> Result<()> {
        let dir_name = dir_path
            .file_name()
            .ok_or_else(|| anyhow!("{} is a non utf-8 path", dir_path.display()))?;

        let zip_options = self.get_options(compression_method);
        let entries: Vec<_> = WalkDir::new(dir_path).into_iter().collect();

        let mut current_file = 1;
        let total_files = entries
            .iter()
            .filter_map(|x| x.as_ref().ok())
            .filter(|x| x.file_type().is_file())
            .count();

        // SECURITY NOTE: Any error after this point may leave a partial or corrupt zip file.
        // This can lead to data integrity issues or race conditions (e.g., TOCTOU) if other processes access the incomplete file.
        // A robust application should mitigate this, for example by writing to a temporary file and renaming it on success to ensure atomicity.

        for entry in entries.into_iter() {
            let entry = entry?;
            let path = entry.path();
            let normalized_path = PathBuf::from(dir_name).join(path.strip_prefix(dir_path)?);
            let path_str = normalized_path
                .to_str()
                .ok_or_else(|| anyhow!("{} is a non utf-8 path", normalized_path.display()))?
                .to_string();

            if path.is_file() {
                self.start_file(path_str, zip_options)?;
                io::copy(&mut File::open(path)?, self)?;

                on_progress
                    .as_ref()
                    .inspect(|cb| cb(current_file, total_files));
                current_file += 1;
            } else if !path.as_os_str().is_empty() {
                self.add_directory(path_str, zip_options)?;
            }
        }

        Ok(())
    }

    fn add_text_file(
        &mut self,
        file_name: &str,
        content: &str,
        compression_method: Option<CompressionMethod>,
    ) -> Result<()> {
        let zip_options = self.get_options(compression_method);

        self.start_file(file_name, zip_options)?;
        self.write_all(content.as_bytes())?;

        Ok(())
    }
}

pub trait ZipExt {
    fn add_all_files_from_dir(
        &mut self,
        path: &Path,
        compression_method: Option<CompressionMethod>,
        on_progress: Option<impl Fn(usize, usize)>,
    ) -> Result<()>;

    fn add_text_file(
        &mut self,
        file_name: &str,
        content: &str,
        compression_method: Option<CompressionMethod>,
    ) -> Result<()>;

    fn get_options(&self, compression_method: Option<CompressionMethod>) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(compression_method.unwrap_or(CompressionMethod::Stored))
            .unix_permissions(0o755)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub output_path: PathBuf,
    pub games: BTreeMap<String, ConfigGameEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigGameEntry {
    pub data_path: Option<PathBuf>,
    pub installation_path: Option<PathBuf>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl Config {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        Ok(serde_json::from_reader(&File::open(path)?)?)
    }

    pub fn find(name: impl AsRef<str>, levels: usize) -> Result<PathBuf> {
        let name = name.as_ref();
        let path = env::current_exe()?;

        for ancestor in path.ancestors().skip(1).take(levels) {
            let path = ancestor.join(name);

            if path.exists() {
                return Ok(path);
            }
        }

        Err(anyhow!("config: {name} not found"))
    }
}
