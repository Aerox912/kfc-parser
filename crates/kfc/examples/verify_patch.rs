//! Read back every changed resource and new content blob from an offline patch.
//! Output includes game data: keep the JSON private; do not redistribute it.
use kfc::{
    container::{KFCFile, KFCReader},
    guid::ContentHash,
    reflection::{LookupKey, TypeRegistry},
    resource::value::{ConversionOptions, Value},
};
use serde_json::json;
use std::{fs::File, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: verify_patch GAME_COPY BASELINE_KFC TYPES_JSON NEW_OUTPUT_JSON".into());
    }
    let path = PathBuf::from(&args[1]);
    let baseline = KFCFile::from_path(&args[2], false)?;
    let current = KFCFile::from_path(path.join("enshrouded.kfc"), false)?;
    let registry: TypeRegistry = serde_json::from_reader(File::open(&args[3])?)?;
    if current.game_version() != baseline.game_version() || current.game_version() != registry.version {
        return Err("baseline, current directory and reflection versions must match".into());
    }
    let mut cursor = KFCReader::new(&path, "enshrouded")?.into_cursor()?;
    let mut resources = Vec::new();
    let mut contents = Vec::new();
    let mut bytes = Vec::new();
    for (id, entry) in current.resources().iter() {
        if baseline.resources().get(id).is_some_and(|old| old.offset == entry.offset && old.size == entry.size) {
            continue;
        }
        bytes.clear();
        if !cursor.read_resource_into(id, &mut bytes).map_err(|e| format!("reading {id} offset={} size={}: {e}", entry.offset, entry.size))? {
            return Err(format!("missing resource bytes: {id}").into());
        }
        let ty = registry.get_by_hash(LookupKey::Qualified(id.type_hash())).ok_or("unknown resource type")?;
        let value = Value::from_bytes_with_options(&registry, ty, &bytes, ConversionOptions::HUMAN_READABLE)
            .map_err(|e| format!("decoding {id} {} offset={} size={}: {e}", ty.qualified_name, entry.offset, bytes.len()))?;
        resources.push(json!({"guid": id.guid().to_string(), "type": ty.qualified_name,
            "part": id.part_index(), "bytes": bytes.len(), "value": value}));
    }
    for hash in current.contents().keys() {
        if baseline.contents().contains_key(hash) { continue; }
        bytes.clear();
        if !cursor.read_content_into(hash, &mut bytes)? { return Err(format!("missing content bytes: {hash}").into()); }
        if ContentHash::from_data(&bytes) != *hash {
            return Err(format!("content hash mismatch: {hash}").into());
        }
        contents.push(json!({"hash": hash, "bytes": bytes.len()}));
    }
    let report = json!({"game_version": current.game_version(), "resources": resources, "contents": contents});
    let output = File::options().write(true).create_new(true).open(&args[4])?;
    serde_json::to_writer_pretty(output, &report)?;
    println!("PASS: decoded {} changed resources and verified {} new content hashes", resources.len(), contents.len());
    Ok(())
}
