use std::{fs, ops::Deref, path::Path, str::FromStr};

use serde::{Deserialize, Deserializer, de};

#[derive(Debug)]
pub struct Directory {
    pub path: Box<Path>,
    pub tag: Box<str>,
}

impl Deref for Directory {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl<'de> Deserialize<'de> for Directory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            path: Box<Path>,
            tag: Box<str>,
        }

        let Raw { path, tag } = Raw::deserialize(deserializer)?;
        if !path.is_absolute() {
            return Err(de::Error::custom(format!(
                "path `{}` is not an absolute path",
                path.display(),
            )));
        }

        let meta = fs::metadata(&path).map_err(de::Error::custom)?;
        if !meta.is_dir() {
            return Err(de::Error::custom(format!(
                "path `{}` is not a directory",
                path.display(),
            )));
        }

        Ok(Directory { path, tag })
    }
}

fn deserialize_abs_paths<'de, D>(deserializer: D) -> Result<Box<[Box<Path>]>, D::Error>
where
    D: Deserializer<'de>,
{
    let paths: Box<[Box<Path>]> = Box::deserialize(deserializer)?;

    for path in paths.iter() {
        if !path.is_absolute() {
            return Err(de::Error::custom(format!(
                "path `{}` is not an absolute path",
                path.display()
            )));
        }
    }

    Ok(paths)
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(deserialize_with = "deserialize_abs_paths")]
    pub exclude: Box<[Box<Path>]>,
    pub directory: Box<[Directory]>,
}

impl FromStr for Config {
    type Err = toml::de::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s)
    }
}
