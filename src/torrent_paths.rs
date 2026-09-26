//! Where a torrent's content is on disk, moving and removing it, and noticing when it is
//! gone.

use std::path::{Component, Path, PathBuf};

use mtorrent::utils::re_exports::mtorrent_base::input::MagnetLink;

/// The file or folder mtorrent writes a torrent's content to: the magnet's name, or the
/// stem of the .torrent file, inside `output_dir`. `None` when that name would not be a
/// single entry of `output_dir`.
pub fn content_path(uri: &str, output_dir: &Path) -> Option<PathBuf> {
    use std::str::FromStr;

    let sanitized = crate::engine::sanitize_magnet_dn(uri);
    if let Ok(magnet) = MagnetLink::from_str(&sanitized) {
        return contained_path(output_dir, magnet.name().unwrap_or("unnamed"));
    }
    Path::new(uri)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| contained_path(output_dir, stem))
}

/// The folder to show for a torrent: the one its content is in, or `output_dir` until that
/// exists.
pub fn folder_to_open(uri: &str, output_dir: &Path) -> PathBuf {
    content_path(uri, output_dir)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| output_dir.to_path_buf())
}

/// The .torrent file of a torrent: the one it was added from, or for a magnet link the one
/// mtorrent saves in `output_dir` once it has fetched the metadata, named like the content.
pub fn metainfo_path(uri: &str, output_dir: &Path) -> Option<PathBuf> {
    use std::str::FromStr;

    let sanitized = crate::engine::sanitize_magnet_dn(uri);
    if let Ok(magnet) = MagnetLink::from_str(&sanitized) {
        let name = magnet.name().unwrap_or("unnamed");
        return contained_path(output_dir, &format!("{name}.torrent"));
    }
    Some(PathBuf::from(uri))
}

/// `output_dir/name`, provided `name` is one plain path component: a name taken from a
/// torrent must not reach outside the download folder.
pub fn contained_path(output_dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\\') || name.contains('\0') {
        return None;
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) => Some(output_dir.join(component)),
        _ => None,
    }
}

/// Moves a torrent's content, and the .torrent file kept beside it, from `old_dir` to
/// `new_dir`. Returns whether anything moved. Either everything moves or nothing does:
/// what is in the way is found before the first move, and a move that fails part of the
/// way through is undone.
pub fn move_content(uri: &str, old_dir: &Path, new_dir: &Path) -> std::io::Result<bool> {
    use std::io::{Error, ErrorKind};

    let pairs = [
        (content_path(uri, old_dir), content_path(uri, new_dir)),
        // For a torrent added as a file this is the file itself, which lives wherever
        // the user keeps it and stays there.
        (metainfo_path(uri, old_dir), metainfo_path(uri, new_dir)),
    ];
    let mut to_move = Vec::new();
    for (from, to) in pairs {
        let (Some(from), Some(to)) = (from, to) else {
            continue;
        };
        if !from.starts_with(old_dir) || !from.exists() {
            continue;
        }
        if to.exists() {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                format!("{} is already there", to.display()),
            ));
        }
        to_move.push((from, to));
    }
    if to_move.is_empty() {
        return Ok(false);
    }

    std::fs::create_dir_all(new_dir)?;
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to) in to_move {
        if let Err(e) = std::fs::rename(&from, &to) {
            for (from, to) in moved {
                // Back where it was, so that Rill and the disk still agree.
                if let Err(e) = std::fs::rename(&to, &from) {
                    log::error!(
                        "Failed to move {} back to {}: {e}",
                        to.display(),
                        from.display()
                    );
                }
            }
            return Err(e);
        }
        moved.push((from, to));
    }
    Ok(true)
}

/// What is left of a torrent whose files were removed, some or all, behind Rill's back.
#[derive(Debug, PartialEq)]
pub struct MissingContent {
    /// The bytes of the files still there.
    pub present_bytes: u64,
    /// The pieces still counted as downloaded.
    pub present_pieces: u64,
}

/// Name of the file mtorrent keeps a torrent's downloaded pieces in, in its content folder.
const PROGRESS_FILE: &str = ".mtorrent";

