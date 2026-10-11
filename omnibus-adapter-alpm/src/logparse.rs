//! Pure pacman.log parsing and cache-file matching — no ALPM handle involved, so
//! these functions are unit-testable on any machine (roadmap 1.6.3: `pkg_history`
//! is the basis of the cache-downgrade undo).

use omnibus_contracts::dto::packages::{PackageAction, PackageHistoryEntry};

/// Parses one `[ALPM]` transaction line into a history entry; `None` for any
/// other line (`[PACMAN]`, `[ALPM-SCRIPTLET]`, blank, ...).
///
/// Shapes (all timestamped `[YYYY-MM-DDThh:mm:ss+ZZZZ] [ALPM]`):
/// - `installed name version`
/// - `upgraded name (old -> new)`
/// - `downgraded name (old -> new)`
/// - `reinstalled name (old -> new)`
/// - `removed name (old)`
pub fn parse_line(line: &str) -> Option<PackageHistoryEntry> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let ts = &rest[..close];
    let body = rest[close + 1..].trim_start();
    let body = body.strip_prefix("[ALPM]")?.trim_start();
    let (word, tail) = body.split_once(' ')?;
    let action = match word {
        "installed" => PackageAction::Install,
        "upgraded" => PackageAction::Upgrade,
        "downgraded" => PackageAction::Downgrade,
        "reinstalled" => PackageAction::Reinstall,
        "removed" => PackageAction::Remove,
        _ => return None,
    };
    let (name, from_version, to_version) = if matches!(action, PackageAction::Install) {
        let (name, version) = tail.split_once(' ')?;
        (name, None, Some(version))
    } else if matches!(action, PackageAction::Remove) {
        let (name, version) = tail.split_once(' ')?;
        let version = version.strip_prefix('(')?.strip_suffix(')')?;
        (name, Some(version), None)
    } else {
        let (name, pair) = tail.split_once(' ')?;
        let pair = pair.strip_prefix('(')?.strip_suffix(')')?;
        let (old, new) = pair.split_once(" -> ")?;
        (name, Some(old), Some(new))
    };
    Some(PackageHistoryEntry {
        ts_epoch_ms: parse_log_timestamp(ts)?,
        action,
        name: String::from(name),
        from_version: from_version.map(String::from),
        to_version: to_version.map(String::from),
    })
}

/// Parses `YYYY-MM-DDThh:mm:ss+ZZZZ` (the pacman log timestamp) to epoch ms.
pub fn parse_log_timestamp(ts: &str) -> Option<i64> {
    let bytes = ts.as_bytes();
    if bytes.len() != 24 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| ts.get(range)?.parse::<i64>().ok();
    let year = num(0..4)?;
    let month = num(5..7)?;
    let day = num(8..10)?;
    let hour = num(11..13)?;
    let minute = num(14..16)?;
    let second = num(17..19)?;
    let sign = match bytes[19] {
        b'+' => 1_i64,
        b'-' => -1_i64,
        _ => return None,
    };
    let off_h = num(20..22)?;
    let off_m = num(22..24)?;
    // Honest None for impossible civil values (month 13, day 32, ...).
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || off_m > 59
    {
        return None;
    }
    let offset = sign * (off_h * 3600 + off_m * 60);
    let secs = days_from_civil(year, month as u32, day as u32) * 86400
        + hour * 3600
        + minute * 60
        + second;
    Some((secs - offset) * 1000)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whether a file name is a cached package file for exactly `name`-`version`.
/// Cache layout: `{name}-{version}-{arch}.pkg.tar.{zst|xz|gz|bz2|lz4}`.
pub fn is_cached_pkg_file(file_name: &str, name: &str, version: &str) -> bool {
    if !file_name.starts_with(&format!("{name}-{version}-")) {
        return false;
    }
    match file_name.find(".pkg.tar.") {
        Some(idx) => {
            matches!(&file_name[idx + 9..], "zst" | "xz" | "gz" | "bz2" | "lz4")
        }
        None => false,
    }
}

/// Picks the best cached file for a downgrade: x86_64 beats `any` beats the rest;
/// ties resolve to the lexicographically largest name (deterministic).
pub fn pick_cached_file<'a>(files: &[&'a str], name: &str, version: &str) -> Option<&'a str> {
    let rank = |file: &&'a str| -> i32 {
        if file.contains("-x86_64.") {
            3
        } else if file.contains("-any.") {
            2
        } else {
            1
        }
    };
    files
        .iter()
        .copied()
        .filter(|file| is_cached_pkg_file(file, name, version))
        .max_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)))
}

