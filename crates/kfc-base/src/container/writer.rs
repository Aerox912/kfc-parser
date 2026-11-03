use std::{
    borrow::Borrow,
    fs::File,
    io::{BufWriter, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crate::{
    container::header::ResourceChunkInfo,
    guid::{ContentHash, ResourceId},
    io::{WriteExt, WriteSeekExt},
    reflection::TypeRegistry,
};

use super::{
    KFCFile, KFCReadError, KFCWriteError, StaticMapBuilder,
    header::{ContainerInfo, ContentEntry, ResourceEntry},
};

const RESOURCE_ALIGNMENT: u64 = 16;
const CONTENT_ALIGNMENT: u64 = 4096;
const RESOURCE_CHUNK_ALIGNMENT: u64 = 4096;
const RESOURCE_CHUNK_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

pub struct KFCWriter<F, T> {
    type_registry: T,

    path: PathBuf,
    file_name: String,

    file: File,

    chunk_writer: Cursor<Vec<u8>>,
    resource_writer: BufWriter<File>,
    uncompressed_offset: u64,
    uncompressed_chunk_offset: u64,
    resource_chunks: Vec<ResourceChunkInfo>,

    container_writers: Vec<ContainerWriter>,

    game_version: String,
    resources: StaticMapBuilder<ResourceId, ResourceEntry>,
    contents: StaticMapBuilder<ContentHash, ContentEntry>,

    incremental_data: Option<IncrementalData<F>>,
    // Read-only source for a generated replacement. Its bytes are streamed into
    // the final resource output only after the generated suffix exists.
    generated_prefix: Option<(File, u64)>,
    options: KFCWriteOptions,
}

struct IncrementalData<F> {
    reference_file: F,
}

#[derive(Debug)]
pub struct KFCWriteOptions {
    /// Preserve all bytes in an incremental resource file, including unreferenced
    /// trailing data. Used when generating an output from composed assets.
    pub append_after_resource_file: bool,
    /// The extension to use for kfc files.
    pub kfc_extension: String,
    /// The extension to use for kfc resource files.
    pub resource_extension: String,
    /// The extension to use for dat files.
    pub dat_extension: String,
    /// If true, the writer will overwrite existing data in the .dat files,
    /// instead of causing an error.
    /// **NOTE:** For incremental writes, this will only affect non-default .dat files.
    pub overwrite_containers: bool,
    // TEST: does the game use a i32, u32 or an artifical limit for the dat max size?
    /// The maximum size of a single .dat file before a new one is created.
    pub max_container_size: u32,
    /// Whether to truncate existing .dat files or not.
    /// This is used to remove any leftover data from previous writes.
    /// **NOTE:** For incremental writes, this will only affect non-default .dat files.
    pub truncate_containers: bool,
}

impl Default for KFCWriteOptions {
    fn default() -> Self {
        Self {
            append_after_resource_file: false,
            kfc_extension: "kfc".to_string(),
            resource_extension: "kfc_resources".to_string(),
            dat_extension: "dat".to_string(),
            overwrite_containers: false,
            max_container_size: 1024 * 1024 * 1024, // 1 GiB
            truncate_containers: false,
        }
    }
}

impl<F, T> KFCWriter<F, T>
where
    F: Borrow<KFCFile>,
    T: Borrow<TypeRegistry>,
{
    #[inline]
    pub fn new(
        path: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        type_registry: T,
        game_version: impl AsRef<str>,
    ) -> Result<Self, KFCReadError> {
        Self::new_with_options(
            path,
            file_name,
            type_registry,
            game_version,
            KFCWriteOptions::default(),
        )
    }

    pub fn new_with_options(
        path: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        type_registry: T,
        game_version: impl AsRef<str>,
        options: KFCWriteOptions,
    ) -> Result<Self, KFCReadError> {
        let kfc_path =
            path.as_ref()
                .join(format!("{}.{}", file_name.as_ref(), options.kfc_extension));
        let file = File::options().write(true).read(true).open(&kfc_path)?;

        let resources_path = path.as_ref().join(format!(
            "{}.{}",
            file_name.as_ref(),
            options.resource_extension
        ));
        let resource_file = File::options().write(true).open(&resources_path)?;
        let resource_writer = BufWriter::new(resource_file);

        Ok(Self {
            type_registry,

            path: path.as_ref().to_path_buf(),
            file_name: file_name.as_ref().to_string(),

            file,

            chunk_writer: Cursor::new(Vec::new()),
            resource_writer,
            uncompressed_offset: 0,
            uncompressed_chunk_offset: 0,
            resource_chunks: Vec::new(),

            container_writers: Vec::new(),

            game_version: game_version.as_ref().to_string(),
            resources: StaticMapBuilder::default(),
            contents: StaticMapBuilder::default(),

            incremental_data: None,
            generated_prefix: None,
            options,
        })
    }

    #[inline]
    pub fn new_incremental(
        path: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        reference_file: F,
        type_registry: T,
    ) -> Result<Self, KFCReadError> {
        Self::new_incremental_with_options(
            path,
            file_name,
            reference_file,
            type_registry,
            KFCWriteOptions::default(),
        )
    }

    pub fn new_incremental_with_options(
        path: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        reference_file: F,
        type_registry: T,
        options: KFCWriteOptions,
    ) -> Result<Self, KFCReadError> {
        Self::new_incremental_output(
            path,
            file_name,
            reference_file,
            type_registry,
            options,
            None,
        )
    }

    /// Emit a final edited replacement from a read-only current composition.
    /// No original index or resource beforeimage is materialized in the output.
    /// Changed resource chunks are written beyond the preserved EOF first. At
    /// finalize, the required original prefix is streamed into that same edited
    /// resource output, then the new index is serialized from its updated maps.
    pub fn new_generated_incremental(
        source: impl AsRef<Path>,
        output: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        reference_file: F,
        type_registry: T,
    ) -> Result<Self, KFCReadError> {
        Self::new_incremental_output(
            output,
            file_name,
            reference_file,
            type_registry,
            KFCWriteOptions {
                append_after_resource_file: true,
                ..Default::default()
            },
            Some(source.as_ref()),
        )
    }

    fn new_incremental_output(
        path: impl AsRef<Path>,
        file_name: impl AsRef<str>,
        reference_file: F,
        type_registry: T,
        options: KFCWriteOptions,
        source: Option<&Path>,
    ) -> Result<Self, KFCReadError> {
        let kfc_path =
            path.as_ref()
                .join(format!("{}.{}", file_name.as_ref(), options.kfc_extension));
        let current_file = if source.is_none() {
            Some(KFCFile::from_path(&kfc_path, true)?)
        } else {
            None
        };

        let file = reference_file.borrow();

        let resources = file.resources().as_builder();
        let contents = file.contents().as_builder();

        drop(current_file);

        let file = File::options()
            .write(true)
            .read(true)
            .create_new(source.is_some())
            .open(&kfc_path)?;

        let resources_path = path.as_ref().join(format!(
            "{}.{}",
            file_name.as_ref(),
            options.resource_extension
        ));
        let generated_prefix = source
            .map(|source| {
                let file = File::open(source.join(format!(
                    "{}.{}",
                    file_name.as_ref(),
                    options.resource_extension
                )))?;
                let length = file.metadata()?.len();
                Ok::<_, std::io::Error>((file, length))
            })
            .transpose()?;
        let resource_file = File::options()
            .write(true)
            .create_new(source.is_some())
            .open(&resources_path)?;
        let mut resource_writer = BufWriter::new(resource_file);

        let previous_resource_size = reference_file
            .borrow()
            .resource_chunks()
            .iter()
            .map(|chunk| chunk.offset + chunk.size)
            .max()
            .unwrap_or(0);

        let previous_uncompressed_size = reference_file
            .borrow()
            .resource_chunks()
            .iter()
            .map(|chunk| chunk.uncompressed_offset + chunk.uncompressed_size)
            .max()
            .unwrap_or(0);

        if options.append_after_resource_file {
            let end = if let Some((_, length)) = &generated_prefix {
                resource_writer.seek(SeekFrom::Start(*length))?
            } else {
                resource_writer.seek(SeekFrom::End(0))?
            };
            if end < previous_resource_size {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "resource file is shorter than the reference index",
                )
                .into());
            }
            resource_writer.align(RESOURCE_CHUNK_ALIGNMENT as usize)?;
        } else {
            resource_writer.seek(SeekFrom::Start(previous_resource_size))?;
        }

        Ok(Self {
            type_registry,

            path: path.as_ref().into(),
            file_name: file_name.as_ref().to_string(),

            file,
            chunk_writer: Cursor::new(Vec::new()),
            resource_writer,
            uncompressed_offset: previous_uncompressed_size,
            uncompressed_chunk_offset: previous_uncompressed_size,
            resource_chunks: Vec::new(),

            container_writers: Vec::new(),

            game_version: reference_file.borrow().game_version().to_string(),
            resources,
            contents,

            incremental_data: Some(IncrementalData { reference_file }),
            generated_prefix,
            options,
        })
    }

    #[inline]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[inline]
    pub fn type_registry(&self) -> &TypeRegistry {
        self.type_registry.borrow()
    }

    #[inline]
    pub fn reference_file(&self) -> Option<&KFCFile> {
        self.incremental_data
            .as_ref()
            .map(|data| data.reference_file.borrow())
    }

    pub fn write_resource(&mut self, guid: &ResourceId, mut bytes: &[u8]) -> std::io::Result<()> {
        self.resources.insert(
            *guid,
            ResourceEntry {
                offset: self.uncompressed_offset,
                size: bytes.len() as u64,
            },
        );

        let previous_offset = self.uncompressed_offset;

        self.uncompressed_offset += bytes.len() as u64;
        self.uncompressed_offset += (RESOURCE_ALIGNMENT
            - (self.uncompressed_offset % RESOURCE_ALIGNMENT))
            % RESOURCE_ALIGNMENT;

        let mut aligned_size = self.uncompressed_offset - previous_offset;

        // write resource data or compress it if needed

        while aligned_size + self.chunk_writer.position() >= RESOURCE_CHUNK_SIZE {
            let available_size = RESOURCE_CHUNK_SIZE - self.chunk_writer.position();
            let split_index = (available_size as usize).min(bytes.len());

            let (chunk_bytes, remaining_bytes) = bytes.split_at(split_index);

            self.chunk_writer.write_all(chunk_bytes)?;
            self.chunk_writer.align(RESOURCE_CHUNK_SIZE as usize)?;

            // compress and submit chunk

            self.submit_resource_data()?;

            // continue with remaining bytes

            bytes = remaining_bytes;
            aligned_size -= available_size;
        }

        if aligned_size > 0 {
            self.chunk_writer.write_all(bytes)?;

            let remaining = aligned_size - bytes.len() as u64;

            if remaining > 0 {
                self.chunk_writer.padding(remaining)?;
            }
        }

        Ok(())
    }

    /// Restore a resource map entry into the retained input prefix without
    /// rewriting its bytes. `expected_current` binds the operation to the map
    /// entry observed by the caller; `original` is either the prior prefix
    /// mapping or `None` for a resource that was originally absent.
    pub fn restore_resource_mapping(
        &mut self,
        guid: &ResourceId,
        expected_current: Option<(u64, u64)>,
        original: Option<(u64, u64)>,
    ) -> std::io::Result<()> {
        let current = self.resources.get(guid);
        let matches = match (current, expected_current) {
            (Some(current), Some((offset, size))) => {
                current.offset == offset && current.size == size
            }
            (None, None) => true,
            _ => false,
        };
        if !matches {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resource mapping changed before restoration",
            ));
        }

        if let Some((offset, size)) = original {
            let reference = self
                .incremental_data
                .as_ref()
                .map(|data| data.reference_file.borrow())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "resource restoration requires an incremental reference",
                    )
                })?;
            let end = offset.checked_add(size).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "resource mapping range overflow",
                )
            })?;
            let mut cursor = offset;
            if offset > u32::MAX as u64 || size > u32::MAX as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "resource mapping exceeds the KFC index range",
                ));
            }
            if size == 0 {
                cursor = end;
            } else {
                for chunk in reference.resource_chunks() {
                    let chunk_start = chunk.uncompressed_offset;
                    let chunk_end = chunk_start
                        .checked_add(chunk.uncompressed_size)
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "resource chunk range overflow",
                            )
                        })?;
                    if chunk_end <= cursor {
                        continue;
                    }
                    if chunk_start > cursor {
                        break;
                    }
                    cursor = end.min(chunk_end);
                    if cursor == end {
                        break;
                    }
                }
            }
            if cursor != end {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "resource mapping does not lie in the retained reference prefix",
                ));
            }
            self.resources
                .insert(*guid, ResourceEntry::new(offset, size));
        } else {
            self.resources.remove(guid);
        }

        Ok(())
    }

    fn submit_resource_data(&mut self) -> std::io::Result<()> {
        let offset = self.resource_writer.stream_position()?;
        let chunk_data = self.chunk_writer.get_ref();

        zstd::stream::copy_encode(&mut &chunk_data[..], &mut self.resource_writer, 0)?;

        let compressed_size = self.resource_writer.stream_position()? - offset;

        self.resource_writer
            .align(RESOURCE_CHUNK_ALIGNMENT as usize)?;

        let size = self.resource_writer.stream_position()? - offset;
        let uncompressed_offset = self.uncompressed_chunk_offset;
        let uncompressed_size = chunk_data.len() as u64;

        self.resource_chunks.push(ResourceChunkInfo {
            offset,
            size,
            compressed_size,
            uncompressed_offset,
            uncompressed_size,
        });

        self.uncompressed_chunk_offset += uncompressed_size;

        // reset chunk writer

        self.chunk_writer.set_position(0);
        self.chunk_writer.get_mut().clear();

        Ok(())
    }

    pub fn write_content(&mut self, guid: &ContentHash, data: &[u8]) -> std::io::Result<()> {
        if guid.size() != data.len() as u32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Content size mismatch",
            ));
        }

        if self.contents.contains_key(guid) {
            // if the content already exists, we don't need to write it again since they are unique
            return Ok(());
        }

        let container_writer = self.get_container_writer()?;
        let index = container_writer.index;

        let writer = container_writer.aquire()?;
        let offset = writer.stream_position()?;

        writer.write_all(data)?;
        writer.align(CONTENT_ALIGNMENT as usize)?;

        self.contents
            .insert(*guid, ContentEntry::new(offset, 0, index));

        Ok(())
    }

    pub fn finalize(self) -> Result<(), KFCWriteError> {
        self.finalize_inner(true)
    }

    /// Finalize map-only edits without emitting an empty compressed resource
    /// chunk. This is for exact mapping inverses; pending resource bytes refuse.
    pub fn finalize_without_resource_data(self) -> Result<(), KFCWriteError> {
        if !self.chunk_writer.get_ref().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resource bytes are pending during mapping-only finalization",
            )
            .into());
        }
        self.finalize_inner(false)
    }

    fn finalize_inner(mut self, submit_resource_data: bool) -> Result<(), KFCWriteError> {
        if submit_resource_data {
            self.submit_resource_data()?;
        }
        if let Some((mut source, length)) = self.generated_prefix.take() {
            stream_generated_prefix(&mut source, &mut self.resource_writer, length)?;
        }

        // prepare container infos

        let mut containers = self
            .incremental_data
            .as_ref()
            .map(|data| data.reference_file.borrow().containers().to_vec())
            .unwrap_or_default();

        containers.reserve_exact(self.container_writers.len());

        self.container_writers.sort_by_key(|w| w.index);

        for container_writer in &mut self.container_writers {
            container_writer.flush()?;

            containers.push(ContainerInfo {
                size: container_writer.position()?,
                count: container_writer.count,
            });
        }

        // make sure to have a power of two container files

        let required_container_count = containers.len().next_power_of_two();

        while containers.len() < required_container_count {
            let path = self.dat_path(containers.len());

            if !path.exists() || self.options.truncate_containers {
                File::create(path)?;
            }

            containers.push(ContainerInfo { size: 0, count: 0 });
        }

        // prepare resource chunks

        let mut chunks = self
            .incremental_data
            .as_ref()
            .map(|data| data.reference_file.borrow().resource_chunks().to_vec())
            .unwrap_or_default();

        chunks.extend(self.resource_chunks.iter().cloned());

        // header construction

        let mut file_writer = BufWriter::new(&mut self.file);
        let mut file = KFCFile::default();

        file.set_game_version(self.game_version);
        file.set_resources(
            self.resources.build(),
            self.type_registry.borrow(),
            self.incremental_data
                .as_ref()
                .map(|data| data.reference_file.borrow().resource_bundles()),
        )?;
        file.set_contents(self.contents.build());
        file.set_containers(containers);
        file.set_resource_chunks(chunks);

        file.write(&mut file_writer)?;

        Ok(())
    }

    pub fn dat_path(&self, index: usize) -> PathBuf {
        // Format: FILE_NAME_{INDEX}.dat where INDEX is 3 digits with leading zeros
        let name = format!(
            "{}_{:03}.{}",
            self.file_name, index, self.options.dat_extension
        );
        self.path.join(name)
    }

    fn get_container_writer(&mut self) -> std::io::Result<&mut ContainerWriter> {
        if !self.container_writers.is_empty() {
            let writer = self.container_writers.last_mut().unwrap();
            let pos = writer.position()?;

            if pos < self.options.max_container_size as u64 {
                return Ok(self.container_writers.last_mut().unwrap());
            }
        }

        // create a new container writer

        let base_index = self
            .incremental_data
            .as_ref()
            .map(|data| data.reference_file.borrow().containers().len())
            .unwrap_or(0);
        let next_index = base_index + self.container_writers.len();
        let path = self.dat_path(next_index);

        if !self.options.overwrite_containers && path.exists() {
            // make sure we don't accidentally overwrite an existing file
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Data file already exists: {}", path.display()),
            ));
        }

        let writer = BufWriter::new(File::create(&path)?);
        let writer = ContainerWriter::new(next_index, writer);

        self.container_writers.push(writer);

        Ok(self.container_writers.last_mut().unwrap())
    }
}

