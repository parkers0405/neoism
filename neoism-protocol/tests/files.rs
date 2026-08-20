//! Roundtrip every files-protocol variant through serde_json.

use neoism_protocol::files::{
    DirEntry, FilesClientMessage, FilesServerMessage, TreeEntry,
};

fn roundtrip_client(msg: &FilesClientMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: FilesClientMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

fn roundtrip_server(msg: &FilesServerMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: FilesServerMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

#[test]
fn client_list_dir_roundtrip() {
    roundtrip_client(&FilesClientMessage::ListDir { path: "src".into() });
    roundtrip_client(&FilesClientMessage::ListDir { path: "".into() });
}

#[test]
fn client_read_file_roundtrip() {
    roundtrip_client(&FilesClientMessage::ReadFile {
        path: "README.md".into(),
    });
}

#[test]
fn client_write_file_roundtrip() {
    roundtrip_client(&FilesClientMessage::WriteFile {
        path: "out/log.txt".into(),
        bytes: vec![0, 1, 2, 255, 128],
    });
    roundtrip_client(&FilesClientMessage::WriteFile {
        path: "empty.bin".into(),
        bytes: Vec::new(),
    });
}

#[test]
fn client_walk_tree_roundtrip() {
    roundtrip_client(&FilesClientMessage::WalkTree {
        path: ".".into(),
        max_depth: Some(3),
    });
    roundtrip_client(&FilesClientMessage::WalkTree {
        path: "src".into(),
        max_depth: None,
    });
}

#[test]
fn server_dir_listing_roundtrip() {
    roundtrip_server(&FilesServerMessage::DirListing {
        path: "src".into(),
        entries: vec![
            DirEntry {
                name: "lib.rs".into(),
                is_dir: false,
                size: Some(1024),
                icon: None,
            },
            DirEntry {
                name: "submod".into(),
                is_dir: true,
                size: None,
                icon: None,
            },
        ],
    });
    roundtrip_server(&FilesServerMessage::DirListing {
        path: "empty".into(),
        entries: Vec::new(),
    });
}

#[test]
fn server_file_content_roundtrip() {
    roundtrip_server(&FilesServerMessage::FileContent {
        path: "hello.txt".into(),
        bytes: b"hello\n".to_vec(),
    });
    roundtrip_server(&FilesServerMessage::FileContent {
        path: "empty.bin".into(),
        bytes: Vec::new(),
    });
}

#[test]
fn server_file_written_roundtrip() {
    roundtrip_server(&FilesServerMessage::FileWritten {
        path: "out.bin".into(),
        bytes_written: 4096,
    });
}

#[test]
fn server_tree_listing_roundtrip() {
    roundtrip_server(&FilesServerMessage::TreeListing {
        path: ".".into(),
        entries: vec![
            TreeEntry {
                path: "Cargo.toml".into(),
                is_dir: false,
                depth: 0,
            },
            TreeEntry {
                path: "src".into(),
                is_dir: true,
                depth: 0,
            },
            TreeEntry {
                path: "src/lib.rs".into(),
                is_dir: false,
                depth: 1,
            },
        ],
    });
}

#[test]
fn server_error_roundtrip() {
    roundtrip_server(&FilesServerMessage::Error {
        message: "nope".into(),
    });
}

#[test]
fn client_stat_roundtrip() {
    roundtrip_client(&FilesClientMessage::Stat {
        path: "src/lib.rs".into(),
    });
}

#[test]
fn server_stat_roundtrip() {
    roundtrip_server(&FilesServerMessage::Stat {
        path: "src/lib.rs".into(),
        entry: DirEntry {
            name: "lib.rs".into(),
            is_dir: false,
            size: Some(2048),
            icon: None,
        },
    });
}

#[test]
fn client_create_file_roundtrip() {
    roundtrip_client(&FilesClientMessage::CreateFile {
        dir: "src".into(),
        name: "main.rs".into(),
    });
    roundtrip_client(&FilesClientMessage::CreateFile {
        dir: "".into(),
        name: "Cargo.toml".into(),
    });
}

#[test]
fn client_create_dir_roundtrip() {
    roundtrip_client(&FilesClientMessage::CreateDir {
        dir: "src".into(),
        name: "submod".into(),
    });
}

#[test]
fn client_rename_roundtrip() {
    roundtrip_client(&FilesClientMessage::Rename {
        from: "src/old.rs".into(),
        to: "src/new.rs".into(),
    });
    roundtrip_client(&FilesClientMessage::Rename {
        from: "a.txt".into(),
        to: "moved/a.txt".into(),
    });
}

#[test]
fn client_delete_roundtrip() {
    roundtrip_client(&FilesClientMessage::Delete {
        path: "junk.tmp".into(),
    });
    roundtrip_client(&FilesClientMessage::Delete {
        path: "build".into(),
    });
}

#[test]
fn server_file_created_roundtrip() {
    roundtrip_server(&FilesServerMessage::FileCreated {
        path: "src/main.rs".into(),
        is_dir: false,
    });
    roundtrip_server(&FilesServerMessage::FileCreated {
        path: "src/submod".into(),
        is_dir: true,
    });
}

#[test]
fn server_renamed_roundtrip() {
    roundtrip_server(&FilesServerMessage::Renamed {
        from: "src/old.rs".into(),
        to: "src/new.rs".into(),
    });
}

#[test]
fn server_deleted_roundtrip() {
    roundtrip_server(&FilesServerMessage::Deleted {
        path: "junk.tmp".into(),
        was_dir: false,
    });
    roundtrip_server(&FilesServerMessage::Deleted {
        path: "build".into(),
        was_dir: true,
    });
}
