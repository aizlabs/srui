//! Resource CAS conformance tests (§14).

use sha2::{Digest, Sha256};
use srui_resources::{
    chunk_payloads, infer_media_type, PublishOutcome, ResourceError, ResourceLimits, ResourceStore,
    CHUNK_PAYLOAD_SIZE,
};
use srui_semantic_tree::ResourceHash;

fn sha256_hash(bytes: &[u8]) -> ResourceHash {
    let digest = Sha256::digest(bytes);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&digest);
    ResourceHash::new(arr)
}

#[test]
fn publish_matches_known_sha256_vector() {
    // FIPS 180-2 / NIST empty-string vector.
    let empty = b"";
    let expected =
        ResourceHash::from_hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            .unwrap();
    assert_eq!(sha256_hash(empty), expected);

    let mut store = ResourceStore::new();
    let outcome = store.publish_resource(empty).unwrap();
    assert_eq!(outcome.hash, expected);
    assert!(outcome.inserted);
    assert_eq!(outcome.entry.encoded_length, 0);
    assert_eq!(outcome.entry.media_type, "application/octet-stream");
}

#[test]
fn publish_abc_matches_known_vector() {
    let bytes = b"abc";
    let expected =
        ResourceHash::from_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
            .unwrap();
    let mut store = ResourceStore::new();
    let outcome = store.publish_resource(bytes).unwrap();
    assert_eq!(outcome.hash, expected);
    assert!(store.contains(&expected));
    assert_eq!(store.lookup(&expected).unwrap().bytes.as_ref(), bytes);
}

#[test]
fn duplicate_publication_dedupes_without_overwrite() {
    let mut store = ResourceStore::new();
    let first = store.publish_resource(b"duplicate-payload").unwrap();
    assert!(first.inserted);
    assert_eq!(store.len(), 1);
    assert_eq!(store.total_bytes(), b"duplicate-payload".len());

    let second = store.publish_resource(b"duplicate-payload").unwrap();
    assert!(!second.inserted);
    assert_eq!(second.hash, first.hash);
    assert_eq!(store.len(), 1);
    assert_eq!(store.total_bytes(), b"duplicate-payload".len());
}

#[test]
fn missing_lookup_returns_none() {
    let store = ResourceStore::new();
    let missing = sha256_hash(b"not-present");
    assert!(!store.contains(&missing));
    assert!(store.lookup(&missing).is_none());
}

#[test]
fn exact_limit_accepted_over_limit_rejected_before_store() {
    let limits = ResourceLimits {
        max_resource_bytes: 8,
        max_entries: 4,
        max_total_bytes: 32,
    };
    let mut store = ResourceStore::with_limits(limits);

    let exact = vec![0u8; 8];
    let ok = store.publish_resource(&exact).unwrap();
    assert!(ok.inserted);
    assert_eq!(ok.entry.encoded_length, 8);

    let over = vec![0u8; 9];
    match store.publish_resource(&over) {
        Err(ResourceError::ResourceTooLarge { length, limit }) => {
            assert_eq!(length, 9);
            assert_eq!(limit, 8);
        }
        other => panic!("expected ResourceTooLarge, got {other:?}"),
    }
    // Oversized rejection must not have hashed/copied into the store.
    assert_eq!(store.len(), 1);
    assert_eq!(store.total_bytes(), 8);
}

#[test]
fn entry_and_total_byte_limits_evict_oldest() {
    let limits = ResourceLimits {
        max_resource_bytes: 16,
        max_entries: 2,
        max_total_bytes: 20,
    };
    let mut store = ResourceStore::with_limits(limits);
    let first = store.publish_resource(b"one").unwrap().hash;
    store.publish_resource(b"two-bytes!!").unwrap(); // 11 bytes; total 14
    assert_eq!(store.len(), 2);

    // Third distinct entry evicts the oldest rather than permanently failing.
    let third = store.publish_resource(b"three").unwrap();
    assert!(third.inserted);
    assert_eq!(store.len(), 2);
    assert!(!store.contains(&first));
    assert!(store.contains(&third.hash));

    let mut store = ResourceStore::with_limits(limits);
    let oversized_first = store.publish_resource(vec![1u8; 12]).unwrap().hash;
    let second = store.publish_resource(vec![2u8; 12]).unwrap();
    assert!(second.inserted);
    assert_eq!(store.len(), 1);
    assert!(!store.contains(&oversized_first));
    assert!(store.contains(&second.hash));
    assert_eq!(store.total_bytes(), 12);
}

