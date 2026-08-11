//! Calculation history: an in-memory model plus JSON persistence to the XDG
//! data directory.
//!
//! Entries are stored newest-last in a `Vec` and capped at [`MAX_ENTRIES`]. The
//! file lives at `<XDG_DATA_HOME or ~/.local/share>/Calculator/history.json`,
//! located with `directories::ProjectDirs::from("io", "matv", "Calculator")` so
//! it tracks the app id. A missing or corrupt file yields an empty history
//! rather than an error — history is convenience data, never load-bearing.
//!
//! Day grouping for the UI ("Today" / "Yesterday" / "Month D, YYYY") is computed
//! from the unix timestamp with a small hand-rolled civil-date function, so no
//! date-library dependency is pulled in.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Maximum number of entries retained on disk / in memory (most recent kept).
pub const MAX_ENTRIES: usize = 200;

/// One completed calculation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The expression exactly as shown to the user (pretty glyphs).
    pub expression: String,
    /// The formatted result string.
    pub result: String,
    /// Unix seconds when the calculation was committed (via [`equals`]).
    ///
    /// [`equals`]: crate::state::Calculator::equals
    pub timestamp: u64,
}

impl HistoryEntry {
    /// Build an entry stamped with the current wall-clock time.
    pub fn new(expression: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            expression: expression.into(),
            result: result.into(),
            timestamp: now_unix(),
        }
    }
}

/// The history collection. Persisted as a flat JSON array of [`HistoryEntry`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    entries: Vec<HistoryEntry>,
}

impl History {
    /// Load history from disk, or return an empty history if the file is
    /// missing or corrupt.
    pub fn load() -> History {
        let Some(path) = history_path() else {
            return History::default();
        };
        let Ok(bytes) = fs::read(&path) else {
            return History::default();
        };
        match serde_json::from_slice::<Vec<HistoryEntry>>(&bytes) {
            Ok(mut entries) => {
                // Enforce the cap even if an old file grew larger.
                if entries.len() > MAX_ENTRIES {
                    let excess = entries.len() - MAX_ENTRIES;
                    entries.drain(0..excess);
                }
                History { entries }
            }
            Err(_) => History::default(),
        }
    }

    /// Persist history to disk. Best-effort: errors (no home dir, read-only FS)
    /// are swallowed since history is non-critical.
    pub fn save(&self) {
        let Some(path) = history_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.entries) {
            let _ = fs::write(&path, json);
        }
    }

    /// Append an entry, evicting the oldest if over [`MAX_ENTRIES`].
    pub fn push(&mut self, entry: HistoryEntry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..excess);
        }
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remove the entry at `index` (no-op if out of bounds). The caller is
    /// responsible for calling [`save`](Self::save) afterward to persist.
    #[allow(dead_code)]
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
    }

    /// Remove the first entry equal to `entry` (by value), returning whether
    /// one was removed. Deleting by identity (not positional index) stays
    /// correct as rows are removed one at a time in the UI. The caller is
    /// responsible for calling [`save`](Self::save) afterward to persist.
    pub fn remove_entry(&mut self, entry: &HistoryEntry) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e == entry) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The on-disk path, or `None` if no data directory can be determined.
fn history_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("io", "matv", "Calculator")?;
    Some(dirs.data_dir().join("history.json"))
}

/// Current time in unix seconds (0 if the clock is before the epoch).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A civil (Gregorian) date, `year`/`month`/`day` with `month` in `1..=12`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilDate {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// Convert unix seconds to a UTC civil date.
///
/// Uses Howard Hinnant's well-known `days_from_civil` inverse
/// (`civil_from_days`). UTC is fine for grouping labels in a calculator; we do
/// not attempt local-timezone conversion here.
pub fn civil_from_unix(secs: u64) -> CivilDate {
    let days = (secs / 86_400) as i64;
    // Shift epoch to 0000-03-01 for the algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    CivilDate {
        year,
        month: m as u32,
        day: d as u32,
    }
}

