//! Reports the bot's process health.
//!
//! The `.health` command replies with a snapshot of the process: physical and virtual memory
//! in MiB (via `memory-stats`), the tokio runtime's worker count, alive task count, and global
//! queue depth — and, when the `database` feature is compiled in, the sqlx connection pool
//! stats (established/max connections, idle, or `closed`).
//!
//! The command description changes with the database feature, so `.help` advertises the right
//! fields either way.

use std::fmt::Display;

use tokio::runtime::Handle;

#[cfg(feature = "database")]
use crate::database::Database;
use crate::plugin::prelude::*;

/// The `.health` command description.
#[cfg(feature = "database")]
const HEALTH_DESCRIPTION: &str = "Show memory usage, runtime task stats, and database pool stats";

/// The `.health` command description.
#[cfg(not(feature = "database"))]
const HEALTH_DESCRIPTION: &str = "Show memory usage and runtime task stats";

/// The `.health` command.
const HEALTH: CommandSpec = CommandSpec::new(".health", HEALTH_DESCRIPTION);

/// The health plugin: reports process telemetry for `.health` commands.
pub struct Health;

/// Process telemetry snapshot.
pub struct Snapshot {
    /// The RSS memory usage, as bytes.
    pub phys_mem: f64,
    /// The VMS memory usage, as bytes.
    pub virt_mem: f64,
    /// The number of tasks currently scheduled in the runtime's global queue.
    pub global_queue_depth: usize,
    /// The current number of alive tasks in the runtime.
    pub num_alive_tasks: usize,
    /// The number of worker threads used by the runtime.
    pub num_workers: usize,
    /// Database connection pool statistics.
    #[cfg(feature = "database")]
    pub db: Option<PoolStats>,
}

/// Database connection pool statistics.
#[cfg(feature = "database")]
pub struct PoolStats {
    /// The number of connections currently established by the pool.
    pub size: u32,
    /// The number of established connections not currently in use.
    pub idle: usize,
    /// The maximum number of connections the pool may establish.
    pub max: u32,
    /// Whether the pool has been closed.
    pub closed: bool,
}

#[cfg(feature = "database")]
impl PoolStats {
    /// Captures the statistics of the database connection pool.
    #[must_use]
    pub fn capture(db: &Database) -> PoolStats {
        PoolStats {
            size: db.size(),
            idle: db.num_idle(),
            max: db.options().get_max_connections(),
            closed: db.is_closed(),
        }
    }
}

#[async_trait]
impl Plugin<Context> for Health {
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Health, ZetaError> {
        subscriptions.command(HEALTH);
        Ok(Health)
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        if let Some(mut snapshot) = Snapshot::capture() {
            capture_pool_stats(&mut snapshot, ctx);

            client.send_privmsg(command.channel(), reply("Health", snapshot))?;
        }

        Ok(())
    }
}

/// Attaches database connection pool statistics to the snapshot.
#[cfg(feature = "database")]
fn capture_pool_stats(snapshot: &mut Snapshot, ctx: &Context) {
    snapshot.db = Some(PoolStats::capture(&ctx.db));
}

/// Does nothing when the database feature is disabled.
#[cfg(not(feature = "database"))]
fn capture_pool_stats(_: &mut Snapshot, _: &Context) {}

impl Snapshot {
    /// Captures a snapshot of the process' memory usage and runtime task stats.
    ///
    /// Returns `None` when the platform cannot report memory usage, in which case the command
    /// replies with nothing.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn capture() -> Option<Snapshot> {
        if let Some(memory) = memory_stats::memory_stats() {
            // Capture memory information
            let phys_mem = memory.physical_mem as f64 / 1024.0 / 1024.0;
            let virt_mem = memory.virtual_mem as f64 / 1024.0 / 1024.0;

            // Capture tokio runtime information
            let metrics = Handle::current().metrics();
            let num_workers = metrics.num_workers();
            let num_alive_tasks = metrics.num_alive_tasks();
            let global_queue_depth = metrics.global_queue_depth();

            return Some(Snapshot {
                phys_mem,
                virt_mem,
                global_queue_depth,
                num_alive_tasks,
                num_workers,
                #[cfg(feature = "database")]
                db: None,
            });
        }

        None
    }
}

impl Display for Snapshot {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let phys_mem = self.phys_mem;
        let virt_mem = self.virt_mem;

        write!(fmt, "Memory usage:\x0f {phys_mem:.2} MiB\x0310 ")?;
        write!(fmt, "(\x0f{virt_mem:.2} MiB\x0310 virtual) ")?;

        write!(fmt, "Workers:\x0f {}\x0310 ", self.num_workers)?;
        write!(fmt, "Tasks:\x0f {}\x0310 ", self.num_alive_tasks)?;
        write!(fmt, "(\x0f{}\x0310 scheduled)", self.global_queue_depth)?;

        #[cfg(feature = "database")]
        if let Some(db) = &self.db {
            if db.closed {
                write!(fmt, " DB:\x0f closed\x0310")?;
            } else {
                write!(fmt, " DB:\x0f {}\x0310/\x0f{}\x0310 ", db.size, db.max)?;
                write!(fmt, "(\x0f{}\x0310 idle)", db.idle)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use irc::proto::FormattedStringExt;
    use wildmatch::WildMatch;

    #[tokio::test]
    async fn it_should_format_message() {
        let snapshot = Snapshot::capture().expect("could not capture");
        let snapshot_message = snapshot.to_string().strip_formatting();
        let wildmatcher =
            WildMatch::new("Memory usage: * MiB (* MiB virtual) Workers: * Tasks: * (* scheduled)");
        assert!(wildmatcher.matches(&snapshot_message));
    }

    #[cfg(feature = "database")]
    #[tokio::test]
    async fn it_should_format_database_pool_stats() {
        let mut snapshot = Snapshot::capture().expect("could not capture");
        snapshot.db = Some(PoolStats {
            size: 3,
            idle: 2,
            max: 10,
            closed: false,
        });

        let snapshot_message = snapshot.to_string().strip_formatting();
        let wildmatcher = WildMatch::new(
            "Memory usage: * MiB (* MiB virtual) Workers: * Tasks: * (* scheduled) DB: 3/10 (2 idle)",
        );
        assert!(wildmatcher.matches(&snapshot_message));
    }

    #[cfg(feature = "database")]
    #[tokio::test]
    async fn it_should_format_closed_database_pool() {
        let mut snapshot = Snapshot::capture().expect("could not capture");
        snapshot.db = Some(PoolStats {
            size: 0,
            idle: 0,
            max: 10,
            closed: true,
        });

        let snapshot_message = snapshot.to_string().strip_formatting();
        let wildmatcher = WildMatch::new(
            "Memory usage: * MiB (* MiB virtual) Workers: * Tasks: * (* scheduled) DB: closed",
        );
        assert!(wildmatcher.matches(&snapshot_message));
    }
}