// The generated suffix must already exist before this transfer. A failed read
// or write leaves only an unpublishable edited output, never a pristine clone.
fn stream_generated_prefix(
    source: &mut impl Read,
    output: &mut (impl Write + Seek),
    length: u64,
) -> std::io::Result<()> {
    let end = output.stream_position()?;
    output.seek(SeekFrom::Start(0))?;
    let copied = std::io::copy(&mut source.take(length), output)?;
    if copied != length {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "resource prefix changed during generated output",
        ));
    }
    output.seek(SeekFrom::Start(end))?;
    Ok(())
}

#[cfg(test)]
mod generated_output_tests {
    use super::*;
    use crate::reflection::{PrimitiveType, TypeFlags, TypeIndex, TypeMetadata};
    use std::io;

    fn fixture() -> (PathBuf, PathBuf, TypeRegistry, KFCFile) {
        let root = std::env::temp_dir().join(format!(
            "kfc-generated-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let output = root.join("output");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir(&output).unwrap();
        File::create(source.join("game.kfc")).unwrap();
        File::create(source.join("game.kfc_resources")).unwrap();
        let registry = TypeRegistry::default();
        let mut writer =
            KFCWriter::<KFCFile, _>::new(&source, "game", &registry, "fixture").unwrap();
        let content = b"existing mapped content";
        writer
            .write_content(&ContentHash::from_data(content), content)
            .unwrap();
        writer.finalize().unwrap();
        File::options()
            .append(true)
            .open(source.join("game.kfc_resources"))
            .unwrap()
            .write_all(b"unreferenced EOF canary")
            .unwrap();
        let reference = KFCFile::from_path(source.join("game.kfc"), false).unwrap();
        (source, output, registry, reference)
    }

    fn assert_no_beforeimage(output: &Path, original: &[Vec<u8>]) {
        for entry in std::fs::read_dir(output).unwrap() {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            assert!(
                original.iter().all(|before| bytes != *before),
                "raw original clone in generated output"
            );
        }
    }

    fn typed_resource_fixture() -> (
        PathBuf,
        PathBuf,
        TypeRegistry,
        KFCFile,
        ResourceId,
        ResourceId,
    ) {
        const TYPE_HASH: u32 = 0x51ea_c0de;
        let root = std::env::temp_dir().join(format!(
            "kfc-resource-mapping-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let applied = root.join("applied");
        let restored = root.join("restored");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir(&applied).unwrap();
        std::fs::create_dir(&restored).unwrap();
        File::create(source.join("game.kfc")).unwrap();
        File::create(source.join("game.kfc_resources")).unwrap();

        let metadata = TypeMetadata {
            index: TypeIndex::new(0),
            name: "Resource".into(),
            impact_name: "Resource".into(),
            qualified_name: "fixture::Resource".into(),
            namespace: vec!["fixture".into()],
            inner_type: None,
            size: 0,
            alignment: 0,
            element_alignment: 0,
            field_count: 0,
            primitive_type: PrimitiveType::Struct,
            flags: TypeFlags::HAS_DS,
            name_hash: 1,
            impact_hash: 2,
            qualified_hash: TYPE_HASH,
            internal_hash: 3,
            struct_fields: Default::default(),
            enum_fields: Default::default(),
            default_value: None,
            attributes: Default::default(),
        };
        let registry: TypeRegistry = serde_json::from_value(serde_json::json!({
            "version": "fixture",
            "types": [metadata]
        }))
        .unwrap();
        let existing =
            ResourceId::parse("00000001-0000-0000-0000-000000000001", TYPE_HASH, 0).unwrap();
        let created =
            ResourceId::parse("00000002-0000-0000-0000-000000000002", TYPE_HASH, 0).unwrap();
        let mut writer =
            KFCWriter::<KFCFile, _>::new(&source, "game", &registry, "fixture").unwrap();
        writer
            .write_resource(&existing, b"reviewed original resource")
            .unwrap();
        writer.finalize().unwrap();
        File::options()
            .append(true)
            .open(source.join("game.kfc_resources"))
            .unwrap()
            .write_all(b"unreferenced retained prefix")
            .unwrap();
        let reference = KFCFile::from_path(source.join("game.kfc"), false).unwrap();
        (source, applied, registry, reference, existing, created)
    }

    #[test]
    fn generated_constructor_and_early_abort_never_materialize_originals() {
        let (source, output, registry, reference) = fixture();
        let original: Vec<_> = ["game.kfc", "game.kfc_resources"]
            .iter()
            .map(|name| std::fs::read(source.join(name)).unwrap())
            .collect();
        let mut writer =
            KFCWriter::new_generated_incremental(&source, &output, "game", &reference, &registry)
                .unwrap();
        assert_eq!(std::fs::metadata(output.join("game.kfc")).unwrap().len(), 0);
        assert_no_beforeimage(&output, &original);
        let content = b"new generated content before early failure";
        writer
            .write_content(&ContentHash::from_data(content), content)
            .unwrap();
        drop(writer); // Same boundary as a Lua or semantic failure before finalize.
        assert_no_beforeimage(&output, &original);
        assert!(KFCFile::from_path(output.join("game.kfc"), false).is_err());
        for (name, before) in ["game.kfc", "game.kfc_resources"].iter().zip(original) {
            assert_eq!(std::fs::read(source.join(name)).unwrap(), before);
        }
    }

    #[test]
    fn generated_output_preserves_exact_eof_and_existing_content_mappings() {
        let (source, output, registry, reference) = fixture();
        let prefix = std::fs::read(source.join("game.kfc_resources")).unwrap();
        let mut writer =
            KFCWriter::new_generated_incremental(&source, &output, "game", &reference, &registry)
                .unwrap();
        let content = b"new generated content";
        let id = ContentHash::from_data(content);
        writer.write_content(&id, content).unwrap();
        writer.finalize().unwrap();
        let resources = std::fs::read(output.join("game.kfc_resources")).unwrap();
        assert!(resources.starts_with(&prefix));
        assert!(resources.len() > prefix.len());
        let after = KFCFile::from_path(output.join("game.kfc"), false).unwrap();
        for (id, before) in reference.contents().iter() {
            let entry = after.contents().get(id).unwrap();
            assert_eq!(
                (before.offset, before.flags, before.container_index),
                (entry.offset, entry.flags, entry.container_index)
            );
        }
        let mut reader = super::super::KFCReader::new(&output, "game")
            .unwrap()
            .into_cursor()
            .unwrap();
        assert_eq!(reader.read_content(&id).unwrap().unwrap(), content);
        assert!(
            !output.join("game_000.dat").exists(),
            "original DAT must stay in the source only"
        );
    }

    #[test]
    fn generated_resource_mapping_inverse_rebinds_and_removes_without_a_chunk() {
        let (source, applied_dir, registry, before, existing, created) = typed_resource_fixture();
        let original = before.resources().get(&existing).unwrap();
        let original_mapping = (original.offset, original.size);
        let mut writer =
            KFCWriter::new_generated_incremental(&source, &applied_dir, "game", &before, &registry)
                .unwrap();
        writer
            .write_resource(&existing, b"applied resource value")
            .unwrap();
        writer
            .write_resource(&created, b"new appended resource")
            .unwrap();
        writer.finalize().unwrap();

        let applied = KFCFile::from_path(applied_dir.join("game.kfc"), false).unwrap();
        let existing_after = applied.resources().get(&existing).unwrap();
        let created_after = applied.resources().get(&created).unwrap();
        let applied_chunks = applied.resource_chunks().len();
        let applied_resources = std::fs::read(applied_dir.join("game.kfc_resources")).unwrap();
        let restored_dir = source.parent().unwrap().join("restored");
        let mut inverse = KFCWriter::new_generated_incremental(
            &applied_dir,
            &restored_dir,
            "game",
            &applied,
            &registry,
        )
        .unwrap();
        inverse
            .restore_resource_mapping(
                &existing,
                Some((existing_after.offset, existing_after.size)),
                Some(original_mapping),
            )
            .unwrap();
        inverse
            .restore_resource_mapping(
                &created,
                Some((created_after.offset, created_after.size)),
                None,
            )
            .unwrap();
        inverse.finalize_without_resource_data().unwrap();

        let restored = KFCFile::from_path(restored_dir.join("game.kfc"), false).unwrap();
        assert_eq!(restored.resources().len(), before.resources().len());
        let restored_entry = restored.resources().get(&existing).unwrap();
        assert_eq!(
            (restored_entry.offset, restored_entry.size),
            original_mapping
        );
        assert!(!restored.resources().contains_key(&created));
        assert_eq!(restored.resource_chunks().len(), applied_chunks);
        assert!(
            std::fs::read(restored_dir.join("game.kfc_resources"))
                .unwrap()
                .starts_with(&applied_resources),
            "inverse retains the append-only prefix"
        );
        let by_type: std::collections::HashSet<_> = restored
            .resources_by_type(existing.type_hash())
            .copied()
            .collect();
        assert_eq!(by_type, [existing].into_iter().collect());
        let mut reader = super::super::KFCReader::new(&restored_dir, "game")
            .unwrap()
            .into_cursor()
            .unwrap();
        assert_eq!(
            reader.read_resource(&existing).unwrap().unwrap(),
            b"reviewed original resource"
        );
        assert!(reader.read_resource(&created).unwrap().is_none());
        std::fs::remove_dir_all(source.parent().unwrap()).unwrap();
    }

    #[test]
    fn resource_mapping_inverse_rejects_stale_and_out_of_prefix_mappings() {
        let (source, applied_dir, registry, before, existing, _) = typed_resource_fixture();
        let original = before.resources().get(&existing).unwrap();
        let mut writer =
            KFCWriter::new_generated_incremental(&source, &applied_dir, "game", &before, &registry)
                .unwrap();
        assert!(
            writer
                .restore_resource_mapping(&existing, Some((u64::MAX, 4)), None)
                .is_err()
        );
        assert!(
            writer
                .restore_resource_mapping(
                    &existing,
                    Some((original.offset, original.size)),
                    Some((u64::MAX, 4)),
                )
                .is_err()
        );
        drop(writer);
        std::fs::remove_dir_all(source.parent().unwrap()).unwrap();
    }

    #[test]
    fn prefix_transfer_failure_preserves_inputs_and_leaves_no_publishable_index() {
        struct FailAfter {
            source: File,
            remaining: usize,
        }
        impl Read for FailAfter {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                if self.remaining == 0 {
                    return Err(io::Error::other("injected prefix read failure"));
                }
                let count = bytes.len().min(self.remaining);
                let count = self.source.read(&mut bytes[..count])?;
                self.remaining -= count;
                Ok(count)
            }
        }
        let (source, output, registry, reference) = fixture();
        let original: Vec<_> = ["game.kfc", "game.kfc_resources"]
            .iter()
            .map(|name| std::fs::read(source.join(name)).unwrap())
            .collect();
        let mut writer =
            KFCWriter::new_generated_incremental(&source, &output, "game", &reference, &registry)
                .unwrap();
        writer.submit_resource_data().unwrap();
        let (prefix, length) = writer.generated_prefix.take().unwrap();
        let mut failing = FailAfter {
            source: prefix,
            remaining: 5,
        };
        assert!(
            stream_generated_prefix(&mut failing, &mut writer.resource_writer, length).is_err()
        );
        writer.resource_writer.flush().unwrap();
        assert_no_beforeimage(&output, &original);
        assert_eq!(std::fs::metadata(output.join("game.kfc")).unwrap().len(), 0);
        assert!(KFCFile::from_path(output.join("game.kfc"), false).is_err());
        for (name, before) in ["game.kfc", "game.kfc_resources"].iter().zip(original) {
            assert_eq!(std::fs::read(source.join(name)).unwrap(), before);
        }
    }
}

struct ContainerWriter {
    index: usize,
    count: usize,
    writer: BufWriter<File>,
}

impl ContainerWriter {
    #[inline]
    fn new(index: usize, writer: BufWriter<File>) -> Self {
        Self {
            index,
            count: 0,
            writer,
        }
    }

    #[inline]
    fn aquire(&mut self) -> std::io::Result<&mut BufWriter<File>> {
        self.count += 1;
        Ok(&mut self.writer)
    }

    #[inline]
    fn position(&mut self) -> std::io::Result<u64> {
        self.writer.stream_position()
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
