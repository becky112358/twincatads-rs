/*
 * (C) Copyright 2022 Automated Design Corp. All Rights Reserved.
 * File Created: Wednesday, 12th January 2022 3:33:59 pm
 * Author: Thomas C. Bitsky Jr. (support@automateddesign.com)
 */

extern crate bindgen;

use std::env;
use std::path::PathBuf;

fn main() {

    //
    // Select bindings based upon platform.
    //

    let target = env::var("TARGET").expect("Cargo build scripts always have TARGET");

    let lib_dir = if target.contains("i686") {
        // Path for 32-bit library
        "/TwinCAT/AdsApi/TcAdsDll/lib"
    } else {
        // Path for 64-bit library
        "/TwinCAT/AdsApi/TcAdsDll/x64/lib"
    };

    let bindings_filename = if target.contains("i686") {
        "bindings_32.rs"
    } else {
        "bindings_64.rs"
    };


    // add link library include path
    println!("cargo:rustc-link-search=native={}", lib_dir);
    println!("cargo:rustc-link-lib=dylib=TcAdsDll");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=wrapper.h");

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.        
        .header("wrapper.h")
        // Add lines to ignore certain Rust linter warnings
        .raw_line("#[allow(non_upper_case_globals)]")
        .raw_line("#[allow(non_camel_case_types)]")
        .raw_line("#[allow(non_snake_case)]")        
        // adds include dir search path
        .clang_arg("-I/TwinCAT/AdsApi/TcAdsDll/Include")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings_path = out_path.join(bindings_filename);

    println!("Bindings being written to: {}", bindings_path.to_str().unwrap());

    bindings
        .write_to_file(&bindings_path)
        .expect("Couldn't write bindings!");

    // Copy the bindings to the ./src directory if we want to support using pre-generated bindings.
    // let src_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("src");
    // let dst_path = src_path.join(bindings_filename);
    // fs::copy(&bindings_path, &dst_path).expect("Couldn't copy bindings to src directory");
    // println!("Bindings copied to: {}", dst_path.to_str().unwrap());    
}