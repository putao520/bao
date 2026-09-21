use crate::common::*;

const INCLUDE_DIRS: &[&str] = &["libffi", "libffi/include", "include/msvc"];

// libffi expects us to include the same folder in case of x86 and x86_64 architectures
const INCLUDE_DIRS_X86: &[&str] = &["libffi/src/x86"];
const INCLUDE_DIRS_X86_64: &[&str] = &["libffi/src/x86"];
const INCLUDE_DIRS_AARCH64: &[&str] = &["libffi/src/aarch64"];

const BUILD_FILES: &[&str] = &[
    "tramp.c",
    "closures.c",
    "prep_cif.c",
    "raw_api.c",
    "types.c",
];
const BUILD_FILES_X86: &[&str] = &["x86/ffi.c"];
const BUILD_FILES_X86_64: &[&str] = &["x86/ffi.c", "x86/ffiw64.c"];
const BUILD_FILES_AARCH64: &[&str] = &["aarch64/ffi.c"];

fn add_file(build: &mut cc::Build, file: &str) {
    build.file(format!("libffi/src/{}", file));
}

fn unsupported(arch: &str) -> ! {
    panic!("Unsupported architecture: {}", arch)
}

pub fn build_and_link() {
    let target = env::var("TARGET").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    // we should collect all include dirs together with platform specific ones
    // to pass them over to the asm pre-processing step
    let mut all_includes = vec![];
    for each_include in INCLUDE_DIRS {
        all_includes.push(*each_include);
    }
    for each_include in match target_arch.as_str() {
        "x86" => INCLUDE_DIRS_X86,
        "x86_64" => INCLUDE_DIRS_X86_64,
        "aarch64" => INCLUDE_DIRS_AARCH64,
        _ => unsupported(&target_arch),
    } {
        all_includes.push(*each_include);
    }

    // BAO FORK (2.3.0, #18 — msvc cross unlock; upstream tov/libffi-rs):
    // 4. x86_64 assembles the GAS-variant `libffi/src/x86/win64.S` directly
    //    through the cc build's clang-cl (integrated assembler, GNU .seh_*
    //    directives) instead of the MASM route: pre-processed
    //    win64_intel.asm needs MSVC ml64 (or llvm-ml, which cannot parse
    //    `extern sym:near` / `.seh_*` even with -m64) — neither exists on a
    //    cross host. Same exported symbols (ffi_call_win64 /
    //    ffi_closure_win64), same tree, mingw-proven variant. The MASM
    //    pre-processing path stays for x86/aarch64 (armasm64 territory).
    let asm_path = if target_arch == "x86_64" {
        String::from("libffi/src/x86/win64.S")
    } else {
        pre_process_asm(all_includes.as_slice(), &target, &target_arch)
    };
    let mut build = cc::Build::new();

    for each_include in all_includes {
        build.include(each_include);
    }

    for each_source in BUILD_FILES {
        add_file(&mut build, each_source);
    }
    for each_source in match target_arch.as_str() {
        "x86" => BUILD_FILES_X86,
        "x86_64" => BUILD_FILES_X86_64,
        "aarch64" => BUILD_FILES_AARCH64,
        _ => unsupported(&target_arch),
    } {
        add_file(&mut build, each_source);
    }

    build
        .file(asm_path)
        .define("WIN32", None)
        .define("_LIB", None)
        .define("FFI_BUILDING", None)
        .warnings(false)
        .compile("libffi");
}

pub fn probe_and_link() {
    // At the time of writing it wasn't clear if MSVC builds will support
    // dynamic linking of libffi; assuming it's even installed. To ensure
    // existing MSVC setups continue to work, we just compile libffi from source
    // and statically link it.
    build_and_link();
}

pub fn pre_process_asm(include_dirs: &[&str], target: &str, target_arch: &str) -> String {
    let folder_name = match target_arch {
        "x86" => "x86",
        "x86_64" => "x86",
        "aarch64" => "aarch64",
        _ => unsupported(target_arch),
    };

    let file_name = match target_arch {
        "x86" => "sysv_intel",
        "x86_64" => "win64_intel",
        "aarch64" => "win64_armasm",
        _ => unsupported(target_arch),
    };

    // BAO FORK (2.3.0, #18 — msvc cross unlock; upstream tov/libffi-rs):
    // 2. Pre-process with the env-contract compiler the cc build below uses
    //    (CC_<triple> / TARGET_CC / CC, defaulting to clang-cl) instead of
    //    `cc::windows_registry::find(target, "cl.exe")` — that lookup demands
    //    a real MSVC installation, which a linux cross host does not have.
    //    clang-cl's /EP is cl-compatible. INCLUDE comes straight from the
    //    environment (the xwin-style sysroot contract) with the libffi faces
    //    appended, matching the previous pass-through order.
    let mut cmd = Command::new(compiler_for_target(target));

    let mut include_env = env::var("INCLUDE").unwrap_or_default();
    if !include_env.is_empty() {
        include_env.push(';');
    }
    include_env.push_str(&include_dirs.join(";"));
    cmd.env("INCLUDE", include_env);

    cmd.arg("/EP");
    cmd.arg(format!("libffi/src/{}/{}.S", folder_name, file_name));

    // 3. The pre-processed .asm lands in OUT_DIR instead of the package
    //    source tree: the previous relative path wrote into the (possibly
    //    shared, possibly read-only) crate directory on every build, racing
    //    concurrent builds and dirtying vendored checkouts.
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set by cargo");
    let out_path = Path::new(&out_dir)
        .join(format!("{}.asm", file_name))
        .display()
        .to_string();
    let asm_file = fs::File::create(&out_path).expect("Could not create output file");

    cmd.stdout(asm_file);

    run_command("Pre-process ASM", &mut cmd);

    out_path
}

/// BAO FORK: cc-rs-compatible per-target compiler probe, mirroring cc's own
/// resolution order ({key}_{target-hyphen} → {key}_{target-underscore} →
/// TARGET_{key} → {key}); clang-cl is the default for the msvc target (the
/// shape upstream bun/bao build their windows C with).
fn compiler_for_target(target: &str) -> String {
    let probe = |name: &str| -> Option<String> {
        env::var(name).ok().filter(|v| !v.is_empty())
    };
    for key in [
        format!("CC_{}", target),
        format!("CC_{}", target.replace('-', "_")),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ] {
        if let Some(v) = probe(&key) {
            return v;
        }
    }
    "clang-cl".to_string()
}
