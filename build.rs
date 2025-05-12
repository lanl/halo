// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=halo.capnp");

    capnpc::CompilerCommand::new().file("halo.capnp").run()?;

    Ok(())
}