/// Where a torrent's files are and how its pieces cover them, from its metadata. Reading
/// the metadata can take a while for a torrent of many files; this is worth keeping.
#[derive(Debug)]
pub struct ContentLayout {
    content: PathBuf,
    info_hash: [u8; 20],
    piece_length: usize,
    /// Size and path inside `content` of each file, in torrent order.
    files: Vec<(usize, PathBuf)>,
}

/// The layout of a torrent's content, or `None` while its metadata cannot be read: a magnet
/// link's is not on disk until peers send it.
pub fn content_layout(uri: &str, output_dir: &Path) -> Option<ContentLayout> {
    use mtorrent::utils::re_exports::mtorrent_base::input::Metainfo;

    let content = content_path(uri, output_dir)?;
    let metainfo = Metainfo::from_file(metainfo_path(uri, output_dir)?).ok()?;
    // Laid out as mtorrent lays them out: a single file inside the content folder too.
    let files = match metainfo.files() {
        Some(files) => files.collect(),
        None => vec![(metainfo.length()?, PathBuf::from(metainfo.name()?))],
    };
    Some(ContentLayout {
        content,
        info_hash: *metainfo.info_hash(),
        piece_length: metainfo.piece_length().filter(|&length| length > 0)?,
        files,
    })
}

/// Looks for a torrent's files, and returns what is left when some are gone or cut short.
/// `None` when every file is there, or when there is no telling: without the metadata only
/// a missing content folder says the files are gone.
///
/// mtorrent takes the pieces its progress file lists as downloaded without reading them
/// again. With `forget`, the pieces of the missing files are struck from that file, for the
/// torrent to download them again rather than finish at once with nothing on disk; that
/// must only be done while no run of the torrent can write the file.
pub fn find_missing_content(
    uri: &str,
    output_dir: &Path,
    layout: Option<&ContentLayout>,
    forget: bool,
) -> Option<MissingContent> {
    // An unmounted disk takes the download folder with it, and every torrent in it would
    // look removed; its files may well be back once it is mounted.
    if !output_dir.is_dir() {
        return None;
    }
    let Some(layout) = layout else {
        let content = content_path(uri, output_dir)?;
        return (!content.exists()).then_some(MissingContent {
            present_bytes: 0,
            present_pieces: 0,
        });
    };

    let mut offset = 0usize;
    let mut present_bytes = 0;
    let mut missing = Vec::new();
    for (length, path) in &layout.files {
        // Sizes that add up to more bytes than there are say nothing of the files.
        let end = offset.checked_add(*length)?;
        let there = *length == 0
            || std::fs::metadata(layout.content.join(path))
                .is_ok_and(|meta| meta.is_file() && meta.len() >= *length as u64);
        if there {
            present_bytes += *length as u64;
        } else {
            missing.push(offset..end);
        }
        offset = end;
    }
    if missing.is_empty() {
        return None;
    }
    let present_pieces = forget_pieces(
        &layout.content.join(PROGRESS_FILE),
        &layout.info_hash,
        layout.piece_length,
        &missing,
        forget,
    );
    Some(MissingContent {
        present_bytes,
        present_pieces,
    })
}

/// Clears the pieces that hold any of the `missing` byte ranges from the progress file, or
/// with `write` false only works out the result, and returns how many pieces it still lists.
/// Nothing is listed without a progress file, and the file is left alone when no piece it
/// lists is cleared.
fn forget_pieces(
    progress_file: &Path,
    info_hash: &[u8; 20],
    piece_length: usize,
    missing: &[std::ops::Range<usize>],
    write: bool,
) -> u64 {
    use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;

    let Ok(bytes) = std::fs::read(progress_file) else {
        return 0;
    };
    let Ok(Element::Dictionary(mut root)) = Element::from_bytes(&bytes) else {
        return 0;
    };
    let key = Element::ByteString(info_hash.to_vec());
    let Some(Element::ByteString(mut bitfield)) = root.remove(&key) else {
        return 0;
    };
    let count = |bitfield: &[u8]| bitfield.iter().map(|byte| byte.count_ones() as u64).sum();
    let listed: u64 = count(&bitfield);
    // Piece 0 is the highest bit of the first byte.
    for range in missing.iter().filter(|range| !range.is_empty()) {
        for piece in range.start / piece_length..=(range.end - 1) / piece_length {
            if let Some(byte) = bitfield.get_mut(piece / 8) {
                *byte &= !(0x80 >> (piece % 8));
            }
        }
    }
    let present = count(&bitfield);
    if write && present != listed {
        log::info!(
            "Forgetting {} downloaded pieces of removed files in {}",
            listed - present,
            progress_file.display()
        );
        root.insert(key, Element::ByteString(bitfield));
        if let Err(e) = std::fs::write(progress_file, Element::Dictionary(root).encode()) {
            log::warn!("Failed to update {}: {e}", progress_file.display());
        }
    }
    present
}

