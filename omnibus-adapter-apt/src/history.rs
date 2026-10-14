//! Pure `/var/log/apt/history.log` parsing — no apt process involved, so these
//! functions are unit-testable on any machine (roadmap 2.2.1: `history` reads
//! the apt log; the contract is the same as alpm's, suite unchanged, R2).
//!
//! The on-disk format (written by apt's `history.log`, one transaction block):
//!
//! ```text
//! Start-Date: 2024-05-12  10:23:45
//! Commandline: apt-get install -y foo
//! Requested-By: user (1000)
//! Install: libfoo1:amd64 (1.2.3-1, automatic), foo:amd64 (1.2.3-1)
//! Upgrade: bar:amd64 (2.0-1, 2.1-1)
//! Downgrade: qux:amd64 (3.0-1, 2.9-1)
//! Remove: baz:amd64 (3.3-1)
//! Purge: old:amd64 (1.0-1)
//! Reinstall: again:amd64 (1.1-1)
//! End-Date: 2024-05-12  10:23:47
//! ```
//!
//! Honesty note on timestamps: apt writes local time WITHOUT a zone offset, so
//! the epoch conversion assumes UTC — documented truncation of fidelity, same
//! class as the bounded log tail: entries order correctly, absolute instants
//! may shift by the machine's zone offset. One entry is emitted per package
//! per section, exactly like alpm emits one entry per logged package event.

use omnibus_contracts::dto::packages::{PackageAction, PackageHistoryEntry};

/// One parsed transaction block under construction.
#[derive(Debug, Default)]
struct Block {
    start_epoch_ms: Option<i64>,
    events: Vec<(PackageAction, String, Option<String>, Option<String>)>,
}

/// Parses a history log tail into entries: at most `limit` most recent,
/// filtered by `name` when given, in chronological order. Lossy by design:
/// undecodable bytes become U+FFFD in place (one corrupted line costs one
/// entry, never the whole read), and unparsable lines are skipped.
pub fn parse(bytes: &[u8], name: Option<&str>, limit: Option<u32>) -> Vec<PackageHistoryEntry> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries = Vec::new();
    let mut block = Block::default();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        match key {
            "Start-Date" => {
                // A block boundary also flushes when End-Date is missing
                // (truncated tail): never lose a parsed block to truncation.
                flush(&mut block, &mut entries);
                block.start_epoch_ms = parse_start_date(value.trim());
            }
            "Install" | "Upgrade" | "Downgrade" | "Remove" | "Purge" | "Reinstall" => {
                let action = match key {
                    "Install" => PackageAction::Install,
                    "Upgrade" => PackageAction::Upgrade,
                    "Downgrade" => PackageAction::Downgrade,
                    "Reinstall" => PackageAction::Reinstall,
                    // Purge is a remove that also drops conffiles.
                    _ => PackageAction::Remove,
                };
                for (pkg, versions) in parse_section(value) {
                    let (from, to) = match action {
                        PackageAction::Install => (None, versions.first().cloned()),
                        // Remove/Purge logs the version being removed.
                        PackageAction::Remove => (versions.first().cloned(), None),
                        // Upgrade/Downgrade log `(old, new)`; Reinstall logs a
                        // single version (both states identical).
                        _ => (
                            versions.first().cloned(),
                            versions.get(1).or_else(|| versions.first()).cloned(),
                        ),
                    };
                    block.events.push((action, pkg, from, to));
                }
            }
            // Commandline, Requested-By, End-Date, Error, and anything future
            // apt adds: not history events — skipped, never a parse failure.
            _ => {}
        }
    }
    flush(&mut block, &mut entries);
    entries
        .into_iter()
        .filter(|entry| name.is_none_or(|n| entry.name == n))
        .collect::<Vec<_>>()
        .tap_limit(limit)
}

/// Small helper keeping `parse` readable: applies the most-recent limit.
trait TapLimit {
    fn tap_limit(self, limit: Option<u32>) -> Self;
}
impl TapLimit for Vec<PackageHistoryEntry> {
    fn tap_limit(mut self, limit: Option<u32>) -> Self {
        if let Some(limit) = limit {
            let keep = usize::try_from(limit).unwrap_or(usize::MAX);
            let skip = self.len().saturating_sub(keep);
            self.drain(..skip);
        }
        self
    }
}

