//! Bounded structural selection over original objects. No browser semantics.
use super::*;

const MAX_RECORDS: usize = 10_000;
const INVALID: &str = "local records: invalid-arguments";

fn arguments<'a>(tool: &str, args: &'a Value) -> Result<(&'a Vec<Value>, crate::ArtifactSchema)> {
    crate::schema::check_limits(args, SELECT_ARGUMENT_BYTES, 64, "record arguments")?;
    let object = args.as_object().ok_or_else(|| anyhow!(INVALID))?;
    let ordered = tool == ORDER_TOOL;
    let names: &[&str] = if ordered {
        &[
            "items",
            "schema",
            "field",
            "prefix",
            "fractionDigits",
            "groupSize",
        ]
    } else {
        &["items", "schema"]
    };
    if object
        .keys()
        .any(|key| !(names.contains(&key.as_str()) || ordered && key == "keyExtraction"))
        || names.iter().any(|key| !object.contains_key(*key))
    {
        bail!(INVALID);
    }
    let items = args["items"].as_array().ok_or_else(|| anyhow!(INVALID))?;
    if items.len() > MAX_RECORDS || !items.iter().all(Value::is_object) {
        bail!(INVALID);
    }
    crate::schema::check_limits(&args["items"], SELECT_ITEMS_BYTES, 64, "record items")?;
    crate::schema::check_limits(&args["schema"], 16 * 1024, 16, "record predicate")?;
    let schema = crate::ArtifactSchema::compile(&args["schema"])?;
    if ordered {
        let field = args["field"]
            .as_str()
            .filter(|s| s.len() <= 4096)
            .ok_or_else(|| anyhow!(INVALID))?;
        if !field.is_empty() && !field.starts_with('/') {
            bail!(INVALID);
        }
        let mut bytes = field.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
                bail!(INVALID);
            }
        }
        if let Some(extraction) = args.get("keyExtraction") {
            let object = extraction.as_object().ok_or_else(|| anyhow!(INVALID))?;
            if object.len() != 2 || extraction["repeated"] != "identical" {
                bail!(INVALID);
            }
            let stops = extraction["stopBefore"]
                .as_array()
                .ok_or_else(|| anyhow!(INVALID))?;
            if stops.len() > 16
                || stops
                    .iter()
                    .any(|value| value.as_str().is_none_or(|s| s.is_empty() || s.len() > 256))
                || args["prefix"].as_str().is_none_or(str::is_empty)
            {
                bail!(INVALID);
            }
        }
        if args["prefix"].as_str().is_none_or(|s| s.len() > 256)
            || args["fractionDigits"].as_u64().is_none_or(|n| n > 6)
            || args["groupSize"]
                .as_u64()
                .is_none_or(|n| !(1..=32).contains(&n))
        {
            bail!(INVALID);
        }
    }
    Ok((items, schema))
}

pub(super) fn validate(tool: &str, args: &Value) -> Result<()> {
    arguments(tool, args).map(|_| ())
}

pub(super) fn invoke(tool: &str, args: &Value) -> Result<Value> {
    let (items, schema) = arguments(tool, args)?;
    let source_sha256: String = Sha256::digest(serde_json::to_vec(&args["items"])?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut matches = Vec::new();
    for (index, record) in items.iter().enumerate() {
        if schema.validate(record, SELECT_ITEMS_BYTES).is_ok() {
            matches.push(json!({"index":index,"selected":record}));
        }
    }
    let matched_count = matches.len();
    if tool == MATCH_TOOL {
        if matches.is_empty() {
            bail!("local.select_matching: no-match");
        }
        if matches.len() != 1 {
            bail!("local.select_matching: ambiguous");
        }
        let mut selected = matches.remove(0);
        selected["sourceSha256"] = json!(source_sha256);
        return Ok(selected);
    }
    if tool == ORDER_TOOL {
        let field = args["field"].as_str().expect("validated");
        let prefix = args["prefix"].as_str().expect("validated");
        let digits = args["fractionDigits"].as_u64().expect("validated") as usize;
        let mut ordered = Vec::with_capacity(matches.len());
        for mut record in matches {
            let original = record["selected"]
                .pointer(field)
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("local.order_records: invalid-decimal"))?;
            let value = if let Some(extraction) = args.get("keyExtraction") {
                let stop = extraction["stopBefore"]
                    .as_array()
                    .expect("validated")
                    .iter()
                    .filter_map(|marker| original.find(marker.as_str().expect("validated")))
                    .min()
                    .unwrap_or(original.len());
                let window = original[..stop].trim();
                let tail = window
                    .strip_prefix(prefix)
                    .ok_or_else(|| anyhow!("local.order_records: invalid-decimal"))?;
                let mut key = None;
                for part in tail.split(prefix) {
                    let value = literal_decimal(part.trim(), digits)?;
                    if key.is_some_and(|known| known != value) {
                        bail!("local.order_records: ambiguous-decimal");
                    }
                    key = Some(value);
                }
                key.ok_or_else(|| anyhow!("local.order_records: invalid-decimal"))?
            } else {
                let text = original
                    .strip_prefix(prefix)
                    .ok_or_else(|| anyhow!("local.order_records: invalid-decimal"))?;
                literal_decimal(text, digits)?
            };
            record["decimal"] = json!(format_decimal(value));
            ordered.push((value, record));
        }
        ordered.sort_by_key(|(value, _)| *value); // Stable source order for ties.
        let records: Vec<_> = ordered.into_iter().map(|(_, record)| record).collect();
        let group_size = args["groupSize"].as_u64().expect("validated") as usize;
        let groups: Vec<_> = records
            .chunks(group_size)
            .enumerate()
            .map(|(index, items)| json!({"index":index,"items":items}))
            .collect();
        return Ok(json!({"groups":groups,"matchedCount":matched_count,
            "excludedCount":items.len()-matched_count,"sourceSha256":source_sha256}));
    }
    Ok(json!({"matches":matches,"matchedCount":matched_count,
        "excludedCount":items.len()-matched_count,"sourceSha256":source_sha256}))
}

fn literal_decimal(text: &str, digits: usize) -> Result<i128> {
    let fractional = text
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    if fractional != digits {
        bail!("local.order_records: invalid-decimal");
    }
    parse_decimal(text).map_err(|_| anyhow!("local.order_records: invalid-decimal"))
}