/// Removes a torrent's content without following a symbolic link at `path`.
pub fn remove_content(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contained_path_rejects_path_like_names() {
        let output_dir = Path::new("/tmp/rill-downloads");

        assert_eq!(contained_path(output_dir, ""), None);
        assert_eq!(contained_path(output_dir, "."), None);
        assert_eq!(contained_path(output_dir, ".."), None);
        assert_eq!(contained_path(output_dir, "show/season"), None);
        assert_eq!(contained_path(output_dir, "show\\season"), None);
        assert_eq!(
            contained_path(output_dir, "show"),
            Some(output_dir.join("show"))
        );
    }

    #[test]
    fn content_path_uses_the_sanitized_magnet_name() {
        let output_dir = Path::new("/tmp/rill-downloads");
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show%20%2F%20Season%201";

        assert_eq!(
            content_path(uri, output_dir),
            Some(output_dir.join("Show _ Season 1"))
        );
    }

    #[test]
    fn folder_to_open_is_the_content_folder_once_it_exists() {
        let output_dir = std::env::temp_dir().join(format!("rill-open-{}", std::process::id()));
        // A single-file torrent: the file is film.mkv, its folder is named after the
        // .torrent file.
        let uri = "/tmp/source/film.torrent";
        std::fs::create_dir_all(&output_dir).unwrap();
        assert_eq!(folder_to_open(uri, &output_dir), output_dir);

        std::fs::create_dir(output_dir.join("film")).unwrap();
        let folder = folder_to_open(uri, &output_dir);
        std::fs::remove_dir_all(&output_dir).unwrap();
        assert_eq!(folder, output_dir.join("film"));
    }

    #[test]
    fn metainfo_path_of_a_magnet_is_beside_its_content() {
        let output_dir = Path::new("/tmp/rill-downloads");
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show%20%2F%20Season%201";

        assert_eq!(
            metainfo_path(uri, output_dir),
            Some(output_dir.join("Show _ Season 1.torrent"))
        );
        assert_eq!(
            metainfo_path("/tmp/source/Foo.torrent", output_dir),
            Some(PathBuf::from("/tmp/source/Foo.torrent"))
        );
    }

    #[test]
    fn moving_a_torrent_takes_its_content_and_its_saved_metainfo_along() {
        let root = std::env::temp_dir().join(format!("rill-move-{}", std::process::id()));
        let (old_dir, new_dir) = (root.join("old"), root.join("new"));
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show";
        std::fs::create_dir_all(old_dir.join("Show")).unwrap();
        std::fs::write(old_dir.join("Show/episode.mkv"), b"data").unwrap();
        std::fs::write(old_dir.join("Show.torrent"), b"metainfo").unwrap();

        assert!(move_content(uri, &old_dir, &new_dir).unwrap());
        assert!(new_dir.join("Show/episode.mkv").exists());
        assert!(new_dir.join("Show.torrent").exists());
        assert!(!old_dir.join("Show").exists());

        // Nothing to move the second time, and nothing is overwritten.
        assert!(!move_content(uri, &old_dir, &new_dir).unwrap());
        std::fs::create_dir_all(old_dir.join("Show")).unwrap();
        let err = move_content(uri, &old_dir, &new_dir).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(old_dir.join("Show").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_move_that_cannot_finish_leaves_everything_where_it_was() {
        let root = std::env::temp_dir().join(format!("rill-move-part-{}", std::process::id()));
        let (old_dir, new_dir) = (root.join("old"), root.join("new"));
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show";
        std::fs::create_dir_all(old_dir.join("Show")).unwrap();
        std::fs::write(old_dir.join("Show.torrent"), b"metainfo").unwrap();
        // Something of the metainfo's name is already in the new folder, so the content
        // must not be moved either.
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(new_dir.join("Show.torrent"), b"older").unwrap();

        let err = move_content(uri, &old_dir, &new_dir).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(
            old_dir.join("Show").is_dir(),
            "the content was moved anyway"
        );
        assert!(old_dir.join("Show.torrent").exists());
        assert!(!new_dir.join("Show").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn moving_a_torrent_leaves_the_file_it_was_added_from_alone() {
        let root = std::env::temp_dir().join(format!("rill-move-file-{}", std::process::id()));
        let (source, old_dir, new_dir) = (root.join("source"), root.join("old"), root.join("new"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(old_dir.join("film")).unwrap();
        let uri = source.join("film.torrent");
        std::fs::write(&uri, b"metainfo").unwrap();

        let uri = uri.to_string_lossy().into_owned();
        assert!(move_content(&uri, &old_dir, &new_dir).unwrap());
        assert!(new_dir.join("film").is_dir());
        // The .torrent file the user chose stays where the user keeps it.
        assert!(source.join("film.torrent").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A .torrent file `show.torrent` in `dir` for the files named and sized in `files`, in
    /// pieces of `piece_length` bytes.
    fn write_metainfo(dir: &Path, files: &[(&str, i64)], piece_length: i64) -> PathBuf {
        use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;

        let length = files
            .iter()
            .fold(0i64, |sum, (_, length)| sum.saturating_add(*length));
        // The hashes are never checked; a few do for sizes too big to have them all.
        let pieces = (length as usize)
            .div_ceil(piece_length.max(1) as usize)
            .min(1024);
        let files = files
            .iter()
            .map(|(name, length)| {
                Element::Dictionary(
                    [
                        (Element::from("length"), Element::Integer(*length)),
                        (
                            Element::from("path"),
                            Element::List(vec![Element::from(*name)]),
                        ),
                    ]
                    .into(),
                )
            })
            .collect();
        let info = Element::Dictionary(
            [
                (Element::from("files"), Element::List(files)),
                (Element::from("name"), Element::from("show")),
                (
                    Element::from("piece length"),
                    Element::Integer(piece_length),
                ),
                (
                    Element::from("pieces"),
                    Element::ByteString(vec![0; pieces * 20]),
                ),
            ]
            .into(),
        );
        let metainfo = Element::Dictionary([(Element::from("info"), info)].into());
        let path = dir.join("show.torrent");
        std::fs::write(&path, metainfo.encode()).unwrap();
        path
    }

    /// A .torrent file in `dir` for the files named and sized in `files`, in pieces of four
    /// bytes, with a progress file in its content folder counting every piece downloaded.
    fn torrent_with_progress(dir: &Path, files: &[(&str, usize)]) -> String {
        use mtorrent::utils::re_exports::mtorrent_base::input::Metainfo;
        use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;

        let sizes: Vec<(&str, i64)> = files
            .iter()
            .map(|(name, length)| (*name, *length as i64))
            .collect();
        let uri = write_metainfo(dir, &sizes, 4);
        let length: usize = files.iter().map(|(_, length)| length).sum();
        let pieces = length.div_ceil(4);

        let info_hash = *Metainfo::from_file(&uri).unwrap().info_hash();
        let mut bitfield = vec![0u8; pieces.div_ceil(8)];
        for piece in 0..pieces {
            bitfield[piece / 8] |= 0x80 >> (piece % 8);
        }
        let progress = Element::Dictionary(
            [(
                Element::ByteString(info_hash.to_vec()),
                Element::ByteString(bitfield),
            )]
            .into(),
        );
        std::fs::create_dir_all(dir.join("show")).unwrap();
        std::fs::write(dir.join("show").join(PROGRESS_FILE), progress.encode()).unwrap();
        uri.to_string_lossy().into_owned()
    }

    #[test]
    fn removed_files_are_found_and_their_pieces_forgotten() {
        let dir = crate::test_support::ScratchDir::new("missing-content");
        let output_dir = dir.path();
        // Pieces 0 and 1 are the first file's, piece 2 straddles both, 3 and 4 the second's.
        let uri = torrent_with_progress(output_dir, &[("one", 10), ("two", 10)]);
        std::fs::write(output_dir.join("show/one"), [0; 10]).unwrap();
        std::fs::write(output_dir.join("show/two"), [0; 10]).unwrap();
        let layout = content_layout(&uri, output_dir);
        let check = |forget| find_missing_content(&uri, output_dir, layout.as_ref(), forget);
        assert_eq!(check(true), None);

        std::fs::remove_file(output_dir.join("show/two")).unwrap();
        let missing = MissingContent {
            present_bytes: 10,
            present_pieces: 2,
        };
        // Only looked at, the progress file keeps every piece.
        assert_eq!(check(false), Some(missing));
        let progress = std::fs::read(output_dir.join("show").join(PROGRESS_FILE)).unwrap();
        assert!(progress.ends_with(&[0xf8, b'e']), "{progress:?}");
        // Struck from it for good: nothing more to forget the second time.
        assert_eq!(check(true).map(|m| m.present_pieces), Some(2));
        assert_eq!(check(false).map(|m| m.present_pieces), Some(2));

        // A file cut short is as good as gone.
        std::fs::write(output_dir.join("show/one"), [0; 3]).unwrap();
        assert_eq!(
            check(true),
            Some(MissingContent {
                present_bytes: 0,
                present_pieces: 0
            })
        );
    }

    #[test]
    fn a_torrent_whose_files_were_removed_downloads_them_again_rather_than_finish() {
        use crate::test_support::{Harness, TestTorrent};
        use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;

        let h = Harness::new("missing-resume", 0);
        let output_dir = h.output_dir();
        let torrent = TestTorrent::create(h.dir.path(), "gone", 64 * 1024, 16 * 1024);
        // Downloaded once, all four pieces, and the file removed since.
        std::fs::create_dir_all(output_dir.join("gone")).unwrap();
        let progress = Element::Dictionary(
            [(
                Element::ByteString(torrent.info_hash.to_vec()),
                Element::ByteString(vec![0xf0]),
            )]
            .into(),
        );
        std::fs::write(
            output_dir.join("gone").join(PROGRESS_FILE),
            progress.encode(),
        )
        .unwrap();

        // Left as it was, the progress file would have the torrent report every byte: the
        // engine corrects it before the run.
        let uri = torrent.metainfo_path.to_string_lossy().into_owned();
        let hash = torrent.hex_hash();
        h.engine.start(
            hash.clone(),
            String::new(),
            uri,
            output_dir,
            false,
            h.tx.clone(),
        );
        let update = h.wait_for_update(&hash, std::time::Duration::from_secs(20), |u| u.total > 0);
        assert_eq!(update.downloaded, 0);
    }

    #[test]
    fn a_torrent_whose_sizes_make_no_sense_is_not_looked_into() {
        let dir = crate::test_support::ScratchDir::new("impossible-sizes");
        let output_dir = dir.path();
        std::fs::create_dir_all(output_dir.join("show")).unwrap();

        let uri = write_metainfo(output_dir, &[("one", 10)], 0);
        let uri = uri.to_string_lossy();
        assert!(content_layout(&uri, output_dir).is_none());

        let uri = write_metainfo(
            output_dir,
            &[("one", i64::MAX), ("two", i64::MAX), ("three", i64::MAX)],
            4,
        );
        let uri = uri.to_string_lossy();
        let layout = content_layout(&uri, output_dir);
        assert_eq!(
            find_missing_content(&uri, output_dir, layout.as_ref(), false),
            None
        );
    }

    #[test]
    fn without_metadata_only_a_missing_content_folder_counts() {
        let dir = crate::test_support::ScratchDir::new("missing-magnet");
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=Show";
        std::fs::create_dir_all(dir.path().join("Show")).unwrap();
        assert!(content_layout(uri, dir.path()).is_none());
        assert_eq!(find_missing_content(uri, dir.path(), None, false), None);

        std::fs::remove_dir(dir.path().join("Show")).unwrap();
        assert_eq!(
            find_missing_content(uri, dir.path(), None, false),
            Some(MissingContent {
                present_bytes: 0,
                present_pieces: 0
            })
        );
        // Nor does anything count when the download folder itself is gone.
        let unmounted = dir.path().join("unmounted");
        assert_eq!(find_missing_content(uri, &unmounted, None, false), None);
    }

    #[test]
    fn content_path_uses_the_torrent_file_stem() {
        let output_dir = Path::new("/tmp/rill-downloads");

        assert_eq!(
            content_path("/tmp/source/Foo.torrent", output_dir),
            Some(output_dir.join("Foo"))
        );
    }
}
