//! Pure `dnf history` parsing — no dnf process involved, so these functions
//! are unit-testable on any machine (roadmap 2.3.1: `history` reads the dnf
//! history database through the tool's own CLI; the contract is the same as
//! the alpm/apt family's, suite unchanged, R2).
//!
//! Two on-disk surfaces are parsed (dnf4 and dnf5 print compatible shapes):
//!
//! ```text
//! ID     | Command line             | Date and time    | Action(s)       | Altered
//!     23 | install bar              | 2024-06-01 08:00 | I               |    1
//! ```
//!
//! and the per-transaction detail of `dnf history info <tid>`:
//!
//! ```text
//! Transaction ID : 23
//! Begin time     : 2024-06-01 08:00:02
//! Return-Code    : Success
//! Packages Altered:
//!     Install  bar-2.1.0-1.fc40.x86_64
//!     Erase    foo-1.0.0-1.fc40.x86_64
//! ```
//!
//! Honesty note on timestamps: dnf writes local time WITHOUT a zone offset, so
//! the epoch conversion assumes UTC — documented truncation of fidelity, same
//! class as the bounded log tail of the other family adapters: entries order
//! correctly, absolute instants may shift by the machine's zone offset. One
//! entry is emitted per package per altered line, exactly like the alpm log
//! parser emits one entry per logged package event.

use omnibus_contracts::dto::packages::PackageAction;

/// One parsed `dnf history info <tid>` block: the transaction id, its begin
/// instant, and the packages it altered.
#[derive(Debug, Default, Clone)]
pub struct TransactionInfo {
    pub id: u32,
    pub begin_epoch_ms: Option<i64>,
    /// `(action, name, from_version, to_version)` per altered package. Only
    /// the version dnf prints (the resulting NEVRA's version-release) is
    /// known: upgrade/downgrade lines record no source version, so
    /// `from_version` stays `None` there — never guessed.
    pub events: Vec<(PackageAction, String, Option<String>, Option<String>)>,
}

/// Parses the `dnf history` table into transaction IDs, most recent first
/// (the table's own order — the caller must not re-sort it). Tolerates both
/// the dnf4/dnf5 pipe-table shape and bare leading IDs.
pub fn parse_history_table(bytes: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let head = line.split('|').next().unwrap_or(line).trim();
            head.parse::<u32>().ok()
        })
        .collect()
}

/// Parses one `dnf history info <tid>` answer. `None` when the output carries
/// no `Transaction ID` line at all (a failed lookup degrades to absence).
pub fn parse_history_info(bytes: &[u8]) -> Option<TransactionInfo> {
    let mut info = TransactionInfo::default();
    let mut id_seen = false;
    let mut in_altered = false;
    for line in String::from_utf8_lossy(bytes).lines() {
        // The section header carries its colon with NO trailing space
        // (`Packages Altered:`), so it never matches the `key : value`
        // grammar — recognized as a bare (case-insensitive) line instead.
        if line
            .trim_end()
            .trim_end_matches(':')
            .eq_ignore_ascii_case("packages altered")
        {
            in_altered = true;
            continue;
        }
        if in_altered {
            if line.contains(": ") {
                // A new key (Scriptlet output, End time, ...): the altered
                // section ended; keep scanning only for nothing else.
                in_altered = false;
            } else if !line.trim().is_empty() {
                // Event rows start with a KNOWN action word; anything else
                // (trailing stats, scriptlet text) ends the section — one
                // bad line never costs the whole transaction.
                match parse_altered_line(line) {
                    Some((action, nevra)) if is_action_word(action) => {
                        info.events.push(event_of(action, nevra));
                        continue;
                    }
                    _ => in_altered = false,
                }
            }
        }
        let Some((key, value)) = split_key(line) else {
            continue;
        };
        match key.as_str() {
            "transaction id" => match value.trim().parse::<u32>() {
                Ok(id) => {
                    info.id = id;
                    id_seen = true;
                }
                Err(_) => continue,
            },
            "begin time" => info.begin_epoch_ms = parse_begin_time(value.trim()),
            "packages altered" => in_altered = true,
            // Return-Code, User, Releasever, Command Line, and anything the
            // tool adds later: not history events — skipped, never a failure.
            _ => {}
        }
    }
    id_seen.then_some(info)
}

