//! Parsing the `zfs` JSON documents.

use cdsk::zfs::ZfsJson;

#[test]
fn a_recorded_zfs_get_document_parses() {
    let document = ZfsJson::parse(include_str!("../../fixtures/zfs-get.json"), "zfs get -j")
        .expect("the recorded document parses");

    let data = document
        .datasets
        .get("tank/apps/data")
        .expect("recorded dataset");
    assert_eq!(data.properties["recordsize"].value, "32768");
    assert!(data.properties["recordsize"].is_local());
    assert_eq!(
        data.properties["compression"].source.as_deref(),
        Some("local")
    );
    assert!(
        !document.datasets["tank/media"].properties["compression"].is_local(),
        "a default is not local"
    );
}

#[test]
fn a_recorded_zfs_list_document_parses() {
    let document = ZfsJson::parse(include_str!("../../fixtures/zfs-list.json"), "zfs list -j")
        .expect("the recorded document parses");
    assert_eq!(
        document.names(),
        vec![
            "tank",
            "tank/apps",
            "tank/apps/data",
            "tank/media",
            "tank/media/archive",
        ]
    );
}
