/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

fn main() {
    let static_atoms =
        Path::new(&env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("static_atoms.txt");
    let static_atoms = BufReader::new(File::open(&static_atoms).unwrap());
    let mut atom_type = string_cache_codegen::AtomType::new("Atom", "atom!");

    macro_rules! predefined {
        ($($name: expr,)+) => {
            {
                $(
                    atom_type.atom($name);
                )+
            }
        }
    }
    include!("./predefined_counter_styles.rs");

    atom_type
        .atoms(static_atoms.lines().map(Result::unwrap))
        .write_to_file(&Path::new(&env::var_os("OUT_DIR").unwrap()).join("atom.rs"))
        .unwrap();

    // BAO patch (fork-maintained, 2026-09-28): declare the atom-table inputs —
    // without this, appending to static_atoms.txt after a fingerprint rotation
    // leaves a stale generated atom.rs (missing newly appended atoms).
    println!("cargo:rerun-if-changed=static_atoms.txt");
    println!("cargo:rerun-if-changed=predefined_counter_styles.rs");
}
