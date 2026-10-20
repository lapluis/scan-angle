//! GUI language preference only. Batch/CLI runs never read or write it.
use crate::i18n::LocalizedMessage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("scan-angle/language.toml"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .map(|p| p.join("scan-angle/language.toml"))
    }
}

#[derive(Serialize, Deserialize)]
struct Preference {
    locale: String,
}

pub fn read(path: &Path) -> Result<Option<String>, LocalizedMessage> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LocalizedMessage::new("preferences.error.read")
                .with_param("path", path.display())
                .with_param("error", e));
        }
    };
    toml::from_str::<Preference>(&text)
        .map(|p| Some(p.locale))
        .map_err(|e| {
            LocalizedMessage::new("preferences.error.read")
                .with_param("path", path.display())
                .with_param("error", e)
        })
}

pub fn write(path: &Path, locale: &str) -> Result<(), LocalizedMessage> {
    let save = || -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            toml::to_string(&Preference {
                locale: locale.to_owned(),
            })?,
        )?;
        Ok(())
    };
    save().map_err(|e| {
        LocalizedMessage::new("preferences.error.write")
            .with_param("path", path.display())
            .with_param("error", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_roundtrip_and_corrupt_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings/language.toml");
        assert_eq!(read(&p).unwrap(), None);
        write(&p, "en-US").unwrap();
        assert_eq!(read(&p).unwrap().as_deref(), Some("en-US"));
        std::fs::write(&p, "broken = [").unwrap();
        assert!(read(&p).is_err());
    }
}
