use std::fmt::Display;

use tokio::runtime::Handle;

use crate::plugin::prelude::*;

pub struct Health {
    /// The `.health` command trigger.
    command: ZetaCommand,
}

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
}

#[async_trait]
impl Plugin for Health {
    fn new() -> Health {
        let command = ZetaCommand::new(".health");

        Health { command }
    }

    fn name() -> Name {
        Name("health")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(_) = self.command.parse(user_message)
            && let Some(snapshot) = Snapshot::capture()
        {
            client.send_privmsg(
                channel,
                format!("\x0310>\x0f\x02 Health\x02\x0310: {snapshot}"),
            )?;
        }

        Ok(())
    }
}

impl Snapshot {
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
}
