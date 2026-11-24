//! Compiles boxd's vendored `api.proto` into the gRPC client. `protox` is
//! a pure-Rust protobuf compiler, so building needs no `protoc` install.
//! `scripts/update_boxd_proto.sh` refreshes the vendored file.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto = "proto/boxd/api/v1/api.proto";
    println!("cargo:rerun-if-changed={proto}");
    let descriptors = protox::compile([proto], ["proto"])?;
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .compile_fds(descriptors)?;
    Ok(())
}
