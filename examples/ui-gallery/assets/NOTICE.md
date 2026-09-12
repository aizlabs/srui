# Gallery image provenance

`gallery.png` is the only binary asset in this example. It exists so the gallery
exercises the real resource path (§14, §19.2): publish → chunked transfer →
hash validation → client CAS → decode → in-place `NSImageView` refresh.

## Source

| Field | Value |
| --- | --- |
| Title | *The Earth seen from Apollo 17* ("The Blue Marble") |
| Author | NASA / Apollo 17 crew — Harrison Schmitt or Ron Evans |
| NASA identifier | AS17-148-22727 |
| Date | 1972-12-07 |
| Source file | <https://commons.wikimedia.org/wiki/File:The_Earth_seen_from_Apollo_17.jpg> |
| Original | <https://upload.wikimedia.org/wikipedia/commons/9/97/The_Earth_seen_from_Apollo_17.jpg> (3000×3002 JPEG) |
| License | Public domain — a work of NASA, created by a U.S. federal government employee in the course of their duties (17 U.S.C. §105). Wikimedia tag: `PD-USGov-NASA`. |

NASA's media usage guidelines additionally state that NASA content is generally
not copyrighted and may be used for any purpose without requesting permission:
<https://www.nasa.gov/nasa-brand-center/images-and-media/>.

## Derivation

The committed file is a derivative of the original, produced with macOS `sips`:

```bash
curl -o apollo17.jpg \
  https://upload.wikimedia.org/wikipedia/commons/9/97/The_Earth_seen_from_Apollo_17.jpg
sips -Z 384 -s format png apollo17.jpg --out assets/gallery.png
```

| Field | Value |
| --- | --- |
| Dimensions | 383 × 384 |
| Encoding | PNG, 8-bit/color RGB, non-interlaced |
| Size | 259,617 bytes |
| SHA-256 | `84330d74a9d30b26c3ca49428ba02bf04711f81d8d0057583f34529021691175` |
| Chunks on the wire | 16 (`ceil(259617 / 16384)`, `CHUNK_PAYLOAD_SIZE` = 16 KiB) |

A derivative of a public-domain work is itself unrestricted; no additional
licence applies to the resized PNG.

## Why not Lena

The original brief named `lena.png`. The classic "Lenna" test image is a 1972
*Playboy* centrefold crop and has never been released under terms that permit
redistribution; IEEE stopped accepting it in 2024 and the original photographic
subject has asked that it be retired. Committing it to a public repository would
be a copyright problem and a poor signal, so the gallery uses a genuine
public-domain photograph instead. Everything else about the workflow — publish,
chunk, validate, cache, decode, refresh — is unchanged.
