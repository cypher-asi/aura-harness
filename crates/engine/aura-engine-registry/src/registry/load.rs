//! Opening the registry: the shared database handle, loading or seeding
//! it, and the codes and prefixes it hands out.

use aura_store_db::{AgentStore, RocksStore};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use super::{
    canonical, code, legacy_or_new_zns_id, persist, read_registry_file, retire_registry_file,
    AgentRegistry, AgentRegistryError, ImportSource, RegisteredAgent, DB_DIR, DEFAULT_AGENT_NAME,
    REGISTRY_VERSION,
};
use crate::error::chain;

impl AgentRegistry {
    /// [`Self::load_or_init_in`] with the process's working directory as
    /// the launch directory.
    ///
    /// # Errors
    /// As [`Self::load_or_init_in`].
    pub fn load_or_init(data_dir: &Path) -> Result<Self, AgentRegistryError> {
        let launch_dir =
            std::env::current_dir().map_err(|source| AgentRegistryError::WorkingDir { source })?;
        Self::load_or_init_in(data_dir, &launch_dir)
    }

    /// [`Self::open`] on the database at `<data_dir>/db`, opened here (or
    /// shared with another registry on it in this process). For tests and
    /// benches; the app opens the database itself and calls
    /// [`Self::open`].
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened, or as
    /// [`Self::open`].
    #[doc(hidden)]
    pub fn load_or_init_in(data_dir: &Path, launch_dir: &Path) -> Result<Self, AgentRegistryError> {
        let rocks = shared_store(&data_dir.join(DB_DIR))?;
        let mut registry = Self::open(AgentStore::for_store(&rocks), data_dir, launch_dir)?;
        registry._keep_open = Some(rocks);
        Ok(registry)
    }

    /// Load the registry from `store`, importing `<data_dir>/agents.json`
    /// once when the store holds no registry yet (or, when only
    /// `agents.json.migrated` is left, re-importing that: the store lost
    /// the earlier import), seeding it from the
    /// legacy identity file (or a fresh identity) when there is neither,
    /// and bringing an older registry up to date. Agents nothing places
    /// go in the project for `launch_dir`.
    ///
    /// # Errors
    /// Returns an error if the store, registry or identity file cannot be
    /// read, parsed, or written.
    pub fn open(
        store: AgentStore,
        data_dir: &Path,
        launch_dir: &Path,
    ) -> Result<Self, AgentRegistryError> {
        let imported = store
            .is_empty()
            .map_err(|source| AgentRegistryError::Read {
                data_dir: data_dir.to_path_buf(),
                source,
            })?;
        let (file, persisted, source) = if imported {
            let (file, source) = read_registry_file(data_dir)?;
            (file, persist::Persisted::default(), source)
        } else {
            let snapshot = store.load().map_err(|source| AgentRegistryError::Load {
                data_dir: data_dir.to_path_buf(),
                source,
            })?;
            let file = persist::from_snapshot(snapshot);
            let persisted = persist::Persisted::of(&persist::to_rows(&file));
            (file, persisted, ImportSource::None)
        };

        let mut registry = Self {
            store,
            persisted,
            data_dir: data_dir.to_path_buf(),
            launch_dir: canonical(launch_dir),
            file,
            channel_members: HashSet::new(),
            _keep_open: None,
        };
        let changed = registry.upgrade(data_dir)?;
        let codes_changed = registry.refresh_codes();
        registry.save_opened(
            data_dir,
            changed || imported,
            imported,
            source,
            codes_changed,
        )?;
        Ok(registry)
    }

    /// Bring an older registry's version up to date and seed an empty one
    /// with its first agent. Returns whether anything changed.
    fn upgrade(&mut self, data_dir: &Path) -> Result<bool, AgentRegistryError> {
        let mut changed = false;
        if self.file.version < 2 {
            self.migrate(data_dir);
        }
        if self.file.version < 3 {
            self.hide_empty_projects();
        }
        if self.file.version < REGISTRY_VERSION {
            self.file.version = REGISTRY_VERSION;
            changed = true;
        }
        if self.file.agents.is_empty() {
            let zns_id = legacy_or_new_zns_id(data_dir)?;
            let project_id = self.launch_project();
            self.file.agents.push(RegisteredAgent {
                zns_id,
                project_id: Some(project_id),
                ..Self::new_agent(DEFAULT_AGENT_NAME.to_string())
            });
            changed = true;
        }
        Ok(changed)
    }

