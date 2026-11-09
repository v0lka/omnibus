//! snap CLI parsing: `snap list` TSV tables, mapped to package DTOs. The snap
//! name is the package identity (it round-trips into `pkg_info`). `snap list`
//! prints a leading header row (`Name\tVersion\tRev\t…`) — skipped, never a
//! package. The store carries no origin column (every snap comes from one
//! store) and no description or size: those stay honest `None`s instead of a
//! second, unbounded probing round per package.

use omnibus_contracts::dto::packages::PackageBrief;
use omnibus_contracts::dto::packages::PackageSource;

/// One parsed `snap list` row.
pub struct SnapRow {
    pub name: String,
    pub version: String,
}

/// Parses a `snap list` table: the first line is the header (skipped), every
/// further line yields `(name, version)` when both fields are present.
pub fn parse_list(stdout: &[u8]) -> Vec<SnapRow> {
    let mut rows = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines().skip(1) {
        let mut fields = line.split('\t');
        let Some(name) = fields
            .next()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from)
        else {
            continue;
        };
        let Some(version) = fields
            .next()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
        else {
            continue;
        };
        rows.push(SnapRow { name, version });
    }
    rows
}

/// DTO mapping: every row is a `snap`-sourced package.
pub fn briefs_of(rows: Vec<SnapRow>) -> Vec<PackageBrief> {
    rows.into_iter()
        .map(|row| PackageBrief {
            name: row.name,
            version: row.version,
            description: None,
            repo: None,
            source: PackageSource::Snap,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `snap list` table (tab-separated): header plus two snaps,
    /// one carrying the `-` notes placeholder and a verified-publisher glyph
    /// (both ignored — only Name and Version are read).
    const TABLE: &str = "Name\tVersion\tRev\tTracking\tPublisher\tNotes\n\
         core22\t20231115\t1380\tlatest/stable\tcanonical✓\tbase\n\
         firefox\t121.0.1\t2340\tlatest/stable\tmozilla✓\t-\n";

    #[test]
    fn header_is_skipped_and_rows_parse() {
        let rows = parse_list(TABLE.as_bytes());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "core22");
        assert_eq!(rows[0].version, "20231115");
        assert_eq!(rows[1].name, "firefox");
        assert_eq!(rows[1].version, "121.0.1");
    }

    #[test]
    fn briefs_carry_the_snap_source() {
        let briefs = briefs_of(parse_list(TABLE.as_bytes()));
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].source, PackageSource::Snap);
        assert_eq!(briefs[0].name, "core22");
        // The store publishes no origin column; absence stays honest.
        assert_eq!(briefs[0].repo, None);
        assert_eq!(briefs[0].description, None);
    }

    #[test]
    fn rows_without_name_or_version_are_skipped() {
        let table = "Name\tVersion\tRev\n\n\tbroken\nok\t1.0\n";
        let rows = parse_list(table.as_bytes());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "ok");
    }
}
