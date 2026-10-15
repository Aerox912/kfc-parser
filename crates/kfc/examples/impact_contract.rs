//! Compare reflected Impact call signatures and resolve a private program's references.
use kfc::{container::KFCReader, guid::{Guid, ResourceId}, reflection::{LookupKey, TypeRegistry}, resource::value::{ConversionOptions, Value}};
use serde_json::{json, Value as Json};
use std::{fs::File, path::PathBuf};

fn signature(registry: &TypeRegistry, name: &str) -> Result<Json, Box<dyn std::error::Error>> {
    let ty = registry.get_by_name(LookupKey::Qualified(name)).ok_or("missing Impact node")?;
    let fields: Vec<Json> = ty.struct_fields.values().map(|field| {
        let field_type = registry.get(field.r#type).unwrap();
        json!({"name":field.name,"type":field_type.qualified_name,"size":field_type.size,
            "offset":field.data_offset,"attributes":field.attributes})
    }).collect();
    Ok(json!({"size":ty.size,"alignment":ty.alignment,"fields":fields,"attributes":ty.attributes}))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 { return Err("usage: impact_contract GAME_COPY STEM OLD_EXE PRIVATE_PROGRAM_JSON".into()); }
    let path = PathBuf::from(&args[1]);
    let current = TypeRegistry::load_from_executable(path.join(format!("{}.exe",args[2])))?;
    let old = TypeRegistry::load_from_executable(&args[3])?;
    for short in ["GetRoot","FetchAttribute","ApplyChange","AddValues","MulValues","DivValues"] {
        let name = format!("keen::impact_nodes::{short}");
        if signature(&current,&name)? != signature(&old,&name)? { return Err(format!("changed node signature: {name}").into()); }
    }
    let program: Json = serde_json::from_reader(File::open(&args[4])?)?;
    let data: Vec<u8> = serde_json::from_value(program["data"].clone())?;
    let mut reader = KFCReader::new(&path,&args[2])?.into_cursor()?;
    let attribute_type = current.get_by_name(LookupKey::Qualified("keen::BaseAttributeResource")).ok_or("missing attribute registry type")?;
    let attribute_ids: Vec<ResourceId> = reader.file().resources().keys().iter()
        .filter(|id| id.type_hash()==attribute_type.qualified_hash).copied().collect();
    let mut attribute_names = std::collections::HashMap::new();
    for id in attribute_ids {
        let bytes = reader.read_resource(&id)?.ok_or("missing attribute registry bytes")?;
        let value = Value::from_bytes_with_options(&current,attribute_type,bytes,ConversionOptions::HUMAN_READABLE)?;
        let value = serde_json::to_value(value)?;
        for (key,name) in value["ids"].as_array().ok_or("missing attribute IDs")?.iter()
            .zip(value["debugNames"].as_array().ok_or("missing attribute names")?) {
            attribute_names.insert(key["value"].as_u64().ok_or("invalid attribute ID")? as u32,name.clone());
        }
    }
    let container_type=current.get_by_name(LookupKey::Qualified("keen::AttributeContainerResource")).ok_or("missing attribute container type")?;
    let containers:Vec<_>=reader.file().resources().keys().iter().filter(|id|id.type_hash()==container_type.qualified_hash).copied().collect();
    for id in containers {
        let bytes=reader.read_resource(&id)?.ok_or("missing attribute container bytes")?;
        let value=Value::from_bytes_with_options(&current,container_type,bytes,ConversionOptions::HUMAN_READABLE)?;
        let value=serde_json::to_value(value)?;
        for attribute in value["attributes"].as_array().ok_or("missing nested attributes")? {
            let ids=attribute["ids"].as_array().ok_or("missing nested IDs")?;
            let names=attribute["debugNames"].as_array().ok_or("missing nested names")?;
            if ids.len()!=names.len() { return Err("nested attribute metadata length mismatch".into()); }
            for (key,name) in ids.iter().zip(names) {
                attribute_names.insert(key["value"].as_u64().ok_or("invalid nested attribute ID")? as u32,name.clone());
            }
        }
    }
    let mut references = 0;
    let mut unresolved = 0;
    for layout in program["dataLayout"].as_array().ok_or("missing layout")? {
        let hash = layout["type"]["value"].as_u64().ok_or("missing type")? as u32;
        let ty = current.get_by_hash(LookupKey::Impact(hash)).ok_or("unknown layout type")?;
        let old_type = old.get_by_hash(LookupKey::Impact(hash)).ok_or("unknown old layout type")?;
        let size = layout["size"].as_u64().ok_or("missing size")?;
        if u64::from(ty.size) != size || ty.size != old_type.size || ty.primitive_type != old_type.primitive_type {
            return Err(format!("changed layout type: {}", layout["dbgName"]).into());
        }
        let mut resolved = ty;
        while resolved.primitive_type == kfc::reflection::PrimitiveType::Typedef {
            resolved = current.get(resolved.inner_type.ok_or("typedef target missing")?).ok_or("unknown typedef target")?;
        }
        if resolved.qualified_name.starts_with("keen::ObjectReference<") {
            let offset = layout["offsetInBytes"].as_u64().ok_or("missing offset")? as usize;
            let guid = Guid::new(data.get(offset..offset+16).ok_or("reference bounds")?.try_into()?);
            if guid.is_none() { continue; }
            let target_base = current.get(resolved.inner_type.ok_or("reference target type missing")?).ok_or("reference target missing")?;
            let candidates: Vec<ResourceId> = reader.file().resources().keys().iter().filter(|id| id.guid()==guid && id.part_index()==0).copied().collect();
            if candidates.is_empty() && target_base.qualified_name == "keen::impact::AttributeDeclerationBase" {
                let Some(name) = attribute_names.get(&guid.hash32()) else {
                    eprintln!("UNRESOLVED {} => {guid} hash {}",layout["dbgName"],guid.hash32());
                    unresolved += 1;
                    continue;
                };
                println!("{} => {} registry ID {} {}",layout["dbgName"],guid,guid.hash32(),name);
                references += 1;
                continue;
            }
            if candidates.len() != 1 { return Err(format!("expected one attribute resource: {guid}, found {}",candidates.len()).into()); }
            let id = candidates[0];
            let target = current.get_by_hash(LookupKey::Qualified(id.type_hash())).ok_or("unknown target type")?;
            let compiled_attribute = target_base.qualified_name == "keen::impact::AttributeDeclerationBase"
                && target.qualified_name == "keen::BaseAttributeResource";
            if !compiled_attribute && target.index != target_base.index && !current.is_sub_type(target_base,target) {
                return Err(format!("reference {} expects {}, resolved {}",layout["dbgName"],target_base.qualified_name,target.qualified_name).into());
            }
            let bytes = reader.read_resource(&id)?.ok_or_else(|| format!("missing reference {guid}"))?;
            let value = Value::from_bytes_with_options(&current,target,bytes,ConversionOptions::HUMAN_READABLE)?;
            let value = serde_json::to_value(value)?;
            println!("{} => {} {}",layout["dbgName"],guid,if compiled_attribute { &value["debugNames"] } else { &value["debugName"] });
            references += 1;
        }
    }
    if references == 0 { return Err("no attribute references validated".into()); }
    if unresolved != 0 {
        for (id,name) in &attribute_names {
            let text = name.as_str().unwrap_or("");
            if ["Mana","Crit","Healing","Melee","Magic","Ranged","Unarmed","Cast"].iter().any(|s| text.contains(s)) { println!("CANDIDATE {id} {name}"); }
        }
        return Err(format!("{unresolved} unresolved attribute references").into());
    }
    println!("PASS: 6 node signatures, {} layout types, {references} resolved references", program["dataLayout"].as_array().unwrap().len());
    Ok(())
}