    /// Bring the codes, prefixes and highest numbers up to date. Returns
    /// whether any of them changed.
    fn refresh_codes(&mut self) -> bool {
        let migrated = code::migrate(
            &mut self.file.agents,
            &mut self.file.prefixes,
            &mut self.file.last_numbers,
        );
        let reserved = self.file.prefixes.fill_from_codes(&self.file.agents);
        let seeded = self.file.last_numbers.raise_to_codes(&self.file.agents);
        let coded = self.assign_missing_codes();
        migrated || coded || reserved || seeded
    }

    /// Write what [`Self::open`] changed: durably, retiring `agents.json`
    /// afterwards, after an import; best effort when only codes changed.
    fn save_opened(
        &mut self,
        data_dir: &Path,
        changed: bool,
        imported: bool,
        source: ImportSource,
        codes_changed: bool,
    ) -> Result<(), AgentRegistryError> {
        if changed {
            if imported {
                // The rename below must never outlive the import after a
                // power loss, so the batch is fsynced first.
                self.persist_durable()?;
                if source == ImportSource::File {
                    retire_registry_file(data_dir);
                }
            } else {
                self.persist()?;
            }
        } else if codes_changed {
            // Codes alone are no reason to refuse to start, say on a
            // read-only data directory: the next load assigns the same
            // ones again, as backfilling is deterministic.
            if let Err(error) = self.persist() {
                tracing::warn!(
                    data_dir = %self.data_dir.display(),
                    error = %chain(&error),
                    "could not save the agents' new codes"
                );
            }
        }
        Ok(())
    }

    /// Give every agent without a code one, in registry order, so the same
    /// registry always gets the same codes (see [`code::backfill`]).
    /// Returns whether any agent got one.
    fn assign_missing_codes(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.file.agents.len() {
            if self.file.agents[index].code.is_some() {
                continue;
            }
            let assigned = code::backfill(
                &self.file.agents[index],
                &self.file.agents,
                &mut self.file.prefixes,
                &mut self.file.last_numbers,
            );
            self.file.agents[index].code = Some(assigned);
            changed = true;
        }
        changed
    }

    /// The code prefix (the abbreviation, such as `REN` for
    /// `rust-engineer`) of each of `keys`, in the same order. A key is a
    /// definition name, or `agent` for agents without one.
    ///
    /// Keys without a prefix yet reserve one now, in the order given, so
    /// the same registry and keys always give the same prefixes. A
    /// reserved prefix is the one the key's instances' codes get, is never
    /// changed, and no other key has it. The registry is saved only when
    /// something was reserved.
    ///
    /// # Errors
    /// Returns an error if a new reservation cannot be saved. The
    /// reservations stay in memory regardless.
    pub fn reserve_prefixes(&mut self, keys: &[&str]) -> Result<Vec<String>, AgentRegistryError> {
        let before = self.file.prefixes.len();
        let prefixes = keys
            .iter()
            .map(|key| self.file.prefixes.prefix_for(key, &self.file.agents))
            .collect();
        if self.file.prefixes.len() != before {
            self.persist()?;
        }
        Ok(prefixes)
    }
}

/// Database handles [`AgentRegistry::load_or_init_in`] opened, by path,
/// so a second registry on a data directory shares the live handle
/// instead of failing on the database lock.
static OPEN: Mutex<Option<HashMap<PathBuf, Weak<RocksStore>>>> = Mutex::new(None);

/// The live handle on the database at `path`, opened when there is none.
pub(super) fn shared_store(path: &Path) -> Result<Arc<RocksStore>, AgentRegistryError> {
    let mut open = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = open.get_or_insert_with(HashMap::new);
    open.retain(|_, store| store.strong_count() > 0);
    if let Some(store) = open.get(path).and_then(Weak::upgrade) {
        return Ok(store);
    }
    let store = Arc::new(RocksStore::open(path, false).map_err(|source| {
        AgentRegistryError::OpenDatabase {
            path: path.to_path_buf(),
            source,
        }
    })?);
    open.insert(path.to_path_buf(), Arc::downgrade(&store));
    Ok(store)
}
