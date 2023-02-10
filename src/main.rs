use std::collections::HashMap;
use std::path::Path;
use std::{fs, io, iter};

use anyhow::{bail, Context, Result};
use collection_literals::collection;
use serde_json::{self as json, Value as JsonValue};
use serde_yaml::{self as yaml, from_value, Mapping, Value as YamlValue};
use yaml::value::TaggedValue;

pub struct KeyPath<'a>(&'a str);

impl KeyPath<'_> {
    /// Checks if the file component contains a `.`
    pub fn is_file(&self) -> bool {
        self.0
            .rsplit_once('/')
            .map(|(_, t)| t)
            .unwrap_or(&self.0)
            .contains('.')
    }
}

pub enum DiskData {
    Folder(HashMap<String, DiskData>),
    File(Vec<u8>),
}

fn parse_tag(tagged: TaggedValue) -> Result<Vec<Result<JsonValue>>> {
    Ok(match tagged.tag.to_string().as_str() {
        "!namespaced" => {
            let map: HashMap<String, Vec<String>> = yaml::from_value(tagged.value)
                .context("namespaced only supports {string: string} maps")?;
            map.into_iter()
                .flat_map(|(namespace, entries)| {
                    entries
                        .into_iter()
                        .map(move |entry| format!("{namespace}:{entry}"))
                })
                .map(JsonValue::String)
                .map(Ok)
                .collect()
        }
        tag => bail!("unexpected tag `{tag}`"),
    })
}

fn yaml_to_json_flattable(yaml: YamlValue) -> std::vec::IntoIter<Result<JsonValue>> {
    match yaml {
        YamlValue::Tagged(tagged) => parse_tag(*tagged).unwrap_or_else(|e| vec![Err(e)]),
        YamlValue::Mapping(map)
            if map.len() == 1 && matches!(map.keys().next().unwrap(), YamlValue::Tagged(_)) =>
        {
            let (YamlValue::Tagged(tagged), value) = map.into_iter().next().unwrap() else {
                unreachable!()
            };
            return yaml_to_json_flattable(YamlValue::Tagged(Box::new(TaggedValue {
                tag: tagged.tag,
                value: YamlValue::Mapping(iter::once((tagged.value, value)).collect()),
            })));
        }
        _ => vec![yaml_to_json(yaml)],
    }
    .into_iter()
}

fn yaml_to_json(yaml: YamlValue) -> Result<JsonValue> {
    Ok(match yaml {
        YamlValue::Sequence(seq) => seq
            .into_iter()
            .flat_map(yaml_to_json_flattable)
            .collect::<Result<_>>()?,
        YamlValue::Mapping(map) => map
            .into_iter()
            .map(|(key, value)| {
                Ok((
                    from_value::<String>(key).context("key should be string")?,
                    yaml_to_json(value)?,
                ))
            })
            .collect::<Result<_>>()?,
        YamlValue::Tagged(tagged) => match tagged.tag.to_string().as_str() {
            tag => bail!("unexpected tag `{tag}`"),
        },
        yaml => json::to_value(yaml)?,
    })
}

impl DiskData {
    fn write_to_disk(&self, location: impl AsRef<Path>) -> io::Result<()> {
        match self {
            DiskData::Folder(data) => {
                fs::create_dir_all(location.as_ref())?;
                for (name, data) in data {
                    data.write_to_disk(location.as_ref().join(name))?
                }
                Ok(())
            }
            DiskData::File(data) => fs::write(location.as_ref(), data),
        }
    }

    fn from_yaml(yaml: YamlValue) -> Result<Self> {
        let mapping: Mapping = from_value(yaml).context("expected mapping")?;
        Ok(Self::Folder(
            mapping
                .into_iter()
                .map(|(key, value)| {
                    let Some(key) = key.as_str() else { todo!() };
                    let tail = key.rsplit_once('/').map(|p| p.1).unwrap_or(key);
                    let data = if tail.contains('.') {
                        Self::File(json::to_vec_pretty(&yaml_to_json(value)?)?)
                    } else {
                        Self::from_yaml(value)?
                    };
                    let (top, subs) = key.split_once('/').unwrap_or((key, ""));
                    let top = top.to_owned();
                    Ok((
                        top,
                        if subs.is_empty() {
                            data
                        } else {
                            subs.rsplit('/').fold(anyhow::Ok(data), |acc, curr| {
                                acc.map(|acc| {
                                    DiskData::Folder(collection! {curr.to_owned() => acc})
                                })
                            })?
                        },
                    ))
                })
                .collect::<Result<_>>()?,
        ))
    }
}

fn main() -> Result<()> {
    Ok(if Path::exists("datapack.yaml".as_ref()) {
        let yaml: YamlValue = yaml::from_str(&fs::read_to_string("datapack.yaml")?)?;
        DiskData::from_yaml(yaml)?.write_to_disk("")?;
    })
}