/// Splits `Key   : value` on the first `: `; the key is lowercased and its
/// inner `-`/`_` collapsed so dnf4 (`Return-Code`) and dnf5 (`Return Code`)
/// spellings normalize to one shape. Shared with the stanza parser in the
/// adapter (`dnf info` answers use the same `Key : value` grammar).
pub(crate) fn split_key(line: &str) -> Option<(String, &str)> {
    let (key, value) = line.split_once(": ")?;
    let normalized = key.trim().to_ascii_lowercase().replace(['-', '_'], " ");
    Some((normalized, value))
}

/// Whether a `Packages Altered` row's first word is one of dnf's action
/// words (dnf4 and dnf5 spellings) — the guard that keeps trailing scriptlet
/// text and stats lines out of the event list.
fn is_action_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "install"
            | "upgrade"
            | "downgrade"
            | "erase"
            | "removed"
            | "remove"
            | "obsoleted"
            | "obsolete"
            | "reinstall"
    )
}

/// Parses one `Packages Altered` row: the action word, then the package NEVRA
/// (`Install  bar-2.1.0-1.fc40.x86_64`). Unknown action words and malformed
/// rows degrade to skips — one bad line never costs the whole transaction.
fn parse_altered_line(line: &str) -> Option<(&str, &str)> {
    let mut words = line.split_whitespace();
    let action = words.next()?;
    let nevra = words.next()?;
    Some((action, nevra))
}

/// Maps an altered-line action word onto the contract action and extracts the
/// version facts the line honestly carries: installs record the new version,
/// removals record the removed one, upgrades/downgrades/reinstalls record the
/// resulting version (dnf prints a single NEVRA per line).
fn event_of(action: &str, nevra: &str) -> (PackageAction, String, Option<String>, Option<String>) {
    let (name, version) = split_nevra(nevra);
    match action.to_ascii_lowercase().as_str() {
        "install" | "reinstall" => (PackageAction::Install, name, None, version),
        "upgrade" => (PackageAction::Upgrade, name, None, version),
        "downgrade" => (PackageAction::Downgrade, name, None, version),
        // Erase/Removed/Obsoleted all end with the package absent; dnf4 calls
        // it Erase, dnf5 Removed, and Obsoleted replaces the package away.
        _ => (PackageAction::Remove, name, version, None),
    }
}

/// Splits one NEVRA (`name-epoch:version-release.arch`) into
/// `(name, version-release)` — the arch suffix goes (stripped at the last
/// dot), then the last two dash-separated fields are release and version
/// (optionally epoch-prefixed), leaving the name, however many dashes it
/// carries. An unparseable NEVRA yields `(name-ish, None)`: the name is still
/// worth matching on, the version is never guessed.
pub fn split_nevra(nevra: &str) -> (String, Option<String>) {
    let body = match nevra.rsplit_once('.') {
        // The trailing dot-segment is the arch (`noarch`, `x86_64`, ...).
        Some((body, arch)) if !arch.is_empty() => body,
        _ => nevra,
    };
    let mut parts: Vec<&str> = body.rsplitn(3, '-').collect();
    parts.reverse();
    match parts.as_slice() {
        [name, version, release] => (String::from(*name), Some(format!("{version}-{release}"))),
        // Two segments (`name-version`, no release): keep what is there.
        [name, version] => (String::from(*name), Some(String::from(*version))),
        [single] => (String::from(*single), None),
        _ => (String::from(body), None),
    }
}

