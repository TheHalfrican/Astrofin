use std::fmt;
use std::io::{self, ErrorKind};
use std::ops::Deref;
use std::path::Path;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
pub struct InstanceId(Uuid);

impl InstanceId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Deref for InstanceId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.simple())
    }
}

impl Serialize for InstanceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0.simple())
    }
}

impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Uuid::try_parse(&raw).map(Self).map_err(de::Error::custom)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Instance {
    id: InstanceId,
}

const FILE_NAME: &str = "instance.json";

enum Read {
    Parsed(Instance),
    Missing,
    Invalid(serde_json::Error),
}

impl Instance {
    pub fn for_config_dir(config_dir: &Path) -> io::Result<Self> {
        let path = config_dir.join(FILE_NAME);
        let instance = match Self::read(&path)? {
            Read::Parsed(instance) => return Ok(instance),
            Read::Missing => Self::mint(),
            Read::Invalid(e) => {
                tracing::warn!("overwriting invalid instance file {}: {e}", path.display());
                Self::mint()
            }
        };
        instance.save(&path)?;
        Ok(instance)
    }

    #[must_use]
    pub fn id(&self) -> InstanceId {
        self.id
    }

    fn mint() -> Self {
        Self {
            id: InstanceId::new(),
        }
    }

    fn read(path: &Path) -> io::Result<Read> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Read::Missing),
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!("read {}: {e}", path.display()),
                ));
            }
        };
        Ok(match serde_json::from_slice(&bytes) {
            Ok(instance) => Read::Parsed(instance),
            Err(e) => Read::Invalid(e),
        })
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
        jfn_paths::write_atomic(path, &bytes)
            .map_err(|e| io::Error::new(e.kind(), format!("write {}: {e}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn a_first_run_mints_an_instance_and_persists_it() {
        let dir = temp_dir();
        let minted = Instance::for_config_dir(dir.path()).expect("mint");
        let file = dir.path().join(FILE_NAME);
        assert!(file.is_file(), "the minted instance must be written out");

        let reloaded = Instance::for_config_dir(dir.path()).expect("reload");
        assert_eq!(minted.id().to_string(), reloaded.id().to_string());
    }

    #[test]
    fn an_unparsable_instance_file_is_replaced_with_a_fresh_one() {
        let dir = temp_dir();
        let file = dir.path().join(FILE_NAME);
        std::fs::write(&file, b"{ not json").expect("seed");

        let minted = Instance::for_config_dir(dir.path()).expect("mint over garbage");
        let reloaded = Instance::for_config_dir(dir.path()).expect("reload");
        assert_eq!(minted.id().to_string(), reloaded.id().to_string());
    }

    #[test]
    fn an_instance_file_holding_a_bad_uuid_is_replaced_too() {
        let dir = temp_dir();
        let file = dir.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"id":"not-a-uuid"}"#).expect("seed");

        let minted = Instance::for_config_dir(dir.path()).expect("mint over a bad id");
        let raw = std::fs::read_to_string(&file).expect("read back");
        assert!(raw.contains(&minted.id().to_string()));
    }

    #[test]
    fn a_missing_config_dir_surfaces_the_write_error() {
        let dir = temp_dir();
        let missing = dir.path().join("no").join("such").join("dir");
        assert!(Instance::for_config_dir(&missing).is_err());
    }

    #[test]
    fn an_id_displays_and_serializes_as_the_simple_uuid_form() {
        let dir = temp_dir();
        let instance = Instance::for_config_dir(dir.path()).expect("mint");
        let shown = instance.id().to_string();
        assert_eq!(shown.len(), 32, "{shown} must be the unhyphenated form");
        assert!(shown.chars().all(|c| c.is_ascii_hexdigit()));
        // Deref reaches the wrapped uuid itself.
        assert_eq!(instance.id().simple().to_string(), shown);

        let json = serde_json::to_string(&instance).expect("serialize");
        assert_eq!(json, format!(r#"{{"id":"{shown}"}}"#));
    }

    #[test]
    fn a_stored_instance_round_trips_through_json() {
        let dir = temp_dir();
        let instance = Instance::for_config_dir(dir.path()).expect("mint");
        let json = serde_json::to_string(&instance).expect("serialize");
        let back: Instance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id().to_string(), instance.id().to_string());
    }

    #[test]
    fn the_hyphenated_uuid_form_is_accepted_and_a_malformed_one_is_not() {
        let hyphenated: Instance =
            serde_json::from_str(r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8"}"#)
                .expect("hyphenated uuid");
        assert_eq!(
            hyphenated.id().to_string(),
            "67e5504410b1426f9247bb680e5fe0c8"
        );

        for bad in [
            r#"{"id":""}"#,
            r#"{"id":"nope"}"#,
            r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c"}"#,
            r#"{"id":42}"#,
        ] {
            assert!(
                serde_json::from_str::<Instance>(bad).is_err(),
                "{bad} must not parse"
            );
        }
    }
}
