use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// A fake process environment.
#[derive(Default)]
struct FakeEnv(RefCell<HashMap<String, String>>);

impl FakeEnv {
    fn with(pairs: &[(&str, &str)]) -> Self {
        let env = Self::default();
        for (name, value) in pairs {
            env.0
                .borrow_mut()
                .insert((*name).to_owned(), (*value).to_owned());
        }
        env
    }

    fn get(&self, name: &str) -> Option<String> {
        self.0.borrow().get(name).cloned()
    }

    fn load(&self, cwd: &Path, aura_home: Option<&Path>) -> EnvFileReport {
        let home = aura_home.map(Path::to_path_buf);
        load_from(
            Some(cwd),
            move || Ok(home),
            |name, value| {
                self.0
                    .borrow_mut()
                    .insert(name.to_owned(), value.to_owned());
            },
            |name| self.0.borrow().contains_key(name),
        )
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("write test .env");
}

#[test]
fn cwd_env_file_loads() {
    let project = tempfile::tempdir().expect("tempdir");
    write(&project.path().join(".env"), "AURA_ENVFILE_T1=from-cwd\n");
    let env = FakeEnv::default();

    let report = env.load(project.path(), None);

    assert_eq!(env.get("AURA_ENVFILE_T1").as_deref(), Some("from-cwd"));
    assert_eq!(report.loaded_files(), [project.path().join(".env")]);
}

#[test]
fn key_only_in_aura_home_loads() {
    let project = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("tempdir");
    write(&home.path().join(".env"), "AURA_ENVFILE_T2=from-home\n");
    let env = FakeEnv::default();

    let report = env.load(project.path(), Some(home.path()));

    assert_eq!(env.get("AURA_ENVFILE_T2").as_deref(), Some("from-home"));
    assert!(report.loaded.contains(&home.path().join(".env")));
}

#[test]
fn preset_value_beats_both_files() {
    let project = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("tempdir");
    write(&project.path().join(".env"), "AURA_ENVFILE_T3=from-cwd\n");
    write(&home.path().join(".env"), "AURA_ENVFILE_T3=from-home\n");
    let env = FakeEnv::with(&[("AURA_ENVFILE_T3", "from-shell")]);

    env.load(project.path(), Some(home.path()));

    assert_eq!(env.get("AURA_ENVFILE_T3").as_deref(), Some("from-shell"));
}

#[test]
fn cwd_file_beats_aura_home_file() {
    let project = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("tempdir");
    write(&project.path().join(".env"), "AURA_ENVFILE_T4=from-cwd\n");
    write(
        &home.path().join(".env"),
        "AURA_ENVFILE_T4=from-home\nAURA_ENVFILE_T4B=home-only\n",
    );
    let env = FakeEnv::default();

    let report = env.load(project.path(), Some(home.path()));

    assert_eq!(env.get("AURA_ENVFILE_T4").as_deref(), Some("from-cwd"));
    assert_eq!(env.get("AURA_ENVFILE_T4B").as_deref(), Some("home-only"));
    assert_eq!(
        report.loaded_files(),
        [project.path().join(".env"), home.path().join(".env")]
    );
}

#[test]
fn parent_directory_env_file_is_found() {
    let project = tempfile::tempdir().expect("tempdir");
    write(
        &project.path().join(".env"),
        "AURA_ENVFILE_T5=from-parent\n",
    );
    let nested = project.path().join("a").join("b");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    let env = FakeEnv::default();

    let report = env.load(&nested, None);

    assert_eq!(env.get("AURA_ENVFILE_T5").as_deref(), Some("from-parent"));
    assert_eq!(report.loaded_files(), [project.path().join(".env")]);
}

#[test]
fn same_file_is_loaded_once() {
    let project = tempfile::tempdir().expect("tempdir");
    write(&project.path().join(".env"), "AURA_ENVFILE_T6=x\n");
    let env = FakeEnv::default();

    let report = env.load(project.path(), Some(project.path()));

    assert_eq!(report.loaded.len(), 1);
}

#[test]
fn a_second_pass_picks_up_a_newly_added_value() {
    let project = tempfile::tempdir().expect("tempdir");
    let file = project.path().join(".env");
    write(&file, "AURA_ENVFILE_T7=first\n");
    let env = FakeEnv::default();
    env.load(project.path(), None);

    write(&file, "AURA_ENVFILE_T7=changed\nAURA_ENVFILE_T7B=added\n");
    env.load(project.path(), None);

    assert_eq!(env.get("AURA_ENVFILE_T7").as_deref(), Some("first"));
    assert_eq!(env.get("AURA_ENVFILE_T7B").as_deref(), Some("added"));
}

#[test]
fn parse_error_warns_without_quoting_the_line() {
    let project = tempfile::tempdir().expect("tempdir");
    write(
        &project.path().join(".env"),
        "AURA_ENVFILE_T8=ok\nBAD LINE sekrit-value\n",
    );
    let env = FakeEnv::default();

    let report = env.load(project.path(), None);

    assert_eq!(env.get("AURA_ENVFILE_T8").as_deref(), Some("ok"));
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(!report.warnings[0].contains("sekrit-value"));
}

#[test]
fn missing_aura_home_file_is_skipped_silently() {
    let project = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("tempdir");
    let env = FakeEnv::default();

    let report = env.load(project.path(), Some(home.path()));

    assert!(!report.loaded.contains(&home.path().join(".env")));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[cfg(unix)]
mod permissions {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SECRET: &str = "bxd_supersecretvalue1234";

    fn key_file_with_mode(mode: u32) -> (tempfile::TempDir, EnvFileReport) {
        let home = tempfile::tempdir().expect("tempdir");
        let project = tempfile::tempdir().expect("tempdir");
        let file = home.path().join(".env");
        write(&file, &format!("BOXD_API_KEY={SECRET}\n"));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).expect("chmod");
        let report = FakeEnv::default().load(project.path(), Some(home.path()));
        (home, report)
    }

    #[test]
    fn world_readable_key_file_warns() {
        let (home, report) = key_file_with_mode(0o644);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        let warning = &report.warnings[0];
        assert!(warning.contains("chmod 600"), "{warning}");
        assert!(
            warning.contains(&home.path().join(".env").display().to_string()),
            "{warning}"
        );
        assert!(!warning.contains(SECRET), "{warning}");
    }

    #[test]
    fn private_key_file_does_not_warn() {
        let (_home, report) = key_file_with_mode(0o600);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn readable_file_without_key_does_not_warn() {
        let project = tempfile::tempdir().expect("tempdir");
        let file = project.path().join(".env");
        write(&file, "AURA_ENVFILE_T9=x\n");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let report = FakeEnv::default().load(project.path(), None);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}
