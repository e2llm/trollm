// Reuse the shared temporary directory without the HTTP server fixture.
#[allow(dead_code, unused_imports)]
mod common;
use common::TempDir;
use serde_json::json;
use trollm_mcp::CacheStore;

#[test]
fn page_scan_validates_unselected_items_and_complete_item_count() {
    let dirs = TempDir::new("page-scan");
    let cache = CacheStore::new(dirs.path());
    let args = json!({});
    let hash = CacheStore::hash_args(&args, None, None);
    let first = vec![json!({"text":"é🦀\n\"\\"})];
    let second = vec![json!({"id":2}), json!({"id":3})];
    cache
        .begin_paginated("server", "list", &hash, "items", &first, 900, &args)
        .unwrap();
    cache.append_page("server", "list", &hash, &[]).unwrap();
    cache.append_page("server", "list", &hash, &second).unwrap();
    cache
        .finalize_paginated("server", "list", &hash, 900, &args)
        .unwrap();
    assert_eq!(
        cache
            .read_page("server", "list", &hash, 1)
            .unwrap()
            .unwrap()
            .0,
        first
    );
    assert!(cache
        .read_page("server", "list", &hash, 2)
        .unwrap()
        .unwrap()
        .0
        .is_empty());
    assert_eq!(
        cache
            .read_page("server", "list", &hash, 3)
            .unwrap()
            .unwrap()
            .0,
        second
    );

    let data = dirs
        .path()
        .join("server/list")
        .join(format!("{hash}.jsonl"));
    // A truncated or extra record cannot silently produce a partial page.
    std::fs::write(&data, "{\"id\":1}\n").unwrap();
    assert!(cache.read_page("server", "list", &hash, 1).is_err());
    std::fs::write(&data, "{}\n{}\n{}\n{}\n").unwrap();
    assert!(cache.read_page("server", "list", &hash, 1).is_err());
    // Parse failures outside the selected page must still reject the dataset.
    std::fs::write(&data, "{}\ninvalid-json\n{}\n").unwrap();
    assert!(cache.read_page("server", "list", &hash, 1).is_err());
}
