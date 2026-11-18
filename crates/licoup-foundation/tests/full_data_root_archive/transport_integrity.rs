//! Transport fixtures adopted from the interrupted review's diagnostic probes.
//! Assertions describe the supported contract, not acceptance of the defective input.
use super::*;
use licoup_foundation::core::safe_archive::{default_zip_extraction_limits, extract_zip_safe};

fn crc(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for b in bytes {
        c ^= u32::from(*b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(c & 1));
        }
    }
    !c
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend(x.to_le_bytes());
}
fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend(x.to_le_bytes());
}

// Independent ZIP layout: two local records, optional descriptor, central records, EOCD.
fn raw_zip(body: &[u8], uncompressed: &[u8], method: u16, descriptor: bool, hide: bool) -> Vec<u8> {
    let manifest =
        serde_json::to_vec(&manifest_json("zip", vec![entry_json("d", "directory", 0)])).unwrap();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, plain, compression, dd, hidden, directory) in [
        (
            MANIFEST_MEMBER,
            manifest.as_slice(),
            manifest.as_slice(),
            0u16,
            false,
            false,
            false,
        ),
        (
            "data/d/",
            body,
            uncompressed,
            method,
            descriptor,
            hide,
            true,
        ),
    ] {
        let offset = out.len() as u32;
        let checksum = crc(plain);
        u32le(&mut out, 0x04034b50);
        for x in [20, if dd { 8 } else { 0 }, compression, 0, 0] {
            u16le(&mut out, x);
        }
        for x in [checksum, data.len() as u32, plain.len() as u32] {
            u32le(&mut out, if dd { 0 } else { x });
        }
        u16le(&mut out, name.len() as u16);
        u16le(&mut out, 0);
        out.extend(name.as_bytes());
        out.extend(data);
        if dd {
            u32le(&mut out, 0x08074b50);
            for x in [checksum, data.len() as u32, plain.len() as u32] {
                u32le(&mut out, x);
            }
        }
        u32le(&mut central, 0x02014b50);
        for x in [0x0314, 20, if dd { 8 } else { 0 }, compression, 0, 0] {
            u16le(&mut central, x);
        }
        for x in [checksum, data.len() as u32, plain.len() as u32] {
            u32le(&mut central, if hidden { 0 } else { x });
        }
        for x in [name.len() as u16, 0, 0, 0, 0] {
            u16le(&mut central, x);
        }
        u32le(
            &mut central,
            if directory {
                (0o040755u32 << 16) | 16
            } else {
                0o100600u32 << 16
            },
        );
        u32le(&mut central, offset);
        central.extend(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend(central);
    u32le(&mut out, 0x06054b50);
    for x in [0, 0, 2, 2] {
        u16le(&mut out, x);
    }
    u32le(&mut out, central_size);
    u32le(&mut out, central_offset);
    u16le(&mut out, 0);
    out
}

#[test]
fn zip_local_and_descriptor_bodies_must_match_the_index() {
    let root = scratch("zip-hidden");
    for descriptor in [false, true] {
        let bytes = raw_zip(b"bodybody", b"bodybody", 0, descriptor, true);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let entry = archive.by_index(1).unwrap();
        assert_eq!(entry.unix_mode().unwrap() & 0o170000, 0o040000);
        assert_eq!(
            (entry.size(), entry.compressed_size(), entry.crc32()),
            (0, 0, 0)
        );
        let extracted = root.join(format!("shared-{descriptor}"));
        assert!(extract_zip_safe(&bytes, &extracted, default_zip_extraction_limits()).is_err());
        assert!(!extracted.exists());
        let path = root.join(format!("hidden-{descriptor}.zip"));
        fs::write(&path, &bytes).unwrap();
        let target = root.join(format!("restore-{descriptor}"));
        assert!(
            restore_data_root(&RestoreRequest {
                archive_path: path.clone(),
                target_root: target.clone()
            })
            .is_err()
        );
        assert!(!target.exists());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_stored_and_deflated_directories_with_descriptors_are_supported() {
    let root = scratch("zip-empty");
    for descriptor in [false, true] {
        let stored = raw_zip(&[], &[], 0, descriptor, false);
        extract_zip_safe(
            &stored,
            &root.join(format!("stored-{descriptor}")),
            default_zip_extraction_limits(),
        )
        .unwrap();
        let deflated = raw_zip(&[3, 0], &[], 8, descriptor, false);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&deflated)).unwrap();
        let mut entry = archive.by_index(1).unwrap();
        assert_eq!(std::io::copy(&mut entry, &mut std::io::sink()).unwrap(), 0);
        assert_eq!(
            (entry.size(), entry.compressed_size(), entry.crc32()),
            (0, 2, 0)
        );
        extract_zip_safe(
            &deflated,
            &root.join(format!("deflated-{descriptor}")),
            default_zip_extraction_limits(),
        )
        .expect("empty decoded directory is supported");
        let path = root.join(format!("empty-{descriptor}.zip"));
        fs::write(&path, &deflated).unwrap();
        let target = root.join(format!("restore-{descriptor}"));
        restore_data_root(&RestoreRequest {
            archive_path: path,
            target_root: target.clone(),
        })
        .unwrap();
        assert!(target.join("d").is_dir());
        assert_eq!(fs::read_dir(target.join("d")).unwrap().count(), 0);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_fixture_proves_the_type_and_body_guards_separately() {
    let root = scratch("zip-fixture");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(0o040755);
    writer.start_file("data/d/", options).unwrap();
    writer.write_all(b"body").unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let central = {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.unix_mode().unwrap() & 0o170000, 0o100000);
        entry.central_header_start() as usize
    };
    let error = extract_zip_safe(
        &bytes,
        &root.join("as-written"),
        default_zip_extraction_limits(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "zip_entry_type_unsupported");
    bytes[central + 38..central + 42].copy_from_slice(&((0o040755u32 << 16) | 16).to_le_bytes());
    let error = extract_zip_safe(
        &bytes,
        &root.join("real-directory"),
        default_zip_extraction_limits(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "zip_entry_directory_body_unsupported");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn relative_paths_roundtrip_in_both_containers() {
    let source = scratch("relative-source");
    fs::write(source.join("a"), b"abc").unwrap();
    for extension in ["zip", "tar.gz"] {
        let archive = PathBuf::from(format!(
            "archive-relative-{}.{}",
            std::process::id(),
            extension
        ));
        let target = PathBuf::from(format!(
            "archive-relative-{}-{}",
            std::process::id(),
            extension
        ));
        export(&source, &archive);
        restore(&archive, &target);
        assert_eq!(payload(&source), payload(&target));
        fs::remove_file(archive).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
    fs::remove_dir_all(source).unwrap();
}

#[test]
fn zip_decoder_and_end_records_must_finish_without_unindexed_bytes() {
    let root = scratch("zip-completion");
    let valid = raw_zip(&[3, 0], &[], 8, true, false);
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"trailing");
    let mut descriptor_mismatch = valid.clone();
    let descriptor = descriptor_mismatch
        .windows(4)
        .position(|bytes| bytes == [0x50, 0x4b, 7, 8])
        .unwrap();
    descriptor_mismatch[descriptor + 8] = 3;
    for (index, bytes) in [
        raw_zip(&[3, 0, 0], &[], 8, false, false),
        raw_zip(&[3], &[], 8, false, false),
        trailing,
        descriptor_mismatch,
    ]
    .iter()
    .enumerate()
    {
        assert!(
            extract_zip_safe(
                bytes,
                &root.join(format!("shared-{index}")),
                default_zip_extraction_limits()
            )
            .is_err()
        );
        let path = root.join(format!("incomplete-{index}.zip"));
        fs::write(&path, bytes).unwrap();
        let target = root.join(format!("target-{index}"));
        assert!(
            restore_data_root(&RestoreRequest {
                archive_path: path,
                target_root: target.clone()
            })
            .is_err()
        );
        assert!(!target.exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tar_pax_size_override_reaches_both_owner_passes() {
    let root = scratch("pax-owner");
    let manifest =
        serde_json::to_vec(&manifest_json("tar.gz", vec![entry_json("a", "file", 3)])).unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o600);
    builder
        .append_data(&mut header, MANIFEST_MEMBER, manifest.as_slice())
        .unwrap();
    builder
        .append_pax_extensions([("size", b"3".as_slice())])
        .unwrap();
    let mut header = tar::Header::new_ustar();
    header.set_path("data/a").unwrap();
    header.set_size(0);
    header.set_mode(0o600);
    header.set_cksum();
    builder.append(&header, b"abc".as_slice()).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&builder.into_inner().unwrap()).unwrap();
    let bytes = encoder.finish().unwrap();
    let archive = root.join("pax.tar.gz");
    fs::write(&archive, &bytes).unwrap();
    let target = root.join("restored");
    let result = restore(&archive, &target);
    assert_eq!(result.total_bytes, 3);
    assert_eq!(fs::read(target.join("a")).unwrap(), b"abc");
    assert_eq!(fs::read(&archive).unwrap(), bytes);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn long_portable_paths_roundtrip_through_the_owner() {
    let root = scratch("long-owner");
    let source = root.join("source");
    let path = format!("{}/{}/payload", "a".repeat(150), "b".repeat(150));
    write(&source.join(&path), b"payload");
    for extension in ["zip", "tar.gz"] {
        let archive = root.join(format!("long.{extension}"));
        let target = root.join(format!("target-{extension}"));
        export(&source, &archive);
        restore(&archive, &target);
        assert_eq!(payload(&source), payload(&target));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn zip64_local_sizes_and_end_records_preserve_standard_small_archives() {
    let root = scratch("zip64-transport");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "payload",
            zip::write::SimpleFileOptions::default().large_file(true),
        )
        .unwrap();
    writer.write_all(b"payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    extract_zip_safe(&bytes, &root.join("local"), default_zip_extraction_limits()).unwrap();
    assert_eq!(fs::read(root.join("local/payload")).unwrap(), b"payload");

    let bytes = raw_zip(&[], &[], 0, false, false);
    let end = bytes.len() - 22;
    let central_size = u32::from_le_bytes(bytes[end + 12..end + 16].try_into().unwrap());
    let central_offset = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap());
    let mut extended = bytes[..end].to_vec();
    u32le(&mut extended, 0x06064b50);
    extended.extend(44_u64.to_le_bytes());
    u16le(&mut extended, 45);
    u16le(&mut extended, 45);
    u32le(&mut extended, 0);
    u32le(&mut extended, 0);
    for value in [2, 2, u64::from(central_size), u64::from(central_offset)] {
        extended.extend(value.to_le_bytes());
    }
    u32le(&mut extended, 0x07064b50);
    u32le(&mut extended, 0);
    extended.extend((end as u64).to_le_bytes());
    u32le(&mut extended, 1);
    extended.extend(&bytes[end..]);
    extract_zip_safe(
        &extended,
        &root.join("end"),
        default_zip_extraction_limits(),
    )
    .unwrap();
    assert!(root.join("end/data/d").is_dir());
    fs::remove_dir_all(root).unwrap();
}
