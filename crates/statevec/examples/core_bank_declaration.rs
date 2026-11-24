//! Prints the source or additive Bank package declaration for one execution revision.
//! Build: cargo build -p statevec --example core_bank_declaration.
//! The revision is the caller's claim; `sv-package verify` loads the
//! packed artifact, configures it and refuses a declaration it does not match.
#[allow(dead_code)]
#[path = "core_bank_additive/schema.rs"]
mod additive;
#[allow(dead_code)]
#[path = "core_bank/schema.rs"]
mod bank;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let (revision, schema) = match &args[1..] {
        [revision] => (revision, bank::registry().identity()),
        [flag, revision] if flag == "--additive" => (revision, additive::registry().identity()),
        _ => panic!("usage: core_bank_declaration [--additive] EXECUTION_REVISION"),
    };
    let declaration = serde_json::json!({
        "format": statevec_package::DECLARATION_FORMAT,
        "version": statevec_package::DECLARATION_VERSION,
        "schema_identity": {
            "version": hex(&schema.schema_version.to_bytes()),
            "types": hex(&schema.types_schema_fingerprint),
            "records": hex(&schema.record_schema_fingerprint),
            "commands": hex(&schema.command_schema_fingerprint),
            "events": hex(&schema.event_schema_fingerprint),
        },
        "execution_revision": revision,
        "has_tx_invariants": true,
    });
    println!("{}", serde_json::to_string_pretty(&declaration).unwrap());
}