#[test]
fn eviction_skips_protected_hashes() {
    use std::collections::HashSet;

    let limits = ResourceLimits {
        max_resource_bytes: 16,
        max_entries: 1,
        max_total_bytes: 32,
    };
    let mut store = ResourceStore::with_limits(limits);
    let first = store.publish_resource(b"one").unwrap().hash;
    let mut protected = HashSet::new();
    protected.insert(first);

    match store.publish_resource_protecting(b"two", &protected) {
        Err(ResourceError::EntryLimitExceeded { limit }) => assert_eq!(limit, 1),
        other => panic!("expected EntryLimitExceeded while protected, got {other:?}"),
    }
    assert!(store.contains(&first));
    assert_eq!(store.len(), 1);
}

#[test]
fn failed_protected_publish_does_not_partially_evict() {
    use std::collections::HashSet;

    let limits = ResourceLimits {
        max_resource_bytes: 8,
        max_entries: 3,
        max_total_bytes: 10,
    };
    let mut store = ResourceStore::with_limits(limits);
    let oldest = store.publish_resource(b"one").unwrap().hash;
    let locked = store.publish_resource(b"locked").unwrap().hash;
    let protected = HashSet::from([locked]);

    assert!(matches!(
        store.publish_resource_protecting(b"new-new", &protected),
        Err(ResourceError::TotalBytesLimitExceeded { limit: 10 })
    ));
    assert!(store.contains(&oldest));
    assert!(store.contains(&locked));
    assert_eq!(store.total_bytes(), 9);
    assert_eq!(
        store
            .retained_entries()
            .into_iter()
            .map(|entry| entry.hash)
            .collect::<Vec<_>>(),
        vec![oldest, locked]
    );
}

#[test]
fn multi_chunk_reconstruction_matches_source() {
    let len = CHUNK_PAYLOAD_SIZE * 3 + 7;
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let mut store = ResourceStore::new();
    let PublishOutcome { hash, entry, .. } = store.publish_resource(&bytes).unwrap();

    let chunks = chunk_payloads(entry.bytes.clone());
    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|c| c.data.len() <= CHUNK_PAYLOAD_SIZE));

    let mut reconstructed = Vec::with_capacity(len);
    let mut offset = 0u64;
    for chunk in chunks {
        assert_eq!(chunk.byte_offset, offset);
        reconstructed.extend_from_slice(&chunk.data);
        offset += chunk.data.len() as u64;
    }
    assert_eq!(reconstructed, bytes);
    assert_eq!(sha256_hash(&reconstructed), hash);
}

#[test]
fn media_type_inference_for_common_rasters() {
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        v.extend_from_slice(&[0; 8]);
        v
    };
    assert_eq!(infer_media_type(&png), "image/png");

    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];
    assert_eq!(infer_media_type(&jpeg), "image/jpeg");

    let gif = b"GIF89a......";
    assert_eq!(infer_media_type(gif), "image/gif");

    let mut webp = b"RIFF".to_vec();
    webp.extend_from_slice(&[16, 0, 0, 0]);
    webp.extend_from_slice(b"WEBP");
    webp.extend_from_slice(&[0; 4]);
    assert_eq!(infer_media_type(&webp), "image/webp");

    assert_eq!(
        infer_media_type(b"not-an-image"),
        "application/octet-stream"
    );
}

#[test]
fn retained_enumeration_preserves_insertion_order() {
    let mut store = ResourceStore::new();
    let a = store.publish_resource(b"alpha").unwrap().hash;
    let b = store.publish_resource(b"bravo").unwrap().hash;
    let c = store.publish_resource(b"charlie").unwrap().hash;
    let hashes: Vec<_> = store
        .retained_entries()
        .into_iter()
        .map(|e| e.hash)
        .collect();
    assert_eq!(hashes, vec![a, b, c]);
}
