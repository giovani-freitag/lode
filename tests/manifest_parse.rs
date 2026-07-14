use lode::loader::Loader;
use lode::manifest::Manifest;
use lode::side::Side;

#[test]
fn parses_strict_json_and_rejects_jsonc_extras() {
    // Strict JSON only (like package.json): plain JSON parses; comments and trailing commas do not.
    let text = r#"{
        "pack": { "name": "Test", "author": "A", "version": "0.1.0" },
        "loader": { "name": "forge", "minecraft": "1.20.1", "version": "47.3.0" },
        "mods": {
            "sodium": "^0.5.8",
            "jei": { "version": "*", "side": "client" }
        }
    }"#;

    let manifest = Manifest::parse(text).expect("plain JSON should parse");
    assert_eq!(manifest.pack.name, "Test");
    assert_eq!(manifest.loader.name, Loader::Forge);
    assert_eq!(manifest.mods["sodium"].constraint(), "^0.5.8");
    assert_eq!(manifest.mods["jei"].side(), Some(Side::Client));

    // A // comment and a trailing comma are now hard errors.
    assert!(
        Manifest::parse(&format!("// core\n{text}")).is_err(),
        "a comment must be rejected"
    );
    let trailing = r#"{
        "pack": { "name": "T", "author": "A", "version": "0.1.0" },
        "loader": { "name": "forge", "minecraft": "1.20.1", "version": "47.3.0" },
        "mods": { "sodium": "*", }
    }"#;
    assert!(
        Manifest::parse(trailing).is_err(),
        "a trailing comma must be rejected"
    );
}

#[test]
fn round_trips_through_canonical_json() {
    let text = r#"{
        "pack": { "name": "Test", "author": "A", "version": "0.1.0" },
        "loader": { "name": "fabric", "minecraft": "1.20.1", "version": "0.15.0" },
        "mods": { "sodium": "*" }
    }"#;

    let manifest = Manifest::parse(text).unwrap();
    let json = manifest.to_json().unwrap();
    let reparsed = Manifest::parse(&json).unwrap();

    assert_eq!(reparsed.loader.name, Loader::Fabric);
    assert_eq!(reparsed.mods.len(), 1);
    assert_eq!(reparsed.mods["sodium"].constraint(), "*");
}

#[test]
fn preserves_mod_declaration_order() {
    let text = r#"{
        "pack": { "name": "T", "author": "A", "version": "0.1.0" },
        "loader": { "name": "quilt", "minecraft": "1.20.1", "version": "0.23" },
        "mods": { "zzz": "*", "aaa": "*", "mmm": "*" }
    }"#;

    let manifest = Manifest::parse(text).unwrap();
    let order: Vec<&str> = manifest.mods.keys().map(String::as_str).collect();

    assert_eq!(order, ["zzz", "aaa", "mmm"]);
}
