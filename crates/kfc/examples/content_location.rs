//! Locate a content hash without reading or copying the game's content archives.
use kfc::{container::KFCFile, guid::ContentHash};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 { return Err("usage: content_location KFC SIZE HASH0 HASH1 HASH2".into()); }
    let hash = ContentHash::new(args[2].parse()?, args[3].parse()?, args[4].parse()?, args[5].parse()?);
    let file = KFCFile::from_path(&args[1], false)?;
    let entry = file.contents().get(&hash).ok_or("content hash not found")?;
    println!("container={} offset={} size={}", entry.container_index, entry.offset, hash.size());
    Ok(())
}
