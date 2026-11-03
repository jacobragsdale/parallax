//! Compiles `proto/parallax.proto` with protox, a pure-Rust protobuf compiler, so
//! no `protoc` binary is needed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "../../proto/parallax.proto";
    println!("cargo:rerun-if-changed={proto}");
    let descriptors = protox::compile([proto], ["../../proto"])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}