fn flush(block: &mut Block, entries: &mut Vec<PackageHistoryEntry>) {
    let ts = block.start_epoch_ms.unwrap_or_default();
    for (action, name, from, to) in block.events.drain(..) {
        entries.push(PackageHistoryEntry {
            ts_epoch_ms: ts,
            action,
            name,
            from_version: from,
            to_version: to,
        });
    }
}

/// Splits one section value into `(name, versions)` pairs. Item shape:
/// `name:arch (v1)` / `name:arch (old, new)`; the config tokens apt appends
/// (`automatic`, `manual`, ...) are filtered from the version tuple.
fn parse_section(value: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for item in split_items(value) {
        let Some(open) = item.find('(') else {
            continue;
        };
        let Some(close) = item.rfind(')') else {
            continue;
        };
        if close < open {
            continue;
        }
        let full_name = item[..open].trim();
        let full_name = full_name.strip_suffix(',').unwrap_or(full_name).trim();
        if full_name.is_empty() {
            continue;
        }
        // Strip the `:arch`/`:amd64` multiarch suffix; package names never
        // contain a colon, versions may (epochs) — this field is the name.
        let name = full_name.split(':').next().unwrap_or(full_name);
        let versions: Vec<String> = item[open + 1..close]
            .split(", ")
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .filter(|token| token.chars().any(|c| c.is_ascii_digit()))
            .map(String::from)
            .collect();
        out.push((String::from(name), versions));
    }
    out
}

/// Splits `a:amd64 (1.0), b:amd64 (2.0, automatic)` on item boundaries: a new
/// item starts where a top-level `), ` appears (parenthesis depth zero).
fn split_items(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0_usize;
    let mut current = String::new();
    for ch in value.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                items.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        items.push(current);
    }
    items
}

/// Parses `2024-05-12  10:23:45` (the apt Start-Date value, double space and
/// all) to epoch milliseconds, assuming UTC — apt logs no zone offset.
pub fn parse_start_date(value: &str) -> Option<i64> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let (date, time) = value.split_once(' ')?;
    let (year, month, day) = three_nums(date, '-')?;
    let (hour, minute, second) = three_nums(time, ':')?;
    if !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 60
        || day < 1
        || day > i64::from(days_in_month(year, month as u32))
    {
        return None;
    }
    let secs = days_from_civil(year, month as u32, day as u32) * 86400
        + hour * 3600
        + minute * 60
        + second;
    Some(secs.saturating_mul(1000))
}

/// Parses three `sep`-separated integers (`YYYY-MM-DD` / `hh:mm:ss`).
fn three_nums(text: &str, sep: char) -> Option<(i64, i64, i64)> {
    let (a, rest) = text.split_once(sep)?;
    let (b, c) = rest.split_once(sep)?;
    Some((a.parse().ok()?, b.parse().ok()?, c.parse().ok()?))
}

/// Length of `month` of `year` under the full Gregorian rule (leap-aware).
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A real-shaped history.log tail (Debian trixie format, verbatim shapes).
    const FIXTURE: &str = "\
Start-Date: 2024-05-12  10:23:45
Commandline: apt-get install -y foo
Requested-By: user (1000)
Install: libfoo1:amd64 (1.2.3-1, automatic), foo:amd64 (1.2.3-1)
Upgrade: bar:amd64 (2.0-1, 2.1-1)
End-Date: 2024-05-12  10:23:47

