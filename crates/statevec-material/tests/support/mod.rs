use statevec_material::{CorePackageDeclaration, PluginTarget};
use statevec_model::{SchemaIdentity, Version};

const LOAD: usize = 64;
const DYNAMIC: usize = 120;
const BODY: usize = 176;
const STRINGS: usize = BODY + 4 * 16;

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

// Format bytes only: this fixture is never loaded or executed. The actual
// Rust-produced cdylib positive control is separate below.
pub fn elf_bytes(target: PluginTarget) -> Vec<u8> {
    let mut bytes = vec![0; STRINGS + 10];
    let length = bytes.len() as u64;
    bytes[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\0");
    put16(&mut bytes, 16, 3);
    put16(&mut bytes, 18, target.machine());
    put32(&mut bytes, 20, 1);
    put64(&mut bytes, 32, LOAD as u64);
    put16(&mut bytes, 52, 64);
    put16(&mut bytes, 54, 56);
    put16(&mut bytes, 56, 2);
    put32(&mut bytes, LOAD, 1);
    put64(&mut bytes, LOAD + 32, length);
    put64(&mut bytes, LOAD + 40, length);
    put32(&mut bytes, DYNAMIC, 2);
    put64(&mut bytes, DYNAMIC + 8, BODY as u64);
    put64(&mut bytes, DYNAMIC + 16, BODY as u64);
    put64(&mut bytes, DYNAMIC + 32, 64);
    put64(&mut bytes, DYNAMIC + 40, 64);
    put64(&mut bytes, BODY, 5);
    put64(&mut bytes, BODY + 8, STRINGS as u64);
    put64(&mut bytes, BODY + 16, 10);
    put64(&mut bytes, BODY + 24, 10);
    put64(&mut bytes, BODY + 32, 1);
    bytes[STRINGS..].copy_from_slice(b"libc.so.6\0");
    bytes
}

pub fn declaration() -> CorePackageDeclaration {
    CorePackageDeclaration {
        schema_identity: SchemaIdentity {
            schema_version: Version::new(1, 0),
            types_schema_fingerprint: [1; 16], record_schema_fingerprint: [2; 16],
            command_schema_fingerprint: [3; 16], event_schema_fingerprint: [4; 16],
        },
        execution_revision: "bank/package-test/rules-1".into(), has_tx_invariants: true,
    }
}
