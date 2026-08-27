fn main() {
    let protoc_path = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc path");
    std::env::set_var("PROTOC", protoc_path);

    println!("cargo:rerun-if-changed=../../protocol/srui.proto");

    prost_build::compile_protos(&["../../protocol/srui.proto"], &["../../protocol"])
        .expect("Failed to compile SRUI protocol buffers");
}
