//! Per-compilation Typst world.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use typst::diag::FileResult;
use typst::foundations::{Bytes, Datetime, Dict, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Features, Library, LibraryExt, World};
use typst_kit::diagnostics::DiagnosticWorld;
use typst_kit::files::FileStore;

use crate::fonts::{font_cache, FontCache};
use crate::loader::MathemeLoader;

/// Name shown for the in-memory source string passed from Wolfram.
pub const MAIN_SOURCE_NAME: &str = "<typst source>";

// Reuse a single virtual ID. Looking up /main.typ by path still refers to disk.
static MAIN_FILE: LazyLock<FileId> = LazyLock::new(|| {
    FileId::unique(RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new("/main.typ").expect("valid virtual path"),
    ))
});

/// How `World::today` resolves the current date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorldDate {
    /// Use the current system clock in UTC (plus any requested hour offset).
    #[default]
    System,
    /// Use a fixed Unix timestamp (seconds since 1970-01-01T00:00:00Z).
    Fixed(i64),
    /// Use a fixed Unix timestamp and its local offset in minutes.
    FixedLocal {
        unix_seconds: i64,
        offset_minutes: i32,
    },
    /// Disable `datetime.today()`.
    None,
}

/// Convert Unix epoch seconds (with optional timezone offset in minutes) into a
/// Typst [`Datetime`].
pub fn datetime_from_unix(unix_seconds: i64, offset_minutes: Option<i32>) -> Option<Datetime> {
    let utc = time::OffsetDateTime::from_unix_timestamp(unix_seconds).ok()?;
    let dt = match offset_minutes {
        Some(mins) => {
            if mins.unsigned_abs() >= 24 * 60 {
                return None;
            }
            let offset_secs = mins.checked_mul(60)?;
            let offset = time::UtcOffset::from_whole_seconds(offset_secs).ok()?;
            utc.checked_to_offset(offset)?
        }
        None => utc,
    };

    Datetime::from_ymd_hms(
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    )
}

/// A per-call World: source, inputs and the file cache live only for this
/// compilation; fonts and the package loader are shared process-wide.
pub struct MathemeWorld {
    library: LazyHash<Library>,
    main: FileId,
    files: FileStore<MathemeLoader>,
    fonts: std::sync::Arc<FontCache>,
    date: WorldDate,
}

impl MathemeWorld {
    pub fn new(source_text: String, inputs: Dict, base_path: PathBuf) -> Self {
        Self::with_options(
            source_text,
            inputs,
            base_path,
            Features::default(),
            WorldDate::System,
        )
    }

    pub fn with_options(
        source_text: String,
        inputs: Dict,
        base_path: PathBuf,
        features: Features,
        date: WorldDate,
    ) -> Self {
        let main = *MAIN_FILE;
        let loader = MathemeLoader::new(main, source_text, base_path);

        Self {
            library: LazyHash::new(
                Library::builder()
                    .with_inputs(inputs)
                    .with_features(features)
                    .build(),
            ),
            main,
            files: FileStore::new(loader),
            fonts: font_cache(),
            date,
        }
    }
}

impl World for MathemeWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.files.source(id)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.file(id)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        let (unix_seconds, default_offset) = match self.date {
            WorldDate::System => (
                SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64,
                0,
            ),
            WorldDate::Fixed(seconds) => (seconds, 0),
            WorldDate::FixedLocal {
                unix_seconds,
                offset_minutes,
            } => (unix_seconds, i64::from(offset_minutes) * 60),
            WorldDate::None => return None,
        };
        let offset_secs = offset
            .map(|d| d.seconds().round() as i64)
            .unwrap_or(default_offset);
        datetime_from_unix(unix_seconds.checked_add(offset_secs)?, None)
    }
}

/// File naming for diagnostics, following `typst-cli`'s strategy:
/// project files are relative to the project root, packages are shown as
/// `@namespace/name:version/path`, and the in-memory main file gets a stable
/// synthetic name instead of a meaningless virtual path.
impl DiagnosticWorld for MathemeWorld {
    fn name(&self, id: FileId) -> String {
        if id == self.main {
            return MAIN_SOURCE_NAME.into();
        }

        match id.root() {
            VirtualRoot::Project => self
                .files
                .loader()
                .resolve(id)
                .and_then(|path| {
                    path.strip_prefix(self.files.loader().project.path())
                        .ok()
                        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                })
                .unwrap_or_else(|| id.vpath().get_without_slash().into()),
            VirtualRoot::Package(spec) => format!("{spec}{}", id.vpath().get_with_slash()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_and_modern_dates_convert_accurately() {
        let epoch = datetime_from_unix(0, None).unwrap();
        assert_eq!((epoch.year(), epoch.month(), epoch.day()), (Some(1970), Some(1), Some(1)));

        // 2026-11-18T12:00:00Z = 1795003200
        let sample = datetime_from_unix(1_795_003_200, Some(480)).unwrap();
        assert_eq!(
            (sample.year(), sample.month(), sample.day(), sample.hour()),
            (Some(2026), Some(11), Some(18), Some(20))
        );

        // Pre-epoch date: 1969-12-31T23:59:59Z
        let before_epoch = datetime_from_unix(-1, None).unwrap();
        assert_eq!(
            (
                before_epoch.year(),
                before_epoch.month(),
                before_epoch.day(),
                before_epoch.hour(),
                before_epoch.minute(),
                before_epoch.second(),
            ),
            (Some(1969), Some(12), Some(31), Some(23), Some(59), Some(59))
        );

        // Out-of-range offset (> 24h) is rejected cleanly.
        assert!(datetime_from_unix(0, Some(25 * 60)).is_none());
    }
}
