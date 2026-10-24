//! Structural JSON views preserve the original artifact.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_VIEW_BYTES: usize = 1_048_576;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawArtifactView")]
pub struct ArtifactView {
    pub fields: BTreeMap<String, Value>,
    pub max_bytes: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawArtifactView {
    fields: BTreeMap<String, Value>,
    max_bytes: usize,
}
impl TryFrom<RawArtifactView> for ArtifactView {
    type Error = anyhow::Error;
    fn try_from(raw: RawArtifactView) -> Result<Self, Self::Error> {
        let view = Self {
            fields: raw.fields,
            max_bytes: raw.max_bytes,
        };
        view.validate()?;
        Ok(view)
    }
}
impl ArtifactView {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            (1..=MAX_VIEW_BYTES).contains(&self.max_bytes),
            "view.maxBytes must be 1 through {MAX_VIEW_BYTES}"
        );
        fn mask(value: &Value) -> bool {
            match value {
                Value::Bool(true) => true,
                Value::Object(fields) => !fields.is_empty() && fields.values().all(mask),
                _ => false,
            }
        }
        anyhow::ensure!(
            !self.fields.is_empty() && self.fields.values().all(mask),
            "view.fields must be a nonempty recursive object mask with only true leaves"
        );
        Ok(())
    }
    pub(crate) fn apply(&self, value: &Value) -> anyhow::Result<Value> {
        self.validate()?;
        fn project(
            value: &Value,
            fields: &serde_json::Map<String, Value>,
        ) -> anyhow::Result<Value> {
            match value {
                Value::Array(items) => Ok(Value::Array(
                    items
                        .iter()
                        .map(|v| project(v, fields))
                        .collect::<anyhow::Result<_>>()?,
                )),
                Value::Object(object) => {
                    let mut out = serde_json::Map::new();
                    for (key, mask) in fields {
                        if let Some(value) = object.get(key) {
                            out.insert(
                                key.clone(),
                                if mask == &Value::Bool(true) {
                                    value.clone()
                                } else {
                                    project(value, mask.as_object().expect("validated mask"))?
                                },
                            );
                        }
                    }
                    Ok(Value::Object(out))
                }
                _ => anyhow::bail!("view object mask encountered a non-object/non-array value"),
            }
        }
        let fields = self
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let projected = project(value, &fields)?;
        let bytes = crate::json_size::compact_json_bytes(&projected)?;
        anyhow::ensure!(
            bytes <= self.max_bytes,
            "artifact view is {bytes} UTF-8 bytes, exceeding view.maxBytes {}",
            self.max_bytes
        );
        Ok(projected)
    }
}
