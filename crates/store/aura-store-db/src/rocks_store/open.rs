//! Opening the database: column families and their tuning.

use super::{RocksStore, APPEND_SHARDS};
use crate::cf;
use crate::error::{backend, StoreError};
use crate::seal::SealCipher;
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, DBCompressionType, DBWithThreadMode,
    MultiThreaded, Options,
};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

/// The column families this version of the store knows about, in the
/// order they are opened.
pub(super) const EXPECTED_COLUMN_FAMILIES: [&str; 23] = [
    cf::RECORD,
    cf::AGENT_META,
    cf::INBOX,
    cf::MEMORY_FACTS,
    cf::MEMORY_EVENTS,
    cf::MEMORY_PROCEDURES,
    cf::MEMORY_EVENT_INDEX,
    cf::MEMORY_CONFIG,
    cf::AGENT_SKILLS,
    cf::RUNTIME_CAPABILITIES,
    cf::USER_TOOL_DEFAULTS,
    cf::SECRETS,
    cf::PROCESSES,
    cf::PROCESS_RUNS,
    cf::TRACE_RUNS,
    cf::TRACE_STEPS,
    cf::TRACE_INDEX,
    cf::AGENTS,
    cf::PROJECTS,
    cf::REGISTRY_META,
    cf::CHANNELS,
    cf::CHANNEL_MEMBERS,
    cf::CHANNEL_POSTS,
];

impl RocksStore {
    const BLOCK_CACHE_BYTES: usize = 64 * 1024 * 1024;
    const WRITE_BUFFERS_BYTES: usize = 128 * 1024 * 1024;
    const BACKGROUND_JOBS: i32 = 4;

    /// One block cache for every store this process opens, so each open
    /// doesn't add its own.
    fn shared_block_cache() -> &'static Cache {
        static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| Cache::new_lru_cache(Self::BLOCK_CACHE_BYTES))
    }

    /// Open or create a `RocksDB` store at the given path.
    ///
    /// # Errors
    /// Returns error if the database cannot be opened.
    pub fn open(path: impl AsRef<Path>, sync_writes: bool) -> Result<Self, StoreError> {
        Self::open_sealed(path, sync_writes, None)
    }

    /// Open or create a `RocksDB` store with optional value sealing.
    ///
    /// When `cipher` is `Some`, content-bearing values are encrypted at
    /// rest (sealed mode); when `None` this is exactly [`Self::open`].
    ///
    /// # Errors
    /// Returns error if the database cannot be opened.
    pub fn open_sealed(
        path: impl AsRef<Path>,
        sync_writes: bool,
        cipher: Option<Arc<SealCipher>>,
    ) -> Result<Self, StoreError> {
        let path = path.as_ref();
        debug!(?path, sealed = cipher.is_some(), "Opening RocksDB store");

        let opts = Self::db_options();
        let cf_names = Self::column_family_names(path);
        let block_cache = Self::shared_block_cache();
        let cf_descriptors: Vec<_> = cf_names
            .iter()
            .map(|name| {
                let cf_opts = Self::column_family_options(name, block_cache);
                ColumnFamilyDescriptor::new(name.clone(), cf_opts)
            })
            .collect();

        let db =
            DBWithThreadMode::<MultiThreaded>::open_cf_descriptors(&opts, path, cf_descriptors)
                .map_err(backend)?;

        Ok(Self {
            db: Arc::new(db),
            sync_writes,
            processing_claim_lock: Mutex::new(()),
            append_locks: (0..APPEND_SHARDS).map(|_| Mutex::new(())).collect(),
            cipher,
        })
    }

    /// Database-wide options.
    fn db_options() -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        // Many agents append at once: let their writes overlap the WAL
        // and flush and compact on more than the default two threads.
        opts.set_enable_pipelined_write(true);
        opts.set_max_background_jobs(Self::BACKGROUND_JOBS);
        // Each column family has its own memtables; bound them together.
        opts.set_db_write_buffer_size(Self::WRITE_BUFFERS_BYTES);
        opts
    }

    /// The column families to open at `path`.
    ///
    /// If a newer harness version has already created additional
    /// families, include them as opaque pass-through handles. RocksDB
    /// refuses to open a database when any existing family is omitted,
    /// which previously made a desktop rollback or a briefly
    /// out-of-order app/sidecar update fatal at startup.
    fn column_family_names(path: &Path) -> Vec<String> {
        let mut cf_names = EXPECTED_COLUMN_FAMILIES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        if path.join("CURRENT").is_file() {
            Self::add_existing_column_families(path, &mut cf_names);
        }
        cf_names
    }

    /// Append the families already in the database at `path` that
    /// `cf_names` lacks.
    fn add_existing_column_families(path: &Path, cf_names: &mut Vec<String>) {
        match DBWithThreadMode::<MultiThreaded>::list_cf(&Options::default(), path) {
            Ok(existing) => {
                for name in existing {
                    // The default family is opened implicitly by
                    // `open_cf_descriptors`.
                    if name != "default" && !cf_names.contains(&name) {
                        warn!(
                            column_family = %name,
                            ?path,
                            "opening unknown RocksDB column family for forward compatibility"
                        );
                        cf_names.push(name);
                    }
                }
            }
            Err(error) => {
                warn!(
                    %error,
                    ?path,
                    "could not enumerate existing RocksDB column families before open"
                );
            }
        }
    }

    fn column_family_options(name: &str, block_cache: &Cache) -> Options {
        let mut opts = Options::default();
        opts.set_block_based_table_factory(&Self::block_based_table_options(block_cache));

        if Self::should_compress_column_family(name) {
            opts.set_compression_type(DBCompressionType::Lz4);
        }

        opts
    }

    fn block_based_table_options(block_cache: &Cache) -> BlockBasedOptions {
        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_block_cache(block_cache);
        block_opts.set_bloom_filter(10.0, false);
        block_opts.set_whole_key_filtering(true);
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
        block_opts
    }

    fn should_compress_column_family(name: &str) -> bool {
        matches!(
            name,
            cf::RECORD
                | cf::INBOX
                | cf::MEMORY_FACTS
                | cf::MEMORY_EVENTS
                | cf::MEMORY_CONFIG
                | cf::MEMORY_PROCEDURES
                | cf::AGENT_SKILLS
                | cf::RUNTIME_CAPABILITIES
                | cf::USER_TOOL_DEFAULTS
                | cf::SECRETS
                | cf::PROCESSES
                | cf::PROCESS_RUNS
                | cf::TRACE_RUNS
                | cf::TRACE_STEPS
                | cf::AGENTS
                | cf::PROJECTS
                | cf::REGISTRY_META
                | cf::CHANNELS
                | cf::CHANNEL_MEMBERS
                | cf::CHANNEL_POSTS
        )
    }
}
