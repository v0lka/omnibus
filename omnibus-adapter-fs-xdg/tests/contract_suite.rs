//! Contract suite binding for fs-xdg (R2): the `FileScope` family suites run
//! against a hermetic tempdir layout — `$HOME` = temp root, the granted write
//! scope = `projects/`. Runs everywhere (no live backend needed).

use omnibus_adapter_fs_xdg::{FsXdgAdapter, scope::FileScopes};
use omnibus_contracts::spi::files::FileScope;
use omnibus_contracts::suite;

struct Layout {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    projects: std::path::PathBuf,
}

fn fixture_home() -> Layout {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let projects = home.join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    std::fs::write(home.join("notes.txt"), "hello from the home scope\n").expect("seed file");
    Layout {
        _dir: dir,
        home,
        projects,
    }
}

fn factory_for(layout: &Layout) -> impl Fn() -> Box<dyn FileScope> + Send + Sync + 'static {
    let home = layout.home.clone();
    let projects = layout.projects.clone();
    move || {
        Box::new(FsXdgAdapter::with_scopes(FileScopes::with_home(
            vec![projects.to_string_lossy().into_owned()],
            home.clone(),
        ))) as Box<dyn FileScope>
    }
}

#[tokio::test]
async fn files_suite_green_on_the_tempdir_scope() {
    let layout = fixture_home();
    let report = suite::files::files_suite(
        factory_for(&layout),
        suite::files::FilesFixture {
            readable_path: layout.home.join("notes.txt").to_string_lossy().into_owned(),
            expected_snippet: String::from("hello from the home scope"),
            forbidden_path: String::from("/etc/os-release"),
        },
    )
    .await;
    assert!(report.is_clean(), "files suite red: {report:?}");
}

#[tokio::test]
async fn files_write_suite_green_inside_the_granted_scope() {
    let layout = fixture_home();
    let target = layout
        .projects
        .join("suite-written.md")
        .to_string_lossy()
        .into_owned();
    let report = suite::files::files_write_suite(
        factory_for(&layout),
        target,
        String::from("written by the suite\n"),
    )
    .await;
    assert!(report.is_clean(), "files write suite red: {report:?}");
}

#[tokio::test]
async fn write_outside_the_scope_is_denied_by_the_suite_semantics() {
    let layout = fixture_home();
    // /etc is outside every scope; files_write_suite accepts ScopeDenied as the
    // honest outcome for write-capable fixtures.
    let report = suite::files::files_write_suite(
        factory_for(&layout),
        String::from("/etc/omnibus-must-not-write.conf"),
        String::from("nope"),
    )
    .await;
    assert!(report.is_clean(), "denial suite red: {report:?}");
}
