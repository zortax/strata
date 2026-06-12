//! The minimal [`typst::World`] backing briefing compilation.
//!
//! Fully self-contained: the template and fonts are embedded in the binary
//! (`include_str!`/`include_bytes!`), the input data arrives as JSON via
//! `sys.inputs`, and **no filesystem or network access happens at render
//! time** — any file the template would ask for beyond its own source is
//! answered with `FileError::NotFound`. "Today" is derived from the
//! caller-provided generation timestamp, so rendering stays deterministic.

use chrono::{DateTime, Datelike, Timelike, Utc};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Dict, Value};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt as _, World};

/// The embedded briefing template.
const TEMPLATE: &str = include_str!("../../assets/briefing.typ");

/// Embedded OFL fonts (see `assets/fonts/OFL.txt` for licenses): Noto Sans
/// for body text, JetBrains Mono for raw reports and table numerals.
const FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/NotoSans-Regular.ttf"),
    include_bytes!("../../assets/fonts/NotoSans-Bold.ttf"),
    include_bytes!("../../assets/fonts/NotoSans-Italic.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
];

/// World for one compilation: embedded template + fonts, the serialized
/// input under `sys.inputs.data`, and a fixed "today".
pub(crate) struct BriefWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    source: Source,
    now: DateTime<Utc>,
}

impl BriefWorld {
    /// Builds the world with `data_json` exposed to the template as
    /// `sys.inputs.data` and `now` backing `datetime.today()`.
    pub(crate) fn new(data_json: String, now: DateTime<Utc>) -> Self {
        let mut inputs = Dict::new();
        inputs.insert("data".into(), Value::Str(data_json.into()));
        let library = Library::builder().with_inputs(inputs).build();

        let fonts: Vec<Font> = FONTS
            .iter()
            .flat_map(|data| Font::iter(Bytes::new(*data)))
            .collect();
        debug_assert_eq!(fonts.len(), FONTS.len(), "every embedded font loads");
        let book = FontBook::from_fonts(&fonts);

        let source = Source::new(
            FileId::new(None, VirtualPath::new("/briefing.typ")),
            TEMPLATE.to_owned(),
        );

        Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
            source,
            now,
        }
    }
}

impl World for BriefWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(
                id.vpath().as_rootless_path().to_path_buf(),
            ))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        // No file access at render time: everything the template needs is
        // embedded or passed through `sys.inputs`.
        Err(FileError::NotFound(
            id.vpath().as_rootless_path().to_path_buf(),
        ))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        // Fixed at the caller-provided generation time (determinism);
        // `offset` is the requested UTC offset in hours.
        let with_offset = self.now + chrono::Duration::hours(offset.unwrap_or(0));
        Datetime::from_ymd_hms(
            with_offset.year(),
            with_offset.month() as u8,
            with_offset.day() as u8,
            with_offset.hour() as u8,
            with_offset.minute() as u8,
            with_offset.second() as u8,
        )
    }
}
