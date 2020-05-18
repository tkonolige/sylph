extern crate cbindgen;

use std::env;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let res = cbindgen::Builder::new()
        .with_config(cbindgen::Config {
            style: cbindgen::Style::Type,
            language: cbindgen::Language::C,
            no_includes: true,
            ..cbindgen::Config::default()
        })
        .with_crate(crate_dir)
        .generate();
    match res {
        Ok(gen) => gen,
        Err(e) => match e {
            // Ignore syntax errors because those will be handled later on by cargo build.
            cbindgen::Error::ParseSyntaxError {
                crate_name: _,
                src_path: _,
                error: _,
            } => return,
            _ => panic!("{:?}", e),
        },
    }
    .write_to_file("target/bindings.h");
}
