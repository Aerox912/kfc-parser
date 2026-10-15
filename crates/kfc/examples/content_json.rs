//! Export a typed content blob for private readback validation.
use kfc::{container::KFCReader, guid::ContentHash, reflection::{LookupKey, TypeRegistry}, resource::value::{ConversionOptions, Value}};
use std::{fs::File,path::PathBuf};
fn main() -> Result<(),Box<dyn std::error::Error>> {
    let args:Vec<String>=std::env::args().collect();
    if args.len()!=6 { return Err("usage: content_json GAME_COPY TYPES_JSON CONTENT_HASH TYPE NEW_OUTPUT_JSON".into()); }
    let path=PathBuf::from(&args[1]);
    let registry:TypeRegistry=serde_json::from_reader(File::open(&args[2])?)?;
    let ty=registry.get_by_name(LookupKey::Qualified(&args[4])).ok_or("unknown type")?;
    let hash=ContentHash::parse(&args[3]).ok_or("invalid content hash")?;
    let mut reader=KFCReader::new(&path,"enshrouded")?.into_cursor()?;
    if reader.file().game_version()!=registry.version { return Err("reflection version mismatch".into()); }
    let bytes=reader.read_content(&hash)?.ok_or("missing content")?;
    if ContentHash::from_data(&bytes)!=hash { return Err("content hash mismatch".into()); }
    let value=Value::from_bytes_with_options(&registry,ty,&bytes,ConversionOptions::HUMAN_READABLE)?;
    serde_json::to_writer(File::options().write(true).create_new(true).open(&args[5])?,&value)?;
    println!("PASS: decoded content {hash}");
    Ok(())
}
