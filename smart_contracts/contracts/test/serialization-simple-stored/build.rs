fn main() {
    capnpc::CompilerCommand::new()
        .file("u512.capnp")
        .run()
        .expect("compiling schema");
}
