//! Bounded admission inspection, not a substitute for the dynamic loader.
//! All offsets refer to one opened descriptor; no section headers are needed.
//! The server repeats this inspection on sealed bytes before loading.

use std::fs::File;

use crate::{PluginMaterialFailure, PluginMaterialIoStep, PluginTarget};
use crate::artifact_io as port;

const HEADER_BYTES: usize = 64;
const PROGRAM_BYTES: usize = 56;
const DYNAMIC_BYTES: usize = 16;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;

pub use crate::PluginElfFailure;

fn invalid(cause: PluginElfFailure) -> PluginMaterialFailure {
    PluginMaterialFailure::Elf(cause)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("fixed ELF field"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed ELF field"))
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed ELF field"))
}

fn file_range(offset: u64, length: u64, byte_len: u64) -> Result<(), PluginMaterialFailure> {
    if offset.checked_add(length).is_none_or(|end| end > byte_len) {
        return Err(invalid(PluginElfFailure::FileRange { offset, length }));
    }
    Ok(())
}

struct LoadRange {
    offset: u64,
    address: u64,
    file_bytes: u64,
    memory_bytes: u64,
}

pub fn inspect(file: &File, byte_len: u64, target: PluginTarget) -> Result<(), PluginMaterialFailure> {
    inspect_range(file, 0, byte_len, target)
}

struct ArtifactRange<F> {
    start: u64,
    length: u64,
    read_at: F,
}
impl<F: Fn(&mut [u8], u64) -> Result<(), PluginMaterialFailure>> ArtifactRange<F> {
    fn read(&self, bytes: &mut [u8], offset: u64) -> Result<(), PluginMaterialFailure> {
        file_range(offset, bytes.len() as u64, self.length)?;
        let absolute = self.start.checked_add(offset)
            .ok_or_else(|| invalid(PluginElfFailure::FileRange { offset, length: bytes.len() as u64 }))?;
        (self.read_at)(bytes, absolute)
    }
}

pub fn inspect_range(file: &File, start: u64, byte_len: u64, target: PluginTarget) -> Result<(), PluginMaterialFailure> {
    inspect_range_with(start, byte_len, target, |bytes, offset| {
        port::read_exact(file, bytes, offset, PluginMaterialIoStep::ReadElf)
    })
}

/// The host may observe positioned reads without owning another ELF parser.
/// The physical reader must fill the complete requested range or return its typed error.
pub fn inspect_range_with(
    start: u64,
    byte_len: u64,
    target: PluginTarget,
    read_at: impl Fn(&mut [u8], u64) -> Result<(), PluginMaterialFailure>,
) -> Result<(), PluginMaterialFailure> {
    let file = &ArtifactRange { start, length: byte_len, read_at };
    // Only an artifact for this host can be loaded here. For a foreign target,
    // inspect exact ELF byte ranges without applying this host's mmap rounding
    // rules. Its own host repeats the page-aware checks before loading.
    let page = inspection_page(target, PluginTarget::host().ok(), rustix::param::page_size() as u64);
    if !page.is_power_of_two() {
        return Err(invalid(PluginElfFailure::InvalidLoadRange));
    }
    if byte_len < HEADER_BYTES as u64 {
        return Err(invalid(PluginElfFailure::InvalidHeader));
    }
    let mut header = [0; HEADER_BYTES];
    file.read(&mut header, 0)?;
    if &header[..4] != b"\x7fELF" || header[6] != 1 || u32_at(&header, 20) != 1 || u16_at(&header, 52) != 64 {
        return Err(invalid(PluginElfFailure::InvalidHeader));
    }
    if header[4] != 2 || header[5] != 1 || !matches!(header[7], 0 | 3) {
        return Err(invalid(PluginElfFailure::UnsupportedEncoding));
    }
    if u16_at(&header, 16) != 3 {
        return Err(invalid(PluginElfFailure::NotSharedObject));
    }
    let machine = u16_at(&header, 18);
    if machine != target.machine() {
        return Err(invalid(PluginElfFailure::MachineMismatch { expected: target.machine(), observed: machine }));
    }
    let program_offset = u64_at(&header, 32);
    let count = u16_at(&header, 56);
    if count == 0
        || count == u16::MAX
        || u16_at(&header, 54) != PROGRAM_BYTES as u16
        || program_offset < HEADER_BYTES as u64
    {
        return Err(invalid(PluginElfFailure::InvalidProgramTable));
    }
    // Validate the complete table before allocating the small file-backed map.
    // The ELF u16 count and the artifact byte bound cap this allocation.
    file_range(program_offset, u64::from(count) * PROGRAM_BYTES as u64, byte_len)?;
    let mut loads = Vec::new();
    let mut dynamic = None;
    for index in 0..count {
        let mut program = [0; PROGRAM_BYTES];
        file.read(&mut program, program_offset + u64::from(index) * PROGRAM_BYTES as u64)?;
        let kind = u32_at(&program, 0);
        let offset = u64_at(&program, 8);
        let address = u64_at(&program, 16);
        let file_bytes = u64_at(&program, 32);
        let memory_bytes = u64_at(&program, 40);
        file_range(offset, file_bytes, byte_len)?;
        match kind {
            PT_LOAD => {
                // ld.so rounds mappings to the host page size. Establish both
                // congruence and representable rounded ends before resolving any
                // metadata, including zero-file-size segments with a page prefix.
                if file_bytes > memory_bytes
                    || offset % page != address % page
                    || address
                        .checked_add(memory_bytes)
                        .and_then(|end| end.checked_add(page - 1))
                        .is_none()
                {
                    return Err(invalid(PluginElfFailure::InvalidLoadRange));
                }
                loads.push(LoadRange { offset, address, file_bytes, memory_bytes });
            }
            PT_DYNAMIC => {
                if dynamic.replace((offset, address, file_bytes)).is_some() {
                    return Err(invalid(PluginElfFailure::DuplicateDynamic));
                }
            }
            PT_INTERP => return Err(invalid(PluginElfFailure::InterpreterPresent)),
            _ => {}
        }
    }
    let (offset, dynamic_address, dynamic_length) = dynamic.ok_or_else(|| invalid(PluginElfFailure::MissingDynamic))?;
    if dynamic_length == 0 || dynamic_length % DYNAMIC_BYTES as u64 != 0 {
        return Err(invalid(PluginElfFailure::InvalidDynamicLength));
    }
    if resolve_file_range(&loads, dynamic_address, dynamic_length, page)? != offset {
        return Err(invalid(PluginElfFailure::InvalidLoadRange));
    }
    let mut table_address = None;
    let mut table_length = None;
    visit_dynamic(file, offset, dynamic_length, |tag, value| {
        match tag {
            DT_STRTAB | DT_STRSZ => {
                let slot = if tag == DT_STRTAB { &mut table_address } else { &mut table_length };
                if slot.replace(value).is_some() {
                    return Err(invalid(PluginElfFailure::DuplicateStringTableField { tag }));
                }
            }
            // Search paths, audit libraries and filter/auxiliary dependencies
            // would bypass the release's exact DT_NEEDED allowlist.
            15 | 29 | 0x6fff_fefb | 0x6fff_fefc | 0x7fff_fffd | 0x7fff_ffff => {
                return Err(invalid(PluginElfFailure::ForbiddenDynamicTag { tag }));
            }
            0x6fff_fffb if value & 0x0800_0000 != 0 => {
                return Err(invalid(PluginElfFailure::PositionIndependentExecutable));
            }
            _ => {}
        }
        Ok(())
    })?;
    let (Some(address), Some(length)) = (table_address, table_length) else {
        return Err(invalid(PluginElfFailure::MissingStringTable));
    };
    let table_offset = resolve_file_range(&loads, address, length, page)?;
    visit_dynamic(file, offset, dynamic_length, |tag, value| {
        if tag == DT_NEEDED {
            inspect_dependency(file, table_offset, length, value, target)?;
        }
        Ok(())
    })
}

pub fn inspection_page(target: PluginTarget, host: Option<PluginTarget>, host_page: u64) -> u64 {
    if Some(target) == host { host_page } else { 1 }
}

fn resolve_file_range(loads: &[LoadRange], address: u64, length: u64, page: u64) -> Result<u64, PluginMaterialFailure> {
    let end = address
        .checked_add(length)
        .filter(|_| length != 0)
        .ok_or_else(|| invalid(PluginElfFailure::UnmappedMetadata))?;
    let mut mapped = None;
    for load in loads {
        let map_start = load.address & !(page - 1);
        let map_end = (load.address + load.memory_bytes + page - 1) & !(page - 1);
        if address < map_end && map_start < end {
            // Any second mapping of checked metadata is ambiguous here, even
            // when the declared byte ranges are disjoint. Include file-page
            // prefixes/suffixes and zero-fill mappings. Other pages remain the
            // loader's responsibility; this is not general ELF validation.
            if address < load.address || end > load.address + load.file_bytes {
                return Err(invalid(PluginElfFailure::AmbiguousMetadata));
            }
            if mapped.replace(load.offset + (address - load.address)).is_some() {
                return Err(invalid(PluginElfFailure::AmbiguousMetadata));
            }
        }
    }
    mapped.ok_or_else(|| invalid(PluginElfFailure::UnmappedMetadata))
}

fn visit_dynamic(
    file: &ArtifactRange<impl Fn(&mut [u8], u64) -> Result<(), PluginMaterialFailure>>,
    offset: u64,
    length: u64,
    mut visit: impl FnMut(u64, u64) -> Result<(), PluginMaterialFailure>,
) -> Result<(), PluginMaterialFailure> {
    let mut buffer = [0; port::COPY_BYTES];
    let mut read = 0;
    while read < length {
        let count = (length - read).min(buffer.len() as u64) as usize;
        file.read(&mut buffer[..count], offset + read)?;
        for record in buffer[..count].chunks_exact(DYNAMIC_BYTES) {
            let tag = u64_at(record, 0);
            if tag == DT_NULL {
                return Ok(());
            }
            visit(tag, u64_at(record, 8))?;
        }
        read += count as u64;
    }
    Err(invalid(PluginElfFailure::MissingDynamicTerminator))
}

fn inspect_dependency(
    file: &ArtifactRange<impl Fn(&mut [u8], u64) -> Result<(), PluginMaterialFailure>>,
    table_offset: u64,
    table_length: u64,
    name_offset: u64,
    target: PluginTarget,
) -> Result<(), PluginMaterialFailure> {
    if name_offset >= table_length {
        return Err(invalid(PluginElfFailure::InvalidDependencyName { offset: name_offset }));
    }
    // Every allowed SONAME fits here. Long names and paths are not accepted;
    // no allocation is proportional to a string or dependency count.
    let mut name = [0; 64];
    let count = (table_length - name_offset).min(name.len() as u64) as usize;
    file.read(&mut name[..count], table_offset + name_offset)?;
    let end = name[..count]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid(PluginElfFailure::InvalidDependencyName { offset: name_offset }))?;
    if !target.permits_soname(&name[..end]) {
        return Err(invalid(PluginElfFailure::DependencyNotAllowed { offset: name_offset }));
    }
    Ok(())
}