/// A human day label for grouping in the history list.
///
/// `now` is passed in (rather than read from the clock) so the labelling is
/// testable and deterministic. Returns "Today", "Yesterday", or
/// "Month D, YYYY" (e.g. "August 8, 2026").
pub fn day_label(entry_ts: u64, now_ts: u64) -> String {
    let today = civil_from_unix(now_ts);
    let entry = civil_from_unix(entry_ts);
    if entry == today {
        return "Today".to_string();
    }
    // Yesterday = today's day-number minus one day.
    let yesterday = civil_from_unix(now_ts.saturating_sub(86_400));
    if entry == yesterday {
        return "Yesterday".to_string();
    }
    format!("{} {}, {}", month_name(entry.month), entry.day, entry.year)
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_cap() {
        let mut h = History::default();
        for i in 0..(MAX_ENTRIES + 50) {
            h.push(HistoryEntry {
                expression: format!("{i}+1"),
                result: format!("{}", i + 1),
                timestamp: i as u64,
            });
        }
        assert_eq!(h.entries().len(), MAX_ENTRIES);
        // The oldest 50 were evicted; the first retained entry is #50.
        assert_eq!(h.entries()[0].expression, "50+1");
    }

    #[test]
    fn clear_empties() {
        let mut h = History::default();
        h.push(HistoryEntry::new("1+1", "2"));
        assert!(!h.is_empty());
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn remove_by_index() {
        let mut h = History::default();
        h.push(HistoryEntry::new("a", "1"));
        h.push(HistoryEntry::new("b", "2"));
        h.push(HistoryEntry::new("c", "3"));
        h.remove(1);
        assert_eq!(h.entries().len(), 2);
        assert_eq!(h.entries()[0].expression, "a");
        assert_eq!(h.entries()[1].expression, "c");
        // Out-of-bounds is a no-op (no panic).
        h.remove(99);
        assert_eq!(h.entries().len(), 2);
    }

    #[test]
    fn remove_entry_by_identity() {
        let mut h = History::default();
        let a = HistoryEntry::new("a", "1");
        let b = HistoryEntry::new("b", "2");
        let c = HistoryEntry::new("c", "3");
        h.push(a.clone());
        h.push(b.clone());
        h.push(c.clone());
        // Remove B then A — order-independent because it's by value.
        assert!(h.remove_entry(&b));
        assert!(h.remove_entry(&a));
        assert_eq!(h.entries().len(), 1);
        assert_eq!(h.entries()[0].expression, "c");
        // Removing something absent is a no-op returning false.
        assert!(!h.remove_entry(&b));
        assert_eq!(h.entries().len(), 1);
    }

    #[test]
    fn corrupt_json_yields_empty() {
        // Directly exercise the parse path the loader uses.
        let parsed = serde_json::from_slice::<Vec<HistoryEntry>>(b"not json{");
        assert!(parsed.is_err());
    }

    #[test]
    fn roundtrip_serde() {
        let e = HistoryEntry {
            expression: "2\u{00D7}3".to_string(),
            result: "6".to_string(),
            timestamp: 1_700_000_000,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn civil_date_epoch() {
        // 1970-01-01
        let d = civil_from_unix(0);
        assert_eq!(
            d,
            CivilDate {
                year: 1970,
                month: 1,
                day: 1
            }
        );
    }

    #[test]
    fn civil_date_known() {
        // 1_700_000_000 = 2023-11-14 22:13:20 UTC
        let d = civil_from_unix(1_700_000_000);
        assert_eq!(d.year, 2023);
        assert_eq!(d.month, 11);
        assert_eq!(d.day, 14);
    }

    #[test]
    fn day_labels() {
        let now = 1_700_000_000u64; // 2023-11-14
        assert_eq!(day_label(now, now), "Today");
        assert_eq!(day_label(now - 86_400, now), "Yesterday");
        // Two days earlier → a dated label.
        let label = day_label(now - 2 * 86_400, now);
        assert!(label.contains("2023"), "got {label}");
        assert!(label.contains("November"), "got {label}");
    }
}
