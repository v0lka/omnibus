//! Manjaro-specific probe branch (roadmap tasks 2.1.1 and 2.4.1): delayed
//! repos, mhwd driver management, pamac — surfaced as report facts, never
//! silently. Manjaro is arch-family (alpm adapter works unchanged) but is its
//! own T1 distribution with its own QA matrix.

use omnibus_contracts::dto::probe::ManjaroFacts;

use crate::runner::CommandRunner;

/// Collects the Manjaro facts from the system (pacman.conf, mirrorlist, mhwd,
/// pamac). Called only when the distro family is Manjaro.
pub async fn collect(runner: &dyn CommandRunner) -> ManjaroFacts {
    let repos = runner
        .read_file("/etc/pacman.conf")
        .map(|conf| crate::detect::parse_pacman_repos(&conf))
        .unwrap_or_default();
    let delayed_repos = runner
        .read_file("/etc/pacman.d/mirrorlist")
        .map(|list| crate::detect::mirrorlist_is_manjaro(&list))
        .unwrap_or(false);
    let mhwd_present = command_present(runner, "mhwd").await || runner.file_exists("/var/lib/mhwd");
    let pamac_present = command_present(runner, "pamac").await;
    ManjaroFacts {
        repos,
        delayed_repos,
        mhwd_present,
        pamac_present,
    }
}

async fn command_present(runner: &dyn CommandRunner, name: &str) -> bool {
    let on_path = runner.env("PATH").is_some_and(|path| {
        path.split(':')
            .any(|dir| runner.file_exists(&format!("{dir}/{name}")))
    });
    on_path || runner.run("/usr/bin/which", &[name]).await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        files: BTreeMap<String, String>,
        executables: Vec<String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn with_file(mut self, path: &str, body: &str) -> Self {
            self.files.insert(String::from(path), String::from(body));
            self
        }
        fn with_executable(mut self, name: &str) -> Self {
            self.executables.push(String::from(name));
            self
        }
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, _prog: &str, args: &[&str]) -> Option<String> {
            self.calls.lock().ok()?.push(args.join(" "));
            let name = args.last()?;
            self.executables
                .iter()
                .find(|e| e.ends_with(name))
                .map(|e| format!("{e} present\n"))
        }
        fn file_exists(&self, path: &str) -> bool {
            self.files.contains_key(path)
                || self
                    .executables
                    .iter()
                    .any(|e| e.ends_with(path) || path.ends_with(e))
        }
        fn env(&self, name: &str) -> Option<String> {
            if name == "PATH" {
                Some(String::from("/usr/bin"))
            } else {
                None
            }
        }
        fn read_file(&self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }
    }

    #[tokio::test]
    async fn manjaro_facts_surfaced_from_conf_and_tools() {
        let runner = FakeRunner::default()
            .with_file(
                "/etc/pacman.conf",
                "[options]\n[core]\n[extra]\n[multilib]\n",
            )
            .with_file(
                "/etc/pacman.d/mirrorlist",
                "Server = https://mirrors.example/manjaro/stable/$repo/os/$arch\n",
            )
            .with_executable("/usr/bin/mhwd");
        let facts = collect(&runner).await;
        assert_eq!(facts.repos, vec!["core", "extra", "multilib"]);
        assert!(facts.delayed_repos, "Manjaro mirrors are delayed by design");
        assert!(facts.mhwd_present);
        assert!(!facts.pamac_present);
    }

    #[tokio::test]
    async fn missing_files_degrade_to_empty_facts() {
        let runner = FakeRunner::default();
        let facts = collect(&runner).await;
        assert!(facts.repos.is_empty());
        assert!(!facts.delayed_repos);
        assert!(!facts.mhwd_present);
        assert!(!facts.pamac_present);
    }
}