/// Parses `2024-06-01 08:00:02` (the dnf Begin time value) to epoch
/// milliseconds, assuming UTC — dnf logs no zone offset.
pub fn parse_begin_time(value: &str) -> Option<i64> {
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

    /// Real-shaped dnf4 `history info` answer (verbatim field spellings).
    const INFO: &str = "\
Transaction ID   : 23
Begin time       : 2024-06-01 08:00:02
Begin rpmdb      : 1717200001.5
End time         : 2024-06-01 08:00:09
User             : root <root>
Return-Code      : Success
Releasever       : 40
Command Line     : install bar
Packages Altered:
    Install  bar-2.1.0-1.fc40.x86_64
    Upgrade  baz-2:3.0-1.fc40.noarch
Scriptlet output:
   bar-2.1.0-1.fc40.x86_64 scriptlet ran
history stats
";

    #[test]
    fn info_block_parses_id_time_and_altered() {
        let info = parse_history_info(INFO.as_bytes()).expect("info block");
        assert_eq!(info.id, 23);
        // 2024-06-01 08:00:02 UTC == 1717228802.
        assert_eq!(info.begin_epoch_ms, Some(1_717_228_802_000));
        assert_eq!(info.events.len(), 2);
        assert_eq!(info.events[0].0, PackageAction::Install);
        assert_eq!(info.events[0].1, "bar");
        assert_eq!(info.events[0].3.as_deref(), Some("2.1.0-1.fc40"));
        // The epoch-prefixed NEVRA keeps its epoch in the version.
        assert_eq!(info.events[1].1, "baz");
        assert_eq!(info.events[1].3.as_deref(), Some("2:3.0-1.fc40"));
    }

    #[test]
    fn remove_downgrade_and_dnf5_spellings_map() {
        let raw = b"Transaction ID : 22\n\
                    Begin time : 2024-05-12 10:23:45\n\
                    Packages Altered:\n\
                    \x20   Erase foo-1.0.0-1.fc40.x86_64\n\
                    \x20   Removed old-2.0-1.fc40.noarch\n\
                    \x20   Obsoleted legacy-1.0-1.noarch\n\
                    \x20   Downgrade pkg-1.4-2.fc40.x86_64\n";
        let info = parse_history_info(raw).expect("info block");
        assert_eq!(info.begin_epoch_ms, Some(1_715_509_425_000));
        let actions: Vec<(PackageAction, &str)> = info
            .events
            .iter()
            .map(|(a, n, _, _)| (*a, n.as_str()))
            .collect();
        assert_eq!(
            actions,
            vec![
                (PackageAction::Remove, "foo"),
                (PackageAction::Remove, "old"),
                (PackageAction::Remove, "legacy"),
                (PackageAction::Downgrade, "pkg"),
            ]
        );
        // A removal records the version that went away, not a target.
        assert_eq!(info.events[0].2.as_deref(), Some("1.0.0-1.fc40"));
        assert_eq!(info.events[0].3, None);
    }

    #[test]
    fn dashed_names_survive_nevra_splitting() {
        assert_eq!(
            split_nevra("java-1.8.0-openjdk-1.8.0.312.b07-1.fc35.x86_64"),
            (
                String::from("java-1.8.0-openjdk"),
                Some(String::from("1.8.0.312.b07-1.fc35"))
            )
        );
        assert_eq!(
            split_nevra("foo-1.2-1.noarch"),
            (String::from("foo"), Some(String::from("1.2-1")))
        );
        assert_eq!(split_nevra("weird"), (String::from("weird"), None));
    }

    #[test]
    fn history_table_yields_ids_most_recent_first() {
        let raw = b"ID     | Command line             | Date and time    | Action(s)       | Altered\n\
                    \x20   23 | install bar              | 2024-06-01 08:00 | I               |    1\n\
                    \x20   22 | erase foo                | 2024-05-12 10:23 | E               |    1\n\
                    \x20   21 | install foo              | 2024-05-01 09:00 | I               |    1\n";
        assert_eq!(parse_history_table(raw), vec![23, 22, 21]);
        assert!(parse_history_table(b"").is_empty());
        assert!(parse_history_table(b"no table here").is_empty());
    }

    #[test]
    fn malformed_info_degrades_to_none_or_skips() {
        assert!(parse_history_info(b"Error: No such transaction").is_none());
        // A bad timestamp must not cost the events.
        let raw = b"Transaction ID : 5\nBegin time : not-a-date\nPackages Altered:\nInstall a-1-1.fc40.x86_64\n";
        let info = parse_history_info(raw).expect("info block");
        assert_eq!(info.begin_epoch_ms, None);
        assert_eq!(info.events.len(), 1);
    }

    #[test]
    fn begin_time_converts_as_utc_and_validates() {
        assert_eq!(
            parse_begin_time("2024-06-01 08:00:02"),
            Some(1_717_228_802_000)
        );
        assert_eq!(parse_begin_time("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_begin_time("not-a-date"), None);
        assert_eq!(parse_begin_time("2024-13-01 00:00:00"), None);
        assert_eq!(parse_begin_time("2023-02-29 00:00:00"), None);
        assert!(parse_begin_time("2024-02-29 00:00:00").is_some());
    }
}
