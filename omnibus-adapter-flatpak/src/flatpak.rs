//! flatpak CLI parsing: `flatpak list --app --columns=…` TSV tables, mapped to
//! package DTOs. The application ID is the package identity (it round-trips
//! into `pkg_info`); the friendly display name never becomes `name` because
//! `pkg_info` matches IDs exactly. Absent column values arrive as `-` or as
//! empty strings depending on the field — both normalize to `None` (honest
//! absence, never a fabricated empty string).

use omnibus_contracts::dto::packages::PackageBrief;
use omnibus_contracts::dto::packages::PackageSource;

/// The columns the adapter requests, in request order. `description` and
/// `size` come from the appstream/metadata of the installation and may be
/// absent — absence normalizes to `None`.
pub const LIST_COLUMNS: [&str; 6] = [
    "application",
    "name",
    "version",
    "origin",
    "description",
    "size",
];

/// One parsed `flatpak list` row (the raw fields, pre-DTO).
pub struct FlatpakRow {
    pub application: String,
    pub version: Option<String>,
    pub origin: Option<String>,
    pub description: Option<String>,
    pub size_bytes: Option<u64>,
}

/// `-` and empty both mean "no value" in flatpak column output.
fn field(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty() && trimmed != "-").then(|| String::from(trimmed))
}

/// Parses a human size (`12.3 MB`, `512 B`, `1.5 GiB`) into bytes. Decimal
/// prefixes (kB/MB/GB/TB) use powers of 1000, binary prefixes (KiB/MiB/GiB/
/// TiB) powers of 1024 — each prefix is read the way it is spelled.
pub fn parse_size(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
        .unwrap_or(raw.len());
    let (digits, unit) = raw.split_at(split);
    let value: f64 = digits.replace(',', ".").parse().ok()?;
    let factor = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = (value * factor).round();
    // Saturate instead of wrapping: a bogus huge size must not become a tiny
    // byte count through a float roundtrip.
    u64::try_from(bytes as u128).ok()
}

/// Parses a `flatpak list --app --columns=…` TSV table (one package per line,
/// fields in [`LIST_COLUMNS`] order). Non-app rows and malformed lines are
/// skipped; the application ID must be present for the row to be a package.
pub fn parse_list(stdout: &[u8]) -> Vec<FlatpakRow> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                return None;
            }
            let mut fields = line.split('\t');
            let application = field(fields.next()?)?;
            let name = fields.next();
            let version = fields.next();
            let origin = fields.next();
            let description = fields.next();
            let size = fields.next();
            // A row without an ID is not a package; the display name may be
            // absent (appstream metadata missing) without invalidating it.
            (name.is_some()).then(|| FlatpakRow {
                application,
                version: version.and_then(field),
                origin: origin.and_then(field),
                description: description.and_then(field),
                size_bytes: size.and_then(field).and_then(|s| parse_size(&s)),
            })
        })
        .collect()
}

/// DTO mapping: every row is a `flatpak`-sourced package.
pub fn briefs_of(rows: Vec<FlatpakRow>) -> Vec<PackageBrief> {
    rows.into_iter()
        .map(|row| PackageBrief {
            name: row.application.clone(),
            version: row.version.unwrap_or_default(),
            description: row.description,
            repo: row.origin,
            source: PackageSource::Flatpak,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `flatpak list --app
    /// --columns=application,name,version,origin,description,size` table:
    /// every column filled for one app, missing appstream fields (`-`) for
    /// another.
    const TABLE: &str = "org.gnome.Epiphany\tWeb\t46.3\tflathub\tSimple and clean web browser shown in a thumbnail-style window\t256.4 MB\n\
         org.mozilla.firefox\tFirefox\t-\tflathub\t-\t512.0 MB\n\
         \n";

    #[test]
    fn list_table_parses_into_rows() {
        let rows = parse_list(TABLE.as_bytes());
        assert_eq!(rows.len(), 2, "blank lines are skipped");
        assert_eq!(rows[0].application, "org.gnome.Epiphany");
        assert_eq!(rows[0].version.as_deref(), Some("46.3"));
        assert_eq!(rows[0].origin.as_deref(), Some("flathub"));
        assert_eq!(
            rows[0].description.as_deref(),
            Some("Simple and clean web browser shown in a thumbnail-style window")
        );
        assert_eq!(rows[0].size_bytes, Some(256_400_000));
        // The `-` fields normalize to honest absence.
        assert_eq!(rows[1].version, None);
        assert_eq!(rows[1].description, None);
    }

    #[test]
    fn briefs_carry_the_flatpak_source_and_the_id_as_name() {
        let briefs = briefs_of(parse_list(TABLE.as_bytes()));
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].name, "org.gnome.Epiphany");
        assert_eq!(briefs[0].source, PackageSource::Flatpak);
        assert_eq!(briefs[0].repo.as_deref(), Some("flathub"));
    }

    #[test]
    fn sizes_parse_across_prefix_families() {
        assert_eq!(parse_size("512 B"), Some(512));
        assert_eq!(parse_size("12.3 MB"), Some(12_300_000));
        assert_eq!(parse_size("1.5 GiB"), Some(1_610_612_736));
        assert_eq!(parse_size("1,5 MB"), Some(1_500_000));
        assert_eq!(parse_size("-"), None);
        assert_eq!(parse_size("nonsense"), None);
    }

    #[test]
    fn rows_without_an_id_are_not_packages() {
        // A display name alone (no application ID) must not become a package.
        let table = "\tFirefox\t121.0\tflathub\t-\t-\n";
        assert!(parse_list(table.as_bytes()).is_empty());
    }
}
