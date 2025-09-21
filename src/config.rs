use std::path::PathBuf;

use toml::Value;

#[derive(Default)]
pub struct Configuration {
    pub src_root: PathBuf,
    // TODO source file pattern
}

// TODO implements defaults manually to have custom default values

impl TryFrom<&toml::map::Map<String, toml::Value>> for Configuration {
    type Error = &'static str;
    
    fn try_from(value: &toml::map::Map<String, toml::Value>) -> Result<Self, Self::Error> {
        let default_src: PathBuf = PathBuf::from("../src");
        Ok(Configuration {
            src_root: match value.get("src-root") {
                Some(Value::String(src_root)) => PathBuf::from(src_root),
                None => default_src,
                _ => {
                    log::error!("field `src-root` has invalid data type (expected string)");
                    default_src
                }
            },
        })
    }
}