/// The `(current, previous)` version pair implied by one package's chronological
/// history. `None` when there is no previous distinct version to undo to.
pub fn previous_version(entries: &[PackageHistoryEntry]) -> Option<(String, String)> {
    // Chronological version states; `None` means the package was removed.
    let mut states: Vec<Option<String>> = Vec::new();
    for entry in entries {
        let next = entry.to_version.clone();
        if states.last() != Some(&next) {
            states.push(next);
        }
    }
    let current = states.last().cloned().flatten()?;
    let previous = states
        .iter()
        .rev()
        .skip(1)
        .find(|state| state.as_deref().is_some_and(|v| v != current))
        .cloned()
        .flatten()?;
    Some((current, previous))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_convert_to_epoch_ms() {
        assert_eq!(
            parse_log_timestamp("2025-01-01T00:00:00+0000"),
            Some(1_735_689_600_000)
        );
        assert_eq!(
            parse_log_timestamp("2025-01-01T03:00:00+0300"),
            Some(1_735_689_600_000)
        );
        assert_eq!(parse_log_timestamp("1970-01-01T00:00:00+0000"), Some(0));
        assert_eq!(parse_log_timestamp("not-a-date"), None);
        assert_eq!(parse_log_timestamp("2025-13-01T00:00:00+0000"), None);
    }

    #[test]
    fn alpm_transaction_lines_parse() {
        let upgraded =
            parse_line("[2025-06-01T12:00:00+0300] [ALPM] upgraded bash (5.2.037-1 -> 5.2.040-1)")
                .expect("upgraded line must parse");
        assert_eq!(upgraded.name, "bash");
        assert_eq!(upgraded.action, PackageAction::Upgrade);
        assert_eq!(upgraded.from_version.as_deref(), Some("5.2.037-1"));
        assert_eq!(upgraded.to_version.as_deref(), Some("5.2.040-1"));

        let installed = parse_line("[2025-06-01T12:00:00+0000] [ALPM] installed ripgrep 14.1.1-1")
            .expect("installed line must parse");
        assert_eq!(installed.action, PackageAction::Install);
        assert_eq!(installed.from_version, None);
        assert_eq!(installed.to_version.as_deref(), Some("14.1.1-1"));

        let removed = parse_line("[2025-06-02T09:30:00+0000] [ALPM] removed ripgrep (14.1.1-1)")
            .expect("removed line must parse");
        assert_eq!(removed.action, PackageAction::Remove);
        assert_eq!(removed.to_version, None);

        let downgraded =
            parse_line("[2025-06-03T08:00:00+0000] [ALPM] downgraded linux (6.12.9-1 -> 6.12.8-1)")
                .expect("downgraded line must parse");
        assert_eq!(downgraded.action, PackageAction::Downgrade);

        let reinstalled =
            parse_line("[2025-06-04T08:00:00+0000] [ALPM] reinstalled htop (3.3.0-1 -> 3.3.0-1)")
                .expect("reinstalled line must parse");
        assert_eq!(reinstalled.action, PackageAction::Reinstall);
    }

    #[test]
    fn non_transaction_lines_are_ignored() {
        assert_eq!(
            parse_line("[2025-06-01T12:00:00+0000] [PACMAN] running 'pacman -S bash'"),
            None
        );
        assert_eq!(
            parse_line("[2025-06-01T12:00:00+0000] [ALPM-SCRIPTLET] warning: ..."),
            None
        );
        assert_eq!(parse_line("garbage"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn cache_file_matching_is_name_version_exact() {
        assert!(is_cached_pkg_file(
            "bash-5.2.037-1-x86_64.pkg.tar.zst",
            "bash",
            "5.2.037-1"
        ));
        assert!(is_cached_pkg_file(
            "backup-5.2-1-any.pkg.tar.xz",
            "backup",
            "5.2-1"
        ));
        assert!(!is_cached_pkg_file(
            "bash-5.2.040-1-x86_64.pkg.tar.zst",
            "bash",
            "5.2.037-1"
        ));
        assert!(!is_cached_pkg_file(
            "bash-5.2.037-1-x86_64.pkg.tar.zst",
            "b",
            "5.2.037-1"
        ));
        assert!(!is_cached_pkg_file(
            "bash-5.2.037-1-x86_64.pkg.tar.zst.tmp",
            "bash",
            "5.2.037-1"
        ));
    }

    #[test]
    fn cache_file_pick_prefers_x86_64_over_any() {
        let files = [
            "bash-5.2.037-1-any.pkg.tar.zst",
            "bash-5.2.037-1-x86_64.pkg.tar.zst",
            "bash-5.2.037-1-armv7h.pkg.tar.xz",
        ];
        assert_eq!(
            pick_cached_file(&files, "bash", "5.2.037-1"),
            Some("bash-5.2.037-1-x86_64.pkg.tar.zst")
        );
        assert_eq!(pick_cached_file(&files, "zsh", "5.9-1"), None);
    }

    #[test]
    fn previous_version_walks_the_timeline() {
        let entries: Vec<PackageHistoryEntry> = [
            "[2025-01-01T00:00:00+0000] [ALPM] installed bash 5.2.032-1",
            "[2025-02-01T00:00:00+0000] [ALPM] upgraded bash (5.2.032-1 -> 5.2.037-1)",
            "[2025-03-01T00:00:00+0000] [ALPM] upgraded bash (5.2.037-1 -> 5.2.040-1)",
        ]
        .iter()
        .filter_map(|line| parse_line(line))
        .collect();
        let (current, previous) = previous_version(&entries).expect("timeline must resolve");
        assert_eq!(current, "5.2.040-1");
        assert_eq!(previous, "5.2.037-1");
    }

    #[test]
    fn previous_version_skips_removals_and_needs_a_real_previous() {
        let removed_then_installed: Vec<PackageHistoryEntry> = [
            "[2025-01-01T00:00:00+0000] [ALPM] installed bash 5.2.032-1",
            "[2025-02-01T00:00:00+0000] [ALPM] removed bash (5.2.032-1)",
            "[2025-03-01T00:00:00+0000] [ALPM] installed bash 5.2.040-1",
        ]
        .iter()
        .filter_map(|line| parse_line(line))
        .collect();
        let (current, previous) =
            previous_version(&removed_then_installed).expect("timeline must resolve");
        assert_eq!(current, "5.2.040-1");
        assert_eq!(previous, "5.2.032-1");

        let single: Vec<PackageHistoryEntry> =
            ["[2025-01-01T00:00:00+0000] [ALPM] installed bash 5.2.032-1"]
                .iter()
                .filter_map(|line| parse_line(line))
                .collect();
        assert_eq!(previous_version(&single), None);
        assert_eq!(previous_version(&[]), None);
    }
}