Start-Date: 2024-06-01  08:00:02
Commandline: apt-get remove -y baz
Downgrade: qux:amd64 (3.0-1, 2.9-1)
Remove: baz:amd64 (3.3-1)
Purge: old:amd64 (1.0-1)
Reinstall: again:amd64 (1.1-1)
End-Date: 2024-06-01  08:00:09
";

    #[test]
    fn sections_parse_into_per_package_entries() {
        let entries = parse(FIXTURE.as_bytes(), None, None);
        // Install: 2 + Upgrade: 1 + Downgrade/Remove/Purge/Reinstall: 4.
        assert_eq!(entries.len(), 7);
        let foo = entries.iter().find(|e| e.name == "foo").expect("foo entry");
        assert_eq!(foo.action, PackageAction::Install);
        assert_eq!(foo.from_version, None);
        assert_eq!(foo.to_version.as_deref(), Some("1.2.3-1"));
        let bar = entries.iter().find(|e| e.name == "bar").expect("bar entry");
        assert_eq!(bar.action, PackageAction::Upgrade);
        assert_eq!(bar.from_version.as_deref(), Some("2.0-1"));
        assert_eq!(bar.to_version.as_deref(), Some("2.1-1"));
        let qux = entries.iter().find(|e| e.name == "qux").expect("qux entry");
        assert_eq!(qux.action, PackageAction::Downgrade);
        assert_eq!(qux.from_version.as_deref(), Some("3.0-1"));
        assert_eq!(qux.to_version.as_deref(), Some("2.9-1"));
        let old = entries.iter().find(|e| e.name == "old").expect("old entry");
        assert_eq!(old.action, PackageAction::Remove, "purge maps to remove");
        assert_eq!(old.from_version.as_deref(), Some("1.0-1"));
        let again = entries
            .iter()
            .find(|e| e.name == "again")
            .expect("again entry");
        assert_eq!(again.action, PackageAction::Reinstall);
        assert_eq!(again.from_version.as_deref(), Some("1.1-1"));
        assert_eq!(again.to_version.as_deref(), Some("1.1-1"));
    }

    #[test]
    fn multiarch_names_and_config_tokens_are_handled() {
        let entries = parse(
            b"Start-Date: 2024-05-12  10:23:45\nInstall: libfoo1:amd64 (1.2.3-1, automatic), foo:i386 (1.2.3-1)\n",
            None,
            None,
        );
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["libfoo1", "foo"], "arch suffixes are stripped");
        assert!(
            entries
                .iter()
                .all(|e| e.to_version.as_deref() == Some("1.2.3-1")),
            "the `automatic` config token never becomes a version"
        );
    }

    #[test]
    fn name_filter_and_limit_keep_the_most_recent() {
        let foo = parse(FIXTURE.as_bytes(), Some("foo"), None);
        assert_eq!(foo.len(), 1);
        assert_eq!(foo[0].action, PackageAction::Install);

        let recent_two = parse(FIXTURE.as_bytes(), None, Some(2));
        assert_eq!(recent_two.len(), 2);
        assert_eq!(recent_two[0].ts_epoch_ms, recent_two[1].ts_epoch_ms);
        // The kept tail is the LAST block (purge `old` + reinstall `again`),
        // not the first.
        assert_eq!(recent_two[0].name, "old");
        assert_eq!(recent_two[1].name, "again");
    }

    #[test]
    fn timestamps_convert_as_utc() {
        // 2024-05-12 10:23:45 UTC == 1715509425.
        assert_eq!(
            parse_start_date("2024-05-12  10:23:45"),
            Some(1_715_509_425_000)
        );
        // The double space and single space both normalize.
        assert_eq!(
            parse_start_date("2024-05-12 10:23:45"),
            parse_start_date("2024-05-12  10:23:45")
        );
        assert_eq!(parse_start_date("1970-01-01  00:00:00"), Some(0));
        assert_eq!(parse_start_date("not-a-date"), None);
        assert_eq!(parse_start_date("2024-13-01  00:00:00"), None);
        assert_eq!(parse_start_date("2024-02-31  00:00:00"), None);
        assert_eq!(parse_start_date("2023-02-29  00:00:00"), None);
        assert!(parse_start_date("2024-02-29  00:00:00").is_some());
    }

    #[test]
    fn truncated_blocks_still_flush() {
        // A tail cut before End-Date must not lose the parsed events.
        let truncated = b"Start-Date: 2024-05-12  10:23:45\nInstall: foo:amd64 (1.2.3-1)\n";
        let entries = parse(truncated, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "foo");
    }

    #[test]
    fn corrupted_and_unparsable_lines_degrade_to_skips() {
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"Start-Date: 2024-05-12  10:23:45\n");
        // One undecodable byte inside a name: U+FFFD in place, entry survives.
        bytes.extend_from_slice(b"Install: fo\xFFo:amd64 (1.2.3-1)\n");
        bytes.extend_from_slice(b"Install: broken-line-no-parens\n");
        bytes.extend_from_slice(b"garbage without separator\n");
        let entries = parse(&bytes, None, None);
        assert_eq!(entries.len(), 1, "the bad byte costs nothing whole");
        assert_eq!(entries[0].name, "fo\u{FFFD}o");
    }
}
