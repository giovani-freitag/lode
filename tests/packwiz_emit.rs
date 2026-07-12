use std::fs;

use lode::hash::sha256_hex;
use lode::loader::Loader;
use lode::lock::{Download, Lock, LockedMod, ResolverMeta, LOCKFILE_VERSION};
use lode::manifest::{Defaults, LoaderSpec, Manifest, Overlay, PackMeta};
use lode::packwiz;
use lode::provider::{DownloadMode, Provider};
use lode::side::Side;

fn sample() -> (Manifest, Lock) {
    let manifest = Manifest {
        pack: PackMeta {
            name: "Test Pack".into(),
            author: "A".into(),
            version: "0.1.0".into(),
            description: None,
        },
        loader: LoaderSpec {
            name: Loader::Forge,
            minecraft: "1.20.1".into(),
            version: "47.3.0".into(),
        },
        defaults: Defaults { side: Side::Both },
        overlays: Vec::new(),
        mods: Default::default(),
    };

    let sodium = LockedMod {
        slug: "sodium".into(),
        name: "Sodium".into(),
        provider: Provider::Modrinth,
        project_id: "AANobbMI".into(),
        file_id: Some("abcd1234".into()),
        version: "0.5.8".into(),
        filename: "sodium-fabric-0.5.8.jar".into(),
        download: Download {
            url: Some("https://cdn.modrinth.com/data/AANobbMI/versions/abcd1234/sodium.jar".into()),
            mode: DownloadMode::Url,
            hash_format: "sha512".into(),
            hash: "deadbeef".into(),
            size: Some(1234),
        },
        side: Side::Client,
        optional: false,
        dependencies: Vec::new(),
        requested_by: vec!["manifest".into()],
    };

    let lock = Lock {
        lockfile_version: LOCKFILE_VERSION,
        manifest_hash: "sha256:0".into(),
        loader: manifest.loader.clone(),
        resolver: ResolverMeta {
            lode_version: "0.1.0".into(),
        },
        mods: vec![sodium],
    };

    (manifest, lock)
}

#[test]
fn emits_a_packwiz_metafile_with_the_interop_fields() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, lock) = sample();

    packwiz::emit(dir.path(), dir.path(), &manifest, &lock).unwrap();

    let pw = fs::read_to_string(dir.path().join("mods/sodium.pw.toml")).unwrap();
    assert!(pw.contains("name = \"Sodium\""), "{pw}");
    assert!(pw.contains("filename = \"sodium-fabric-0.5.8.jar\""));
    assert!(pw.contains("side = \"client\""));
    assert!(pw.contains("[download]"));
    assert!(pw.contains("hash-format = \"sha512\""));
    assert!(pw.contains("[update.modrinth]"));
    assert!(pw.contains("mod-id = \"AANobbMI\""));
    assert!(pw.contains("version = \"abcd1234\""));
}

#[test]
fn pack_toml_references_the_real_index_hash() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, lock) = sample();

    packwiz::emit(dir.path(), dir.path(), &manifest, &lock).unwrap();

    let index = fs::read_to_string(dir.path().join("index.toml")).unwrap();
    let pack = fs::read_to_string(dir.path().join("pack.toml")).unwrap();

    let index_hash = sha256_hex(index.as_bytes());
    assert!(
        pack.contains(&format!("hash = \"{index_hash}\"")),
        "pack.toml:\n{pack}"
    );
    assert!(pack.contains("pack-format = \"packwiz:1.1.0\""));
    assert!(pack.contains("forge = \"47.3.0\""));
}

#[test]
fn index_lists_the_metafile_with_its_sha256() {
    let dir = tempfile::tempdir().unwrap();
    let (manifest, lock) = sample();

    packwiz::emit(dir.path(), dir.path(), &manifest, &lock).unwrap();

    let pw_bytes = fs::read(dir.path().join("mods/sodium.pw.toml")).unwrap();
    let index = fs::read_to_string(dir.path().join("index.toml")).unwrap();

    assert!(index.contains("file = \"mods/sodium.pw.toml\""));
    assert!(index.contains("metafile = true"));
    assert!(index.contains(&sha256_hex(&pw_bytes)));
}

#[test]
fn overlay_files_are_copied_and_indexed_without_metafile() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/example.toml"),
        b"greeting = \"hi\"\n",
    )
    .unwrap();

    let (mut manifest, lock) = sample();
    manifest.overlays.push(Overlay {
        path: "config/**".into(),
        side: None,
    });

    let pack_dir = root.path().join("pack");
    packwiz::emit(&pack_dir, root.path(), &manifest, &lock).unwrap();

    assert!(pack_dir.join("config/example.toml").exists());
    let index = fs::read_to_string(pack_dir.join("index.toml")).unwrap();
    assert!(index.contains("file = \"config/example.toml\""), "{index}");
    // Overlay files are plain files, not metafiles — check only this entry's own block.
    let after = index
        .split("file = \"config/example.toml\"")
        .nth(1)
        .unwrap();
    let config_block = after.split("[[files]]").next().unwrap();
    assert!(!config_block.contains("metafile"), "{index}");
}

#[test]
fn local_jars_are_bundled_as_raw_files() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("local")).unwrap();
    fs::write(root.path().join("local/mymod.jar"), b"jar bytes").unwrap();

    let (manifest, mut lock) = sample();
    lock.mods.push(LockedMod {
        slug: "mymod".into(),
        name: "mymod".into(),
        provider: Provider::Local,
        project_id: "mymod.jar".into(),
        file_id: None,
        version: "local".into(),
        filename: "mymod.jar".into(),
        download: Download {
            url: None,
            mode: DownloadMode::Url,
            hash_format: "sha512".into(),
            hash: "x".into(),
            size: Some(9),
        },
        side: Side::Both,
        optional: false,
        dependencies: Vec::new(),
        requested_by: vec!["manifest".into()],
    });

    let pack_dir = root.path().join("pack");
    packwiz::emit(&pack_dir, root.path(), &manifest, &lock).unwrap();

    assert!(pack_dir.join("mods/mymod.jar").exists());
    let index = fs::read_to_string(pack_dir.join("index.toml")).unwrap();
    assert!(index.contains("file = \"mods/mymod.jar\""), "{index}");
}
