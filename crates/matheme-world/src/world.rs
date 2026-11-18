//! Per-compilation Typst world.

use std::path::PathBuf;
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

/// How `World::today` resolves the current date.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorldDate {
    /// Use the current system clock in UTC (plus any requested hour offset).
    #[default]
    System,
    /// Use a fixed Unix timestamp (seconds since 1970-01-01T00:00:00Z).
    Fixed(i64),
    /// Disable `datetime.today()`.
    None,
}

/// Convert Unix epoch seconds (with optional timezone offset in minutes) into a
/// Typst [`Datetime`].
pub fn datetime_from_unix(unix_seconds: i64, offset_minutes: Option<i32>) -> Option<Datetime> {
    let adjusted = unix_seconds.checked_add(i64::from(offset_minutes.unwrap_or(0)) * 60)?;
    let days = adjusted.div_euclid(86_400);
    let secs_of_day = adjusted.rem_euclid(86_400);

    // Howard Hinnant's civil-from-days algorithm (exact over the Gregorian calendar).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    let hour = (secs_of_day / 3_600) as u8;
    let minute = ((secs_of_day % 3_600) / 60) as u8;
    let second = (secs_of_day % 60) as u8;

    Datetime::from_ymd_hms(year.try_into().ok()?, m as u8, d as u8, hour, minute, second)
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
        let main = FileId::new(RootedPath::new(
            VirtualRoot::Project,
            VirtualPath::new("/main.typ").expect("valid virtual path"),
        ));
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
        let unix_seconds = match self.date {
            WorldDate::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_secs() as i64,
            WorldDate::Fixed(secs) => secs,
            WorldDate::None => return None,
        };
        let offset_secs = offset.map(|d| d.seconds().round() as i64).unwrap_or(0);
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
    }
}
