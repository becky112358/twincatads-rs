extern crate bindgen;

use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").expect("Cargo build scripts always have TARGET");

    let lib_dir = if target.contains("i686") {
        "/TwinCAT/AdsApi/TcAdsDll/lib"
    } else {
        "/TwinCAT/AdsApi/TcAdsDll/x64/lib"
    };

    let bindings_filename = if target.contains("i686") {
        "bindings_32.rs"
    } else {
        "bindings_64.rs"
    };

    println!("cargo:rustc-link-search=native={}", lib_dir);
    println!("cargo:rustc-link-lib=dylib=TcAdsDll");

    println!("cargo:rerun-if-changed=wrapper.h");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .raw_line("#[allow(non_upper_case_globals)]")
        .raw_line("#[allow(non_camel_case_types)]")
        .raw_line("#[allow(non_snake_case)]")
        .clang_arg("-I/TwinCAT/AdsApi/TcAdsDll/Include")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings_path = out_path.join(bindings_filename);

    println!(
        "Bindings being written to: {}",
        bindings_path.to_str().unwrap()
    );

    bindings
        .write_to_file(&bindings_path)
        .expect("Couldn't write bindings!");
}
