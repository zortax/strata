//! In-memory unpacking of RV composite tarballs (`.tar.bz2`, ~2.4 MB
//! compressed, 25 RADOLAN frames of ~2.6 MB each).
//!
//! The bzip2 stream is decompressed lazily while walking the tar entries,
//! so pulling the `_000` analysis frame (the first member) stops after a
//! fraction of the archive.

use std::io::Read;

use bzip2::read::MultiBzDecoder;
use tar::Archive;

use super::DwdRadarError;

/// Extracts the archive member named `member_name` (path compared by file
/// name — RV tarballs store flat members).
pub(super) fn extract_member(tar_bz2: &[u8], member_name: &str) -> Result<Vec<u8>, DwdRadarError> {
    let mut archive = Archive::new(MultiBzDecoder::new(tar_bz2));
    for entry in archive.entries().map_err(DwdRadarError::Archive)? {
        let mut entry = entry.map_err(DwdRadarError::Archive)?;
        let path = entry.path().map_err(DwdRadarError::Archive)?;
        if path.file_name().is_some_and(|n| n == member_name) {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(DwdRadarError::Archive)?;
            return Ok(bytes);
        }
    }
    Err(DwdRadarError::MemberNotFound {
        name: member_name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Builds an in-memory `.tar.bz2` with the given flat members.
    fn tar_bz2(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extracts_the_named_member() {
        let archive = tar_bz2(&[
            ("DE1200_RV2606101710_000", b"analysis".as_slice()),
            ("DE1200_RV2606101710_005", b"nowcast".as_slice()),
        ]);
        assert_eq!(
            extract_member(&archive, "DE1200_RV2606101710_005").unwrap(),
            b"nowcast"
        );
        assert_eq!(
            extract_member(&archive, "DE1200_RV2606101710_000").unwrap(),
            b"analysis"
        );
    }

    #[test]
    fn missing_member_is_an_error() {
        let archive = tar_bz2(&[("DE1200_RV2606101710_000", b"analysis".as_slice())]);
        assert!(matches!(
            extract_member(&archive, "DE1200_RV2606101710_120"),
            Err(DwdRadarError::MemberNotFound { name }) if name == "DE1200_RV2606101710_120"
        ));
    }

    #[test]
    fn garbage_input_is_an_error() {
        assert!(matches!(
            extract_member(b"not a bzip2 stream", "x"),
            Err(DwdRadarError::Archive(_))
        ));
    }
}
