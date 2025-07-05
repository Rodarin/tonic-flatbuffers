#[cfg(feature = "vendored")]
#[test]
fn test_protobuf_src_protoc_path_is_valid() {
    let protoc_path = protobuf_src::protoc();
    assert!(
        protoc_path.exists(),
        "protoc path does not exist: {:?}",
        protoc_path
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = protoc_path.metadata().expect("failed to get metadata");
        assert!(
            metadata.permissions().mode() & 0o111 != 0,
            "protoc is not executable"
        );
    }
    #[cfg(windows)]
    {
        // On Windows, just check for existence and .exe extension
        assert!(
            protoc_path.extension().map(|e| e == "exe").unwrap_or(false),
            "protoc is not an .exe on Windows"
        );
    }
}
