//! Thin build script: the generation itself lives in the `zeta-unicode-build` crate.

use std::env;
use std::path::PathBuf;

/// Budget the generated name sections (names + index + patterns) must fit into.
const NAMES_BUDGET: usize = zeta_unicode_build::NAMES_BUDGET;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ZETA_UNICODE_STATS");
    for file in [
        "DerivedName.txt",
        "DerivedGeneralCategory.txt",
        "NameAliases.txt",
        "NamedSequences.txt",
        "emoji-test.txt",
        "derived-names.rules",
    ] {
        println!("cargo:rerun-if-changed=data/{file}");
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let report = env::var_os("ZETA_UNICODE_STATS").is_some();

    let output = zeta_unicode_build::generate(&zeta_unicode_build::Request {
        data_dir: "data".into(),
        rules: "data/derived-names.rules".into(),
        out_dir,
        table_file: "zeta_unicode_tables.bin".into(),
        derive_file: "zeta_unicode_derive.rs".into(),
        report,
    })
    .expect("failed to generate the Unicode tables");

    assert!(
        output.names.table <= NAMES_BUDGET,
        "names + index + patterns = {} B, over the {NAMES_BUDGET} B budget",
        output.names.table
    );

    if report {
        println!(
            "cargo:warning=zeta-unicode: {} gc ranges -> {} table bytes",
            output.gc_ranges, output.table_bytes
        );
        println!(
            "cargo:warning=zeta-unicode: emoji: {} bytes, {} entries ({} ruled bases, \
             {} verbatim variants, {} groups with no base)",
            output.emoji.bytes,
            output.emoji.entries,
            output.emoji.ruled_bases,
            output.emoji.verbatim_variants,
            output.emoji.missing_bases
        );
        println!(
            "cargo:warning=zeta-unicode: names: {} entries ({} derived, {} stored), \
             {} arms over {} atoms ({} B), streams {}/{}/{}, names {} + runs {} + patterns {} \
             = {} B ({} B charging the atoms)",
            output.names.entries,
            output.names.derived,
            output.names.stored,
            output.names.arms,
            output.names.atoms,
            output.names.atom_bytes,
            output.names.streams[0].compressed,
            output.names.streams[1].compressed,
            output.names.streams[2].compressed,
            output.names.blob,
            output.names.runs_section.len(),
            output.names.pattern_section.len(),
            output.names.table,
            output.names.table_strict
        );
    }
}
