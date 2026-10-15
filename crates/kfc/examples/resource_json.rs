//! Private diagnostic export of one resource. Do not redistribute game data.
use kfc::{container::KFCReader, guid::ResourceId, reflection::{LookupKey, TypeRegistry}, resource::value::{ConversionOptions, Value}};
use std::{fs::File, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 { return Err("usage: resource_json GAME_COPY STEM GUID TYPE NEW_OUTPUT_JSON".into()); }
    let path = PathBuf::from(&args[1]);
    let registry = TypeRegistry::load_from_executable(path.join(format!("{}.exe", args[2])))?;
    let ty = registry.get_by_name(LookupKey::Qualified(&args[4])).ok_or("unknown resource type")?;
    let id = ResourceId::parse(&args[3], ty.qualified_hash, 0).ok_or("invalid GUID")?;
    let mut reader = KFCReader::new(&path, &args[2])?.into_cursor()?;
    let bytes = reader.read_resource(&id)?.ok_or("resource not found")?;
    let value = Value::from_bytes_with_options(&registry, ty, &bytes, ConversionOptions::HUMAN_READABLE)?;
    let output = File::options().write(true).create_new(true).open(&args[5])?;
    serde_json::to_writer_pretty(output, &value)?;
    println!("Exported {id}, {} bytes, directory version {}", bytes.len(), reader.file().game_version());
    Ok(())
}
