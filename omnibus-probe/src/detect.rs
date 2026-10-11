//! Detection parsers: pure functions over runner outputs (unit-tested with
//! fixtures; roadmap tasks 1.4.1 and 2.1.1).

use omnibus_contracts::dto::probe::{DistroFamily, DistroInfo};

/// Parses /etc/os-release into the distro DTO; `id` falls back to "unknown" so
/// the report always carries a present distro row.
pub fn parse_os_release(text: &str) -> DistroInfo {
    let mut id = String::from("unknown");
    let mut name = String::from("unknown");
    let mut release = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim_matches('"');
        match key.trim() {
            "ID" => id = String::from(value),
            "NAME" => name = String::from(value),
            "VERSION_ID" | "BUILD_ID" if release.is_empty() => release = String::from(value),
            _ => {}
        }
    }
    DistroInfo {
        family: family_of(&id),
        id,
        name,
        release,
    }
}

/// The adapter-family rule (task 2.1.1): Manjaro is arch-family but a separate
/// probe branch; apt family = Debian/Ubuntu/Mint; Fedora = dnf.
pub fn family_of(id: &str) -> DistroFamily {
    match id {
        "arch" => DistroFamily::Arch,
        "manjaro" => DistroFamily::Manjaro,
        "debian" => DistroFamily::Debian,
        "ubuntu" => DistroFamily::Ubuntu,
        "linuxmint" => DistroFamily::Mint,
        "fedora" => DistroFamily::Fedora,
        _ => DistroFamily::Other,
    }
}

/// `systemctl --version` line 1: `systemd 257 (257.8-1-arch)` -> `257`.
pub fn parse_systemd_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let rest = line.strip_prefix("systemd ")?;
    let version = rest.split_whitespace().next()?;
    if version.is_empty() {
        None
    } else {
        Some(String::from(version))
    }
}

/// `pacman -Q <pkg>` -> `at-spi2-core 2.60.6-1` -> `2.60.6-1`; dpkg/rpm variants
/// normalize to the same leading-version shape.
pub fn parse_pkg_query_version(output: &str, package: &str) -> Option<String> {
    for line in output.lines() {
        // pacman: "name version", dpkg-query -W -f: "name\tversion", rpm -q: "name-version-release"
        if let Some(rest) = line.strip_prefix(package) {
            let rest = rest.trim_start();
            let candidate = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_start_matches(['-', '\t']);
            if candidate.is_empty() || candidate == package {
                continue;
            }
            // rpm prints "package-version-release"; drop the package prefix.
            let candidate = candidate
                .strip_prefix(&format!("{package}-"))
                .unwrap_or(candidate);
            return Some(String::from(candidate));
        }
    }
    None
}

/// `uname -r`-style kernel release (first whitespace token).
pub fn parse_kernel(output: &str) -> String {
    output
        .split_whitespace()
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// Repository sections of /etc/pacman.conf (task 2.4.1): `[repo]` headers other
/// than the `[options]` block.
pub fn parse_pacman_repos(conf: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
            if inner.is_empty() || inner == "options" {
                return None;
            }
            Some(String::from(inner))
        })
        .collect()
}

/// True when the mirrorlist points at Manjaro mirrors: those repos sync from
/// Arch with a deliberate delay — a fact the report must surface, never hide.
pub fn mirrorlist_is_manjaro(mirrorlist: &str) -> bool {
    mirrorlist.to_ascii_lowercase().contains("manjaro")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS_RELEASE: &str =
        "NAME=\"Manjaro Linux\"\nID=manjaro\nVERSION_ID=\"25.0\"\nPRETTY_NAME=\"Manjaro Linux\"\n";

    #[test]
    fn os_release_parses_into_the_dto() {
        let info = parse_os_release(OS_RELEASE);
        assert_eq!(info.id, "manjaro");
        assert_eq!(info.name, "Manjaro Linux");
        assert_eq!(info.release, "25.0");
        assert_eq!(info.family, DistroFamily::Manjaro);
    }

    #[test]
    fn unknown_distro_degrades_to_other_family() {
        let info = parse_os_release("ID=nixos\n");
        assert_eq!(info.family, DistroFamily::Other);
        assert_eq!(info.id, "nixos");
    }

    #[test]
    fn family_mapping_covers_the_matrix() {
        assert_eq!(family_of("arch"), DistroFamily::Arch);
        assert_eq!(family_of("debian"), DistroFamily::Debian);
        assert_eq!(family_of("ubuntu"), DistroFamily::Ubuntu);
        assert_eq!(family_of("linuxmint"), DistroFamily::Mint);
        assert_eq!(family_of("fedora"), DistroFamily::Fedora);
    }

    #[test]
    fn systemd_version_line_parses() {
        assert_eq!(
            parse_systemd_version("systemd 257 (257.8-1-arch)\nOther features…\n"),
            Some(String::from("257"))
        );
        assert_eq!(parse_systemd_version("not systemd"), None);
        assert_eq!(parse_systemd_version(""), None);
    }

    #[test]
    fn pkg_query_versions_parse_across_managers() {
        assert_eq!(
            parse_pkg_query_version("at-spi2-core 2.60.6-1\n", "at-spi2-core"),
            Some(String::from("2.60.6-1"))
        );
        assert_eq!(
            parse_pkg_query_version("at-spi2-core\t2.54.0-5\n", "at-spi2-core"),
            Some(String::from("2.54.0-5"))
        );
        assert_eq!(
            parse_pkg_query_version("at-spi2-core-2.48.0-4.fc40.x86_64\n", "at-spi2-core"),
            Some(String::from("2.48.0-4.fc40.x86_64"))
        );
        assert_eq!(
            parse_pkg_query_version("error: package not found\n", "at-spi2-core"),
            None
        );
    }

    #[test]
    fn kernel_takes_the_first_token() {
        assert_eq!(
            parse_kernel("7.1.13-2-MANJARO x86_64\n"),
            "7.1.13-2-MANJARO"
        );
        assert_eq!(parse_kernel(""), "unknown");
    }

    #[test]
    fn pacman_repos_and_manjaro_mirrors() {
        let conf =
            "[options]\nHoldPkg = pacman\n[core]\nInclude = /etc/pacman.d/mirrorlist\n[extra]\n\n";
        assert_eq!(parse_pacman_repos(conf), vec!["core", "extra"]);
        assert!(mirrorlist_is_manjaro(
            "Server = https://mirror.example.org/manjaro/stable/$repo/os/$arch\n"
        ));
        assert!(!mirrorlist_is_manjaro(
            "Server = https://mirror.example.org/archlinux/$repo/os/$arch\n"
        ));
    }
}
